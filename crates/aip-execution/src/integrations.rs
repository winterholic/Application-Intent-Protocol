//! Server-configured outbound HTTP broker. Extension input selects a registered
//! name and supplies JSON only; it cannot choose a URL, method, header, or key.

use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue, RETRY_AFTER};
use reqwest::{Client, Method, Url};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MAX_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_RETRY_AFTER_SECONDS: u64 = 3600;
const BROKER_ABI: &str = "aip-http-broker-v1";

#[derive(Debug, PartialEq)]
pub enum Error {
    InvalidConfiguration,
    UnknownEndpoint,
    RequestTooLarge,
    ResponseTooLarge,
    InvalidJsonResponse,
    Transport(String),
    /// The connection failed before any HTTP request was sent. Safe to retry without reconciliation.
    NotSent,
    /// The endpoint's circuit breaker is open, so the request was not sent. Unlike a transport
    /// error this outcome is certain, so a durable caller can retry it without reconciliation.
    CircuitOpen,
    /// The OAuth token endpoint did not issue a token, so the request was not sent.
    TokenUnavailable,
    /// A subscriber URL was malformed, not HTTPS or resolved to a non-public address; nothing was sent.
    DestinationRejected,
}

const DNS_TIMEOUT: Duration = Duration::from_secs(2);

/// An outbound webhook sender: the server signs and sends events to subscriber URLs from business data.
pub struct WebhookSenderConfig {
    pub name: String,
    /// Raw signing key bytes (a `whsec_` secret already base64-decoded).
    pub secret: Vec<u8>,
    pub timeout: Duration,
    pub max_request_bytes: usize,
    /// Test/development-only escape hatch for loopback HTTP subscribers.
    pub allow_loopback: bool,
}

struct WebhookSender {
    config: WebhookSenderConfig,
}

/// `base64(HMAC-SHA256(secret, "{id}.{timestamp}.{body}"))`, the Standard Webhooks v1 signature.
pub fn standard_webhook_signature(secret: &[u8], id: &str, timestamp: u64, body: &[u8]) -> String {
    use base64::Engine;
    use hmac::Mac;
    let mut mac = <hmac::Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(format!("{id}.{timestamp}.").as_bytes());
    mac.update(body);
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// OAuth 2.0 client-credentials grant (RFC 6749 §4.4) for an endpoint, like Spring OAuth2 Client or
/// authlib. The server fetches and caches the access token; the extension never sees it.
#[derive(Clone)]
pub struct OAuthConfig {
    pub token_url: String,
    /// Vetted DNS answers for the token endpoint, pinned like the API endpoint's.
    pub resolved: Vec<SocketAddr>,
    pub client_id: String,
    pub client_secret: String,
    pub scope: Option<String>,
    /// `client_secret_basic` (HTTP Basic) when true, otherwise `client_secret_post` (form fields).
    pub basic_auth: bool,
}

// A derived Debug would print the client secret into any log line that formats the config.
impl std::fmt::Debug for OAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthConfig")
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

struct OAuthClient {
    config: OAuthConfig,
    token_url: Url,
    client: Client,
    cached: Mutex<Option<(HeaderValue, Instant)>>,
}

/// Per-endpoint circuit breaker, like Resilience4j or opossum, kept per server process.
/// Consecutive transport failures or 5xx responses open it; after `open_for` one trial request
/// is let through (half-open) and its outcome closes or reopens the circuit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CircuitBreaker {
    pub failure_threshold: u32,
    pub open_for: Duration,
}

pub const MAX_BREAKER_THRESHOLD: u32 = 1000;
pub const MAX_BREAKER_OPEN: Duration = Duration::from_secs(3600);

enum BreakerState {
    Closed {
        failures: u32,
    },
    Open {
        until: Instant,
    },
    /// A trial request is in flight. If it never reports (its future was dropped), another trial
    /// is allowed after `open_for`, so a lost trial cannot hold the circuit open forever.
    HalfOpen {
        trial_expires: Instant,
    },
}

struct Breaker {
    config: CircuitBreaker,
    state: Mutex<BreakerState>,
}

impl Breaker {
    fn admit(&self) -> bool {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match *state {
            BreakerState::Closed { .. } => true,
            BreakerState::Open { until } | BreakerState::HalfOpen { trial_expires: until } if now < until => false,
            _ => {
                *state = BreakerState::HalfOpen { trial_expires: now + self.config.open_for };
                true
            }
        }
    }

    fn record(&self, failed: bool) {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = match (&*state, failed) {
            (_, false) => BreakerState::Closed { failures: 0 },
            (BreakerState::Closed { failures }, true) if failures + 1 < self.config.failure_threshold => {
                BreakerState::Closed { failures: failures + 1 }
            }
            (_, true) => BreakerState::Open { until: Instant::now() + self.config.open_for },
        };
    }
}

/// Trusted server configuration, built at startup. `resolved` must contain the
/// DNS answers vetted by the caller; the broker pins those answers per request.
pub struct EndpointConfig {
    pub name: String,
    pub url: String,
    pub method: String,
    pub timeout: Duration,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub secret_header: Option<(String, String)>,
    pub resolved: Vec<SocketAddr>,
    /// Test/development-only escape hatch for a loopback HTTP endpoint.
    pub allow_loopback: bool,
    /// Explicitly permits private unicast and loopback destinations for this endpoint.
    pub allow_private_networks: bool,
    /// Not part of the fingerprint: it changes when requests are sent, not what a request means.
    pub circuit_breaker: Option<CircuitBreaker>,
    pub oauth: Option<OAuthConfig>,
}

struct Endpoint {
    url: Url,
    method: Method,
    timeout: Duration,
    max_request_bytes: usize,
    max_response_bytes: usize,
    secret_header: Option<(HeaderName, HeaderValue)>,
    client: Client,
    breaker: Option<Breaker>,
    oauth: Option<OAuthClient>,
}

pub struct Broker {
    endpoints: HashMap<String, Endpoint>,
    fingerprint: String,
    webhooks: HashMap<String, WebhookSender>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IntegrationResponse {
    pub status: u16,
    pub body: Value,
    pub retry_after_seconds: Option<u64>,
}

impl Broker {
    pub fn new(configs: Vec<EndpointConfig>) -> Result<Self, Error> {
        let fingerprint = fingerprint(&configs)?;
        let mut endpoints = HashMap::new();
        for config in configs {
            let endpoint = build_endpoint(config)?;
            let name = endpoint.0;
            if endpoints.insert(name, endpoint.1).is_some() {
                return Err(Error::InvalidConfiguration);
            }
        }
        Ok(Self { endpoints, fingerprint, webhooks: HashMap::new() })
    }

    /// Stable digest of the effective server-side endpoint configuration.
    /// Secret values only contribute through a one-way digest.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Checks whether an exact server-configured endpoint name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.endpoints.contains_key(name) || self.webhooks.contains_key(name)
    }

    /// Adds outbound webhook senders. Their names share the endpoint namespace (`access http.<name>`),
    /// and their configuration joins the fingerprint so a changed signing secret fences old jobs.
    pub fn with_webhooks(mut self, senders: Vec<WebhookSenderConfig>) -> Result<Self, Error> {
        let mut entries = Vec::new();
        for sender in senders {
            if sender.name.is_empty()
                || sender.name.len() > 128
                || !sender.name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
                || self.endpoints.contains_key(&sender.name)
                || sender.secret.len() < 16
                || sender.timeout.is_zero()
                || sender.timeout > MAX_TIMEOUT
                || sender.max_request_bytes == 0
                || sender.max_request_bytes > MAX_BODY_BYTES
            {
                return Err(Error::InvalidConfiguration);
            }
            let secret = Sha256::digest(&sender.secret).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            entries.push(serde_json::json!([sender.name, secret, sender.timeout.as_millis(), sender.max_request_bytes, sender.allow_loopback]));
            let name = sender.name.clone();
            if self.webhooks.insert(name, WebhookSender { config: sender }).is_some() {
                return Err(Error::InvalidConfiguration);
            }
        }
        if !entries.is_empty() {
            entries.sort_by_key(|entry| entry[0].as_str().unwrap_or_default().to_string());
            let canonical = serde_json::to_vec(&(&self.fingerprint, "aip-webhook-sender-v1", entries)).map_err(|_| Error::InvalidConfiguration)?;
            self.fingerprint = Sha256::digest(canonical).iter().map(|byte| format!("{byte:02x}")).collect();
        }
        Ok(self)
    }

    /// Whether `name` is an outbound webhook sender rather than a fixed endpoint.
    pub fn is_webhook(&self, name: &str) -> bool {
        self.webhooks.contains_key(name)
    }

    /// Delivers one signed event to a subscriber URL taken from business data, following the Standard
    /// Webhooks spec (`webhook-id`, `webhook-timestamp`, `webhook-signature`). The destination must be
    /// HTTPS on public addresses only; DNS is resolved per call and pinned for the connection, and
    /// redirects are never followed, so a subscriber URL cannot reach internal services.
    /// Receivers dedupe on `webhook-id`, so a delivery that failed in transport may be sent again.
    pub async fn deliver_webhook(&self, name: &str, url: &str, payload: &Value, id: &str) -> Result<u16, Error> {
        let sender = self.webhooks.get(name).ok_or(Error::UnknownEndpoint)?;
        let config = &sender.config;
        if url.len() > 2048 || id.is_empty() || id.len() > 256 {
            return Err(Error::DestinationRejected);
        }
        let parsed = Url::parse(url).map_err(|_| Error::DestinationRejected)?;
        let port = parsed.port_or_known_default().ok_or(Error::DestinationRejected)?;
        let resolved: Vec<SocketAddr> = match parsed.host() {
            Some(url::Host::Ipv4(ip)) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
            Some(url::Host::Ipv6(ip)) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
            Some(url::Host::Domain(domain)) => tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host((domain, port)))
                .await
                .map_err(|_| Error::DestinationRejected)?
                .map_err(|_| Error::DestinationRejected)?
                .take(32)
                .collect(),
            None => return Err(Error::DestinationRejected),
        };
        // Same rules as a configured endpoint, but private networks are never allowed for subscriber URLs.
        let destination = vet_destination(url, &resolved, config.allow_loopback, false).map_err(|_| Error::DestinationRejected)?;
        let body = serde_json::to_vec(payload).map_err(|_| Error::RequestTooLarge)?;
        if body.len() > config.max_request_bytes {
            return Err(Error::RequestTooLarge);
        }
        let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| Error::InvalidConfiguration)?.as_secs();
        let signature = standard_webhook_signature(&config.secret, id, timestamp, &body);
        let client = pinned_client(&destination, &resolved, config.timeout)?;
        let response = client
            .post(destination)
            .header(CONTENT_TYPE, "application/json")
            .header("webhook-id", id)
            .header("webhook-timestamp", timestamp.to_string())
            .header("webhook-signature", format!("v1,{signature}"))
            .body(body)
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;
        let status = response.status().as_u16();
        // The receiver's body is not part of the contract; read a bounded amount so the connection closes cleanly.
        let _ = read_bounded(response, 64 * 1024).await;
        Ok(status)
    }

    /// Performs one bounded JSON request to a server-registered endpoint.
    /// A response status is returned to the caller; transport and body-limit
    /// failures remain errors so a durable caller can record an unknown result.
    pub async fn call(&self, name: &str, input: &Value) -> Result<IntegrationResponse, Error> {
        self.call_with_key(name, input, None).await
    }

    /// Sends a stable provider idempotency key for a durable effect attempt.
    /// The raw key is never sent or returned; it is scoped to the endpoint name.
    pub async fn call_with_key(&self, name: &str, input: &Value, key: Option<&str>) -> Result<IntegrationResponse, Error> {
        let endpoint = self.endpoints.get(name).ok_or(Error::UnknownEndpoint)?;
        if key.is_some_and(|value| value.is_empty() || value.len() > 512 || value.chars().any(char::is_control)) {
            return Err(Error::InvalidConfiguration);
        }
        let body = serde_json::to_vec(input).map_err(|_| Error::RequestTooLarge)?;
        if body.len() > endpoint.max_request_bytes {
            return Err(Error::RequestTooLarge);
        }
        let mut request = endpoint
            .client
            .request(endpoint.method.clone(), endpoint.url.clone())
            .timeout(endpoint.timeout)
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        if let Some((name, value)) = &endpoint.secret_header {
            request = request.header(name, value.clone());
        }
        if let Some(key) = key {
            request = request.header("Idempotency-Key", effect_key(name, key));
        }
        if endpoint.breaker.as_ref().is_some_and(|breaker| !breaker.admit()) {
            return Err(Error::CircuitOpen);
        }
        if let Some(oauth) = &endpoint.oauth {
            request = request.header(AUTHORIZATION, oauth.bearer().await?);
        }
        let outcome = async {
            // pinned_client disables redirects, proxies and automatic retries. A connect-stage failure therefore
            // precedes the only outbound HTTP attempt; failures after connection stay uncertain.
            let response =
                request.send().await.map_err(|error| if error.is_connect() { Error::NotSent } else { Error::Transport(error.to_string()) })?;
            let status = response.status();
            let retry_after_seconds = response.headers().get(RETRY_AFTER).and_then(parse_retry_after);
            let body = read_bounded(response, endpoint.max_response_bytes).await?;
            let value = if body.is_empty() { Value::Null } else { serde_json::from_slice(&body).map_err(|_| Error::InvalidJsonResponse)? };
            Ok(IntegrationResponse { status: status.as_u16(), body: value, retry_after_seconds })
        }
        .await;
        if let Some(breaker) = &endpoint.breaker {
            // Body-shape errors mean the provider answered, so only an unreachable or failing provider counts.
            breaker.record(
                matches!(&outcome, Err(Error::Transport(_) | Error::NotSent)) || outcome.as_ref().is_ok_and(|response| response.status >= 500),
            );
        }
        // A rejected token (revoked or rotated early) is dropped so the next call fetches a new one.
        if let Some(oauth) = &endpoint.oauth
            && outcome.as_ref().is_ok_and(|response| response.status == 401)
        {
            oauth.invalidate();
        }
        outcome
    }
}

fn fingerprint(configs: &[EndpointConfig]) -> Result<String, Error> {
    let mut entries = Vec::with_capacity(configs.len());
    for config in configs {
        let mut resolved = config.resolved.iter().map(ToString::to_string).collect::<Vec<_>>();
        resolved.sort();
        let secret = config.secret_header.as_ref().map(|(name, value)| {
            let digest = Sha256::digest(value.as_bytes());
            let encoded = digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            (name.to_ascii_lowercase(), encoded)
        });
        entries.push(serde_json::json!({
            "name": config.name,
            "url": config.url,
            "method": config.method,
            "timeout_millis": config.timeout.as_millis(),
            "max_request_bytes": config.max_request_bytes,
            "max_response_bytes": config.max_response_bytes,
            "secret": secret,
            "resolved": resolved,
            "allow_loopback": config.allow_loopback,
            "allow_private_networks": config.allow_private_networks,
            // The token identity decides whose account a call acts on, so it is part of the meaning.
            "oauth": config.oauth.as_ref().map(|oauth| {
                let mut resolved = oauth.resolved.iter().map(ToString::to_string).collect::<Vec<_>>();
                resolved.sort();
                let secret = Sha256::digest(oauth.client_secret.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
                serde_json::json!({"token_url":oauth.token_url,"resolved":resolved,"client_id":oauth.client_id,"secret":secret,"scope":oauth.scope,"basic":oauth.basic_auth})
            }),
        }));
    }
    entries.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    let canonical = serde_json::to_vec(&(BROKER_ABI, entries)).map_err(|_| Error::InvalidConfiguration)?;
    let digest = Sha256::digest(canonical);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn build_endpoint(config: EndpointConfig) -> Result<(String, Endpoint), Error> {
    if config.name.is_empty()
        || config.name.len() > 128
        || !config.name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || config.timeout.is_zero()
        || config.timeout > MAX_TIMEOUT
        || config.max_request_bytes == 0
        || config.max_request_bytes > MAX_BODY_BYTES
        || config.max_response_bytes == 0
        || config.max_response_bytes > MAX_BODY_BYTES
        || config.resolved.is_empty()
    {
        return Err(Error::InvalidConfiguration);
    }
    let url = vet_destination(&config.url, &config.resolved, config.allow_loopback, config.allow_private_networks)?;
    let method = Method::from_bytes(config.method.as_bytes()).map_err(|_| Error::InvalidConfiguration)?;
    if !matches!(method, Method::GET | Method::POST | Method::PUT | Method::PATCH | Method::DELETE) {
        return Err(Error::InvalidConfiguration);
    }
    let secret_header = config
        .secret_header
        .map(|(name, value)| {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| Error::InvalidConfiguration)?;
            let lower_name = name.as_str();
            if matches!(
                lower_name,
                "content-type" | "retry-after" | "idempotency-key" | "host" | "content-length" | "transfer-encoding" | "connection"
            ) || (config.oauth.is_some() && lower_name == "authorization")
            {
                return Err(Error::InvalidConfiguration);
            }
            let mut value = HeaderValue::from_bytes(value.as_bytes()).map_err(|_| Error::InvalidConfiguration)?;
            value.set_sensitive(true);
            Ok((name, value))
        })
        .transpose()?;
    let client = pinned_client(&url, &config.resolved, config.timeout)?;
    let oauth = config
        .oauth
        .map(|oauth| {
            if oauth.client_id.is_empty()
                || oauth.client_id.len() > 512
                || oauth.client_secret.is_empty()
                || oauth.client_secret.len() > 8192
                || oauth.scope.as_ref().is_some_and(|scope| scope.is_empty() || scope.len() > 1024 || scope.chars().any(char::is_control))
            {
                return Err(Error::InvalidConfiguration);
            }
            // The token endpoint passes the same destination rules as the API it authorizes.
            let token_url = vet_destination(&oauth.token_url, &oauth.resolved, config.allow_loopback, config.allow_private_networks)?;
            Ok(OAuthClient {
                client: pinned_client(&token_url, &oauth.resolved, config.timeout)?,
                token_url,
                config: oauth,
                cached: Mutex::new(None),
            })
        })
        .transpose()?;
    let breaker = match config.circuit_breaker {
        Some(breaker)
            if breaker.failure_threshold == 0
                || breaker.failure_threshold > MAX_BREAKER_THRESHOLD
                || breaker.open_for.is_zero()
                || breaker.open_for > MAX_BREAKER_OPEN =>
        {
            return Err(Error::InvalidConfiguration);
        }
        Some(breaker) => Some(Breaker { config: breaker, state: Mutex::new(BreakerState::Closed { failures: 0 }) }),
        None => None,
    };
    Ok((
        config.name,
        Endpoint {
            url,
            method,
            timeout: config.timeout,
            max_request_bytes: config.max_request_bytes,
            max_response_bytes: config.max_response_bytes,
            secret_header,
            client,
            breaker,
            oauth,
        },
    ))
}

fn vet_destination(raw: &str, resolved: &[SocketAddr], allow_loopback: bool, allow_private_networks: bool) -> Result<Url, Error> {
    if resolved.is_empty() {
        return Err(Error::InvalidConfiguration);
    }
    let url = Url::parse(raw).map_err(|_| Error::InvalidConfiguration)?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(Error::InvalidConfiguration);
    }
    let scheme = url.scheme();
    let host = url.host_str().ok_or(Error::InvalidConfiguration)?;
    // host_str() includes brackets for IPv6; parsing it as IpAddr would skip
    // literal validation while reqwest still connects directly to that literal.
    let literal_host = host.strip_prefix('[').and_then(|value| value.strip_suffix(']')).unwrap_or(host);
    let literal = literal_host.parse::<IpAddr>().ok();
    let port = url.port_or_known_default().ok_or(Error::InvalidConfiguration)?;
    let loopback_url = literal.is_some_and(|ip| ip.is_loopback()) || host.eq_ignore_ascii_case("localhost");
    if scheme != "https" && !(allow_loopback && loopback_url && scheme == "http") {
        return Err(Error::InvalidConfiguration);
    }
    for address in resolved {
        if address.port() != port {
            return Err(Error::InvalidConfiguration);
        }
        let permitted = is_public_address(address.ip())
            || (allow_private_networks && is_private_network_address(address.ip()))
            || (allow_loopback && loopback_url && address.ip().is_loopback());
        if !permitted {
            return Err(Error::InvalidConfiguration);
        }
    }
    let loopback_allowed = allow_loopback || (scheme == "https" && allow_private_networks);
    if loopback_url && (!loopback_allowed || resolved.iter().any(|address| !address.ip().is_loopback())) {
        return Err(Error::InvalidConfiguration);
    }
    if let Some(literal) = literal
        && resolved.iter().any(|address| address.ip() != literal)
    {
        return Err(Error::InvalidConfiguration);
    }
    Ok(url)
}

fn pinned_client(url: &Url, resolved: &[SocketAddr], timeout: Duration) -> Result<Client, Error> {
    let mut builder = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .timeout(timeout)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd();
    if let Some(domain) = url.domain() {
        builder = builder.resolve_to_addrs(domain, resolved);
    }
    builder.build().map_err(|error| Error::Transport(error.to_string()))
}

const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
const DEFAULT_TOKEN_LIFETIME: Duration = Duration::from_secs(300);
const MAX_TOKEN_LIFETIME: Duration = Duration::from_secs(24 * 3600);

impl OAuthClient {
    /// A cached token is reused until shortly before it expires. Fetching happens without holding the
    /// lock, so two concurrent misses may fetch twice; both tokens are valid and the later one is kept.
    async fn bearer(&self) -> Result<HeaderValue, Error> {
        if let Some((token, expires)) = self.cached.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref()
            && Instant::now() < *expires
        {
            return Ok(token.clone());
        }
        let mut form = vec![("grant_type", "client_credentials")];
        if let Some(scope) = &self.config.scope {
            form.push(("scope", scope));
        }
        let mut request = self.client.post(self.token_url.clone()).header(ACCEPT, "application/json");
        if self.config.basic_auth {
            request = request.basic_auth(&self.config.client_id, Some(&self.config.client_secret));
        } else {
            form.push(("client_id", &self.config.client_id));
            form.push(("client_secret", &self.config.client_secret));
        }
        let response = request.form(&form).send().await.map_err(|_| Error::TokenUnavailable)?;
        if !response.status().is_success() {
            return Err(Error::TokenUnavailable);
        }
        let body = read_bounded(response, MAX_TOKEN_RESPONSE_BYTES).await.map_err(|_| Error::TokenUnavailable)?;
        let body: Value = serde_json::from_slice(&body).map_err(|_| Error::TokenUnavailable)?;
        let token = body["access_token"].as_str().filter(|token| !token.is_empty() && token.len() <= 8192).ok_or(Error::TokenUnavailable)?;
        if body["token_type"].as_str().is_some_and(|kind| !kind.eq_ignore_ascii_case("bearer")) {
            return Err(Error::TokenUnavailable);
        }
        let lifetime = body["expires_in"].as_u64().map_or(DEFAULT_TOKEN_LIFETIME, Duration::from_secs).min(MAX_TOKEN_LIFETIME);
        // Refresh before expiry so a token does not lapse in flight to the provider.
        let expires = Instant::now() + lifetime.saturating_sub(Duration::from_secs(30).min(lifetime / 2));
        let mut value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| Error::TokenUnavailable)?;
        value.set_sensitive(true);
        *self.cached.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((value.clone(), expires));
        Ok(value)
    }

    fn invalidate(&self) {
        *self.cached.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

fn effect_key(endpoint: &str, key: &str) -> String {
    let digest = Sha256::digest(format!("aip-effect:{endpoint}:{key}").as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn read_bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, Error> {
    if response.content_length().is_some_and(|size| size > limit as u64) {
        return Err(Error::ResponseTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| Error::Transport(error.to_string()))? {
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err(Error::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn parse_retry_after(value: &HeaderValue) -> Option<u64> {
    value.to_str().ok()?.parse::<u64>().ok().map(|seconds| seconds.min(MAX_RETRY_AFTER_SECONDS))
}

pub(crate) fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_documentation()
                || octets[0] == 0 // "this network" is never a public destination
                // Carrier-grade NAT, IETF protocol assignments and benchmarking ranges.
                || (octets[0] == 100 && (octets[1] & 0xc0) == 64)
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
                || (octets[0] == 198 && (octets[1] & 0xfe) == 18)
                || octets[0] >= 240)
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_public_address(IpAddr::V4(mapped));
            }
            let segments = ip.segments();
            // Only global unicast is accepted. Exclude transition and special-use
            // ranges that can carry an embedded IPv4 destination.
            (segments[0] & 0xe000) == 0x2000
                && !(ip.is_unspecified()
                    || ip.is_loopback()
                    || ip.is_multicast()
                    || segments[..6].iter().all(|segment| *segment == 0)
                    || (segments[0] & 0xfe00) == 0xfc00 // unique-local
                    || (segments[0] & 0xffc0) == 0xfe80 // link-local
                    || (segments[0] & 0xffc0) == 0xfec0 // deprecated site-local
                    || (segments[0] == 0x2001 && segments[1] <= 0x01ff) // IETF protocol assignments, Teredo and benchmarking
                    || (segments[0] == 0x2001 && segments[1] == 0x0db8) // documentation
                    || (segments[0] == 0x2001 && (0x0010..=0x003f).contains(&segments[1])) // ORCHID and protocol entity tags
                    || segments[0] == 0x2002 // 6to4
                    || (segments[0] == 0x3fff && segments[1] & 0xf000 == 0) // documentation
                    || segments[0] == 0x5f00 // SRv6 segment identifiers
                    || (segments[0] == 0x0064 && segments[1] == 0xff9b)) // NAT64
        }
    }
}

fn is_private_network_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_loopback(),
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_private_network_address(IpAddr::V4(mapped));
            }
            ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}
