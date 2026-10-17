use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, errors::ErrorKind};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, sync::Mutex};

const MAX_JWKS_BYTES: usize = 256 * 1024;
const MAX_TOKEN_BYTES: usize = 4096;
const KEY_TTL: Duration = Duration::from_secs(60);
const UNKNOWN_KID_REFRESH: Duration = Duration::from_secs(5);
/// Oldest cached key set that may still verify tokens while the key endpoint is unreachable.
const MAX_STALE_KEYS: Duration = Duration::from_secs(300);
const REMOTE_TIMEOUT: Duration = Duration::from_secs(2);
const DB_TIMEOUT: Duration = Duration::from_secs(5);
/// Principal lookups open their own connection. Without a bound, a flood of correctly signed tokens for unknown
/// subjects (checked before any rate limit) opens one PostgreSQL connection per request until `max_connections`.
const MAX_LOOKUP_CONNECTIONS: usize = 16;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub issuer: String,
    pub audience: String,
    pub jwks: JwksSource,
    #[serde(default)]
    pub provisioning: ProvisioningConfig,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProvisioningConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_actor_resource")]
    pub actor_resource: String,
    #[serde(default = "default_username_claim")]
    pub username_claim: String,
    #[serde(default = "default_email_claim")]
    pub email_claim: String,
    #[serde(default = "default_true")]
    pub require_email_verified: bool,
    #[serde(default)]
    pub fixed_defaults: ProvisioningFixedDefaults,
}

fn default_actor_resource() -> String {
    "User".to_string()
}
fn default_username_claim() -> String {
    "preferred_username".to_string()
}
fn default_email_claim() -> String {
    "email".to_string()
}
fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProvisioningFixedDefaults {
    #[serde(default = "default_primary_group")]
    pub primary_group_name: String,
    #[serde(default = "default_true")]
    pub activated: bool,
}

fn default_primary_group() -> String {
    "Member".to_string()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum JwksSource {
    File { path: PathBuf },
    Https { url: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    Invalid,
    Expired,
    Unavailable,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct VerifiedClaims {
    pub issuer: String,
    pub subject: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub claims: HashMap<String, Value>,
}

struct KeyState {
    keys: HashMap<String, Arc<DecodingKey>>,
    loaded: Instant,
    last_attempt: Option<Instant>,
    failed_until: Option<Instant>,
    /// Size, modification time and inode of the JWKS file the cached keys came from.
    file_stamp: Option<FileStamp>,
}

type FileStamp = (u64, SystemTime, u64);

/// A cheap change detector for the JWKS file, so tokens do not re-read and re-parse it every time.
fn stamp_of(metadata: &std::fs::Metadata) -> Option<FileStamp> {
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(metadata);
    #[cfg(not(unix))]
    let inode = 0;
    Some((metadata.len(), metadata.modified().ok()?, inode))
}

fn file_stamp(path: &std::path::Path) -> Option<FileStamp> {
    std::fs::metadata(path).ok().and_then(|metadata| stamp_of(&metadata))
}

enum Lookup<'a> {
    Pooled(spike_v6_transport::pool::PooledConnection),
    Private(spike_v2_read::OwnedConnection, #[allow(dead_code)] tokio::sync::SemaphorePermit<'a>),
}

impl Lookup<'_> {
    fn client(&self) -> &tokio_postgres::Client {
        match self {
            Self::Pooled(connection) => connection,
            Self::Private(connection, _) => connection,
        }
    }

    /// Returns a pooled connection for reuse; a private one is simply dropped.
    async fn finish(self) {
        if let Self::Pooled(connection) = self {
            let _ = connection.release().await;
        }
    }
}

enum CacheDecision {
    Use(Arc<DecodingKey>),
    Reject(AuthError),
    Refresh,
}

pub struct AuthenticatorImpl {
    actor_resource_sql: Option<String>,
    config: Config,
    db_url: String,
    schema: String,
    http: reqwest::Client,
    keys: Mutex<KeyState>,
    /// Single flight for remote key refreshes; the network fetch must not run under the `keys` lock.
    refresh: Mutex<()>,
    lookups: tokio::sync::Semaphore,
    /// The server's own pool, handed over by `use_pool`; principal lookups prefer it to private connections.
    pool: std::sync::OnceLock<Arc<spike_v6_transport::pool::ConnectionPool>>,
}

fn now_secs() -> Result<i64, AuthError> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| AuthError::Unavailable)?.as_secs() as i64)
}

fn valid_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

fn parse_jwks(bytes: &[u8]) -> Result<HashMap<String, Arc<DecodingKey>>, AuthError> {
    if bytes.len() > MAX_JWKS_BYTES {
        return Err(AuthError::Unavailable);
    }
    let json: Value = serde_json::from_slice(bytes).map_err(|_| AuthError::Unavailable)?;
    let items = json.get("keys").and_then(Value::as_array).ok_or(AuthError::Unavailable)?;
    if items.is_empty() || items.len() > 32 {
        return Err(AuthError::Unavailable);
    }
    let mut out = HashMap::new();
    let mut seen = HashSet::new();
    for item in items {
        let kid = item.get("kid").and_then(Value::as_str).ok_or(AuthError::Unavailable)?;
        if kid.is_empty() || kid.len() > 128 || kid.bytes().any(|b| b.is_ascii_control()) || !seen.insert(kid) {
            return Err(AuthError::Unavailable);
        }
        if item.get("kty").and_then(Value::as_str) != Some("RSA") {
            continue;
        }
        if item.get("alg").and_then(Value::as_str).is_some_and(|alg| alg != "RS256") {
            continue;
        }
        if item.get("use").and_then(Value::as_str).is_some_and(|use_| use_ != "sig") {
            continue;
        }
        if let Some(operations) = item.get("key_ops") {
            let operations = operations.as_array().ok_or(AuthError::Unavailable)?;
            if operations.len() != 1 || operations[0].as_str() != Some("verify") {
                return Err(AuthError::Unavailable);
            }
        }
        if ["d", "p", "q", "dp", "dq", "qi", "oth"].iter().any(|name| item.get(*name).is_some()) {
            return Err(AuthError::Unavailable);
        }
        let n = item.get("n").and_then(Value::as_str).ok_or(AuthError::Unavailable)?;
        let e = item.get("e").and_then(Value::as_str).ok_or(AuthError::Unavailable)?;
        let modulus = URL_SAFE_NO_PAD.decode(n).map_err(|_| AuthError::Unavailable)?;
        let exponent = URL_SAFE_NO_PAD.decode(e).map_err(|_| AuthError::Unavailable)?;
        let bits = modulus.first().map(|first| (modulus.len() - 1) * 8 + (8 - first.leading_zeros() as usize)).unwrap_or(0);
        let exponent_value = exponent.iter().fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
        if modulus.first() == Some(&0)
            || !(2048..=4096).contains(&bits)
            || exponent.is_empty()
            || exponent.len() > 8
            || exponent.first() == Some(&0)
            || exponent_value < 3
            || exponent_value % 2 == 0
        {
            return Err(AuthError::Unavailable);
        }
        let key = DecodingKey::from_rsa_components(n, e).map_err(|_| AuthError::Unavailable)?;
        out.insert(kid.to_string(), Arc::new(key));
    }
    if out.is_empty() {
        return Err(AuthError::Unavailable);
    }
    Ok(out)
}

async fn read_jwks(source: &JwksSource, http: &reqwest::Client) -> Result<Vec<u8>, AuthError> {
    match source {
        JwksSource::File { path } => tokio::time::timeout(REMOTE_TIMEOUT, async {
            let metadata = tokio::fs::metadata(path).await.map_err(|_| AuthError::Unavailable)?;
            if !metadata.is_file() || metadata.len() > MAX_JWKS_BYTES as u64 {
                return Err(AuthError::Unavailable);
            }
            let file = tokio::fs::File::open(path).await.map_err(|_| AuthError::Unavailable)?;
            let mut bytes = Vec::new();
            file.take((MAX_JWKS_BYTES + 1) as u64).read_to_end(&mut bytes).await.map_err(|_| AuthError::Unavailable)?;
            if bytes.len() > MAX_JWKS_BYTES {
                return Err(AuthError::Unavailable);
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| AuthError::Unavailable)?,
        JwksSource::Https { url } => {
            let mut response = http.get(url).send().await.map_err(|_| AuthError::Unavailable)?;
            if !response.status().is_success() || response.content_length().is_some_and(|n| n > MAX_JWKS_BYTES as u64) {
                return Err(AuthError::Unavailable);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| AuthError::Unavailable)? {
                if bytes.len().saturating_add(chunk.len()) > MAX_JWKS_BYTES {
                    return Err(AuthError::Unavailable);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        }
    }
}

/// Why an authenticator could not be built. `reason` is a fixed sentence that names the offending setting but
/// never its value, so an operator or an AI can repair the file and the log of a failed start holds no secret.
#[derive(Debug, PartialEq, Eq)]
pub struct ConfigFailure {
    pub kind: AuthError,
    pub reason: &'static str,
}

fn invalid(reason: &'static str) -> ConfigFailure {
    ConfigFailure { kind: AuthError::Invalid, reason }
}

fn unavailable(reason: &'static str) -> ConfigFailure {
    ConfigFailure { kind: AuthError::Unavailable, reason }
}

impl AuthenticatorImpl {
    /// Configure from checked server facts, never from a request. Infrastructure handles stay numeric.
    pub fn with_resource_actor(mut self, facts: &serde_json::Value) -> Self {
        let actor = facts["actor"].as_str().unwrap_or("principal");
        let uuid = spike_v2_read::id_wire::id_kind(facts, actor) == "uuid";
        if uuid || (!facts["externalStorage"].is_null() && facts["actorMode"] != "principal") {
            let table = spike_v2_read::sqlgen::snake(actor);
            let lookup = if uuid {
                format!("SELECT i.resource_id FROM \"{}\".aip_actor_identities i WHERE i.id=p.actor_id", self.schema)
            } else {
                "SELECT p.actor_id".into()
            };
            self.actor_resource_sql = Some(format!("SELECT a.id::text FROM \"{}\".{table} a WHERE a.id=({lookup})", self.schema));
        }
        self
    }

    pub async fn new(config: Config, db_url: String, schema: String) -> Result<Self, AuthError> {
        Self::new_explained(config, db_url, schema).await.map_err(|failure| failure.kind)
    }

    /// Builds an authenticator from verified configuration and database settings.
    ///
    /// Custom CA certificates are read from the environment: `SSL_CERT_FILE` takes precedence,
    /// falling back to `AIP_AUTH_CA_PEM`. If the specified file cannot be read or parsed, it is
    /// ignored (.ok()) and client construction continues with standard WebPKI root certificates.
    /// Untrusted root certificates and hostname mismatches remain rejected under all settings.
    pub async fn new_explained(config: Config, db_url: String, schema: String) -> Result<Self, ConfigFailure> {
        Self::new_explained_with_lookup(config, db_url, schema, &|name| std::env::var(name).ok()).await
    }

    pub(crate) async fn new_explained_with_lookup(
        config: Config,
        db_url: String,
        schema: String,
        lookup: &impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigFailure> {
        if !valid_identifier(&schema) {
            return Err(invalid("배포 schema 이름이 올바르지 않음"));
        }
        if config.audience.is_empty() || config.audience.len() > 256 {
            return Err(invalid("audience는 1~256바이트여야 함"));
        }
        if db_url.is_empty() {
            return Err(invalid("DB 연결 문자열이 비어 있음"));
        }
        let issuer = reqwest::Url::parse(&config.issuer).map_err(|_| invalid("issuer가 URL이 아님"))?;
        if issuer.scheme() != "https" {
            return Err(invalid("issuer는 https여야 함"));
        }
        if issuer.host_str().is_none() {
            return Err(invalid("issuer에 호스트가 없음"));
        }
        if !issuer.username().is_empty() || issuer.password().is_some() {
            return Err(invalid("issuer에 자격 증명을 넣을 수 없음"));
        }
        if issuer.fragment().is_some() || issuer.query().is_some() {
            return Err(invalid("issuer에 query·fragment를 넣을 수 없음"));
        }
        match &config.jwks {
            JwksSource::File { path } if path.as_os_str().is_empty() => return Err(invalid("jwks.path가 비어 있음")),
            JwksSource::Https { url } => {
                let parsed = reqwest::Url::parse(url).map_err(|_| invalid("jwks.url이 URL이 아님"))?;
                if parsed.scheme() != "https" || parsed.host_str().is_none() {
                    return Err(invalid("jwks.url은 호스트가 있는 https여야 함"));
                }
                if !parsed.username().is_empty() || parsed.password().is_some() || parsed.fragment().is_some() {
                    return Err(invalid("jwks.url에 자격 증명·fragment를 넣을 수 없음"));
                }
            }
            _ => {}
        }
        if config.provisioning.enabled {
            if !valid_identifier(&config.provisioning.actor_resource) {
                return Err(invalid("provisioning actor_resource가 유효한 식별자가 아님"));
            }
            if config.provisioning.username_claim.is_empty() {
                return Err(invalid("provisioning username_claim이 비어 있음"));
            }
            if config.provisioning.email_claim.is_empty() {
                return Err(invalid("provisioning email_claim이 비어 있음"));
            }
            if config.provisioning.fixed_defaults.primary_group_name.is_empty() {
                return Err(invalid("provisioning fixed_defaults.primary_group_name이 비어 있음"));
            }
        }
        let mut http_builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(REMOTE_TIMEOUT);
        let custom_ca = lookup("SSL_CERT_FILE")
            .or_else(|| lookup("AIP_AUTH_CA_PEM"))
            .and_then(|ca_path| std::fs::read(ca_path).ok())
            .and_then(|bytes| reqwest::Certificate::from_pem(&bytes).ok());
        if let Some(root) = custom_ca {
            http_builder = http_builder.add_root_certificate(root);
        }
        let http = http_builder.build().map_err(|_| unavailable("HTTP client를 만들 수 없음"))?;
        let document = read_jwks(&config.jwks, &http).await.map_err(|_| match config.jwks {
            JwksSource::File { .. } => unavailable("jwks.path 파일을 읽을 수 없음(없음, 일반 파일 아님, 크기 상한 초과, 읽기 기한 초과 중 하나)"),
            JwksSource::Https { .. } => unavailable("jwks.url에서 공개키 집합을 받지 못함(연결, TLS, 성공 상태 코드, 크기 상한, 기한 중 하나)"),
        })?;
        let keys = parse_jwks(&document)
            .map_err(|_| unavailable("공개키 집합(JWKS) 형식 오류: 키가 하나 이상이고 각 키가 RSA 서명용 kid·n·e와 지원 알고리즘을 가져야 함"))?;
        let now = Instant::now();
        let config_for_stamp = config.jwks.clone();
        Ok(Self {
            actor_resource_sql: None,
            config,
            db_url,
            schema,
            http,
            keys: Mutex::new(KeyState {
                keys,
                loaded: now,
                last_attempt: None,
                failed_until: None,
                file_stamp: match &config_for_stamp {
                    JwksSource::File { path } => file_stamp(path),
                    JwksSource::Https { .. } => None,
                },
            }),
            refresh: Mutex::new(()),
            lookups: tokio::sync::Semaphore::new(MAX_LOOKUP_CONNECTIONS),
            pool: std::sync::OnceLock::new(),
        })
    }

    /// A bounded principal-lookup connection. `shared_pool` is true only where the caller holds no pool connection of
    /// its own: a durable re-check runs while a job already holds one, and waiting for a second could starve the pool.
    /// Through the server pool it shares that pool's size limit, acquire
    /// deadline and connection reuse; without one (tests, tools) it falls back to a private, semaphore-bounded connection.
    async fn lookup_connection(&self, shared_pool: bool) -> Result<Lookup<'_>, AuthError> {
        if let Some(pool) = self.pool.get().filter(|_| shared_pool) {
            let connection =
                tokio::time::timeout(DB_TIMEOUT, pool.acquire()).await.map_err(|_| AuthError::Unavailable)?.map_err(|_| AuthError::Unavailable)?;
            return Ok(Lookup::Pooled(connection));
        }
        let permit = tokio::time::timeout(DB_TIMEOUT, self.lookups.acquire())
            .await
            .map_err(|_| AuthError::Unavailable)?
            .map_err(|_| AuthError::Unavailable)?;
        let db = tokio::time::timeout(DB_TIMEOUT, spike_v2_read::connect_owned_with_url(&self.db_url))
            .await
            .map_err(|_| AuthError::Unavailable)?
            .map_err(|_| AuthError::Unavailable)?;
        Ok(Lookup::Private(db, permit))
    }

    async fn key_for(&self, kid: &str) -> Result<Arc<DecodingKey>, AuthError> {
        match &self.config.jwks {
            JwksSource::File { path } => self.file_key(path, kid).await,
            JwksSource::Https { .. } => self.remote_key(kid).await,
        }
    }

    /// File mode re-reads the file only when its size, modification time or inode changed, so a token costs one `stat`.
    async fn file_key(&self, path: &std::path::Path, kid: &str) -> Result<Arc<DecodingKey>, AuthError> {
        let stamp = tokio::fs::metadata(path).await.ok().and_then(|metadata| stamp_of(&metadata));
        let mut state = self.keys.lock().await;
        if stamp.is_none() || stamp != state.file_stamp {
            let fresh = read_jwks(&self.config.jwks, &self.http).await.and_then(|bytes| parse_jwks(&bytes))?;
            state.keys = fresh;
            state.loaded = Instant::now();
            state.file_stamp = stamp;
        }
        state.keys.get(kid).cloned().ok_or(AuthError::Invalid)
    }

    /// Decides whether `kid` can be answered from the cache or needs a fetch. Never touches the network.
    async fn cache_decision(&self, kid: &str) -> CacheDecision {
        let state = self.keys.lock().await;
        let age = state.loaded.elapsed();
        let known = state.keys.get(kid).cloned();
        if let Some(key) = known.as_ref().filter(|_| age < KEY_TTL) {
            return CacheDecision::Use(key.clone());
        }
        if state.failed_until.is_some_and(|until| Instant::now() < until) {
            // The key endpoint just failed. A key we already hold keeps working for a bounded time; nothing else can be decided.
            return match known {
                Some(key) if age < MAX_STALE_KEYS => CacheDecision::Use(key),
                _ => CacheDecision::Reject(AuthError::Unavailable),
            };
        }
        if known.is_none() && state.last_attempt.is_some_and(|at| at.elapsed() < UNKNOWN_KID_REFRESH) {
            // Unauthenticated senders choose the kid, so unknown kids may trigger a fetch only every few seconds.
            return CacheDecision::Reject(AuthError::Invalid);
        }
        CacheDecision::Refresh
    }

    async fn remote_key(&self, kid: &str) -> Result<Arc<DecodingKey>, AuthError> {
        match self.cache_decision(kid).await {
            CacheDecision::Use(key) => return Ok(key),
            CacheDecision::Reject(error) => return Err(error),
            CacheDecision::Refresh => {}
        }
        let _single_flight = self.refresh.lock().await;
        // Another request may have refreshed (or failed) while this one waited for its turn.
        match self.cache_decision(kid).await {
            CacheDecision::Use(key) => return Ok(key),
            CacheDecision::Reject(error) => return Err(error),
            CacheDecision::Refresh => {}
        }
        self.keys.lock().await.last_attempt = Some(Instant::now());
        let fetched = read_jwks(&self.config.jwks, &self.http).await.and_then(|bytes| parse_jwks(&bytes));
        let mut state = self.keys.lock().await;
        match fetched {
            Ok(fresh) => {
                state.keys = fresh;
                state.loaded = Instant::now();
                state.failed_until = None;
                state.keys.get(kid).cloned().ok_or(AuthError::Invalid)
            }
            Err(error) => {
                state.failed_until = Some(Instant::now() + UNKNOWN_KID_REFRESH);
                match state.keys.get(kid) {
                    Some(key) if state.loaded.elapsed() < MAX_STALE_KEYS => Ok(key.clone()),
                    _ => Err(error),
                }
            }
        }
    }

    pub async fn verify_token(&self, token: &str) -> Result<VerifiedClaims, AuthError> {
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
            return Err(AuthError::Invalid);
        }
        let header = decode_header(token).map_err(|_| AuthError::Invalid)?;
        if header.alg != Algorithm::RS256
            || header.typ.as_deref() != Some("at+jwt")
            || header.jku.is_some()
            || header.jwk.is_some()
            || header.x5u.is_some()
            || header.x5c.is_some()
            || header.crit.is_some()
            || header.zip.is_some()
            || header.cty.is_some()
            || !header.extras.inner().is_empty()
        {
            return Err(AuthError::Invalid);
        }
        let kid = header.kid.as_deref().filter(|kid| !kid.is_empty() && kid.len() <= 128).ok_or(AuthError::Invalid)?;
        let key = self.key_for(kid).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&self.config.audience]);
        validation.set_required_spec_claims(&["iss", "aud", "sub", "exp"]);
        validation.leeway = 0;
        let decoded = decode::<Claims>(token, &key, &validation).map_err(|error| match error.kind() {
            ErrorKind::ExpiredSignature => AuthError::Expired,
            _ => AuthError::Invalid,
        })?;
        let claims = decoded.claims;
        let now = now_secs()?;
        if claims.sub.is_empty()
            || claims.sub.len() > 256
            || claims.sub.chars().any(char::is_control)
            || claims.iss != self.config.issuer
            || claims.iat < 0
            || claims.nbf.is_some_and(|nbf| nbf < 0)
            || claims.exp < 0
            || claims.iat > now
            || claims.nbf.is_some_and(|nbf| nbf > now)
            || claims.exp <= now
            || !matches!(claims.exp.checked_sub(claims.iat), Some(1..=900))
        {
            return Err(if claims.exp <= now { AuthError::Expired } else { AuthError::Invalid });
        }
        Ok(VerifiedClaims {
            issuer: claims.iss,
            subject: claims.sub,
            issued_at: claims.iat,
            expires_at: claims.exp,
            claims: claims.claims,
        })
    }

    pub async fn session_for(&self, token: &str) -> Result<spike_v6_transport::auth::Session, AuthError> {
        let claims = self.verify_token(token).await?;
        let lookup = self.lookup_connection(true).await?;
        let db = lookup.client();
        let extra = self.actor_resource_sql.as_ref().map_or(String::new(), |sql| format!(", ({sql})"));
        let sql = format!("SELECT actor_id, enabled, min_iat{extra} FROM \"{}\".aip_principals p WHERE issuer=$1 AND subject=$2", self.schema);
        let row = tokio::time::timeout(DB_TIMEOUT, db.query_opt(&sql, &[&claims.issuer, &claims.subject]))
            .await
            .map_err(|_| AuthError::Unavailable)?
            .map_err(|_| AuthError::Unavailable)?;
        lookup.finish().await;
        let row = row.ok_or(AuthError::Invalid)?;
        let actor_id: i64 = row.try_get(0).map_err(|_| AuthError::Unavailable)?;
        let enabled: bool = row.try_get(1).map_err(|_| AuthError::Unavailable)?;
        let min_iat: i64 = row.try_get(2).map_err(|_| AuthError::Unavailable)?;
        if !enabled || claims.issued_at < min_iat {
            return Err(AuthError::Invalid);
        }
        let grant = spike_v6_transport::auth::DurablePrincipal {
            issuer: claims.issuer,
            subject: claims.subject,
            actor_id,
            issued_at: claims.issued_at,
            min_iat,
        };
        let public_id = if self.actor_resource_sql.is_some() {
            Some(row.try_get::<_, Option<String>>(3).map_err(|_| AuthError::Unavailable)?.ok_or(AuthError::Invalid)?)
        } else {
            None
        };
        spike_v6_transport::auth::Session::new_durable(grant, claims.expires_at)
            .map(|mut session| {
                session.actor_resource_id = public_id;
                session
            })
            .map_err(|error| match error {
                spike_v6_transport::auth::AuthError::Expired => AuthError::Expired,
                _ => AuthError::Invalid,
            })
    }

    pub async fn verify_durable_principal(&self, grant: &spike_v6_transport::auth::DurablePrincipal) -> Result<(), AuthError> {
        if grant.issuer != self.config.issuer
            || grant.subject.is_empty()
            || grant.subject.len() > 256
            || grant.subject.chars().any(char::is_control)
            || grant.actor_id <= 0
            || grant.issued_at < 0
            || grant.min_iat < 0
        {
            return Err(AuthError::Invalid);
        }
        let lookup = self.lookup_connection(false).await?;
        let db = lookup.client();
        let extra = self.actor_resource_sql.as_ref().map_or(String::new(), |sql| format!(",({sql})"));
        let sql = format!(
            "SELECT actor_id, enabled, min_iat, floor(extract(epoch FROM clock_timestamp()))::bigint{extra} FROM \"{}\".aip_principals p WHERE issuer=$1 AND subject=$2",
            self.schema
        );
        let row = tokio::time::timeout(DB_TIMEOUT, db.query_opt(&sql, &[&grant.issuer, &grant.subject]))
            .await
            .map_err(|_| AuthError::Unavailable)?
            .map_err(|_| AuthError::Unavailable)?;
        lookup.finish().await;
        let row = row.ok_or(AuthError::Invalid)?;
        let actor_id: i64 = row.try_get(0).map_err(|_| AuthError::Unavailable)?;
        let enabled: bool = row.try_get(1).map_err(|_| AuthError::Unavailable)?;
        let min_iat: i64 = row.try_get(2).map_err(|_| AuthError::Unavailable)?;
        let now: i64 = row.try_get(3).map_err(|_| AuthError::Unavailable)?;
        if self.actor_resource_sql.is_some() && row.try_get::<_, Option<String>>(4).map_err(|_| AuthError::Unavailable)?.is_none() {
            return Err(AuthError::Invalid);
        }
        if !enabled || actor_id != grant.actor_id || min_iat != grant.min_iat || grant.issued_at < min_iat || grant.issued_at > now {
            return Err(AuthError::Invalid);
        }
        Ok(())
    }

    pub async fn provision_or_session_for(&self, token: &str) -> Result<spike_v6_transport::auth::Session, AuthError> {
        if !self.config.provisioning.enabled {
            return self.session_for(token).await;
        }
        let claims = self.verify_token(token).await?;

        if self.config.provisioning.require_email_verified {
            let email_verified = claims.claims.get("email_verified").and_then(Value::as_bool);
            if email_verified != Some(true) {
                return Err(AuthError::Invalid);
            }
        }

        let username = claims
            .claims
            .get(&self.config.provisioning.username_claim)
            .and_then(Value::as_str)
            .ok_or(AuthError::Invalid)?;
        if username.is_empty()
            || username.len() > 200
            || username.trim() != username
            || username.chars().any(char::is_control)
        {
            return Err(AuthError::Invalid);
        }

        let email = claims
            .claims
            .get(&self.config.provisioning.email_claim)
            .and_then(Value::as_str)
            .ok_or(AuthError::Invalid)?;
        if email.len() < 3
            || email.len() > 200
            || !email.contains('@')
            || email.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(AuthError::Invalid);
        }

        let lookup = self.lookup_connection(true).await?;
        let db = lookup.client();
        let extra = self.actor_resource_sql.as_ref().map_or(String::new(), |sql| format!(", ({sql})"));

        let sql = format!("SELECT actor_id, enabled, min_iat{extra} FROM \"{}\".aip_principals p WHERE issuer=$1 AND subject=$2", self.schema);
        let existing = tokio::time::timeout(DB_TIMEOUT, db.query_opt(&sql, &[&claims.issuer, &claims.subject]))
            .await
            .map_err(|_| AuthError::Unavailable)?
            .map_err(|_| AuthError::Unavailable)?;

        if let Some(row) = existing {
            lookup.finish().await;
            let actor_id: i64 = row.try_get(0).map_err(|_| AuthError::Unavailable)?;
            let enabled: bool = row.try_get(1).map_err(|_| AuthError::Unavailable)?;
            let min_iat: i64 = row.try_get(2).map_err(|_| AuthError::Unavailable)?;
            if !enabled || claims.issued_at < min_iat {
                return Err(AuthError::Invalid);
            }
            let grant = spike_v6_transport::auth::DurablePrincipal {
                issuer: claims.issuer,
                subject: claims.subject,
                actor_id,
                issued_at: claims.issued_at,
                min_iat,
            };
            let public_id = if self.actor_resource_sql.is_some() {
                Some(row.try_get::<_, Option<String>>(3).map_err(|_| AuthError::Unavailable)?.ok_or(AuthError::Invalid)?)
            } else {
                None
            };
            return spike_v6_transport::auth::Session::new_durable(grant, claims.expires_at)
                .map(|mut session| {
                    session.actor_resource_id = public_id;
                    session
                })
                .map_err(|error| match error {
                    spike_v6_transport::auth::AuthError::Expired => AuthError::Expired,
                    _ => AuthError::Invalid,
                });
        }

        if let Err(_) = tokio::time::timeout(DB_TIMEOUT, db.batch_execute("BEGIN")).await.map_err(|_| AuthError::Unavailable)? {
            lookup.finish().await;
            return Err(AuthError::Unavailable);
        }

        let lock_key = format!("{}:{}:{}:{}:{}:{}", self.schema.len(), self.schema, claims.issuer.len(), claims.issuer, claims.subject.len(), claims.subject);
        let lock_res = tokio::time::timeout(DB_TIMEOUT, db.query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0::bigint))", &[&lock_key]))
            .await
            .map_err(|_| AuthError::Unavailable);

        if lock_res.is_err() || lock_res.as_ref().unwrap().is_err() {
            let _ = db.batch_execute("ROLLBACK").await;
            lookup.finish().await;
            return Err(AuthError::Unavailable);
        }

        let double_check = tokio::time::timeout(DB_TIMEOUT, db.query_opt(&sql, &[&claims.issuer, &claims.subject]))
            .await
            .map_err(|_| AuthError::Unavailable);

        let row_opt = match double_check {
            Ok(Ok(r)) => r,
            _ => {
                let _ = db.batch_execute("ROLLBACK").await;
                lookup.finish().await;
                return Err(AuthError::Unavailable);
            }
        };

        if let Some(row) = row_opt {
            let _ = db.batch_execute("COMMIT").await;
            lookup.finish().await;
            let actor_id: i64 = row.try_get(0).map_err(|_| AuthError::Unavailable)?;
            let enabled: bool = row.try_get(1).map_err(|_| AuthError::Unavailable)?;
            let min_iat: i64 = row.try_get(2).map_err(|_| AuthError::Unavailable)?;
            if !enabled || claims.issued_at < min_iat {
                return Err(AuthError::Invalid);
            }
            let grant = spike_v6_transport::auth::DurablePrincipal {
                issuer: claims.issuer,
                subject: claims.subject,
                actor_id,
                issued_at: claims.issued_at,
                min_iat,
            };
            let public_id = if self.actor_resource_sql.is_some() {
                Some(row.try_get::<_, Option<String>>(3).map_err(|_| AuthError::Unavailable)?.ok_or(AuthError::Invalid)?)
            } else {
                None
            };
            return spike_v6_transport::auth::Session::new_durable(grant, claims.expires_at)
                .map(|mut session| {
                    session.actor_resource_id = public_id;
                    session
                })
                .map_err(|error| match error {
                    spike_v6_transport::auth::AuthError::Expired => AuthError::Expired,
                    _ => AuthError::Invalid,
                });
        }

        let group_sql = format!(
            "SELECT id FROM \"{}\".\"group\" WHERE name = $1 AND admin = FALSE AND super_mod = FALSE AND mod = FALSE AND banned = FALSE AND guest = FALSE ORDER BY id ASC LIMIT 1",
            self.schema
        );
        let group_row = match tokio::time::timeout(DB_TIMEOUT, db.query_opt(&group_sql, &[&self.config.provisioning.fixed_defaults.primary_group_name])).await {
            Ok(Ok(Some(r))) => r,
            _ => {
                let _ = db.batch_execute("ROLLBACK").await;
                lookup.finish().await;
                return Err(AuthError::Unavailable);
            }
        };
        let group_id: i64 = match group_row.try_get(0) {
            Ok(id) => id,
            Err(_) => {
                let _ = db.batch_execute("ROLLBACK").await;
                lookup.finish().await;
                return Err(AuthError::Unavailable);
            }
        };

        let actor_table = spike_v2_read::sqlgen::snake(&self.config.provisioning.actor_resource);
        let user_sql = format!(
            "INSERT INTO \"{}\".\"{}\" (username, email, date_joined, primary_group_id, activated) VALUES ($1, $2, NOW(), $3, $4) RETURNING id",
            self.schema, actor_table
        );
        let activated = self.config.provisioning.fixed_defaults.activated;
        let insert_user_res = tokio::time::timeout(DB_TIMEOUT, db.query_one(&user_sql, &[&username, &email, &group_id, &activated])).await;
        let user_row = match insert_user_res {
            Ok(Ok(row)) => row,
            _ => {
                let _ = db.batch_execute("ROLLBACK").await;
                lookup.finish().await;
                return Err(AuthError::Invalid);
            }
        };
        let actor_id: i64 = match user_row.try_get(0) {
            Ok(id) => id,
            Err(_) => {
                let _ = db.batch_execute("ROLLBACK").await;
                lookup.finish().await;
                return Err(AuthError::Unavailable);
            }
        };

        let principal_sql = format!(
            "INSERT INTO \"{}\".aip_principals (issuer, subject, actor_id, enabled, min_iat) VALUES ($1, $2, $3, TRUE, $4)",
            self.schema
        );
        let insert_principal_res = tokio::time::timeout(DB_TIMEOUT, db.execute(&principal_sql, &[&claims.issuer, &claims.subject, &actor_id, &claims.issued_at])).await;
        if insert_principal_res.is_err() || insert_principal_res.as_ref().unwrap().is_err() {
            let _ = db.batch_execute("ROLLBACK").await;
            lookup.finish().await;
            return Err(AuthError::Invalid);
        }

        let final_row = match tokio::time::timeout(DB_TIMEOUT, db.query_one(&sql, &[&claims.issuer, &claims.subject])).await {
            Ok(Ok(r)) => r,
            _ => {
                let _ = db.batch_execute("ROLLBACK").await;
                lookup.finish().await;
                return Err(AuthError::Unavailable);
            }
        };

        if let Err(_) = tokio::time::timeout(DB_TIMEOUT, db.batch_execute("COMMIT")).await {
            let _ = db.batch_execute("ROLLBACK").await;
            lookup.finish().await;
            return Err(AuthError::Unavailable);
        }
        lookup.finish().await;

        let _enabled: bool = final_row.try_get(1).map_err(|_| AuthError::Unavailable)?;
        let min_iat: i64 = final_row.try_get(2).map_err(|_| AuthError::Unavailable)?;
        let public_id = if self.actor_resource_sql.is_some() {
            Some(final_row.try_get::<_, Option<String>>(3).map_err(|_| AuthError::Unavailable)?.ok_or(AuthError::Invalid)?)
        } else {
            None
        };
        let grant = spike_v6_transport::auth::DurablePrincipal {
            issuer: claims.issuer,
            subject: claims.subject,
            actor_id,
            issued_at: claims.issued_at,
            min_iat,
        };
        spike_v6_transport::auth::Session::new_durable(grant, claims.expires_at)
            .map(|mut session| {
                session.actor_resource_id = public_id;
                session
            })
            .map_err(|error| match error {
                spike_v6_transport::auth::AuthError::Expired => AuthError::Expired,
                _ => AuthError::Invalid,
            })
    }
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    iat: i64,
    #[serde(default, deserialize_with = "deserialize_present_nbf")]
    nbf: Option<i64>,
    exp: i64,
    #[serde(flatten)]
    claims: HashMap<String, Value>,
}

fn deserialize_present_nbf<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    i64::deserialize(deserializer).map(Some)
}

impl spike_v6_transport::auth::Authenticator for AuthenticatorImpl {
    fn use_pool(&self, pool: Arc<spike_v6_transport::pool::ConnectionPool>) {
        let _ = self.pool.set(pool);
    }

    fn verify_session<'a>(
        &'a self,
        token: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<spike_v6_transport::auth::Session, spike_v6_transport::auth::AuthError>> + Send + 'a>> {
        Box::pin(async move {
            self.session_for(token).await.map_err(|error| match error {
                AuthError::Invalid => spike_v6_transport::auth::AuthError::BadSignature,
                AuthError::Expired => spike_v6_transport::auth::AuthError::Expired,
                AuthError::Unavailable => spike_v6_transport::auth::AuthError::Unavailable,
            })
        })
    }

    fn provision_or_verify_session<'a>(
        &'a self,
        token: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<spike_v6_transport::auth::Session, spike_v6_transport::auth::AuthError>> + Send + 'a>> {
        Box::pin(async move {
            self.provision_or_session_for(token).await.map_err(|error| match error {
                AuthError::Invalid => spike_v6_transport::auth::AuthError::BadSignature,
                AuthError::Expired => spike_v6_transport::auth::AuthError::Expired,
                AuthError::Unavailable => spike_v6_transport::auth::AuthError::Unavailable,
            })
        })
    }

    fn verify_durable<'a>(
        &'a self,
        grant: &'a spike_v6_transport::auth::DurablePrincipal,
    ) -> Pin<Box<dyn Future<Output = Result<(), spike_v6_transport::auth::AuthError>> + Send + 'a>> {
        Box::pin(async move {
            self.verify_durable_principal(grant).await.map_err(|error| match error {
                AuthError::Invalid => spike_v6_transport::auth::AuthError::BadSignature,
                AuthError::Expired => spike_v6_transport::auth::AuthError::Expired,
                AuthError::Unavailable => spike_v6_transport::auth::AuthError::Unavailable,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn local_tls_response_with_body(body: Vec<u8>) -> (u16, std::thread::JoinHandle<bool>) {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};

        let cert = CertificateDer::from(include_bytes!("../../../spikes/spike-v2-read/tests/certs/server.der").to_vec());
        let key = PrivateKeyDer::try_from(include_bytes!("../../../spikes/spike-v2-read/tests/certs/server-key.der").to_vec()).expect("test key");
        let config = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(vec![cert], key).expect("test TLS server config");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind TLS test server");
        let port = listener.local_addr().expect("bound TLS address").port();
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept TLS request");
            stream.set_read_timeout(Some(REMOTE_TIMEOUT)).expect("TLS read timeout");
            stream.set_write_timeout(Some(REMOTE_TIMEOUT)).expect("TLS write timeout");
            let connection = rustls::ServerConnection::new(Arc::new(config)).expect("TLS connection");
            let mut tls = rustls::StreamOwned::new(connection, stream);
            let mut request = [0_u8; 4096];
            if !matches!(tls.read(&mut request), Ok(1..)) {
                return false;
            }
            let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            tls.write_all(reply.as_bytes()).expect("write TLS response headers");
            tls.write_all(&body).expect("write TLS JWKS body");
            tls.flush().expect("flush TLS JWKS response");
            true
        });
        (port, worker)
    }

    fn local_tls_response() -> (u16, std::thread::JoinHandle<bool>) {
        local_tls_response_with_body(br#"{"keys":[]}"#.to_vec())
    }

    fn valid_jwks() -> Vec<u8> {
        use rsa::traits::PublicKeyParts;
        let private = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).expect("test RSA key");
        let public = private.to_public_key();
        let n = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
        let e = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
        serde_json::json!({"keys":[{"kty":"RSA","kid":"test-key","alg":"RS256","use":"sig","n":n,"e":e}]}).to_string().into_bytes()
    }

    #[tokio::test]
    async fn rfc9068_access_token_without_optional_nbf_is_accepted() {
        use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
        use rsa::{pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};

        let private = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).expect("test RSA key");
        let public = private.to_public_key();
        let jwks = serde_json::json!({"keys":[{
            "kty":"RSA", "kid":"rfc9068-key", "alg":"RS256", "use":"sig",
            "n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
            "e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())
        }]});
        let jwks_file = tempfile::NamedTempFile::new().expect("temporary JWKS file");
        std::fs::write(jwks_file.path(), jwks.to_string()).expect("write JWKS");
        let der = private.to_pkcs1_der().expect("encode test private key");
        let key = EncodingKey::from_rsa_der(der.as_bytes());
        let now = now_secs().expect("clock");
        let mut header = Header::new(Algorithm::RS256);
        header.typ = Some("at+jwt".into());
        header.kid = Some("rfc9068-key".into());
        let token = encode(
            &header,
            &serde_json::json!({
                "iss":"https://issuer.example", "aud":"aip-api", "sub":"fixture-subject",
                "iat":now, "exp":now + 300, "jti":"fixture-jti", "client_id":"fixture-client"
            }),
            &key,
        )
        .expect("sign RFC 9068 token");
        let auth = AuthenticatorImpl::new_explained(
            Config {
                issuer: "https://issuer.example".into(),
                audience: "aip-api".into(),
                jwks: JwksSource::File { path: jwks_file.path().into() },
                provisioning: ProvisioningConfig::default(),
            },
            "host=unused".into(),
            "aip_auth_test".into(),
        )
        .await
        .expect("load authenticator");

        let claims = auth.verify_token(&token).await.expect("RFC 9068 does not require nbf");
        assert_eq!(claims.subject, "fixture-subject");
    }

    #[tokio::test]
    async fn explicit_null_optional_nbf_is_rejected() {
        use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
        use rsa::{pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};

        let private = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).expect("test RSA key");
        let public = private.to_public_key();
        let jwks = serde_json::json!({"keys":[{
            "kty":"RSA", "kid":"null-nbf-key", "alg":"RS256", "use":"sig",
            "n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
            "e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())
        }]});
        let jwks_file = tempfile::NamedTempFile::new().expect("temporary JWKS file");
        std::fs::write(jwks_file.path(), jwks.to_string()).expect("write JWKS");
        let der = private.to_pkcs1_der().expect("encode test private key");
        let key = EncodingKey::from_rsa_der(der.as_bytes());
        let now = now_secs().expect("clock");
        let mut header = Header::new(Algorithm::RS256);
        header.typ = Some("at+jwt".into());
        header.kid = Some("null-nbf-key".into());
        let token = encode(
            &header,
            &serde_json::json!({
                "iss":"https://issuer.example", "aud":"aip-api", "sub":"fixture-subject",
                "iat":now, "nbf":null, "exp":now + 300
            }),
            &key,
        )
        .expect("sign explicit-null token");
        let auth = AuthenticatorImpl::new_explained(
            Config {
                issuer: "https://issuer.example".into(),
                audience: "aip-api".into(),
                jwks: JwksSource::File { path: jwks_file.path().into() },
                provisioning: ProvisioningConfig::default(),
            },
            "host=unused".into(),
            "aip_auth_test".into(),
        )
        .await
        .expect("load authenticator");

        assert!(auth.verify_token(&token).await.is_err(), "explicit null is present but is not a NumericDate");
    }

    fn tls_client(ca: &[u8], port: u16) -> reqwest::Client {
        let root = reqwest::Certificate::from_pem(ca).expect("test root certificate");
        reqwest::Client::builder()
            .add_root_certificate(root)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REMOTE_TIMEOUT)
            .no_proxy()
            .resolve("localhost", ([127, 0, 0, 1], port).into())
            .build()
            .expect("TLS test client")
    }

    fn local_response(bytes: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let address = format!("http://{}/keys", listener.local_addr().expect("bound address"));
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&bytes);
        });
        (address, worker)
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(REMOTE_TIMEOUT).build().expect("test client")
    }

    #[tokio::test]
    async fn remote_jwks_body_is_bounded_without_content_length() {
        let mut reply = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        reply.extend(vec![b'x'; MAX_JWKS_BYTES + 1]);
        let (url, worker) = local_response(reply);
        let result = read_jwks(&JwksSource::Https { url }, &client()).await;
        worker.join().expect("server thread");
        assert_eq!(result, Err(AuthError::Unavailable));
    }

    #[tokio::test]
    async fn remote_jwks_redirect_is_not_followed() {
        let (url, worker) = local_response(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/private\r\nContent-Length: 0\r\n\r\n".to_vec());
        let result = read_jwks(&JwksSource::Https { url }, &client()).await;
        worker.join().expect("server thread");
        assert_eq!(result, Err(AuthError::Unavailable));
    }

    #[tokio::test]
    async fn remote_jwks_accepts_trusted_tls_with_matching_hostname() {
        let (port, worker) = local_tls_response();
        let ca = include_bytes!("../../../spikes/spike-v2-read/tests/certs/test-ca.pem");
        let url = format!("https://localhost:{port}/keys");
        let result = read_jwks(&JwksSource::Https { url }, &tls_client(ca, port)).await;
        assert_eq!(result, Ok(br#"{"keys":[]}"#.to_vec()));
        assert!(worker.join().expect("TLS server thread"));
    }

    #[tokio::test]
    async fn remote_jwks_rejects_mismatched_tls_hostname() {
        let (port, worker) = local_tls_response();
        let ca = include_bytes!("../../../spikes/spike-v2-read/tests/certs/test-ca.pem");
        let url = format!("https://127.0.0.1:{port}/keys");
        let result = read_jwks(&JwksSource::Https { url }, &tls_client(ca, port)).await;
        assert_eq!(result, Err(AuthError::Unavailable));
        assert!(!worker.join().expect("TLS server thread"));
    }

    #[tokio::test]
    async fn remote_jwks_rejects_untrusted_tls_root() {
        let (port, worker) = local_tls_response();
        let ca = include_bytes!("../../../spikes/spike-v2-read/tests/certs/untrusted-ca.pem");
        let url = format!("https://localhost:{port}/keys");
        let result = read_jwks(&JwksSource::Https { url }, &tls_client(ca, port)).await;
        assert_eq!(result, Err(AuthError::Unavailable));
        assert!(!worker.join().expect("TLS server thread"));
    }

    #[tokio::test]
    async fn remote_jwks_accepts_ca_from_environment() {
        let (port, worker) = local_tls_response_with_body(valid_jwks());
        let ca_file = tempfile::NamedTempFile::new().expect("create temp CA file");
        std::fs::write(ca_file.path(), include_bytes!("../../../spikes/spike-v2-read/tests/certs/test-ca.pem")).expect("write test CA");
        let ca_path = ca_file.path().to_str().expect("valid CA path").to_owned();

        let config = Config {
            issuer: "https://issuer.example".into(),
            audience: "aip-api".into(),
            jwks: JwksSource::Https { url: format!("https://localhost:{port}/keys") },
            provisioning: ProvisioningConfig::default(),
        };

        let env_lookup = |name: &str| if name == "SSL_CERT_FILE" { Some(ca_path.clone()) } else { None };
        let auth = AuthenticatorImpl::new_explained_with_lookup(config, "host=unused".into(), "aip_app".into(), &env_lookup)
            .await
            .expect("SSL_CERT_FILE custom CA should allow HTTPS JWKS fetch via new_explained path");
        assert_eq!(auth.config.audience, "aip-api");
        assert!(worker.join().expect("TLS server thread"));
    }

    #[tokio::test]
    async fn remote_jwks_ca_environment_fallback_and_rejections() {
        let ca_file = tempfile::NamedTempFile::new().expect("create temp CA file");
        std::fs::write(ca_file.path(), include_bytes!("../../../spikes/spike-v2-read/tests/certs/test-ca.pem")).expect("write test CA");
        let ca_path = ca_file.path().to_str().expect("valid CA path").to_owned();

        let untrusted_ca_file = tempfile::NamedTempFile::new().expect("create temp untrusted CA file");
        std::fs::write(untrusted_ca_file.path(), include_bytes!("../../../spikes/spike-v2-read/tests/certs/untrusted-ca.pem"))
            .expect("write untrusted CA");
        let untrusted_path = untrusted_ca_file.path().to_str().expect("valid untrusted CA path").to_owned();

        // 1. SSL_CERT_FILE 부재 시 AIP_AUTH_CA_PEM fallback 성공
        {
            let (port, worker) = local_tls_response_with_body(valid_jwks());
            let config = Config {
                issuer: "https://issuer.example".into(),
                audience: "aip-api".into(),
                jwks: JwksSource::Https { url: format!("https://localhost:{port}/keys") },
                provisioning: ProvisioningConfig::default(),
            };
            let env_lookup = |name: &str| if name == "AIP_AUTH_CA_PEM" { Some(ca_path.clone()) } else { None };
            let auth = AuthenticatorImpl::new_explained_with_lookup(config, "host=unused".into(), "aip_app".into(), &env_lookup)
                .await
                .expect("AIP_AUTH_CA_PEM fallback should allow HTTPS JWKS fetch");
            assert_eq!(auth.config.audience, "aip-api");
            assert!(worker.join().expect("TLS server thread"));
        }

        // 2. SSL_CERT_FILE 우선순위: SSL_CERT_FILE 유효 + AIP_AUTH_CA_PEM 잘못된 파일 -> SSL_CERT_FILE 우선으로 성공
        {
            let (port, worker) = local_tls_response_with_body(valid_jwks());
            let config = Config {
                issuer: "https://issuer.example".into(),
                audience: "aip-api".into(),
                jwks: JwksSource::Https { url: format!("https://localhost:{port}/keys") },
                provisioning: ProvisioningConfig::default(),
            };
            let env_lookup = |name: &str| match name {
                "SSL_CERT_FILE" => Some(ca_path.clone()),
                "AIP_AUTH_CA_PEM" => Some(untrusted_path.clone()),
                _ => None,
            };
            let auth = AuthenticatorImpl::new_explained_with_lookup(config, "host=unused".into(), "aip_app".into(), &env_lookup)
                .await
                .expect("SSL_CERT_FILE must take priority over AIP_AUTH_CA_PEM");
            assert_eq!(auth.config.audience, "aip-api");
            assert!(worker.join().expect("TLS server thread"));
        }

        // 3. CA 환경 변수 부재 시 거부: 자체 서명 인증서를 신뢰할 수 없어 TLS 실패
        {
            let (port, worker) = local_tls_response_with_body(valid_jwks());
            let config = Config {
                issuer: "https://issuer.example".into(),
                audience: "aip-api".into(),
                jwks: JwksSource::Https { url: format!("https://localhost:{port}/keys") },
                provisioning: ProvisioningConfig::default(),
            };
            let no_env = |_: &str| None;
            let err = AuthenticatorImpl::new_explained_with_lookup(config, "host=unused".into(), "aip_app".into(), &no_env)
                .await
                .err()
                .expect("missing custom CA must reject untrusted server cert");
            assert!(err.reason.contains("jwks.url에서 공개키 집합을 받지 못함"));
            assert!(!worker.join().expect("TLS server thread"));
        }

        // 4. 잘못된 CA 파일 경로: .ok()로 무시되어 custom CA 없이 계속하므로 untrusted root로 거부
        {
            let (port, worker) = local_tls_response_with_body(valid_jwks());
            let config = Config {
                issuer: "https://issuer.example".into(),
                audience: "aip-api".into(),
                jwks: JwksSource::Https { url: format!("https://localhost:{port}/keys") },
                provisioning: ProvisioningConfig::default(),
            };
            let bad_path_env = |name: &str| if name == "SSL_CERT_FILE" { Some("/nonexistent/invalid-ca.pem".into()) } else { None };
            let err = AuthenticatorImpl::new_explained_with_lookup(config, "host=unused".into(), "aip_app".into(), &bad_path_env)
                .await
                .err()
                .expect("nonexistent CA path must result in rejected TLS without panicking");
            assert!(err.reason.contains("jwks.url에서 공개키 집합을 받지 못함"));
            assert!(!worker.join().expect("TLS server thread"));
        }

        // 5. 호스트명 불일치 거부 보존: 올바른 CA가 있어도 hostname mismatch (127.0.0.1 vs localhost)는 거부
        {
            let (port, worker) = local_tls_response_with_body(valid_jwks());
            let config = Config {
                issuer: "https://issuer.example".into(),
                audience: "aip-api".into(),
                jwks: JwksSource::Https { url: format!("https://127.0.0.1:{port}/keys") },
                provisioning: ProvisioningConfig::default(),
            };
            let env_lookup = |name: &str| if name == "SSL_CERT_FILE" { Some(ca_path.clone()) } else { None };
            let err = AuthenticatorImpl::new_explained_with_lookup(config, "host=unused".into(), "aip_app".into(), &env_lookup)
                .await
                .err()
                .expect("mismatched hostname must be rejected even with custom CA");
            assert!(err.reason.contains("jwks.url에서 공개키 집합을 받지 못함"));
            assert!(!worker.join().expect("TLS server thread"));
        }
    }

    async fn explain(issuer: &str, audience: &str, jwks: JwksSource) -> ConfigFailure {
        let config = Config { issuer: issuer.into(), audience: audience.into(), jwks, provisioning: ProvisioningConfig::default() };
        AuthenticatorImpl::new_explained(config, "host=x".into(), "aip_app".into()).await.err().expect("configuration must be rejected")
    }

    #[tokio::test]
    async fn each_configuration_failure_names_its_setting_and_no_value() {
        let file = || JwksSource::File { path: "/nonexistent/aip-jwks-probe.json".into() };
        let secret_marker = "very-secret-marker-7731";
        let cases = [
            (explain("https://issuer.example", "", file()).await, "audience"),
            (explain("not a url", "aud", file()).await, "issuer가 URL이 아님"),
            (explain("http://issuer.example", "aud", file()).await, "issuer는 https"),
            (explain(&format!("https://user:{secret_marker}@issuer.example"), "aud", file()).await, "issuer에 자격 증명"),
            (explain("https://issuer.example/?a=1", "aud", file()).await, "issuer에 query"),
            (explain("https://issuer.example", "aud", JwksSource::File { path: "".into() }).await, "jwks.path가 비어"),
            (explain("https://issuer.example", "aud", JwksSource::Https { url: "http://keys.example/jwks".into() }).await, "jwks.url은"),
            (explain("https://issuer.example", "aud", file()).await, "jwks.path 파일을 읽을 수 없음"),
        ];
        let mut distinct = std::collections::HashSet::new();
        for (failure, needle) in cases {
            assert!(failure.reason.contains(needle), "{} should contain {needle}", failure.reason);
            assert!(!failure.reason.contains(secret_marker) && !failure.reason.contains("nonexistent"));
            distinct.insert(failure.reason);
        }
        assert_eq!(distinct.len(), 8, "every cause has its own sentence");
    }

    #[tokio::test]
    async fn a_malformed_key_set_is_told_apart_from_an_unreadable_one() {
        let path = std::env::temp_dir().join(format!("aip-auth-jwks-{}.json", std::process::id()));
        std::fs::write(&path, b"{\"keys\":[]}").expect("write test key set");
        let failure = explain("https://issuer.example", "aud", JwksSource::File { path: path.clone() }).await;
        let _ = std::fs::remove_file(&path);
        assert!(failure.reason.contains("(JWKS) 형식 오류"), "{}", failure.reason);
    }
}

#[cfg(test)]
mod idp_tests;
