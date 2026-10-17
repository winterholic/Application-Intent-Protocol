//! Server-owned broker destinations. Client requests never choose a broker,
//! topic, exchange, or queue; this file only turns deployment configuration
//! into the execution crate's allowlisted routes.

#![cfg_attr(not(feature = "external-brokers"), allow(dead_code))]

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerConfig {
    #[serde(default)]
    providers: Vec<Provider>,
    #[serde(default)]
    outbound: Vec<Outbound>,
    #[serde(default)]
    inbound: Vec<Inbound>,
    #[serde(default = "default_lease_ms")]
    lease_ms: i64,
    #[serde(default = "default_poll_ms")]
    poll_ms: u64,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Provider {
    Kafka {
        name: String,
        bootstrap_servers: Vec<String>,
        security: KafkaSecurity,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
        #[serde(default)]
        allow_private_networks: bool,
    },
    RabbitMq {
        name: String,
        uri_env: String,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
        #[serde(default)]
        allow_private_networks: bool,
        #[serde(default)]
        allow_insecure_transport: bool,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum KafkaSecurity {
    Tls {
        ca_location: Option<String>,
    },
    SaslTls {
        mechanism: String,
        username: String,
        password_env: String,
        ca_location: Option<String>,
    },
    Plaintext {
        #[serde(default)]
        allow_insecure_transport: bool,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Destination {
    Kafka { provider: String, topic: String },
    RabbitMq { provider: String, exchange: String, routing_key: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Outbound {
    owner: String,
    queue: String,
    destination: Destination,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Source {
    Kafka { topic: String, group_id: String, dead_letter_topic: String },
    RabbitMq { queue_name: String, dead_letter_exchange: String, dead_letter_routing_key: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inbound {
    provider: String,
    source: Source,
    owner: String,
    queue: String,
    #[serde(default = "default_max_deliveries")]
    max_deliveries: i32,
    #[serde(default = "default_ttl_ms")]
    ttl_ms: i64,
}

fn default_timeout_ms() -> u64 {
    5_000
}
fn default_lease_ms() -> i64 {
    15_000
}
fn default_poll_ms() -> u64 {
    250
}
fn default_max_deliveries() -> i32 {
    3
}
fn default_ttl_ms() -> i64 {
    24 * 60 * 60 * 1_000
}

#[cfg(feature = "external-brokers")]
type ResolvedBroker = (aip_execution::brokers::BrokerConfig, i64, u64, Vec<(String, String)>, Vec<(String, String)>);

impl BrokerConfig {
    pub fn declared_queues(&self) -> impl Iterator<Item = &str> {
        self.outbound.iter().map(|route| route.queue.as_str()).chain(self.inbound.iter().map(|route| route.queue.as_str()))
    }

    #[cfg(feature = "external-brokers")]
    fn into_parts(self) -> Result<ResolvedBroker, String> {
        let lease_ms = self.lease_ms;
        let poll_ms = self.poll_ms;
        let outbound_scopes = self.outbound.iter().map(|route| (route.owner.clone(), route.queue.clone())).collect();
        let inbound_scopes = self.inbound.iter().map(|route| (route.owner.clone(), route.queue.clone())).collect();
        let config = self.into_execution()?;
        if config.providers.iter().any(|provider| {
            let timeout = match provider {
                aip_execution::brokers::ProviderConfig::Kafka(config) => config.timeout_ms,
                aip_execution::brokers::ProviderConfig::RabbitMq(config) => config.timeout_ms,
            };
            timeout.saturating_add(500) > lease_ms as u64
        }) {
            return Err("broker lease_ms must exceed every provider timeout by 500ms".into());
        }
        Ok((config, lease_ms, poll_ms, outbound_scopes, inbound_scopes))
    }

    #[cfg(feature = "external-brokers")]
    pub fn into_execution(self) -> Result<aip_execution::brokers::BrokerConfig, String> {
        if !(1..=30_000).contains(&self.poll_ms) || !(5_500..=300_000).contains(&self.lease_ms) {
            return Err("broker poll_ms/lease_ms 범위 오류".into());
        }
        let providers = self
            .providers
            .into_iter()
            .map(|provider| match provider {
                Provider::Kafka { name, bootstrap_servers, security, timeout_ms, allow_private_networks } => {
                    let provider_name = name.clone();
                    let security = match security {
                        KafkaSecurity::Tls { ca_location } => aip_execution::brokers::KafkaSecurity::Tls { ca_location },
                        KafkaSecurity::SaslTls { mechanism, username, password_env, ca_location } => {
                            let password = std::env::var(&password_env)
                                .ok()
                                .filter(|value| !value.is_empty())
                                .ok_or_else(|| format!("Kafka provider '{provider_name}'의 SASL password_env가 가리키는 환경 변수가 없거나 비어 있음(변수 이름은 출력하지 않음)"))?;
                            aip_execution::brokers::KafkaSecurity::SaslTls { mechanism, username, password, ca_location }
                        }
                        KafkaSecurity::Plaintext { allow_insecure_transport } => {
                            aip_execution::brokers::KafkaSecurity::Plaintext { allow_insecure_transport }
                        }
                    };
                    Ok(aip_execution::brokers::ProviderConfig::Kafka(aip_execution::brokers::KafkaConfig {
                        name,
                        bootstrap_servers,
                        security,
                        timeout_ms,
                        allow_private_networks,
                    }))
                }
                Provider::RabbitMq { name, uri_env, timeout_ms, allow_private_networks, allow_insecure_transport } => {
                    let uri = std::env::var(&uri_env)
                        .ok()
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| format!("RabbitMQ provider '{name}'의 uri_env가 가리키는 환경 변수가 없거나 비어 있음(변수 이름은 출력하지 않음)"))?;
                    Ok(aip_execution::brokers::ProviderConfig::RabbitMq(aip_execution::brokers::RabbitMqConfig {
                        name,
                        uri,
                        timeout_ms,
                        allow_private_networks,
                        allow_insecure_transport,
                    }))
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        let outbound = self
            .outbound
            .into_iter()
            .map(|route| aip_execution::brokers::OutboundRoute {
                owner: route.owner,
                queue: route.queue,
                destination: match route.destination {
                    Destination::Kafka { provider, topic } => aip_execution::brokers::Destination::Kafka { provider, topic },
                    Destination::RabbitMq { provider, exchange, routing_key } => {
                        aip_execution::brokers::Destination::RabbitMq { provider, exchange, routing_key }
                    }
                },
            })
            .collect();
        let inbound = self
            .inbound
            .into_iter()
            .map(|route| aip_execution::brokers::InboundRoute {
                provider: route.provider,
                owner: route.owner,
                queue: route.queue,
                max_deliveries: route.max_deliveries,
                ttl_ms: route.ttl_ms,
                source: match route.source {
                    Source::Kafka { topic, group_id, dead_letter_topic } => {
                        aip_execution::brokers::ImportSource::Kafka { topic, group_id, dead_letter_topic }
                    }
                    Source::RabbitMq { queue_name, dead_letter_exchange, dead_letter_routing_key } => {
                        aip_execution::brokers::ImportSource::RabbitMq { queue_name, dead_letter_exchange, dead_letter_routing_key }
                    }
                },
            })
            .collect();
        let config = aip_execution::brokers::BrokerConfig { providers, outbound, inbound };
        config.validate().map_err(|e| {
            use aip_execution::brokers::RelayError;
            match e {
                RelayError::DuplicateRoute => "outbound·inbound route의 owner·queue 조합이 중복되었거나 형식이 올바르지 않음",
                RelayError::UnknownProvider => "route가 설정에 없는 provider를 가리킴",
                RelayError::RouteNotAllowed => "route의 종류(Kafka·RabbitMQ)가 가리키는 provider의 종류와 다름",
                _ => "provider 이름·보안·주소 설정 또는 route의 topic·exchange·queue·한계 값 범위 오류",
            }
            .to_owned()
        })?;
        Ok(config)
    }

    pub fn lease_ms(&self) -> i64 {
        self.lease_ms
    }
    pub fn poll_ms(&self) -> u64 {
        self.poll_ms
    }
}

#[cfg(feature = "external-brokers")]
pub struct Worker {
    stop: tokio::sync::watch::Sender<bool>,
    join: tokio::task::JoinHandle<Result<(), &'static str>>,
}

#[cfg(feature = "external-brokers")]
impl Worker {
    pub async fn start(config: BrokerConfig, database_url: String, schema: String) -> Result<Self, String> {
        let (relay_config, lease_ms, poll_ms, outbound, inbound) = config.into_parts()?;
        let store = aip_execution::messages::MessageStore::new(schema).map_err(|_| "메시지 저장소 schema 설정 오류")?;
        let initial = connect_relay(relay_config.clone(), &database_url).await.map_err(str::to_owned)?;
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let join = tokio::spawn(async move {
            let mut connection = Some(initial);
            let mut retry_ms = 1_000u64;
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(poll_ms));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                if connection.is_none() {
                    // Closing consumers before reconnect replays only unacknowledged records.
                    tokio::select! {
                        _ = stopped.changed() => return Ok(()),
                        _ = tokio::time::sleep(std::time::Duration::from_millis(retry_ms)) => {}
                    }
                    let attempt = tokio::select! {
                        _ = stopped.changed() => return Ok(()),
                        attempt = connect_relay(relay_config.clone(), &database_url) => attempt
                    };
                    match attempt {
                        Ok(connected) => {
                            connection = Some(connected);
                        }
                        Err(_) => {
                            retry_ms = retry_ms.saturating_mul(2).min(30_000);
                            eprintln!("aip broker relay reconnect failed; retry_in_ms={retry_ms}");
                            continue;
                        }
                    }
                }
                let (relay, db) = connection.as_mut().ok_or("broker relay state unavailable")?;
                let sweep = async {
                    tick.tick().await;
                    for (owner, queue) in &outbound {
                        let outcomes = relay.relay_batch(&**db, &store, owner, queue, "aip-broker-relay", lease_ms, 20).await?;
                        if outcomes.iter().any(|outcome| matches!(outcome, aip_execution::brokers::RelayOutcome::RetryScheduled { .. })) {
                            return Err(aip_execution::brokers::RelayError::BrokerUnavailable);
                        }
                    }
                    for (owner, queue) in &inbound {
                        relay.import_once_with_wait(db, &store, owner, queue, std::time::Duration::from_millis(1)).await?;
                    }
                    Ok::<(), aip_execution::brokers::RelayError>(())
                };
                let result = tokio::select! {
                    _ = stopped.changed() => return Ok(()),
                    result = sweep => result
                };
                match result {
                    Ok(()) => {
                        retry_ms = 1_000;
                    }
                    Err(error) => {
                        connection.take();
                        retry_ms = retry_ms.saturating_mul(2).min(30_000);
                        eprintln!("aip broker relay disconnected; reason={error:?} retry_in_ms={retry_ms}");
                    }
                }
            }
        });
        Ok(Self { stop, join })
    }

    pub async fn shutdown(&mut self) -> Result<(), &'static str> {
        let _ = self.stop.send(true);
        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut self.join).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("broker worker task 종료 실패"),
            Err(_) => Err("broker worker bounded shutdown 기한 초과"),
        }
    }

    pub async fn wait(&mut self) -> Result<(), &'static str> {
        (&mut self.join).await.map_err(|_| "broker worker task 종료 실패")?
    }
}

#[cfg(feature = "external-brokers")]
async fn connect_relay(
    config: aip_execution::brokers::BrokerConfig,
    database_url: &str,
) -> Result<(aip_execution::brokers::Relay, spike_v2_read::OwnedConnection), &'static str> {
    let relay = tokio::time::timeout(std::time::Duration::from_secs(10), aip_execution::brokers::Relay::connect(config))
        .await
        .map_err(|_| "broker connection deadline exceeded")?
        .map_err(|_| "broker connection failed")?;
    let db = tokio::time::timeout(std::time::Duration::from_secs(5), spike_v2_read::connect_owned_with_url(database_url))
        .await
        .map_err(|_| "broker database connection deadline exceeded")?
        .map_err(|_| "broker database connection failed")?;
    Ok((relay, db))
}

#[cfg(feature = "external-brokers")]
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        self.join.abort();
    }
}

#[cfg(not(feature = "external-brokers"))]
pub struct Worker;

#[cfg(not(feature = "external-brokers"))]
impl Worker {
    pub async fn wait(&mut self) -> Result<(), &'static str> {
        std::future::pending().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_are_conservative_and_credentials_are_env_references() {
        let config: BrokerConfig = serde_json::from_value(json!({
            "providers":[{"kind":"kafka","name":"events","bootstrap_servers":["127.0.0.1:9092"],"allow_private_networks":true,"security":{"kind":"plaintext","allow_insecure_transport":true}}],
            "outbound":[{"owner":"42","queue":"invoice_events","destination":{"kind":"kafka","provider":"events","topic":"invoices"}}]
        })).unwrap();
        assert_eq!(config.lease_ms(), 15_000);
        assert_eq!(config.poll_ms(), 250);
        #[cfg(feature = "external-brokers")]
        assert!(config.into_execution().is_ok());
    }

    #[test]
    fn unknown_config_fields_are_rejected() {
        assert!(serde_json::from_value::<BrokerConfig>(json!({"outbound":[],"debug_password":"oops"})).is_err());
    }

    #[cfg(feature = "external-brokers")]
    #[test]
    fn broker_configuration_failures_name_the_cause_and_not_the_secret_variable() {
        let kafka = |extra: serde_json::Value| -> String {
            let mut value = json!({
                "providers":[{"kind":"kafka","name":"events","bootstrap_servers":["127.0.0.1:9092"],"allow_private_networks":true,
                    "security":{"kind":"sasl_tls","mechanism":"scram-sha-256","username":"u","password_env":"AIP_TEST_BROKER_SECRET_NAME_5521"}}],
                "outbound":[]
            });
            for (key, item) in extra.as_object().into_iter().flatten() {
                value[key] = item.clone();
            }
            let config: BrokerConfig = serde_json::from_value(value).unwrap();
            config.into_execution().err().expect("must be rejected")
        };
        let missing_env = kafka(json!({}));
        assert!(missing_env.contains("'events'") && missing_env.contains("환경 변수가 없"), "{missing_env}");
        assert!(!missing_env.contains("AIP_TEST_BROKER_SECRET_NAME_5521"), "the variable name must not be printed: {missing_env}");
        let unknown_provider = serde_json::from_value::<BrokerConfig>(json!({
            "providers":[{"kind":"kafka","name":"events","bootstrap_servers":["127.0.0.1:9092"],"allow_private_networks":true,"security":{"kind":"plaintext","allow_insecure_transport":true}}],
            "outbound":[{"owner":"42","queue":"q","destination":{"kind":"kafka","provider":"missing","topic":"t"}}]
        }))
        .unwrap()
        .into_execution()
        .err()
        .expect("unknown provider");
        assert!(unknown_provider.contains("없는 provider"), "{unknown_provider}");
        assert_ne!(missing_env, unknown_provider);
    }

    #[cfg(feature = "external-brokers")]
    #[tokio::test]
    #[ignore = "requires broker workflow Kafka and PostgreSQL fixtures"]
    async fn broker_worker_recovers_from_database_disconnect_without_losing_delivery_budget() {
        use aip_execution::messages::{MessageStore, NewMessage};
        use std::time::{Duration, Instant};
        let bootstrap = std::env::var("AIP_TEST_KAFKA_BOOTSTRAP").expect("live Kafka bootstrap required");
        let topic = std::env::var("AIP_TEST_KAFKA_OUT_TOPIC").expect("live Kafka topic required");
        let database_url = aip_test_support::database_url();
        let db = spike_v2_read::connect_owned_with_url(&database_url).await.unwrap();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let schema = format!("aip_relay_recovery_{suffix}");
        db.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
        db.batch_execute(&aip_execution::messages::ddl(&schema).unwrap()).await.unwrap();
        let application = format!("aip_relay_{suffix}");
        let config: BrokerConfig = serde_json::from_value(json!({
            "providers":[{"kind":"kafka","name":"test","bootstrap_servers":[bootstrap],"security":{"kind":"plaintext","allow_insecure_transport":true},"allow_private_networks":true,"timeout_ms":3000}],
            "outbound":[{"owner":"recovery-owner","queue":"outgoing","destination":{"kind":"kafka","provider":"test","topic":topic}}],
            "poll_ms":50
        })).unwrap();
        let mut worker = Worker::start(config, format!("{database_url} application_name={application}"), schema.clone()).await.unwrap();
        let victims = db.query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name=$1", &[&application]).await.unwrap();
        assert_eq!(victims.len(), 1, "terminate the actual worker database connection");
        let store = MessageStore::new(&schema).unwrap();
        let message = store
            .enqueue(
                &*db,
                NewMessage {
                    owner: "recovery-owner".into(),
                    queue: "outgoing".into(),
                    key: "after-disconnect".into(),
                    payload: json!({"valid":true}),
                    max_deliveries: 1,
                    ttl_ms: 60_000,
                    delay_ms: 0,
                },
            )
            .await
            .unwrap();
        let started = Instant::now();
        loop {
            let state: String = db
                .query_one(&format!("SELECT state FROM {schema}.aip_messages WHERE id=$1"), &[&uuid::Uuid::parse_str(&message.id).unwrap()])
                .await
                .unwrap()
                .get(0);
            if state == "ACKED" {
                break;
            }
            assert_ne!(state, "DEAD", "infrastructure outage must not exhaust delivery budget");
            assert!(started.elapsed() < Duration::from_secs(15), "broker relay did not recover: {state}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(tokio::time::timeout(Duration::from_millis(20), worker.wait()).await.is_err(), "worker stays alive after reconnect");
        worker.shutdown().await.unwrap();
        db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    }
}
