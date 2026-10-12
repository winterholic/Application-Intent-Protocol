//! Optional Kafka and RabbitMQ transport for the PostgreSQL-backed message outbox.
//!
//! This module is available only with `external-brokers`. Messages remain in
//! PostgreSQL until a transport confirms publication and the matching lease is
//! acknowledged. A crash after remote publication can publish the stable message
//! id again, so consumers must deduplicate by that id.

use crate::messages::{MessageLease, MessageStore, NewMessage};
use lapin::{
    BasicProperties, Channel, Confirmation, Connection, ConnectionProperties, Consumer,
    options::{BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicPublishOptions, BasicQosOptions, ConfirmSelectOptions},
    types::FieldTable,
};
use rdkafka::{
    ClientConfig,
    consumer::{CommitMode, Consumer as KafkaConsumer, StreamConsumer},
    message::{Message as KafkaMessage, OwnedHeaders},
    producer::{FutureProducer, FutureRecord, Producer},
    util::Timeout,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fmt, time::Duration};
use tokio::net::lookup_host;
use tokio_postgres::{Client, GenericClient};

const MAX_ROUTES: usize = 256;
const MAX_BATCH: usize = 100;
const MAX_INBOUND_TTL_MS: i64 = 365 * 24 * 60 * 60 * 1000;
const MAX_INBOUND_PAYLOAD_BYTES: usize = 512 << 10;
const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 30_000;
const MAX_ENVELOPE_BYTES: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayError {
    InvalidConfiguration,
    DuplicateRoute,
    UnknownProvider,
    RouteNotAllowed,
    InvalidLease,
    MessageStore,
    BrokerUnavailable,
    PublishUnconfirmed,
    ImportRejected,
    OffsetCommitUncertain,
}

#[derive(Clone)]
pub enum ProviderConfig {
    Kafka(KafkaConfig),
    RabbitMq(RabbitMqConfig),
}

impl ProviderConfig {
    fn name(&self) -> &str {
        match self {
            Self::Kafka(config) => &config.name,
            Self::RabbitMq(config) => &config.name,
        }
    }
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kafka(config) => f.debug_tuple("Kafka").field(config).finish(),
            Self::RabbitMq(config) => f.debug_tuple("RabbitMq").field(config).finish(),
        }
    }
}

#[derive(Clone)]
pub struct KafkaConfig {
    pub name: String,
    pub bootstrap_servers: Vec<String>,
    pub security: KafkaSecurity,
    pub timeout_ms: u64,
    /// Private and loopback destinations remain denied until a server operator opts in.
    pub allow_private_networks: bool,
}

impl fmt::Debug for KafkaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KafkaConfig")
            .field("name", &self.name)
            .field("bootstrap_servers", &self.bootstrap_servers)
            .field("security", &self.security)
            .field("timeout_ms", &self.timeout_ms)
            .field("allow_private_networks", &self.allow_private_networks)
            .finish()
    }
}

#[derive(Clone)]
pub enum KafkaSecurity {
    Tls {
        ca_location: Option<String>,
    },
    SaslTls {
        mechanism: String,
        username: String,
        password: String,
        ca_location: Option<String>,
    },
    /// Plaintext is restricted to explicit local or private deployments.
    Plaintext {
        allow_insecure_transport: bool,
    },
}

impl fmt::Debug for KafkaSecurity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tls { ca_location } => f.debug_struct("Tls").field("ca_location", ca_location).finish(),
            Self::SaslTls { mechanism, username, ca_location, .. } => f
                .debug_struct("SaslTls")
                .field("mechanism", mechanism)
                .field("username", username)
                .field("password", &"<redacted>")
                .field("ca_location", ca_location)
                .finish(),
            Self::Plaintext { allow_insecure_transport } => {
                f.debug_struct("Plaintext").field("allow_insecure_transport", allow_insecure_transport).finish()
            }
        }
    }
}

#[derive(Clone)]
pub struct RabbitMqConfig {
    pub name: String,
    /// `amqps://` is required unless both insecure transport and private networking are explicitly enabled.
    pub uri: String,
    pub timeout_ms: u64,
    pub allow_private_networks: bool,
    pub allow_insecure_transport: bool,
}

impl fmt::Debug for RabbitMqConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RabbitMqConfig")
            .field("name", &self.name)
            .field("uri", &"<redacted>")
            .field("timeout_ms", &self.timeout_ms)
            .field("allow_private_networks", &self.allow_private_networks)
            .field("allow_insecure_transport", &self.allow_insecure_transport)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Kafka { provider: String, topic: String },
    RabbitMq { provider: String, exchange: String, routing_key: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboundRoute {
    pub owner: String,
    pub queue: String,
    pub destination: Destination,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportSource {
    Kafka { topic: String, group_id: String, dead_letter_topic: String },
    RabbitMq { queue_name: String, dead_letter_exchange: String, dead_letter_routing_key: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboundRoute {
    pub provider: String,
    pub source: ImportSource,
    pub owner: String,
    pub queue: String,
    pub max_deliveries: i32,
    pub ttl_ms: i64,
}

#[derive(Clone, Debug)]
pub struct BrokerConfig {
    pub providers: Vec<ProviderConfig>,
    pub outbound: Vec<OutboundRoute>,
    pub inbound: Vec<InboundRoute>,
}

impl BrokerConfig {
    pub fn validate(&self) -> Result<(), RelayError> {
        if self.providers.len() > 32 || self.outbound.len() > MAX_ROUTES || self.inbound.len() > MAX_ROUTES {
            return Err(RelayError::InvalidConfiguration);
        }
        let mut providers = HashMap::new();
        for provider in &self.providers {
            if !valid_name(provider.name()) || providers.insert(provider.name(), provider).is_some() {
                return Err(RelayError::InvalidConfiguration);
            }
            match provider {
                ProviderConfig::Kafka(config) => validate_kafka_config(config)?,
                ProviderConfig::RabbitMq(config) => validate_rabbit_config(config)?,
            }
        }
        let mut scopes = std::collections::HashSet::new();
        for route in &self.outbound {
            if !valid_scope(&route.owner, &route.queue) || !scopes.insert((route.owner.as_str(), route.queue.as_str())) {
                return Err(RelayError::DuplicateRoute);
            }
            let provider_name = match &route.destination {
                Destination::Kafka { provider, topic } => {
                    if !valid_kafka_topic(topic) {
                        return Err(RelayError::InvalidConfiguration);
                    }
                    require_provider_kind(&providers, provider, true)?;
                    provider
                }
                Destination::RabbitMq { provider, exchange, routing_key } => {
                    if !valid_broker_name(exchange, 255, true) || !valid_broker_name(routing_key, 255, false) {
                        return Err(RelayError::InvalidConfiguration);
                    }
                    require_provider_kind(&providers, provider, false)?;
                    provider
                }
            };
            let _ = provider_name;
        }
        let mut inbound_scopes = std::collections::HashSet::new();
        let mut inbound_sources = std::collections::HashSet::new();
        for route in &self.inbound {
            if !valid_scope(&route.owner, &route.queue)
                || !inbound_scopes.insert((route.owner.as_str(), route.queue.as_str()))
                || !(1..=100).contains(&route.max_deliveries)
                || !(1..=MAX_INBOUND_TTL_MS).contains(&route.ttl_ms)
            {
                return Err(RelayError::InvalidConfiguration);
            }
            let (kafka, source_key) = match &route.source {
                ImportSource::Kafka { topic, group_id, dead_letter_topic } => {
                    if !valid_kafka_topic(topic) || !valid_name(group_id) || !valid_kafka_topic(dead_letter_topic) || topic == dead_letter_topic {
                        return Err(RelayError::InvalidConfiguration);
                    }
                    (true, format!("{}:kafka:{topic}:{group_id}", route.provider))
                }
                ImportSource::RabbitMq { queue_name, dead_letter_exchange, dead_letter_routing_key } => {
                    if !valid_broker_name(queue_name, 255, false)
                        || !valid_broker_name(dead_letter_exchange, 255, true)
                        || !valid_broker_name(dead_letter_routing_key, 255, false)
                        || (dead_letter_exchange.is_empty() && dead_letter_routing_key == queue_name)
                    {
                        return Err(RelayError::InvalidConfiguration);
                    }
                    (false, format!("{}:rabbit:{queue_name}", route.provider))
                }
            };
            if !inbound_sources.insert(source_key) {
                return Err(RelayError::DuplicateRoute);
            }
            require_provider_kind(&providers, &route.provider, kafka)?;
        }
        Ok(())
    }
}

fn validate_kafka_config(config: &KafkaConfig) -> Result<(), RelayError> {
    if config.bootstrap_servers.is_empty() || config.bootstrap_servers.len() > 32 || !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&config.timeout_ms) {
        return Err(RelayError::InvalidConfiguration);
    }
    for address in &config.bootstrap_servers {
        let (host, port) = parse_host_port(address).ok_or(RelayError::InvalidConfiguration)?;
        if port == 0 || host.is_empty() || host.contains('@') || host.contains('/') || host.contains(char::from(92)) || has_control(host) {
            return Err(RelayError::InvalidConfiguration);
        }
        if !config.allow_private_networks && host_is_private_literal(host) {
            return Err(RelayError::InvalidConfiguration);
        }
    }
    match &config.security {
        KafkaSecurity::Tls { ca_location } => validate_ca_location(ca_location.as_deref())?,
        KafkaSecurity::SaslTls { mechanism, username, password, ca_location } => {
            if !matches!(mechanism.as_str(), "PLAIN" | "SCRAM-SHA-256" | "SCRAM-SHA-512")
                || username.is_empty()
                || username.len() > 256
                || username.chars().any(char::is_control)
                || password.is_empty()
                || password.len() > 4096
                || password.chars().any(char::is_control)
            {
                return Err(RelayError::InvalidConfiguration);
            }
            validate_ca_location(ca_location.as_deref())?;
        }
        KafkaSecurity::Plaintext { allow_insecure_transport } => {
            if !allow_insecure_transport || !config.allow_private_networks {
                return Err(RelayError::InvalidConfiguration);
            }
        }
    }
    Ok(())
}

fn validate_rabbit_config(config: &RabbitMqConfig) -> Result<(), RelayError> {
    if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&config.timeout_ms) || has_control(&config.uri) || config.uri.contains('#') {
        return Err(RelayError::InvalidConfiguration);
    }
    let (host, port, tls) = parse_amqp_endpoint(&config.uri).ok_or(RelayError::InvalidConfiguration)?;
    if !tls && (!config.allow_insecure_transport || !config.allow_private_networks) {
        return Err(RelayError::InvalidConfiguration);
    }
    if !config.allow_private_networks && host_is_private_literal(host) {
        return Err(RelayError::InvalidConfiguration);
    }
    if port == 0 || host.is_empty() {
        return Err(RelayError::InvalidConfiguration);
    }
    Ok(())
}

fn validate_ca_location(path: Option<&str>) -> Result<(), RelayError> {
    if path.is_some_and(|path| path.is_empty() || path.len() > 4096 || has_control(path)) {
        return Err(RelayError::InvalidConfiguration);
    }
    Ok(())
}

fn require_provider_kind<'a>(providers: &'a HashMap<&str, &ProviderConfig>, name: &str, kafka: bool) -> Result<&'a ProviderConfig, RelayError> {
    let provider = providers.get(name).copied().ok_or(RelayError::UnknownProvider)?;
    match (kafka, provider) {
        (true, ProviderConfig::Kafka(_)) | (false, ProviderConfig::RabbitMq(_)) => Ok(provider),
        _ => Err(RelayError::InvalidConfiguration),
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn valid_scope(owner: &str, queue: &str) -> bool {
    !owner.is_empty() && owner.len() <= 512 && !has_control(owner) && valid_name(queue)
}

fn valid_broker_name(value: &str, max: usize, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty()) && value.len() <= max && !has_control(value)
}

fn valid_kafka_topic(topic: &str) -> bool {
    !topic.is_empty()
        && topic.len() <= 249
        && topic != "."
        && topic != ".."
        && topic.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn has_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn parse_host_port(value: &str) -> Option<(&str, u16)> {
    if let Some(rest) = value.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':')?.parse().ok()?;
        Some((host, port))
    } else {
        let (host, port) = value.rsplit_once(':')?;
        if host.contains(':') {
            return None;
        }
        Some((host, port.parse().ok()?))
    }
}

fn parse_amqp_endpoint(uri: &str) -> Option<(&str, u16, bool)> {
    let (rest, tls, default_port) =
        if let Some(rest) = uri.strip_prefix("amqps://") { (rest, true, 5671) } else { (uri.strip_prefix("amqp://")?, false, 5672) };
    let authority = rest.split('/').next()?;
    let host_part = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    if let Some(rest) = host_part.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = if tail.is_empty() { default_port } else { tail.strip_prefix(':')?.parse().ok()? };
        Some((host, port, tls))
    } else if let Some((host, port)) = host_part.rsplit_once(':') {
        Some((host, port.parse().ok()?, tls))
    } else {
        Some((host_part, default_port, tls))
    }
}

fn host_is_private_literal(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
        std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_multicast(),
        std::net::IpAddr::V6(ip) => {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local() || ip.is_unspecified() || ip.is_multicast()
        }
    }) || host.eq_ignore_ascii_case("localhost")
}

fn is_public_address(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast()
                || a == 0
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192 && b == 0 && (c == 0 || c == 2))
                || (a == 198 && (b == 18 || b == 19 || b == 51 && c == 100))
                || (a == 203 && b == 0 && c == 113)
                || a >= 240)
        }
        std::net::IpAddr::V6(ip) => {
            let segments = ip.segments();
            !(ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.to_ipv4_mapped().is_some_and(|v4| !is_public_address(v4.into()))
                || (segments[0] == 0x2001 && segments[1] == 0x0db8))
        }
    }
}

async fn verify_configured_endpoints(config: &BrokerConfig) -> Result<(), RelayError> {
    for provider in &config.providers {
        let (addresses, allow_private, require_private) = match provider {
            ProviderConfig::Kafka(config) => (
                config
                    .bootstrap_servers
                    .iter()
                    .map(|server| parse_host_port(server).ok_or(RelayError::InvalidConfiguration).map(|(host, port)| (host.to_owned(), port)))
                    .collect::<Result<Vec<_>, _>>()?,
                config.allow_private_networks,
                matches!(&config.security, KafkaSecurity::Plaintext { .. }),
            ),
            ProviderConfig::RabbitMq(config) => {
                let (host, port, tls) = parse_amqp_endpoint(&config.uri).ok_or(RelayError::InvalidConfiguration)?;
                (vec![(host.to_owned(), port)], config.allow_private_networks, !tls)
            }
        };
        if allow_private && !require_private {
            continue;
        }
        for (host, port) in addresses {
            let resolved = lookup_host((host.as_str(), port)).await.map_err(|_| RelayError::BrokerUnavailable)?;
            let mut any = false;
            for address in resolved {
                any = true;
                let ip = address.ip();
                if (require_private && is_public_address(ip)) || (!allow_private && !is_public_address(ip)) {
                    return Err(RelayError::InvalidConfiguration);
                }
            }
            if !any {
                return Err(RelayError::BrokerUnavailable);
            }
        }
    }
    Ok(())
}

pub fn validate_batch_limit(limit: usize) -> Result<(), RelayError> {
    (1..=MAX_BATCH).contains(&limit).then_some(()).ok_or(RelayError::InvalidConfiguration)
}

pub fn imported_message(route: &InboundRoute, key: &str, payload: Value) -> Result<NewMessage, RelayError> {
    if key.is_empty() || key.len() > 256 || has_control(key) {
        return Err(RelayError::ImportRejected);
    }
    let body = serde_json::to_vec(&payload).map_err(|_| RelayError::ImportRejected)?;
    if body.len() > MAX_INBOUND_PAYLOAD_BYTES {
        return Err(RelayError::ImportRejected);
    }
    Ok(NewMessage {
        owner: route.owner.clone(),
        queue: route.queue.clone(),
        key: key.to_owned(),
        payload,
        max_deliveries: route.max_deliveries,
        ttl_ms: route.ttl_ms,
        delay_ms: 0,
    })
}

fn envelope(lease: &MessageLease) -> Result<Vec<u8>, RelayError> {
    let bytes = serde_json::to_vec(&json!({
        "id": lease.id,
        "owner": lease.owner,
        "queue": lease.queue,
        "payload": lease.payload,
    }))
    .map_err(|_| RelayError::InvalidConfiguration)?;
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(RelayError::InvalidConfiguration);
    }
    Ok(bytes)
}

enum ConnectedProvider {
    Kafka { producer: FutureProducer, timeout: Duration },
    RabbitMq { _connection: Box<Connection>, channel: Channel, timeout: Duration },
}

enum InboundConsumer {
    Kafka { consumer: StreamConsumer, route: InboundRoute, timeout: Duration },
    RabbitMq { _connection: Box<Connection>, consumer: Consumer, route: InboundRoute, timeout: Duration },
}

pub struct Relay {
    providers: HashMap<String, ConnectedProvider>,
    outbound: HashMap<(String, String), OutboundRoute>,
    inbound: HashMap<(String, String), InboundConsumer>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayOutcome {
    Empty,
    Published { message_id: String },
    PublishedAfterLeaseExpired { message_id: String },
    RetryScheduled { message_id: String, failure: RelayError, lease_released: bool },
}

impl Relay {
    /// Connects only to server-configured endpoints and subscribes to configured import routes.
    pub async fn connect(config: BrokerConfig) -> Result<Self, RelayError> {
        config.validate()?;
        verify_configured_endpoints(&config).await?;
        let mut providers = HashMap::new();
        for provider in &config.providers {
            let connected = match provider {
                ProviderConfig::Kafka(config) => {
                    let mut kafka = kafka_client_config(config);
                    kafka
                        .set("client.id", format!("aip-outbox-{}", config.name))
                        .set("enable.idempotence", "true")
                        .set("acks", "all")
                        .set("delivery.timeout.ms", config.timeout_ms.to_string())
                        .set("message.max.bytes", MAX_ENVELOPE_BYTES.to_string())
                        .set("queue.buffering.max.kbytes", "32768");
                    let producer = kafka.create::<FutureProducer>().map_err(|_| RelayError::BrokerUnavailable)?;
                    let probe = producer.clone();
                    let timeout = Duration::from_millis(config.timeout_ms);
                    tokio::task::spawn_blocking(move || probe.client().fetch_metadata(None, Timeout::After(timeout)))
                        .await
                        .map_err(|_| RelayError::BrokerUnavailable)?
                        .map_err(|_| RelayError::BrokerUnavailable)?;
                    ConnectedProvider::Kafka { producer, timeout: Duration::from_millis(config.timeout_ms) }
                }
                ProviderConfig::RabbitMq(config) => {
                    let timeout = Duration::from_millis(config.timeout_ms);
                    let connection = tokio::time::timeout(timeout, Connection::connect(&config.uri, ConnectionProperties::default()))
                        .await
                        .map_err(|_| RelayError::BrokerUnavailable)?
                        .map_err(|_| RelayError::BrokerUnavailable)?;
                    let channel = tokio::time::timeout(timeout, connection.create_channel())
                        .await
                        .map_err(|_| RelayError::BrokerUnavailable)?
                        .map_err(|_| RelayError::BrokerUnavailable)?;
                    tokio::time::timeout(timeout, channel.confirm_select(ConfirmSelectOptions::default()))
                        .await
                        .map_err(|_| RelayError::BrokerUnavailable)?
                        .map_err(|_| RelayError::BrokerUnavailable)?;
                    ConnectedProvider::RabbitMq { _connection: Box::new(connection), channel, timeout }
                }
            };
            providers.insert(provider.name().to_owned(), connected);
        }

        let providers_config: HashMap<_, _> = config.providers.iter().map(|provider| (provider.name(), provider)).collect();
        let mut inbound = HashMap::new();
        for route in &config.inbound {
            let key = (route.owner.clone(), route.queue.clone());
            let provider_config = providers_config.get(route.provider.as_str()).ok_or(RelayError::UnknownProvider)?;
            let consumer = match (provider_config, &route.source) {
                (ProviderConfig::Kafka(kafka), ImportSource::Kafka { topic, group_id, .. }) => {
                    let mut client = kafka_client_config(kafka);
                    client
                        .set("group.id", group_id)
                        .set("enable.auto.commit", "false")
                        .set("enable.auto.offset.store", "false")
                        .set("isolation.level", "read_committed")
                        .set("auto.offset.reset", "earliest")
                        .set("allow.auto.create.topics", "false")
                        .set("max.poll.interval.ms", "30000")
                        .set("fetch.max.bytes", "2097152")
                        .set("max.partition.fetch.bytes", MAX_ENVELOPE_BYTES.to_string())
                        .set("queued.max.messages.kbytes", "32768");
                    let consumer = client.create::<StreamConsumer>().map_err(|_| RelayError::BrokerUnavailable)?;
                    consumer.subscribe(&[topic]).map_err(|_| RelayError::BrokerUnavailable)?;
                    InboundConsumer::Kafka { consumer, route: route.clone(), timeout: Duration::from_millis(kafka.timeout_ms) }
                }
                (ProviderConfig::RabbitMq(rabbit), ImportSource::RabbitMq { queue_name, .. }) => {
                    let timeout = Duration::from_millis(rabbit.timeout_ms);
                    let connection = tokio::time::timeout(timeout, Connection::connect(&rabbit.uri, ConnectionProperties::default()))
                        .await
                        .map_err(|_| RelayError::BrokerUnavailable)?
                        .map_err(|_| RelayError::BrokerUnavailable)?;
                    let channel = tokio::time::timeout(timeout, connection.create_channel())
                        .await
                        .map_err(|_| RelayError::BrokerUnavailable)?
                        .map_err(|_| RelayError::BrokerUnavailable)?;
                    tokio::time::timeout(timeout, channel.basic_qos(1, BasicQosOptions::default()))
                        .await
                        .map_err(|_| RelayError::BrokerUnavailable)?
                        .map_err(|_| RelayError::BrokerUnavailable)?;
                    let consumer = tokio::time::timeout(
                        timeout,
                        channel.basic_consume(queue_name.clone().into(), "aip-import".into(), BasicConsumeOptions::default(), FieldTable::default()),
                    )
                    .await
                    .map_err(|_| RelayError::BrokerUnavailable)?
                    .map_err(|_| RelayError::BrokerUnavailable)?;
                    InboundConsumer::RabbitMq { _connection: Box::new(connection), consumer, route: route.clone(), timeout }
                }
                _ => return Err(RelayError::InvalidConfiguration),
            };
            if inbound.insert(key, consumer).is_some() {
                return Err(RelayError::DuplicateRoute);
            }
        }

        let outbound = config.outbound.into_iter().map(|route| ((route.owner.clone(), route.queue.clone()), route)).collect();
        Ok(Self { providers, outbound, inbound })
    }

    /// Publishes one leased PostgreSQL row. A successful remote confirmation is
    /// followed by the store's token-and-epoch CAS acknowledgement.
    pub async fn relay_once(
        &self,
        db: &impl GenericClient,
        store: &MessageStore,
        owner: &str,
        queue: &str,
        worker: &str,
        lease_ms: i64,
    ) -> Result<RelayOutcome, RelayError> {
        let route = self.outbound.get(&(owner.to_owned(), queue.to_owned())).ok_or(RelayError::RouteNotAllowed)?;
        let provider_name = match &route.destination {
            Destination::Kafka { provider, .. } | Destination::RabbitMq { provider, .. } => provider,
        };
        let provider = self.providers.get(provider_name).ok_or(RelayError::UnknownProvider)?;
        let timeout = match provider {
            ConnectedProvider::Kafka { timeout, .. } | ConnectedProvider::RabbitMq { timeout, .. } => *timeout,
        };
        if lease_ms < i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX).saturating_add(500) {
            return Err(RelayError::InvalidLease);
        }
        let Some(lease) = store.claim(db, owner, queue, worker, lease_ms).await.map_err(|_| RelayError::MessageStore)? else {
            return Ok(RelayOutcome::Empty);
        };
        let publication = self.publish(provider, &route.destination, &lease).await;
        match publication {
            Ok(()) => {
                if store.ack(db, &lease).await.map_err(|_| RelayError::MessageStore)? {
                    Ok(RelayOutcome::Published { message_id: lease.id })
                } else {
                    Ok(RelayOutcome::PublishedAfterLeaseExpired { message_id: lease.id })
                }
            }
            Err(failure) => {
                let delay = relay_backoff_ms(lease.deliveries);
                let lease_released = store.retry_transport_after(db, &lease, delay).await.map_err(|_| RelayError::MessageStore)?;
                Ok(RelayOutcome::RetryScheduled { message_id: lease.id, failure, lease_released })
            }
        }
    }

    /// Processes up to 100 messages sequentially, bounding memory and remote in-flight work.
    #[allow(clippy::too_many_arguments)]
    pub async fn relay_batch(
        &self,
        db: &impl GenericClient,
        store: &MessageStore,
        owner: &str,
        queue: &str,
        worker: &str,
        lease_ms: i64,
        limit: usize,
    ) -> Result<Vec<RelayOutcome>, RelayError> {
        validate_batch_limit(limit)?;
        let mut results = Vec::with_capacity(limit);
        for _ in 0..limit {
            let result = self.relay_once(db, store, owner, queue, worker, lease_ms).await?;
            if result == RelayOutcome::Empty {
                break;
            }
            let unavailable = matches!(result, RelayOutcome::RetryScheduled { .. });
            results.push(result);
            if unavailable {
                break;
            }
        }
        Ok(results)
    }

    /// Commits a Kafka offset only after the AIP message insert transaction commits.
    /// Provider/topic/partition/offset identify a record across retries without conflating equal application keys.
    pub async fn import_once(&mut self, db: &mut Client, store: &MessageStore, owner: &str, queue: &str) -> Result<bool, RelayError> {
        self.import_once_with_wait(db, store, owner, queue, Duration::from_secs(30)).await
    }

    /// Poll waiting is separate from publish/ACK timeouts so empty routes cannot stall a shared relay.
    pub async fn import_once_with_wait(
        &mut self,
        db: &mut Client,
        store: &MessageStore,
        owner: &str,
        queue: &str,
        wait: Duration,
    ) -> Result<bool, RelayError> {
        if wait.is_zero() || wait > Duration::from_secs(30) {
            return Err(RelayError::InvalidConfiguration);
        }
        let key = (owner.to_owned(), queue.to_owned());
        let (inbound, providers) = (&mut self.inbound, &self.providers);
        let importer = inbound.get_mut(&key).ok_or(RelayError::RouteNotAllowed)?;
        match importer {
            InboundConsumer::Kafka { consumer, route, timeout } => {
                let message = match tokio::time::timeout(wait.min(*timeout), consumer.recv()).await {
                    Ok(Ok(message)) => message,
                    Ok(Err(_)) => return Err(RelayError::BrokerUnavailable),
                    Err(_) => return Ok(false),
                };
                let topic = message.topic().to_owned();
                let offset = message.offset();
                let partition = message.partition();
                let key = format!(
                    "kafka:{:x}",
                    Sha256::digest(serde_json::to_vec(&json!([route.provider, topic, partition, offset])).map_err(|_| RelayError::ImportRejected)?)
                );
                let raw = message.payload().unwrap_or_default();
                let new = (raw.len() <= MAX_INBOUND_PAYLOAD_BYTES)
                    .then(|| serde_json::from_slice(raw).ok())
                    .flatten()
                    .and_then(|payload| imported_message(route, &key, payload).ok());
                let rejected = match new {
                    Some(new) => match insert_in_transaction(db, store, new).await {
                        Ok(()) => false,
                        Err(RelayError::ImportRejected) => true,
                        Err(error) => return Err(error),
                    },
                    None => true,
                };
                if rejected {
                    publish_quarantine(providers, route, &key, raw).await?;
                }
                consumer.commit_message(&message, CommitMode::Sync).map_err(|_| RelayError::OffsetCommitUncertain)?;
                Ok(true)
            }
            InboundConsumer::RabbitMq { consumer, route, timeout, .. } => {
                use futures_util::StreamExt;
                let delivery = match tokio::time::timeout(wait.min(*timeout), consumer.next()).await {
                    Err(_) => return Ok(false),
                    Ok(None) => return Err(RelayError::BrokerUnavailable),
                    Ok(Some(Err(_))) => return Err(RelayError::BrokerUnavailable),
                    Ok(Some(Ok(delivery))) => delivery,
                };
                let source_key = delivery.properties.message_id().as_ref().map(ToString::to_string).filter(|value| !value.is_empty());
                let identity = source_key.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let key = format!(
                    "rabbit:{:x}",
                    Sha256::digest(serde_json::to_vec(&json!([route.provider, identity])).map_err(|_| RelayError::ImportRejected)?)
                );
                let new = (source_key.is_some() && delivery.data.len() <= MAX_INBOUND_PAYLOAD_BYTES)
                    .then(|| serde_json::from_slice(&delivery.data).ok())
                    .flatten()
                    .and_then(|payload| imported_message(route, &key, payload).ok());
                let rejected = match new {
                    Some(new) => match insert_in_transaction(db, store, new).await {
                        Ok(()) => false,
                        Err(RelayError::ImportRejected) => true,
                        Err(error) => {
                            delivery
                                .nack(BasicNackOptions { requeue: true, ..BasicNackOptions::default() })
                                .await
                                .map_err(|_| RelayError::OffsetCommitUncertain)?;
                            return Err(error);
                        }
                    },
                    None => true,
                };
                if rejected {
                    publish_quarantine(providers, route, &key, &delivery.data).await?;
                }
                delivery.ack(BasicAckOptions::default()).await.map_err(|_| RelayError::OffsetCommitUncertain)?;
                Ok(true)
            }
        }
    }

    async fn publish(&self, provider: &ConnectedProvider, target: &Destination, lease: &MessageLease) -> Result<(), RelayError> {
        let body = envelope(lease)?;
        match (provider, target) {
            (ConnectedProvider::Kafka { producer, timeout }, Destination::Kafka { topic, .. }) => {
                publish_kafka(producer, topic, &lease.id, &body, *timeout).await
            }
            (ConnectedProvider::RabbitMq { channel, timeout, .. }, Destination::RabbitMq { exchange, routing_key, .. }) => {
                publish_rabbit(channel, exchange, routing_key, &lease.id, &body, "application/json", *timeout).await
            }
            _ => Err(RelayError::InvalidConfiguration),
        }
    }
}

fn quarantine_envelope(raw: &[u8]) -> Result<Vec<u8>, RelayError> {
    let preview_length = raw.len().min(4096);
    let preview: String = raw[..preview_length].iter().map(|byte| format!("{byte:02x}")).collect();
    serde_json::to_vec(&json!({
        "version":1, "error":"IMPORT_REJECTED", "originalBytes":raw.len(),
        "sha256":format!("{:x}",Sha256::digest(raw)), "previewHex":preview,
        "truncated":raw.len()>preview_length
    }))
    .map_err(|_| RelayError::ImportRejected)
}

async fn publish_quarantine(providers: &HashMap<String, ConnectedProvider>, route: &InboundRoute, key: &str, raw: &[u8]) -> Result<(), RelayError> {
    let body = quarantine_envelope(raw)?;
    let identity = format!("dead:{:x}", Sha256::digest([key.as_bytes(), body.as_slice()].concat()));
    match (providers.get(&route.provider), &route.source) {
        (Some(ConnectedProvider::Kafka { producer, timeout }), ImportSource::Kafka { dead_letter_topic, .. }) => {
            publish_kafka(producer, dead_letter_topic, &identity, &body, *timeout).await
        }
        (
            Some(ConnectedProvider::RabbitMq { channel, timeout, .. }),
            ImportSource::RabbitMq { dead_letter_exchange, dead_letter_routing_key, .. },
        ) => publish_rabbit(channel, dead_letter_exchange, dead_letter_routing_key, &identity, &body, "application/json", *timeout).await,
        _ => Err(RelayError::InvalidConfiguration),
    }
}

async fn insert_in_transaction(db: &mut Client, store: &MessageStore, message: NewMessage) -> Result<(), RelayError> {
    let transaction = db.transaction().await.map_err(|_| RelayError::MessageStore)?;
    store.enqueue(&transaction, message).await.map_err(|error| match error {
        crate::messages::Error::IdempotencyConflict => RelayError::ImportRejected,
        _ => RelayError::MessageStore,
    })?;
    transaction.commit().await.map_err(|_| RelayError::MessageStore)
}

async fn publish_kafka(producer: &FutureProducer, topic: &str, key: &str, body: &[u8], timeout: Duration) -> Result<(), RelayError> {
    let headers = OwnedHeaders::new().insert(rdkafka::message::Header { key: "aip-message-id", value: Some(key.as_bytes()) });
    let record = FutureRecord::to(topic).key(key).payload(body).headers(headers);
    match tokio::time::timeout(timeout, producer.send(record, Timeout::After(timeout))).await {
        Ok(Ok(_)) => Ok(()),
        _ => Err(RelayError::PublishUnconfirmed),
    }
}

async fn publish_rabbit(
    channel: &Channel,
    exchange: &str,
    routing_key: &str,
    message_id: &str,
    body: &[u8],
    content_type: &str,
    timeout: Duration,
) -> Result<(), RelayError> {
    let properties = BasicProperties::default()
        .with_delivery_mode(2)
        .with_content_type(content_type.to_owned().into())
        .with_message_id(message_id.to_owned().into());
    let confirmation = tokio::time::timeout(timeout, async {
        channel
            .basic_publish(
                exchange.to_owned().into(),
                routing_key.to_owned().into(),
                BasicPublishOptions { mandatory: true, ..BasicPublishOptions::default() },
                body,
                properties,
            )
            .await
            .map_err(|_| RelayError::BrokerUnavailable)?
            .await
            .map_err(|_| RelayError::BrokerUnavailable)
    })
    .await
    .map_err(|_| RelayError::PublishUnconfirmed)??;
    match confirmation {
        Confirmation::Ack(None) => Ok(()),
        Confirmation::Ack(Some(_)) | Confirmation::Nack(_) | Confirmation::NotRequested => Err(RelayError::PublishUnconfirmed),
    }
}

fn relay_backoff_ms(deliveries: i32) -> i64 {
    let exponent = deliveries.saturating_sub(1).clamp(0, 10) as u32;
    250_i64.saturating_mul(1_i64 << exponent).min(30_000)
}

fn kafka_client_config(config: &KafkaConfig) -> ClientConfig {
    let mut client = ClientConfig::new();
    client
        .set("bootstrap.servers", config.bootstrap_servers.join(","))
        .set(
            "security.protocol",
            match &config.security {
                KafkaSecurity::Tls { .. } => "ssl",
                KafkaSecurity::SaslTls { .. } => "sasl_ssl",
                KafkaSecurity::Plaintext { .. } => "plaintext",
            },
        )
        .set("enable.ssl.certificate.verification", "true")
        .set("socket.timeout.ms", config.timeout_ms.to_string())
        .set("allow.auto.create.topics", "false");
    match &config.security {
        KafkaSecurity::Tls { ca_location } => {
            if let Some(path) = ca_location {
                client.set("ssl.ca.location", path);
            }
        }
        KafkaSecurity::SaslTls { mechanism, username, password, ca_location } => {
            client.set("sasl.mechanisms", mechanism).set("sasl.username", username).set("sasl.password", password);
            if let Some(path) = ca_location {
                client.set("ssl.ca.location", path);
            }
        }
        KafkaSecurity::Plaintext { .. } => {}
    }
    client
}

#[cfg(test)]
mod quarantine_tests {
    use super::*;
    #[test]
    fn oversized_poison_messages_have_a_bounded_dead_letter_envelope() {
        let raw = vec![b'x'; 2 << 20];
        let body = quarantine_envelope(&raw).unwrap();
        assert!(body.len() < 16_384);
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["originalBytes"], raw.len());
        assert_eq!(value["truncated"], true);
        assert_eq!(value["sha256"], format!("{:x}", Sha256::digest(&raw)));
    }
}
