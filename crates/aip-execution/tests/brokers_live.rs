#![cfg(feature = "external-brokers")]

use aip_execution::{
    brokers::{
        BrokerConfig, Destination, ImportSource, InboundRoute, KafkaConfig, KafkaSecurity, OutboundRoute, ProviderConfig, RabbitMqConfig, Relay,
    },
    messages::{MessageStore, NewMessage, ddl},
};
use rdkafka::{
    ClientConfig,
    consumer::{Consumer, StreamConsumer},
    message::Message,
    producer::{FutureProducer, FutureRecord, Producer},
    util::Timeout,
};
use serde_json::json;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio_postgres::{Client, NoTls};

const DB: &str = "host=localhost user=runner dbname=postgres";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    client: Client,
    schema: String,
}

impl Fixture {
    async fn new() -> Self {
        let schema = format!("aip_broker_live_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let (client, connection) = tokio_postgres::connect(DB, NoTls).await.expect("connect disposable CI Postgres");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
        client.batch_execute(&ddl(&schema).unwrap()).await.unwrap();
        Self { client, schema }
    }
    fn store(&self) -> MessageStore {
        MessageStore::new(&self.schema).unwrap()
    }
    async fn enqueue(&self, owner: &str, queue: &str, key: &str) {
        self.store()
            .enqueue(
                &self.client,
                NewMessage {
                    owner: owner.into(),
                    queue: queue.into(),
                    key: key.into(),
                    payload: json!({"sample": key}),
                    max_deliveries: 3,
                    ttl_ms: 60_000,
                    delay_ms: 0,
                },
            )
            .await
            .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let schema = self.schema.clone();
        let _ = std::process::Command::new("psql").args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")]).status();
    }
}

fn fixture_variable(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[tokio::test]
#[ignore = "requires real Kafka/RabbitMQ and PostgreSQL; executed by brokers workflow"]
async fn real_kafka_confirm_and_import_round_trip() {
    let bootstrap = fixture_variable("AIP_TEST_KAFKA_BOOTSTRAP").expect("live Kafka bootstrap is required");
    let outbound_topic = fixture_variable("AIP_TEST_KAFKA_OUT_TOPIC").expect("Kafka output topic configured in broker CI");
    let inbound_topic = fixture_variable("AIP_TEST_KAFKA_IN_TOPIC").expect("Kafka input topic configured in broker CI");
    let mut fixture = Fixture::new().await;
    let owner = "broker-ci-owner";
    let security = KafkaSecurity::Plaintext { allow_insecure_transport: true };
    let provider = ProviderConfig::Kafka(KafkaConfig {
        name: "ci-kafka".into(),
        bootstrap_servers: vec![bootstrap.clone()],
        security: security.clone(),
        timeout_ms: 3_000,
        allow_private_networks: true,
    });
    let inbound_group = format!("aip-ci-{}", uuid::Uuid::new_v4());
    let config = BrokerConfig {
        providers: vec![provider],
        outbound: vec![OutboundRoute {
            owner: owner.into(),
            queue: "outgoing".into(),
            destination: Destination::Kafka { provider: "ci-kafka".into(), topic: outbound_topic.clone() },
        }],
        inbound: vec![InboundRoute {
            provider: "ci-kafka".into(),
            source: ImportSource::Kafka { topic: inbound_topic.clone(), group_id: inbound_group, dead_letter_topic: format!("{inbound_topic}-dead") },
            owner: owner.into(),
            queue: "incoming".into(),
            max_deliveries: 3,
            ttl_ms: 60_000,
        }],
    };
    let mut relay = Relay::connect(config).await.unwrap_or_else(|error| {
        let mut diagnostic = ClientConfig::new();
        diagnostic
            .set("bootstrap.servers", &bootstrap)
            .set("security.protocol", "plaintext")
            .set("socket.timeout.ms", "3000")
            .set("client.id", "aip-outbox-ci-kafka")
            .set("enable.idempotence", "true")
            .set("acks", "all")
            .set("delivery.timeout.ms", "3000")
            .set("message.max.bytes", "1048576")
            .set("queue.buffering.max.kbytes", "32768")
            .set("allow.auto.create.topics", "false");
        let probe = diagnostic.create::<FutureProducer>().map_err(|cause| format!("producer config: {cause:?}"));
        let probe = probe.and_then(|producer| {
            producer
                .client()
                .fetch_metadata(None, Timeout::After(Duration::from_secs(3)))
                .map(|_| ())
                .map_err(|cause| format!("metadata probe: {cause:?}"))
        });
        let mut consumer_config = ClientConfig::new();
        consumer_config
            .set("bootstrap.servers", &bootstrap)
            .set("security.protocol", "plaintext")
            .set("socket.timeout.ms", "3000")
            .set("group.id", "aip-ci-probe")
            .set("enable.auto.commit", "false")
            .set("enable.auto.offset.store", "false")
            .set("isolation.level", "read_committed")
            .set("auto.offset.reset", "earliest")
            .set("allow.auto.create.topics", "false")
            .set("session.timeout.ms", "10000")
            .set("max.poll.interval.ms", "30000")
            .set("fetch.max.bytes", "2097152")
            .set("max.partition.fetch.bytes", "1048576")
            .set("queued.max.messages.kbytes", "32768");
        let consumer_probe = consumer_config
            .create::<StreamConsumer>()
            .map_err(|cause| format!("consumer config: {cause:?}"))
            .and_then(|consumer| consumer.subscribe(&[&inbound_topic]).map_err(|cause| format!("consumer subscribe: {cause:?}")));
        panic!("connect real Kafka client: {error:?}; direct producer/metadata probe: {probe:?}; consumer probe: {consumer_probe:?}");
    });
    let store = fixture.store();
    let poll_started = std::time::Instant::now();
    assert!(!relay.import_once_with_wait(&mut fixture.client, &store, owner, "incoming", Duration::from_millis(1)).await.unwrap());
    assert!(poll_started.elapsed() < Duration::from_millis(500), "empty inbound must not wait for the provider timeout");
    let outbound_key = format!("out-{}", uuid::Uuid::new_v4());
    fixture.enqueue(owner, "outgoing", &outbound_key).await;
    let result = relay.relay_once(&fixture.client, &store, owner, "outgoing", "ci-publisher", 10_000).await.unwrap();
    assert!(
        matches!(result, aip_execution::brokers::RelayOutcome::Published { .. }),
        "Kafka FutureProducer must confirm before PostgreSQL ack: {result:?}"
    );

    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &bootstrap)
        .set("group.id", format!("verify-{}", uuid::Uuid::new_v4()))
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "earliest")
        .set("allow.auto.create.topics", "false")
        .create()
        .unwrap();
    consumer.subscribe(&[&outbound_topic]).unwrap();
    let published = tokio::time::timeout(Duration::from_secs(10), consumer.recv()).await.unwrap().unwrap();
    let envelope: serde_json::Value = serde_json::from_slice(published.payload().unwrap()).unwrap();
    assert_eq!(envelope["owner"], owner);
    assert_eq!(envelope["queue"], "outgoing");
    assert_eq!(envelope["payload"]["sample"], outbound_key);

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &bootstrap)
        .set("message.timeout.ms", "3000")
        .set("message.max.bytes", "4194304")
        .create()
        .unwrap();
    let incoming = json!({"source":"kafka-ci","unique":uuid::Uuid::new_v4().to_string()}).to_string();
    producer.send(FutureRecord::to(&inbound_topic).key("external-key").payload(&incoming), Timeout::After(Duration::from_secs(3))).await.unwrap();
    let mut imported = false;
    for _ in 0..10 {
        if relay.import_once(&mut fixture.client, &store, owner, "incoming").await.unwrap() {
            imported = true;
            break;
        }
    }
    assert!(imported, "Kafka consumer must import a confirmed source record");
    let row = fixture
        .client
        .query_one(&format!("SELECT payload FROM {}.aip_messages WHERE owner=$1 AND queue='incoming'", fixture.schema), &[&owner])
        .await
        .unwrap();
    assert_eq!(row.get::<_, serde_json::Value>(0), serde_json::from_str::<serde_json::Value>(&incoming).unwrap());
    producer.send(FutureRecord::to(&inbound_topic).key("external-key").payload(&incoming), Timeout::After(Duration::from_secs(3))).await.unwrap();
    assert!(relay.import_once(&mut fixture.client, &store, owner, "incoming").await.unwrap());
    let count: i64 =
        fixture.client.query_one(&format!("SELECT count(*) FROM {}.aip_messages WHERE queue='incoming'", fixture.schema), &[]).await.unwrap().get(0);
    assert_eq!(count, 2, "distinct offsets sharing an application key must not be conflated");

    let dead: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &bootstrap)
        .set("group.id", format!("dead-{}", uuid::Uuid::new_v4()))
        .set("auto.offset.reset", "earliest")
        .create()
        .unwrap();
    dead.subscribe(&[&format!("{inbound_topic}-dead")]).unwrap();
    let poison = vec![b'!'; 2 << 20];
    producer.send(FutureRecord::to(&inbound_topic).key("poison").payload(poison.as_slice()), Timeout::After(Duration::from_secs(3))).await.unwrap();
    assert!(
        relay.import_once(&mut fixture.client, &store, owner, "incoming").await.unwrap(),
        "oversized source records must not block the offset forever"
    );
    let record = tokio::time::timeout(Duration::from_secs(10), dead.recv()).await.unwrap().unwrap();
    let quarantine: serde_json::Value = serde_json::from_slice(record.payload().unwrap()).unwrap();
    assert_eq!(quarantine["originalBytes"], poison.len());
    assert_eq!(quarantine["truncated"], true);
    assert!(record.payload().unwrap().len() < 16_384);
}

#[tokio::test]
#[ignore = "requires real Kafka/RabbitMQ and PostgreSQL; executed by brokers workflow"]
async fn real_rabbitmq_confirm_and_import_round_trip() {
    let uri = fixture_variable("AIP_TEST_RABBITMQ_URI").expect("live RabbitMQ URI is required");
    let outbound_queue = fixture_variable("AIP_TEST_RABBITMQ_OUT_QUEUE").expect("Rabbit output queue configured in broker CI");
    let inbound_queue = fixture_variable("AIP_TEST_RABBITMQ_IN_QUEUE").expect("Rabbit input queue configured in broker CI");
    use lapin::{
        BasicProperties, Connection, ConnectionProperties,
        options::{BasicGetOptions, QueueDeclareOptions},
        types::FieldTable,
    };
    let setup = Connection::connect(&uri, ConnectionProperties::default()).await.unwrap();
    let setup_channel = setup.create_channel().await.unwrap();
    for queue in [&outbound_queue, &inbound_queue] {
        setup_channel
            .queue_declare(queue.clone().into(), QueueDeclareOptions { durable: true, ..QueueDeclareOptions::default() }, FieldTable::default())
            .await
            .unwrap();
    }

    let mut fixture = Fixture::new().await;
    let owner = "broker-ci-owner";
    let config = BrokerConfig {
        providers: vec![ProviderConfig::RabbitMq(RabbitMqConfig {
            name: "ci-rabbit".into(),
            uri,
            timeout_ms: 3_000,
            allow_private_networks: true,
            allow_insecure_transport: true,
        })],
        outbound: vec![OutboundRoute {
            owner: owner.into(),
            queue: "outgoing".into(),
            destination: Destination::RabbitMq { provider: "ci-rabbit".into(), exchange: String::new(), routing_key: outbound_queue.clone() },
        }],
        inbound: vec![InboundRoute {
            provider: "ci-rabbit".into(),
            source: ImportSource::RabbitMq {
                queue_name: inbound_queue.clone(),
                dead_letter_exchange: String::new(),
                dead_letter_routing_key: outbound_queue.clone(),
            },
            owner: owner.into(),
            queue: "incoming".into(),
            max_deliveries: 3,
            ttl_ms: 60_000,
        }],
    };
    let mut relay = Relay::connect(config).await.expect("connect real AMQP client");
    let store = fixture.store();
    let poll_started = std::time::Instant::now();
    assert!(!relay.import_once_with_wait(&mut fixture.client, &store, owner, "incoming", Duration::from_millis(1)).await.unwrap());
    assert!(poll_started.elapsed() < Duration::from_millis(500), "empty inbound must not wait for the provider timeout");
    let outbound_key = format!("out-{}", uuid::Uuid::new_v4());
    fixture.enqueue(owner, "outgoing", &outbound_key).await;
    let result = relay.relay_once(&fixture.client, &store, owner, "outgoing", "ci-publisher", 10_000).await.unwrap();
    assert!(
        matches!(result, aip_execution::brokers::RelayOutcome::Published { .. }),
        "RabbitMQ publisher confirm must precede PostgreSQL ack: {result:?}"
    );
    let delivery =
        setup_channel.basic_get(outbound_queue.clone().into(), BasicGetOptions::default()).await.unwrap().expect("published RabbitMQ message");
    let envelope: serde_json::Value = serde_json::from_slice(&delivery.data).unwrap();
    assert_eq!(envelope["payload"]["sample"], outbound_key);
    delivery.ack(lapin::options::BasicAckOptions::default()).await.unwrap();

    let incoming = json!({"source":"rabbit-ci","unique":uuid::Uuid::new_v4().to_string()}).to_string();
    setup_channel
        .basic_publish(
            String::new().into(),
            inbound_queue.clone().into(),
            lapin::options::BasicPublishOptions::default(),
            incoming.as_bytes(),
            BasicProperties::default().with_message_id("external-ci-record".into()),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    assert!(relay.import_once(&mut fixture.client, &store, owner, "incoming").await.unwrap());
    let row = fixture
        .client
        .query_one(&format!("SELECT payload FROM {}.aip_messages WHERE owner=$1 AND queue='incoming'", fixture.schema), &[&owner])
        .await
        .unwrap();
    assert_eq!(row.get::<_, serde_json::Value>(0), serde_json::from_str::<serde_json::Value>(&incoming).unwrap());
    for payload in [&incoming, &json!({"conflicting":true}).to_string()] {
        setup_channel
            .basic_publish(
                String::new().into(),
                inbound_queue.clone().into(),
                lapin::options::BasicPublishOptions::default(),
                payload.as_bytes(),
                BasicProperties::default().with_message_id("external-ci-record".into()),
            )
            .await
            .unwrap()
            .await
            .unwrap();
        assert!(relay.import_once(&mut fixture.client, &store, owner, "incoming").await.unwrap());
    }
    let count: i64 =
        fixture.client.query_one(&format!("SELECT count(*) FROM {}.aip_messages WHERE queue='incoming'", fixture.schema), &[]).await.unwrap().get(0);
    assert_eq!(count, 1, "same message-id replays without duplicating or replacing the original payload");
    let dead = setup_channel
        .basic_get(outbound_queue.into(), BasicGetOptions::default())
        .await
        .unwrap()
        .expect("conflicting message-id must be quarantined, not endlessly requeued");
    let quarantine: serde_json::Value = serde_json::from_slice(&dead.data).unwrap();
    assert_eq!(quarantine["error"], "IMPORT_REJECTED");
    dead.ack(lapin::options::BasicAckOptions::default()).await.unwrap();
}
