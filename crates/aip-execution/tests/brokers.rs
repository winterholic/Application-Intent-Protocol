#![cfg(feature = "external-brokers")]

use aip_execution::brokers::{BrokerConfig, Destination, ImportSource, InboundRoute, OutboundRoute, ProviderConfig, RelayError};
use serde_json::json;

fn kafka() -> ProviderConfig {
    ProviderConfig::Kafka(aip_execution::brokers::KafkaConfig {
        name: "kafka-main".into(),
        bootstrap_servers: vec!["broker.internal:9093".into()],
        security: aip_execution::brokers::KafkaSecurity::SaslTls {
            mechanism: "SCRAM-SHA-512".into(),
            username: "aip-worker".into(),
            password: "secret-for-test".into(),
            ca_location: None,
        },
        timeout_ms: 5_000,
        allow_private_networks: true,
    })
}

fn config() -> BrokerConfig {
    BrokerConfig {
        providers: vec![kafka()],
        outbound: vec![OutboundRoute {
            owner: "issuer|tenant-a".into(),
            queue: "billing".into(),
            destination: Destination::Kafka { provider: "kafka-main".into(), topic: "aip.billing".into() },
        }],
        inbound: vec![InboundRoute {
            provider: "kafka-main".into(),
            source: ImportSource::Kafka {
                topic: "partner.events".into(),
                group_id: "aip-import".into(),
                dead_letter_topic: "partner.events.dead".into(),
            },
            owner: "issuer|tenant-a".into(),
            queue: "partner-events".into(),
            max_deliveries: 3,
            ttl_ms: 86_400_000,
        }],
    }
}

#[test]
fn routes_are_server_owned_and_reject_missing_providers_or_ambiguous_scopes() {
    let mut value = config();
    value.validate().unwrap();

    value.outbound[0].destination = Destination::Kafka { provider: "client-selected".into(), topic: "aip.billing".into() };
    assert_eq!(value.validate(), Err(RelayError::UnknownProvider));

    let mut value = config();
    value.outbound.push(value.outbound[0].clone());
    assert_eq!(value.validate(), Err(RelayError::DuplicateRoute));
}

#[test]
fn route_validation_rejects_client_influenced_or_invalid_broker_addresses() {
    let mut value = config();
    if let Destination::Kafka { topic, .. } = &mut value.outbound[0].destination {
        *topic = "../other-tenant".into();
    }
    assert_eq!(value.validate(), Err(RelayError::InvalidConfiguration));

    let mut value = config();
    if let ProviderConfig::Kafka(provider) = &mut value.providers[0] {
        provider.bootstrap_servers = vec!["".into()];
    }
    assert_eq!(value.validate(), Err(RelayError::InvalidConfiguration));
}

#[test]
fn inbound_route_injects_owner_and_queue_from_server_config() {
    let route = &config().inbound[0];
    let imported =
        aip_execution::brokers::imported_message(route, "partner-offset-12", json!({"owner":"attacker", "queue":"admin", "value":7})).unwrap();
    assert_eq!(imported.owner, "issuer|tenant-a");
    assert_eq!(imported.queue, "partner-events");
    assert_eq!(imported.key, "partner-offset-12");
    assert_eq!(imported.payload["owner"], "attacker");
}

#[test]
fn external_transport_config_has_no_accidental_secret_debug_output() {
    let debug = format!("{:?}", kafka());
    assert!(!debug.contains("secret-for-test"));
}

#[test]
fn relay_batch_limit_is_bounded_before_any_claim() {
    assert_eq!(aip_execution::brokers::validate_batch_limit(100), Ok(()));
    assert_eq!(aip_execution::brokers::validate_batch_limit(101), Err(RelayError::InvalidConfiguration));
    assert_eq!(aip_execution::brokers::validate_batch_limit(0), Err(RelayError::InvalidConfiguration));
}

#[test]
fn dead_letter_cannot_feed_the_same_kafka_source_and_import_keys_fit_the_store() {
    let mut value = config();
    if let ImportSource::Kafka { topic, dead_letter_topic, .. } = &mut value.inbound[0].source {
        *dead_letter_topic = topic.clone();
    }
    assert_eq!(value.validate(), Err(RelayError::InvalidConfiguration));
    assert!(aip_execution::brokers::imported_message(&config().inbound[0], &"k".repeat(257), json!({})).is_err());
}
