//! Product assembly reuses the validated definition, authorization, SQL and SDK engines.
mod brokers;
mod infrastructure;
mod rag;
mod triggers;
use clap::{Subcommand, ValueEnum};
use serde::Deserialize;
use serde_json::{Value, json};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use std::{
    fs,
    future::Future,
    io::{Read, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

const SOURCE_BYTES: usize = 1 << 20;
const CONFIG_BYTES: usize = 64 << 10;
const INTEGRATION_DNS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Subcommand)]
pub enum Command {
    /// Validate any of the five caller-definition formats
    Check {
        file: PathBuf,
        #[arg(long)]
        dev_checks: bool,
    },
    /// Generate a caller read/apply binding for the shared product SDK
    Gen {
        file: PathBuf,
        #[arg(long, value_enum, default_value = "decimal")]
        wire: Wire,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value = "@aip/sdk")]
        sdk_import: String,
    },
    /// Create a fresh owned schema and its deployment journal
    Init {
        file: PathBuf,
        #[arg(long)]
        config: PathBuf,
    },
    /// Print a deployment plan, or apply exactly the acknowledged plan digest
    Migrate {
        file: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        migration: Option<PathBuf>,
        #[arg(long)]
        apply: Option<String>,
    },
    /// Adopt a verified prototype schema while preserving rows and idempotency records
    Adopt {
        file: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        apply: Option<String>,
    },
    /// Serve a previously initialized deployment behind a TLS reverse proxy
    Serve {
        file: PathBuf,
        #[arg(long)]
        config: PathBuf,
    },
    /// Operator-only persistent issuer/subject to actor mapping
    Principal {
        #[arg(long)]
        config: PathBuf,
        #[command(subcommand)]
        command: Principal,
    },
    /// Operator-only durable job inspection and dead-letter redrive across every owner
    Jobs {
        #[arg(long)]
        config: PathBuf,
        #[command(subcommand)]
        command: Jobs,
    },
}

#[derive(Subcommand)]
pub enum Jobs {
    /// List runs newest first without input, output or grant payloads
    List {
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        operation: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Requeue a FAILED run as the same run; recorded effects replay instead of repeating
    Retry {
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
pub enum Principal {
    Bind {
        #[arg(long)]
        subject: String,
        #[arg(long)]
        actor: i64,
        #[arg(long, default_value_t = 0)]
        not_before: i64,
    },
    Revoke {
        #[arg(long)]
        subject: String,
    },
}

#[derive(Clone, Copy, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Wire {
    Safe,
    Decimal,
}
impl Wire {
    fn deployment_label(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Decimal => "decimal",
        }
    }
    fn id_wire(self) -> IdWire {
        match self {
            Self::Safe => IdWire::SafeNumber,
            Self::Decimal => IdWire::DecimalString,
        }
    }
}
fn default_wire() -> Wire {
    Wire::Decimal
}
fn default_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8080))
}
fn default_object_transfer_lifetime_ms() -> u64 {
    15 * 60 * 1_000
}
fn default_env() -> String {
    "AIP_DATABASE_URL".into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema: String,
    #[serde(default = "default_env")]
    pub database_url_env: String,
    #[serde(default)]
    pub db_pool: infrastructure::DbPoolConfig,
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    #[serde(default = "default_wire")]
    pub wire: Wire,
    pub auth: aip_auth::Config,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    pub workers: Option<Workers>,
    #[serde(default)]
    pub static_mounts: Vec<infrastructure::MountConfig>,
    #[serde(default)]
    pub file_mounts: Vec<infrastructure::FileMountConfig>,
    pub objects: Option<infrastructure::ObjectsConfig>,
    #[serde(default)]
    pub realtime: infrastructure::RealtimeConfig,
    #[serde(default)]
    pub messaging: infrastructure::MessagingConfig,
    #[serde(default)]
    pub broker: Option<brokers::BrokerConfig>,
    #[serde(default)]
    pub rate_limit: Option<infrastructure::RateLimitConfig>,
    #[serde(default)]
    pub limits: Option<infrastructure::ResourceLimitsConfig>,
    #[serde(default = "default_object_transfer_lifetime_ms")]
    pub object_transfer_lifetime_ms: u64,
    pub database_ca_file: Option<PathBuf>,
    pub rag: Option<rag::Config>,
    #[serde(default)]
    pub schedules: Vec<triggers::ScheduleConfig>,
    #[serde(default)]
    pub webhooks: Vec<triggers::WebhookConfig>,
    #[serde(default)]
    pub outbox_handlers: Vec<triggers::OutboxHandlerConfig>,
    #[serde(default)]
    pub job_policies: Vec<triggers::JobPolicyConfig>,
    #[serde(default)]
    pub message_handlers: Vec<triggers::MessageHandlerConfig>,
    pub metrics: Option<infrastructure::MetricsConfig>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workers {
    pub directory: PathBuf,
    pub language: Language,
    #[serde(default)]
    pub enable_writes: bool,
    #[serde(default)]
    pub enable_file_uploads: bool,
    #[serde(default)]
    pub integrations: Vec<IntegrationConfig>,
    #[serde(default)]
    pub outbound_webhooks: Vec<OutboundWebhookConfig>,
    pub runtime_executable: Option<PathBuf>,
    #[serde(default)]
    pub read_only_paths: Vec<PathBuf>,
    #[serde(default = "infrastructure::empty_environment")]
    pub environment: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub isolation: infrastructure::WorkerIsolation,
    #[serde(default)]
    pub cgroup_root: Option<PathBuf>,
    #[serde(default)]
    pub resource_limits: Option<infrastructure::WorkerResourceLimitsConfig>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationConfig {
    pub name: String,
    pub url: String,
    pub method: String,
    pub timeout_ms: u64,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    #[serde(default)]
    pub secret_header_env: Option<SecretHeaderEnv>,
    #[serde(default)]
    pub allow_loopback: bool,
    #[serde(default)]
    pub allow_private_networks: bool,
    #[serde(default)]
    pub circuit_breaker: Option<CircuitBreakerConfig>,
    #[serde(default)]
    pub oauth2: Option<OAuthClientCredentialsConfig>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthClientCredentialsConfig {
    pub token_url: String,
    pub client_id_env: String,
    pub client_secret_env: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub auth_method: OAuthClientAuth,
}
#[derive(Clone, Copy, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OAuthClientAuth {
    #[default]
    ClientSecretBasic,
    ClientSecretPost,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundWebhookConfig {
    pub name: String,
    pub signing_secret_env: String,
    #[serde(default = "default_webhook_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_webhook_body")]
    pub max_request_bytes: usize,
    #[serde(default)]
    pub allow_loopback: bool,
}
fn default_webhook_timeout_ms() -> u64 {
    10_000
}
fn default_webhook_body() -> usize {
    256 * 1024
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,
    pub open_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretHeaderEnv {
    pub name: String,
    pub env: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Node,
    Python,
}

fn error(code: &str, message: &str) -> Value {
    json!({"ok":false,"code":code,"message":message})
}

/// The error of a listener that did not start. Only the server's own validation sentences (`InvalidInput`, `TimedOut`)
/// are fixed text that names a setting and never a value; every other io error is described by its kind, because it can
/// carry an address, a path or the output of a worker runtime.
fn server_start_error(e: &std::io::Error) -> Value {
    use std::io::ErrorKind;
    let message = match e.kind() {
        ErrorKind::InvalidInput | ErrorKind::TimedOut => format!("서버 시작 설정 검증 실패: {e}"),
        ErrorKind::AddrInUse => "listen 주소가 이미 사용 중임(listen 설정의 포트를 바꾸거나 다른 프로세스를 종료)".to_owned(),
        ErrorKind::AddrNotAvailable => "listen 주소를 이 호스트에서 쓸 수 없음(listen 설정의 주소 확인)".to_owned(),
        ErrorKind::PermissionDenied => {
            "권한 거부로 listener 또는 worker 런타임을 시작하지 못함(포트 권한, 런타임 실행 권한, 격리 설정 확인)".to_owned()
        }
        ErrorKind::NotFound => "worker 런타임 실행 파일을 찾지 못함(workers.language의 node 또는 python3가 PATH에 있어야 함)".to_owned(),
        kind => format!(
            "서버 시작 준비 단계 실패(io {kind}): worker 런타임 점검, rate limit·outbox·audit 저장소 설치, listener 바인딩 중 하나이며 DB 연결과 권한을 먼저 확인"
        ),
    };
    error("SERVER_CONFIG", &message)
}

/// The error of a JSON document that does not parse into its type, with the reason and the position. The
/// reason of serde names keys and types (`unknown field`, `missing field`, `invalid type`), which a person
/// or an AI needs to repair the file; the value that was wrong is dropped, because a configuration file can
/// hold a secret and the output of a failed start is logged.
fn json_error(code: &str, what: &str, e: &serde_json::Error) -> Value {
    let reason = redact_json_reason(&e.to_string());
    error(code, &format!("{what}: {reason} (줄 {}, 열 {})", e.line(), e.column()))
}

fn redact_json_reason(full: &str) -> String {
    // serde_json appends " at line L column C"; the position is reported on its own.
    let msg = full.rsplit_once(" at line ").map_or(full, |(m, _)| m);
    let structural =
        ["unknown field", "missing field", "unknown variant", "duplicate field", "invalid length", "EOF while", "trailing characters", "expected "];
    if let Some(rest) = msg.strip_prefix("invalid type: ").or_else(|| msg.strip_prefix("invalid value: ")) {
        // `invalid type: string "abc", expected u64` -> `invalid type: string, expected u64`
        let kind = rest.split([' ', ',']).next().unwrap_or("");
        let expected = rest.split_once(", expected ").map_or("", |(_, x)| x);
        let label = if msg.starts_with("invalid type") { "invalid type" } else { "invalid value" };
        return format!("{label}: {kind}, expected {expected}");
    }
    // The value of an unknown variant is what was typed; the list of the accepted ones is what repairs it.
    if let Some((_, accepted)) = msg.strip_prefix("unknown variant ").and_then(|rest| rest.split_once(", expected ")) {
        return format!("unknown variant, expected {accepted}");
    }
    if structural.iter().any(|p| msg.starts_with(p)) {
        return msg.to_string();
    }
    // A message written by a `Deserialize` implementation may quote the value: say only that it is wrong.
    "a value is not accepted".to_string()
}
fn read_bounded(path: &Path, limit: usize, code: &str) -> Result<String, Value> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A FIFO must not block before the opened descriptor can be checked.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| error(code, "파일을 열 수 없음"))?;
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(error(code, "일반 파일이 필요함"));
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes).map_err(|_| error(code, "파일을 읽을 수 없음"))?;
    if bytes.len() > limit {
        return Err(error("INPUT_TOO_LARGE", "파일의 제품 입력 상한 초과"));
    }
    String::from_utf8(bytes).map_err(|_| error(code, "UTF-8 파일이 필요함"))
}
/// The definition diagnostic registry of the product path, for `aip explain-code` and `aip diagnostics --definition`.
pub use spike_v1_fixture::codes as definition_registry;
/// The codes of the product HTTP API (retry and fix), for `aip explain-code`.
pub use spike_v6_transport::wire_codes as wire_registry;
pub fn load(file: &Path, dev_checks: bool) -> Result<spike_v11_dev_checks::Checked, Value> {
    let form = file.to_str().and_then(spike_v1_fixture::form_of).ok_or_else(|| error("UNSUPPORTED_FORM", "지원하는 작성 형식이 아님"))?;
    let source = read_bounded(file, SOURCE_BYTES, "SOURCE_IO")?;
    spike_v11_dev_checks::load_checked(&source, form, &json!({"devChecks":dev_checks})).map_err(
        |ds| json!({"ok":false,"code":"INVALID_DEFINITION","diagnostics":ds.iter().map(spike_v1_fixture::diag::Diag::json).collect::<Vec<_>>()}),
    )
}
fn configuration(path: &Path) -> Result<Config, Value> {
    let source = read_bounded(path, CONFIG_BYTES, "CONFIG_IO")?;
    let mut config: Config = serde_json::from_str(&source).map_err(|e| json_error("BAD_CONFIG", "제품 설정 형식 오류", &e))?;
    aip_migrate::configure_schema(&config.schema)?;
    if !config.listen.ip().is_loopback() {
        return Err(error("BAD_LISTEN", "TLS reverse proxy 뒤의 loopback listener가 필요함"));
    }
    if config.database_url_env.is_empty() || !config.database_url_env.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(error("BAD_CONFIG", "database_url_env는 영문자·숫자·'_'로 된 환경 변수 이름이어야 함(DB URL 자체가 아님)"));
    }
    spike_v6_transport::server::ServerOptions { allowed_origins: config.allowed_origins.clone() }.validate_product().map_err(|_| {
        error(
            "BAD_ORIGIN",
            "allowed_origins는 정규화된 HTTPS origin(또는 로컬 HTTP origin)이어야 함: 경로·끝 슬래시·와일드카드 없이 scheme://host[:port]",
        )
    })?;
    let parent = path.parent().unwrap_or(Path::new("."));
    if let aip_auth::JwksSource::File { path } = &mut config.auth.jwks
        && path.is_relative()
    {
        *path = parent.join(&*path);
    }
    if let Some(path) = &mut config.database_ca_file
        && path.is_relative()
    {
        *path = parent.join(&*path);
    }
    if let Some(worker) = &mut config.workers
        && worker.directory.is_relative()
    {
        worker.directory = parent.join(&worker.directory);
    }
    if let Some(worker) = &config.workers {
        validate_integrations_shape(&worker.integrations)?;
    }
    Ok(config)
}

fn valid_env_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && (name.as_bytes()[0].is_ascii_uppercase() || name.as_bytes()[0] == b'_')
        && name.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

/// The first rule an integration breaks, in words: which setting and what is allowed. Names and limits are
/// not secrets; the URL and secret values are never part of the text.
fn integration_shape_problem(item: &IntegrationConfig) -> Option<String> {
    let max_body = aip_execution::integrations::MAX_BODY_BYTES;
    let name = &item.name;
    if item.name.is_empty()
        || item.name.len() > 128
        || !item.name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Some("name은 1~128자의 영문자·숫자·'.'·'_'·'-'여야 함".to_string());
    }
    if item.timeout_ms == 0 || item.timeout_ms > 30_000 {
        return Some(format!("'{name}': timeout_ms는 1~30000이어야 함(받은 값 {})", item.timeout_ms));
    }
    if item.max_request_bytes == 0 || item.max_request_bytes > max_body {
        return Some(format!("'{name}': max_request_bytes는 1~{max_body}이어야 함(받은 값 {})", item.max_request_bytes));
    }
    if item.max_response_bytes == 0 || item.max_response_bytes > max_body {
        return Some(format!("'{name}': max_response_bytes는 1~{max_body}이어야 함(받은 값 {})", item.max_response_bytes));
    }
    if item.secret_header_env.as_ref().is_some_and(|secret| !valid_env_name(&secret.env)) {
        return Some(format!("'{name}': secret_header_env.env는 대문자·숫자·'_'로 된 환경 변수 이름이어야 함"));
    }
    if let Some(breaker) = &item.circuit_breaker {
        let max_open = aip_execution::integrations::MAX_BREAKER_OPEN.as_millis();
        if breaker.failure_threshold == 0 || breaker.failure_threshold > aip_execution::integrations::MAX_BREAKER_THRESHOLD {
            return Some(format!("'{name}': circuit_breaker.failure_threshold는 1~1000이어야 함(받은 값 {})", breaker.failure_threshold));
        }
        if breaker.open_ms == 0 || u128::from(breaker.open_ms) > max_open {
            return Some(format!("'{name}': circuit_breaker.open_ms는 1~{max_open}이어야 함(받은 값 {})", breaker.open_ms));
        }
    }
    if let Some(oauth) = &item.oauth2 {
        if !valid_env_name(&oauth.client_id_env) || !valid_env_name(&oauth.client_secret_env) {
            return Some(format!("'{name}': oauth2.client_id_env·client_secret_env는 대문자·숫자·'_'로 된 환경 변수 이름이어야 함"));
        }
        if oauth.scope.as_ref().is_some_and(|scope| scope.is_empty() || scope.len() > 1024 || scope.chars().any(char::is_control)) {
            return Some(format!("'{name}': oauth2.scope는 1~1024자이고 제어 문자가 없어야 함"));
        }
        if item.secret_header_env.as_ref().is_some_and(|secret| secret.name.eq_ignore_ascii_case("authorization")) {
            return Some(format!("'{name}': oauth2를 쓰면 secret_header_env로 authorization 헤더를 설정할 수 없음"));
        }
    }
    None
}

fn validate_integrations_shape(configs: &[IntegrationConfig]) -> Result<(), Value> {
    if configs.len() > 64 {
        return Err(error("INTEGRATION_CONFIG", &format!("외부 연동 설정 형식 오류: integrations는 64개 이하여야 함(받은 수 {})", configs.len())));
    }
    match configs.iter().find_map(integration_shape_problem) {
        Some(problem) => Err(error("INTEGRATION_CONFIG", &format!("외부 연동 설정 형식 오류: {problem}"))),
        None => Ok(()),
    }
}

/// An integration that cannot be resolved, named by its configured name (never by its URL, which may carry a
/// credential) and with the step that failed.
fn integration_resolution_error(name: &str, step: &str) -> Value {
    error("INTEGRATION_CONFIG", &format!("외부 연동 '{name}' 검증 실패: {step}"))
}

async fn resolve_integrations(configs: &[IntegrationConfig]) -> Result<Vec<aip_execution::integrations::EndpointConfig>, Value> {
    validate_integrations_shape(configs)?;
    let mut endpoints = Vec::with_capacity(configs.len());
    for config in configs {
        let resolved = resolve_destination(&config.name, "url", &config.url).await?;
        let oauth = match &config.oauth2 {
            Some(oauth) => {
                let secret = |env: &str, field: &str| {
                    std::env::var(env).ok().filter(|value| !value.is_empty() && value.len() <= 8192).ok_or_else(|| {
                        integration_resolution_error(
                            &config.name,
                            &format!("oauth2.{field}가 가리키는 환경 변수가 없거나 1~8192바이트가 아님(값은 출력하지 않음)"),
                        )
                    })
                };
                Some(aip_execution::integrations::OAuthConfig {
                    resolved: resolve_destination(&config.name, "oauth2.token_url", &oauth.token_url).await?,
                    token_url: oauth.token_url.clone(),
                    client_id: secret(&oauth.client_id_env, "client_id_env")?,
                    client_secret: secret(&oauth.client_secret_env, "client_secret_env")?,
                    scope: oauth.scope.clone(),
                    basic_auth: oauth.auth_method == OAuthClientAuth::ClientSecretBasic,
                })
            }
            None => None,
        };
        let secret_header = config
            .secret_header_env
            .as_ref()
            .map(|secret| {
                std::env::var(&secret.env).map(|value| (secret.name.clone(), value)).map_err(|_| {
                    integration_resolution_error(
                        &config.name,
                        "secret_header_env가 가리키는 환경 변수가 설정되어 있지 않음(변수 이름은 출력하지 않음)",
                    )
                })
            })
            .transpose()?;
        if secret_header.as_ref().is_some_and(|(_, value)| value.is_empty() || value.len() > 8192) {
            return Err(integration_resolution_error(&config.name, "secret_header_env의 값은 1~8192바이트여야 함(값은 출력하지 않음)"));
        }
        endpoints.push(aip_execution::integrations::EndpointConfig {
            name: config.name.clone(),
            url: config.url.clone(),
            method: config.method.clone(),
            timeout: std::time::Duration::from_millis(config.timeout_ms),
            max_request_bytes: config.max_request_bytes,
            max_response_bytes: config.max_response_bytes,
            secret_header,
            resolved,
            allow_loopback: config.allow_loopback,
            allow_private_networks: config.allow_private_networks,
            circuit_breaker: config.circuit_breaker.as_ref().map(|breaker| aip_execution::integrations::CircuitBreaker {
                failure_threshold: breaker.failure_threshold,
                open_for: std::time::Duration::from_millis(breaker.open_ms),
            }),
            oauth,
        });
    }
    Ok(endpoints)
}

async fn resolve_destination(name: &str, field: &str, raw: &str) -> Result<Vec<SocketAddr>, Value> {
    let parsed = url::Url::parse(raw).map_err(|_| integration_resolution_error(name, &format!("{field}을 해석할 수 없음(절대 URL이어야 함)")))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| integration_resolution_error(name, &format!("{field}의 포트를 정할 수 없음(https 또는 명시한 포트가 필요)")))?;
    let mut resolved = match parsed.host().ok_or_else(|| integration_resolution_error(name, &format!("{field}에 호스트가 없음")))? {
        url::Host::Ipv4(ip) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
        url::Host::Ipv6(ip) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
        url::Host::Domain(domain) => tokio::net::lookup_host((domain, port))
            .await
            .map_err(|_| integration_resolution_error(name, &format!("{field} 호스트의 DNS 조회에 실패함(호스트 이름과 서버의 DNS 접근을 확인)")))?
            .collect::<Vec<_>>(),
    };
    resolved.sort_unstable();
    resolved.dedup();
    if resolved.is_empty() || resolved.len() > 32 {
        return Err(integration_resolution_error(name, &format!("{field} 호스트가 1~32개의 주소로 해석되어야 함")));
    }
    Ok(resolved)
}

#[cfg(test)]
async fn build_broker(configs: &[IntegrationConfig]) -> Result<Option<Arc<aip_execution::integrations::Broker>>, Value> {
    build_broker_with_webhooks(configs, &[]).await
}

async fn build_broker_with_webhooks(
    configs: &[IntegrationConfig],
    webhooks: &[OutboundWebhookConfig],
) -> Result<Option<Arc<aip_execution::integrations::Broker>>, Value> {
    if configs.is_empty() && webhooks.is_empty() {
        return Ok(None);
    }
    let senders = webhooks.iter().map(webhook_sender).collect::<Result<Vec<_>, _>>()?;
    let endpoints =
        if configs.is_empty() { Vec::new() } else { with_resolution_deadline(INTEGRATION_DNS_TIMEOUT, resolve_integrations(configs)).await? };
    let broker = aip_execution::integrations::Broker::new(endpoints).map_err(|_| {
        error(
            "INTEGRATION_CONFIG",
            "외부 연동 설정 검증 실패: url은 자격 증명·fragment가 없는 https여야 하고(http는 allow_loopback과 loopback 주소일 때만), method는 GET·POST·PUT·PATCH·DELETE이며, \
             해석된 주소는 공개 주소여야 하고(사설 주소는 allow_private_networks, loopback은 allow_loopback), secret 헤더 이름은 예약어가 아니고, name은 중복되지 않아야 함",
        )
    })?;
    let broker = broker.with_webhooks(senders).map_err(|_| {
        error("WEBHOOK_CONFIG", "발신 webhook 설정 검증 실패: name은 외부 연동과 겹치지 않는 1~128자, 서명 secret은 16바이트 이상, timeout_ms는 1~30000, max_request_bytes는 1~1048576이어야 함")
    })?;
    Ok(Some(Arc::new(broker)))
}

fn webhook_sender(config: &OutboundWebhookConfig) -> Result<aip_execution::integrations::WebhookSenderConfig, Value> {
    use base64::Engine;
    let fail = |step: &str| error("WEBHOOK_CONFIG", &format!("발신 webhook '{}' 검증 실패: {step}", config.name));
    if !valid_env_name(&config.signing_secret_env) {
        return Err(fail("signing_secret_env는 대문자·숫자·'_'로 된 환경 변수 이름이어야 함"));
    }
    let raw =
        std::env::var(&config.signing_secret_env).map_err(|_| fail("signing_secret_env가 가리키는 환경 변수가 없음(변수 이름은 출력하지 않음)"))?;
    // Standard Webhooks secrets are written whsec_<base64>; anything else is used as raw key bytes.
    let secret = match raw.strip_prefix("whsec_") {
        Some(encoded) => base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| fail("whsec_ secret의 base64가 올바르지 않음"))?,
        None => raw.into_bytes(),
    };
    Ok(aip_execution::integrations::WebhookSenderConfig {
        name: config.name.clone(),
        secret,
        timeout: std::time::Duration::from_millis(config.timeout_ms),
        max_request_bytes: config.max_request_bytes,
        allow_loopback: config.allow_loopback,
    })
}

async fn with_resolution_deadline<T, F>(deadline: std::time::Duration, future: F) -> Result<T, Value>
where
    F: Future<Output = Result<T, Value>>,
{
    tokio::time::timeout(deadline, future)
        .await
        .map_err(|_| error("INTEGRATION_CONFIG", "외부 연동 DNS 조회가 기한 안에 끝나지 않음(호스트 이름과 서버의 DNS 접근을 확인)"))?
}
fn database_url(config: &Config) -> Result<String, Value> {
    if let Some(path) = &config.database_ca_file {
        let pem = read_bounded(path, CONFIG_BYTES, "DB_CA_IO")?;
        spike_v2_read::install_ca_bundle(pem.as_bytes()).map_err(|_| error("DB_CA_CONFIG", "DB CA bundle 형식 또는 프로세스 설정 충돌"))?;
    }
    std::env::var(&config.database_url_env)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| error("DB_CONFIG", "설정에 지정한 DB 환경 변수가 필요함"))
}
fn report(value: &Value) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{value}")?;
    out.flush()
}
fn output_error(value: &Value) {
    let _ = writeln!(std::io::stderr().lock(), "{value}");
}

fn write_binding(file: &Path, out: &Path, content: &str) -> Result<(), Value> {
    let source = file.canonicalize().map_err(|_| error("SOURCE_IO", "정의 경로 확인 실패"))?;
    if let Ok(target) = out.canonicalize() {
        let mut conflict = source == target;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(a), Ok(b)) = (fs::metadata(file), fs::metadata(out)) {
                conflict |= a.dev() == b.dev() && a.ino() == b.ino();
            }
        }
        if conflict {
            return Err(error("OUTPUT_CONFLICT", "정의 원본에 생성 계약을 쓸 수 없음"));
        }
    }
    let directory = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| error("OUTPUT_IO", "시계 오류"))?.as_nanos();
    let tmp = directory.join(format!(".aip-gen-{}-{nonce}.tmp", std::process::id()));
    let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|_| error("OUTPUT_IO", "출력 임시 파일 생성 실패"))?;
    let result = f.write_all(content.as_bytes()).and_then(|_| f.sync_all()).and_then(|_| fs::rename(&tmp, out));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|_| error("OUTPUT_IO", "생성 계약 교체 실패"))
}

pub async fn run(command: Command) -> Result<Value, Value> {
    match command {
        Command::Check { file, dev_checks } => {
            let checked = load(&file, dev_checks)?;
            Ok(
                json!({"ok":true,"executionDigest":spike_v1_fixture::digest(&checked.output.execution),"advisories":checked.advisories.iter().map(|a|json!({"code":a.code,"anchor":a.anchor,"message":a.msg})).collect::<Vec<_>>()}),
            )
        }
        Command::Gen { file, wire, out, sdk_import } => {
            let facts = load(&file, false)?.output.execution;
            if sdk_import.trim().is_empty() {
                return Err(error("BAD_SDK_IMPORT", "SDK import 경로가 필요함"));
            }
            let content = spike_v5_sdk::contract_module_with_all_extensions(&facts, &sdk_import, wire.id_wire());
            write_binding(&file, &out, &content)?;
            Ok(
                json!({"ok":true,"fingerprint":spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts,wire.id_wire()),"idWire":wire.id_wire().label()}),
            )
        }
        Command::Init { file, config } => {
            let c = configuration(&config)?;
            let facts = load(&file, false)?.output.execution;
            aip_migrate::init_with_wire(&database_url(&c)?, &c.schema, &facts, c.wire.deployment_label()).await
        }
        Command::Migrate { file, config, migration, apply } => {
            let c = configuration(&config)?;
            let facts = load(&file, false)?.output.execution;
            let migration = migration
                .map(|p| {
                    read_bounded(&p, SOURCE_BYTES, "MIGRATION_IO")
                        .and_then(|text| serde_json::from_str(&text).map_err(|e| json_error("BAD_MIGRATION", "이행 선언 JSON 형식 오류", &e)))
                })
                .transpose()?;
            match apply {
                Some(hash) => aip_migrate::apply(&database_url(&c)?, &c.schema, &facts, &migration, &hash).await,
                None => aip_migrate::plan(&database_url(&c)?, &c.schema, &facts, &migration).await,
            }
        }
        Command::Adopt { file, config, apply } => {
            let c = configuration(&config)?;
            let facts = load(&file, false)?.output.execution;
            match apply {
                Some(hash) => aip_migrate::adopt::apply_with_wire(&database_url(&c)?, &c.schema, &facts, &hash, c.wire.deployment_label()).await,
                None => aip_migrate::adopt::plan_with_wire(&database_url(&c)?, &c.schema, &facts, c.wire.deployment_label()).await,
            }
        }
        Command::Serve { file, config } => serve(&file, configuration(&config)?).await,
        Command::Principal { config, command } => principal(configuration(&config)?, command).await,
        Command::Jobs { config, command } => jobs(configuration(&config)?, command).await,
    }
}

async fn jobs(config: Config, command: Jobs) -> Result<Value, Value> {
    let mut db = tokio::time::timeout(std::time::Duration::from_secs(5), spike_v2_read::connect_owned_with_url(&database_url(&config)?))
        .await
        .map_err(|_| error("DB_CONNECT", "DB 연결 기한 초과"))?
        .map_err(|_| error("DB_CONNECT", "DB 연결 실패"))?;
    let schema = config.schema.clone();
    let store = aip_execution::Store::new(&schema).map_err(|_| error("JOBS_FAILED", "schema 이름 오류"))?;
    let operation = async {
        let tx = db.transaction().await.map_err(|_| error("JOBS_FAILED", "트랜잭션 시작 실패"))?;
        // The shared deployment lock keeps a concurrent migrate from changing the digest mid-command.
        tx.execute("SELECT pg_advisory_xact_lock_shared(1095323725, hashtext($1))", &[&schema])
            .await
            .map_err(|_| error("JOBS_FAILED", "배포 잠금 실패"))?;
        let facts: Value = tx
            .query_one(&format!("SELECT facts FROM {schema}.aip_migrate_meta WHERE singleton=true FOR SHARE"), &[])
            .await
            .map_err(|_| error("JOBS_FAILED", "배포 정의 조회 실패"))?
            .get(0);
        let jobs_table: bool = tx
            .query_one(
                "SELECT to_regclass($1) IS NOT NULL AND to_regclass($2) IS NOT NULL",
                &[&format!("{schema}.aip_jobs"), &format!("{schema}.aip_effects")],
            )
            .await
            .map_err(|_| error("JOBS_FAILED", "job 테이블 확인 실패"))?
            .get(0);
        if !jobs_table {
            return Err(error("JOBS_UNAVAILABLE", "이 배포에는 durable job이 없음"));
        }
        let unsettled = format!("SELECT count(*) FROM {schema}.aip_effects WHERE run_id=$1::text::uuid AND state IN ('PENDING','UNKNOWN')");
        let result = match command {
            Jobs::List { state, operation, limit } => {
                let runs = store
                    .list(&tx, state.as_deref(), operation.as_deref(), limit)
                    .await
                    .map_err(|_| error("BAD_JOBS_QUERY", "state는 QUEUED·RUNNING·CANCEL_REQUESTED·SUCCEEDED·FAILED·CANCELLED, limit은 1에서 500"))?;
                let mut out = Vec::with_capacity(runs.len());
                for run in runs {
                    let pending: i64 = tx.query_one(&unsettled, &[&run.id]).await.map_err(|_| error("JOBS_FAILED", "effect 조회 실패"))?.get(0);
                    let mut value = serde_json::to_value(&run).map_err(|_| error("JOBS_FAILED", "결과 직렬화 실패"))?;
                    value["unsettledEffects"] = json!(pending);
                    out.push(value);
                }
                json!({"ok":true,"jobs":out})
            }
            Jobs::Retry { id } => {
                // A redrive would hit the same uncertain provider call; the operator reconciles it first.
                let pending: i64 = tx.query_one(&unsettled, &[&id]).await.map_err(|_| error("BAD_JOB_ID", "job id는 UUID여야 함"))?.get(0);
                if pending > 0 {
                    return Err(error("EFFECT_UNKNOWN", "결과를 모르는 외부 효과가 있어 재처리하지 않음; provider에서 결과를 확인해야 함"));
                }
                let outcome =
                    store.redrive(&tx, &id, &spike_v1_fixture::digest(&facts)).await.map_err(|_| error("BAD_JOB_ID", "job id는 UUID여야 함"))?;
                match outcome {
                    aip_execution::Redrive::Queued => json!({"ok":true,"id":id,"state":"QUEUED"}),
                    aip_execution::Redrive::NotFound => return Err(error("JOB_NOT_FOUND", "job이 없음")),
                    aip_execution::Redrive::NotFailed => return Err(error("JOB_NOT_FAILED", "FAILED 상태의 job만 재처리할 수 있음")),
                    aip_execution::Redrive::Expired => {
                        return Err(error("JOB_EXPIRED", "입력 보존 기간이 지나 재처리할 수 없음; 새 job으로 제출해야 함"));
                    }
                    aip_execution::Redrive::DeploymentChanged => {
                        return Err(error("DEPLOYMENT_CHANGED", "다른 배포에서 수락한 job이라 재처리할 수 없음; 새 job으로 제출해야 함"));
                    }
                }
            }
        };
        tx.commit().await.map_err(|_| error("JOBS_UNSETTLED", "커밋 결과 불명; 상태 확인 필요"))?;
        Ok(result)
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), operation)
        .await
        .map_err(|_| error("JOBS_UNSETTLED", "job 명령 결과 불명; 상태 확인 필요"))?
}

async fn principal(config: Config, command: Principal) -> Result<Value, Value> {
    let mut db = tokio::time::timeout(std::time::Duration::from_secs(5), spike_v2_read::connect_owned_with_url(&database_url(&config)?))
        .await
        .map_err(|_| error("DB_CONNECT", "DB 연결 기한 초과"))?
        .map_err(|_| error("DB_CONNECT", "DB 연결 실패"))?;
    let issuer = &config.auth.issuer;
    let binding = matches!(&command, Principal::Bind { .. });
    let operation = async {
        let tx = db.transaction().await.map_err(|_| error("PRINCIPAL_FAILED", "사용자 연결 트랜잭션 시작 실패"))?;
        tx.execute("SELECT pg_advisory_xact_lock_shared(1095323725, hashtext($1))", &[&config.schema])
            .await
            .map_err(|_| error("PRINCIPAL_FAILED", "배포 신원 계약 잠금 실패"))?;
        let marker = tx
            .query_one(&format!("SELECT facts, runtime_wire FROM {}.aip_migrate_meta WHERE singleton=true FOR SHARE", config.schema), &[])
            .await
            .map_err(|_| error("PRINCIPAL_FAILED", "배포 신원 계약 조회 실패"))?;
        let facts: Value = marker.get(0);
        let stored_wire: Option<String> = marker.get(1);
        if stored_wire.as_deref().is_some_and(|wire| wire != config.wire.deployment_label()) {
            return Err(error("WIRE_MODE_MISMATCH", "기존 배포 wire와 설정 wire가 다름"));
        }
        let managed = facts["actorMode"] == "principal";
        let count=match command {
            Principal::Bind{subject,actor,not_before}=>{
                if subject.is_empty()||subject.len()>256||actor<=0||not_before<0{return Err(error("BAD_PRINCIPAL","subject·actor·not-before 값 오류"));}
                if managed {
                    if config.wire.id_wire() == IdWire::SafeNumber && actor > spike_v2_read::id_wire::MAX_SAFE_ID {
                        return Err(error("BAD_PRINCIPAL", "safe wire의 actor 안전 정수 상한 초과"));
                    }
                    tx.execute(&format!("INSERT INTO {}.aip_actors(id) VALUES($1) ON CONFLICT DO NOTHING", config.schema), &[&actor])
                        .await.map_err(|_| error("PRINCIPAL_FAILED", "운영 신원 등록 실패"))?;
                }
                tx.execute(&format!("INSERT INTO {}.aip_principals(issuer,subject,actor_id,enabled,min_iat) VALUES($1,$2,$3,true,$4) ON CONFLICT(issuer,subject) DO UPDATE SET enabled=true,min_iat=GREATEST(aip_principals.min_iat,EXCLUDED.min_iat) WHERE aip_principals.actor_id=EXCLUDED.actor_id",config.schema),&[issuer,&subject,&actor,&not_before]).await
            },
            Principal::Revoke{subject}=>{
                if subject.is_empty()||subject.len()>256{return Err(error("BAD_PRINCIPAL","subject 값 오류"));}
                tx.execute(&format!("UPDATE {}.aip_principals SET enabled=false,min_iat=GREATEST(min_iat,floor(extract(epoch FROM clock_timestamp()))::bigint+1) WHERE issuer=$1 AND subject=$2",config.schema),&[issuer,&subject]).await
            },
        }.map_err(|_|error("PRINCIPAL_FAILED","actor 존재·매핑 테이블·DB 권한을 확인해야 함"))?;
        if count == 0 && binding {
            return Err(error("PRINCIPAL_CONFLICT", "이미 연결된 신원의 actor를 바꿀 수 없음"));
        }
        if count == 0 {
            return Err(error("PRINCIPAL_NOT_FOUND", "연결된 신원이 없음"));
        }
        tx.commit().await.map_err(|_| error("PRINCIPAL_UNSETTLED", "사용자 연결 커밋 결과 불명; 상태 확인 필요"))?;
        Ok(json!({"ok":true,"affected":count}))
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), operation)
        .await
        .map_err(|_| error("PRINCIPAL_UNSETTLED", "사용자 연결 변경 결과 불명; 상태 확인 필요"))?
}

/// Rejects configuration that needs an optional package this binary was built without.
/// Runs before any database or network access so the operator sees the missing feature first.
fn require_built_features(config: &Config) -> Result<(), Value> {
    #[cfg(not(feature = "external-brokers"))]
    if config.broker.is_some() {
        return Err(error("BROKER_FEATURE_DISABLED", "broker 설정을 사용하려면 external-brokers feature로 빌드해야 함"));
    }
    let _ = config;
    Ok(())
}

async fn serve(file: &Path, config: Config) -> Result<Value, Value> {
    require_built_features(&config)?;
    let facts = load(file, false)?.output.execution;
    let mut infrastructure = infrastructure::build(&config)?;
    let mut built =
        triggers::build(&config.schedules, &config.webhooks, &config.outbox_handlers, &config.auth.issuer, &|name| std::env::var(name).ok())
            .map_err(|message| error("TRIGGER_CONFIG", message))?;
    built.messages = triggers::build_message_handlers(&config.message_handlers, &config.auth.issuer);
    infrastructure.triggers = Arc::new(built);
    infrastructure.job_policies = triggers::build_job_policies(&config.job_policies).map_err(|message| error("JOB_POLICY_CONFIG", message))?;
    let integrations = config.workers.as_ref().map(|workers| workers.integrations.as_slice()).unwrap_or_default();
    let outbound_webhooks = config.workers.as_ref().map(|workers| workers.outbound_webhooks.as_slice()).unwrap_or_default();
    let broker = build_broker_with_webhooks(integrations, outbound_webhooks).await?;
    let db_url = database_url(&config)?;
    aip_migrate::preflight(&db_url, &config.schema, &facts).await?;
    aip_migrate::bind_wire(&db_url, &config.schema, config.wire.deployment_label()).await?;
    if let Some(rag_config) = &config.rag {
        let (engine, client_index) =
            tokio::time::timeout(std::time::Duration::from_secs(5), rag::build(rag_config, &db_url, &config.schema, infrastructure.db_pool))
                .await
                .map_err(|_| error("RAG_CONFIG", "RAG 초기화가 5초 안에 끝나지 않음(embedding·answer provider나 DB 응답을 확인)"))?
                .map_err(|failure| error("RAG_CONFIG", &format!("RAG 설정 오류: {failure}")))?;
        infrastructure.rag = Some(engine);
        infrastructure.rag_client_index = client_index;
    }

    #[cfg(feature = "external-brokers")]
    let mut broker_worker = if let Some(broker_config) = config.broker {
        if broker_config.declared_queues().any(|queue| !facts_declare_message_queue(&facts, queue)) {
            return Err(error("BROKER_CAPABILITY", "broker route는 배포에 선언된 messages.<queue> capability여야 함"));
        }
        Some(
            brokers::Worker::start(broker_config, db_url.clone(), config.schema.clone())
                .await
                .map_err(|reason| error("BROKER_CONFIG", &format!("broker 설정 오류: {reason}")))?,
        )
    } else {
        None
    };
    #[cfg(not(feature = "external-brokers"))]
    let mut broker_worker: Option<brokers::Worker> = None;
    let deployment = spike_v6_transport::server::DeploymentFence::new(config.schema.clone(), spike_v1_fixture::digest(&facts))
        .map_err(|_| error("SCHEMA_CONTEXT", "배포 fence 설정 실패"))?;
    sqlgen::try_set_schema(&config.schema).map_err(|_| error("SCHEMA_CONTEXT", "프로세스당 한 schema만 실행할 수 있음"))?;
    let auth = Arc::new(
        aip_auth::AuthenticatorImpl::new_explained(config.auth, db_url.clone(), config.schema)
            .await
            .map_err(|failure| error("AUTH_CONFIG", &format!("인증 설정 오류: {}", failure.reason)))?,
    );
    let enable_writes = config.workers.as_ref().is_some_and(|w| w.enable_writes);
    let enable_file_uploads = config.workers.as_ref().is_some_and(|w| w.enable_file_uploads);
    let extensions = config.workers.map(|w| spike_v6_transport::server::ReadExtensions {
        dir: w.directory,
        lang: match w.language {
            Language::Node => spike_v6_transport::server::WorkerLang::Node,
            Language::Python => spike_v6_transport::server::WorkerLang::Python,
        },
    });
    let options = spike_v6_transport::server::ServerOptions { allowed_origins: config.allowed_origins };
    #[cfg(unix)]
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|_| error("SIGNAL_CONFIG", "종료 signal 등록 실패"))?;
    let mut server = spike_v6_transport::server::listen_with_infrastructure(
        facts.clone(),
        config.wire.id_wire(),
        config.listen,
        db_url,
        extensions,
        options,
        enable_writes,
        auth,
        Some(deployment),
        broker,
        enable_file_uploads,
        infrastructure,
    )
    .await
    .map_err(|e| server_start_error(&e))?;
    let event = json!({"ok":true,"event":"listening","address":server.address.to_string(),"fingerprint":spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts,config.wire.id_wire()),"idWire":config.wire.id_wire().label(),"authentication":"jwt-access-token"});
    if report(&event).is_err() {
        server.shutdown().await.map_err(|_| error("SERVER_STOP", "출력 실패 후 종료 실패"))?;
        #[cfg(feature = "external-brokers")]
        if let Some(worker) = broker_worker.as_mut() {
            worker.shutdown().await.map_err(|_| error("BROKER_STOP", "출력 실패 후 broker worker 종료 실패"))?;
        }
        return Err(error("OUTPUT_IO", "기동 결과 출력 실패"));
    }
    #[cfg(unix)]
    tokio::select! {result=tokio::signal::ctrl_c()=>{result.map_err(|_|error("SIGNAL_FAILED","종료 signal 실패"))?;server.shutdown().await.map_err(|_|error("SERVER_STOP","서버 종료 실패"))?;},_=termination.recv()=>{server.shutdown().await.map_err(|_|error("SERVER_STOP","서버 종료 실패"))?;},result=server.wait()=>{result.map_err(|_|error("SERVER_FAILED","서버 실행 실패"))?;},result=async { match broker_worker.as_mut() { Some(worker)=>worker.wait().await, None=>std::future::pending().await } }=>{server.shutdown().await.map_err(|_|error("SERVER_STOP","broker 종료 후 서버 종료 실패"))?;result.map_err(|_|error("BROKER_FAILED","broker worker 실패"))?;return Err(error("BROKER_FAILED","broker worker가 예기치 않게 종료됨"));}}
    #[cfg(not(unix))]
    tokio::select! {result=tokio::signal::ctrl_c()=>{result.map_err(|_|error("SIGNAL_FAILED","종료 signal 실패"))?;server.shutdown().await.map_err(|_|error("SERVER_STOP","서버 종료 실패"))?;},result=server.wait()=>{result.map_err(|_|error("SERVER_FAILED","서버 실행 실패"))?;},result=async { match broker_worker.as_mut() { Some(worker)=>worker.wait().await, None=>std::future::pending().await } }=>{server.shutdown().await.map_err(|_|error("SERVER_STOP","broker 종료 후 서버 종료 실패"))?;result.map_err(|_|error("BROKER_FAILED","broker worker 실패"))?;return Err(error("BROKER_FAILED","broker worker가 예기치 않게 종료됨"));}}
    #[cfg(feature = "external-brokers")]
    if let Some(worker) = broker_worker.as_mut() {
        worker.shutdown().await.map_err(|_| error("BROKER_STOP", "broker worker bounded 종료 실패"))?;
    }
    Ok(json!({"ok":true,"event":"stopped"}))
}

#[cfg(feature = "external-brokers")]
fn facts_declare_message_queue(facts: &Value, queue: &str) -> bool {
    fn visit(value: &Value, key: &str) -> bool {
        match value {
            Value::Object(object) => {
                object.get("access").and_then(Value::as_object).is_some_and(|access| access.get(key) == Some(&Value::Bool(true)))
                    || object.values().any(|child| visit(child, key))
            }
            Value::Array(array) => array.iter().any(|child| visit(child, key)),
            _ => false,
        }
    }
    visit(facts, &format!("messages.{queue}"))
}

pub fn execute(command: Command) -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(_) => {
            output_error(&error("RUNTIME_START", "Rust runtime 기동 실패"));
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(command)) {
        Ok(value) => match report(&value) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
            Err(_) => {
                output_error(&error("OUTPUT_IO", "결과 출력 실패"));
                ExitCode::FAILURE
            }
        },
        Err(value) => {
            output_error(&value);
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod managed_config_tests {
    use super::*;
    use serde_json::json;

    fn config_error(text: &str) -> String {
        let dir = std::env::temp_dir().join(format!("aip-config-error-{}-{}", std::process::id(), text.len()));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let file = dir.join("service.json");
        std::fs::write(&file, text).expect("write");
        let result = configuration(&file);
        std::fs::remove_dir_all(&dir).ok();
        let error = result.err().expect("the configuration is rejected");
        assert_eq!(error["ok"], false);
        error["message"].as_str().expect("message").to_string()
    }

    /// The reason of a rejected configuration names the key, so that the file can be repaired from the output.
    #[test]
    fn a_rejected_configuration_names_the_key_and_the_position() {
        let unknown = config_error("{\n\"schema\":\"app\",\n\"lissten\":\"127.0.0.1:1\"\n}");
        assert!(unknown.contains("unknown field `lissten`") && unknown.contains("listen"), "{unknown}");
        assert!(unknown.contains("줄 3"), "{unknown}");
        let missing = config_error("{\"auth\":{\"issuer\":\"i\",\"audience\":\"a\",\"jwks\":{\"kind\":\"file\",\"path\":\"j\"}}}");
        assert!(missing.contains("missing field `schema`"), "{missing}");
        let wrong_type = config_error("{\"schema\":5}");
        assert!(wrong_type.contains("invalid type: integer, expected a string"), "{wrong_type}");
        let broken = config_error("{\"schema\":\"app\"");
        assert!(broken.starts_with("제품 설정 형식 오류: EOF while parsing"), "{broken}");
    }

    /// The failed start is logged, and a configuration file may hold a secret: the offending value never appears.
    #[test]
    fn a_rejected_configuration_never_echoes_the_value() {
        let secret = "s3cr3t-VALUE-0123456789";
        for text in [
            format!("{{\"schema\":\"{secret}\",\"database_url_env\":7}}"),
            format!("{{\"schema\":\"app\",\"listen\":\"{secret}\"}}"),
            format!("{{\"schema\":\"app\",\"wire\":\"{secret}\"}}"),
            format!("{{\"schema\":\"app\",\"auth\":\"{secret}\"}}"),
            format!("{{\"schema\":\"app\",\"object_transfer_lifetime_ms\":\"{secret}\"}}"),
        ] {
            let message = config_error(&text);
            assert!(!message.contains(secret), "{text}: {message}");
        }
    }

    #[test]
    fn json_reasons_keep_structure_and_drop_values() {
        assert_eq!(redact_json_reason("invalid type: string \"abc\", expected u64 at line 1 column 9"), "invalid type: string, expected u64");
        assert_eq!(
            redact_json_reason("invalid value: string \"abc\", expected a socket address at line 1 column 9"),
            "invalid value: string, expected a socket address"
        );
        assert_eq!(redact_json_reason("missing field `schema` at line 1 column 2"), "missing field `schema`");
        assert_eq!(redact_json_reason("invalid socket address syntax at line 2 column 5"), "a value is not accepted");
    }

    #[test]
    fn an_integration_error_names_the_setting_and_never_the_url() {
        let item = |name: &str, timeout_ms: u64| IntegrationConfig {
            name: name.to_string(),
            url: "https://user:pass@api.example/hook?token=hunter2".to_string(),
            method: "POST".to_string(),
            timeout_ms,
            max_request_bytes: 1024,
            max_response_bytes: 1024,
            secret_header_env: None,
            allow_loopback: false,
            allow_private_networks: false,
            circuit_breaker: None,
            oauth2: None,
        };
        let error = validate_integrations_shape(&[item("crm", 90_000)]).unwrap_err();
        let message = error["message"].as_str().expect("message");
        assert!(message.contains("'crm'") && message.contains("timeout_ms") && message.contains("90000"), "{message}");
        assert!(!message.contains("hunter2") && !message.contains("user:pass"), "{message}");
        let error = validate_integrations_shape(&[item("bad name", 1000)]).unwrap_err();
        assert!(error["message"].as_str().expect("message").contains("name은"), "{error}");
        assert!(validate_integrations_shape(&[item("crm", 1000)]).is_ok());
    }

    #[test]
    fn server_infrastructure_accepts_explicit_backends_and_runtime_overrides() {
        let value = json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
            "static_mounts":[{"prefix":"/media","directory":"./public","access":"public"}],
            "file_mounts":[{"prefix":"/reports","bucket":"reports"}],
            "objects":{"kind":"local","directory":"./objects","max_bytes":1073741824},
            "realtime":{"enabled":true,"max_connections":12,"max_lifetime_ms":60000,"heartbeat_ms":1000,"poll_ms":100},
            "messaging":{"enabled":true},
            "workers":{"directory":"extensions","language":"python","runtime_executable":"./.venv/bin/python","environment":{"PYTHONPATH":"./plugins"},"isolation":"auto"}
        });
        assert!(serde_json::from_value::<Config>(value).is_ok(), "explicit infrastructure configuration must be supported");
    }

    #[test]
    fn managed_file_mount_config_is_built_and_rejects_reserved_routes() {
        let config: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
            "file_mounts":[{"prefix":"/reports","bucket":"reports"}]
        }))
        .unwrap();
        let built = infrastructure::build(&config).unwrap();
        assert_eq!(built.file_mounts.len(), 1);
        assert_eq!(built.file_mounts[0].prefix, "/reports");
        assert_eq!(built.file_mounts[0].bucket, "reports");

        let reserved: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
            "file_mounts":[{"prefix":"/files","bucket":"reports"}]
        }))
        .unwrap();
        assert_eq!(infrastructure::build(&reserved).err().unwrap()["code"], "INFRASTRUCTURE_CONFIG");
    }

    #[test]
    fn db_pool_rate_limit_gcs_and_worker_cgroups_are_configurable() {
        let defaults: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}}
        }))
        .unwrap();
        let baseline = infrastructure::build(&defaults).unwrap();
        assert_eq!(baseline.db_pool, spike_v6_transport::pool::PoolOptions::default());
        assert!(baseline.rate_limit.is_none());

        let access_env = format!("AIP_TEST_GCS_ACCESS_{}", std::process::id());
        let secret_env = format!("AIP_TEST_GCS_SECRET_{}", std::process::id());
        let config: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
            "db_pool":{"max_size":24,"acquire_timeout_ms":3500,"idle_timeout_ms":120000},
            "rate_limit":{"limit":70,"window_ms":60000},
            "objects":{"kind":"gcs_hmac","bucket":"reports","access_key_env":access_env,"secret_key_env":secret_env},
            "workers":{
                "directory":"extensions",
                "language":"python",
                "cgroup_root":"/sys/fs/cgroup/aip-workers",
                "resource_limits":{"memory_bytes":67108864,"pids_max":16,"cpu_quota_micros":50000,"cpu_period_micros":100000}
            }
        }))
        .unwrap();
        let built = infrastructure::build_with_environment(&config, &|name| match name {
            n if n == access_env => Some("test-access-id".to_owned()),
            n if n == secret_env => Some("test-secret-key".to_owned()),
            _ => None,
        })
        .unwrap();
        assert_eq!(built.db_pool.max_size, 24);
        assert_eq!(built.db_pool.acquire_timeout, std::time::Duration::from_millis(3500));
        assert_eq!(built.db_pool.idle_timeout, std::time::Duration::from_millis(120000));
        assert_eq!(built.rate_limit.unwrap().limit, 70);
        assert!(matches!(built.objects, Some(spike_v6_transport::infrastructure::ObjectStorage::GcsHmac(_))));
        assert_eq!(built.worker_launch.cgroup_root.as_deref(), Some(std::path::Path::new("/sys/fs/cgroup/aip-workers")));
        let resource_limits = built.worker_launch.resource_limits.unwrap();
        assert_eq!(resource_limits.memory_bytes, 67108864);
        assert_eq!(resource_limits.pids_max, 16);
        assert_eq!(resource_limits.cpu_quota_micros, 50000);
        assert_eq!(resource_limits.cpu_period_micros, 100000);
    }

    #[test]
    fn invalid_db_pool_rate_limit_and_unpaired_cgroup_settings_are_rejected() {
        for invalid in [
            json!({"db_pool":{"max_size":0}}),
            json!({"db_pool":{"acquire_timeout_ms":30001}}),
            json!({"rate_limit":{"limit":0,"window_ms":1000}}),
            json!({"workers":{"directory":"extensions","language":"python","cgroup_root":"/sys/fs/cgroup/aip-workers"}}),
            json!({"workers":{"directory":"extensions","language":"python","resource_limits":{"memory_bytes":64,"pids_max":1,"cpu_quota_micros":1,"cpu_period_micros":1}}}),
        ] {
            let mut value = json!({
                "schema":"aip_test",
                "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}}
            });
            for (key, field) in invalid.as_object().unwrap() {
                value[key] = field.clone();
            }
            let config: Config = serde_json::from_value(value).unwrap();
            assert_eq!(infrastructure::build(&config).err().unwrap()["code"], "INFRASTRUCTURE_CONFIG");
        }
    }

    #[test]
    fn resource_limit_overrides_are_honored_and_zero_values_are_rejected() {
        let defaults: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}}
        }))
        .unwrap();
        let defaults = infrastructure::build(&defaults).unwrap();
        assert_eq!(defaults.resource_limits, spike_v6_transport::infrastructure::ResourceLimits::default());

        let config: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
            "limits":{"connections":20,"workers":3,"jobs":5,"uploads":2}
        }))
        .unwrap();
        let built = infrastructure::build(&config).unwrap();
        assert_eq!(built.resource_limits.connections, 20);
        assert_eq!(built.resource_limits.workers, 3);
        assert_eq!(built.resource_limits.jobs, 5);
        assert_eq!(built.resource_limits.uploads, 2);

        let partial: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
            "limits":{"jobs":5}
        }))
        .unwrap();
        let partial = infrastructure::build(&partial).unwrap();
        assert_eq!(partial.resource_limits, spike_v6_transport::infrastructure::ResourceLimits { connections: 64, workers: 4, jobs: 5, uploads: 4 });

        for invalid_limits in [json!({"connections":0}), json!({"workers":0}), json!({"jobs":0}), json!({"uploads":0})] {
            let invalid: Config = serde_json::from_value(json!({
                "schema":"aip_test",
                "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
                "limits":invalid_limits
            }))
            .unwrap();
            assert_eq!(infrastructure::build(&invalid).err().unwrap()["code"], "INFRASTRUCTURE_CONFIG");
        }
    }

    #[cfg(not(feature = "external-brokers"))]
    #[test]
    fn broker_config_is_rejected_when_the_feature_is_not_built() {
        let config: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
            "broker":{"providers":[],"outbound":[],"inbound":[]}
        }))
        .unwrap();
        let rejected = require_built_features(&config).unwrap_err();
        assert_eq!(rejected["code"], "BROKER_FEATURE_DISABLED");
    }

    #[test]
    fn client_messaging_access_defaults_to_denied_and_can_be_enabled_separately() {
        let config: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
            "messaging":{"enabled":true}
        }))
        .unwrap();
        let built = infrastructure::build(&config).unwrap();
        assert!(built.messaging);
        assert!(!built.client_publish);
        assert!(!built.client_consume);

        let config: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
            "messaging":{"enabled":true,"client_publish":true}
        }))
        .unwrap();
        let built = infrastructure::build(&config).unwrap();
        assert!(built.client_publish);
        assert!(!built.client_consume);

        let config: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
            "messaging":{"enabled":true,"client_consume":true}
        }))
        .unwrap();
        let built = infrastructure::build(&config).unwrap();
        assert!(!built.client_publish);
        assert!(built.client_consume);
    }

    #[test]
    fn object_transfer_lifetime_defaults_and_overrides_are_applied_and_bounded() {
        use std::time::Duration;

        let defaults: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}}
        }))
        .unwrap();
        assert_eq!(defaults.object_transfer_lifetime_ms, 900_000);
        assert_eq!(infrastructure::build(&defaults).unwrap().object_transfer_lifetime, Duration::from_secs(900));

        let custom: Config = serde_json::from_value(json!({
            "schema":"aip_test",
            "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
            "object_transfer_lifetime_ms": 3000
        }))
        .unwrap();
        assert_eq!(infrastructure::build(&custom).unwrap().object_transfer_lifetime, Duration::from_secs(3));

        for invalid_ms in [0, 999, 86_400_001] {
            let invalid: Config = serde_json::from_value(json!({
                "schema":"aip_test",
                "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}},
                "object_transfer_lifetime_ms": invalid_ms
            }))
            .unwrap();
            assert_eq!(infrastructure::build(&invalid).err().unwrap()["code"], "INFRASTRUCTURE_CONFIG");
        }
    }

    #[test]
    fn old_worker_settings_keep_new_capabilities_disabled_by_default() {
        let workers: Workers = serde_json::from_value(json!({
            "directory": "extensions",
            "language": "node",
            "enable_writes": true
        }))
        .unwrap();
        assert!(workers.enable_writes);
        assert!(!workers.enable_file_uploads);
        assert!(workers.integrations.is_empty());
    }

    #[test]
    fn integration_settings_reject_unknown_fields_and_bad_secret_environment_names() {
        assert!(
            serde_json::from_value::<IntegrationConfig>(json!({
                "name": "crm.lookup",
                "url": "https://crm.example/api",
                "method": "GET",
                "timeout_ms": 1000,
                "max_request_bytes": 1024,
                "max_response_bytes": 1024,
                "arbitrary_header": "not allowed"
            }))
            .is_err()
        );

        let invalid: IntegrationConfig = serde_json::from_value(json!({
            "name": "crm.lookup",
            "url": "https://crm.example/api",
            "method": "GET",
            "timeout_ms": 1000,
            "max_request_bytes": 1024,
            "max_response_bytes": 1024,
            "secret_header_env": {"name":"x-api-key", "env":"bad-name"}
        }))
        .unwrap();
        assert_eq!(validate_integrations_shape(&[invalid]).unwrap_err()["code"], "INTEGRATION_CONFIG");
    }

    #[test]
    fn private_network_access_defaults_to_denied_and_is_endpoint_configurable() {
        let defaulted: IntegrationConfig = serde_json::from_value(json!({
            "name": "internal.billing",
            "url": "https://billing.internal/api",
            "method": "POST",
            "timeout_ms": 1000,
            "max_request_bytes": 1024,
            "max_response_bytes": 1024
        }))
        .unwrap();
        assert!(!defaulted.allow_private_networks);

        let enabled: IntegrationConfig = serde_json::from_value(json!({
            "name": "internal.billing",
            "url": "https://billing.internal/api",
            "method": "POST",
            "timeout_ms": 1000,
            "max_request_bytes": 1024,
            "max_response_bytes": 1024,
            "allow_private_networks": true
        }))
        .unwrap();
        assert!(enabled.allow_private_networks);
    }

    #[tokio::test]
    async fn configured_broker_pins_loopback_only_when_explicitly_enabled() {
        let mut config: IntegrationConfig = serde_json::from_value(json!({
            "name": "local.test",
            "url": "http://127.0.0.1:32123/hook",
            "method": "POST",
            "timeout_ms": 1000,
            "max_request_bytes": 1024,
            "max_response_bytes": 1024
        }))
        .unwrap();
        assert!(build_broker(&[config]).await.is_err());

        config = serde_json::from_value(json!({
            "name": "local.test",
            "url": "http://127.0.0.1:32123/hook",
            "method": "POST",
            "timeout_ms": 1000,
            "max_request_bytes": 1024,
            "max_response_bytes": 1024,
            "allow_loopback": true
        }))
        .unwrap();
        let broker = build_broker(&[config]).await.unwrap().unwrap();
        assert_eq!(broker.fingerprint().len(), 64);
    }

    #[tokio::test]
    async fn private_https_broker_requires_and_honors_endpoint_opt_in() {
        let endpoint = json!({
            "name": "billing.internal",
            "url": "https://10.24.0.8/api",
            "method": "POST",
            "timeout_ms": 1000,
            "max_request_bytes": 1024,
            "max_response_bytes": 1024
        });
        let defaulted: IntegrationConfig = serde_json::from_value(endpoint.clone()).unwrap();
        assert!(build_broker(&[defaulted]).await.is_err());

        let configured: IntegrationConfig = serde_json::from_value(json!({
            "name": "billing.internal",
            "url": "https://10.24.0.8/api",
            "method": "POST",
            "timeout_ms": 1000,
            "max_request_bytes": 1024,
            "max_response_bytes": 1024,
            "allow_private_networks": true
        }))
        .unwrap();
        assert!(build_broker(&[configured]).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn secret_is_loaded_only_from_explicit_environment_reference() {
        let config: IntegrationConfig = serde_json::from_value(json!({
            "name": "local.test",
            "url": "http://127.0.0.1:32124/hook",
            "method": "POST",
            "timeout_ms": 1000,
            "max_request_bytes": 1024,
            "max_response_bytes": 1024,
            "secret_header_env": {"name":"x-api-key", "env":"AIP_TEST_MISSING_INTEGRATION_SECRET_9371"},
            "allow_loopback": true
        }))
        .unwrap();
        let error = build_broker(&[config]).await.err().unwrap();
        assert_eq!(error["code"], "INTEGRATION_CONFIG");
        assert!(!error.to_string().contains("AIP_TEST_MISSING_INTEGRATION_SECRET_9371"));

        let configured_secret: IntegrationConfig = serde_json::from_value(json!({
            "name": "local.test",
            "url": "http://127.0.0.1:32124/hook",
            "method": "POST",
            "timeout_ms": 1000,
            "max_request_bytes": 1024,
            "max_response_bytes": 1024,
            "secret_header_env": {"name":"x-api-key", "env":"PATH"},
            "allow_loopback": true
        }))
        .unwrap();
        assert!(build_broker(&[configured_secret]).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn dns_resolution_deadline_returns_a_generic_configuration_error() {
        let result = with_resolution_deadline(std::time::Duration::from_millis(1), async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            Ok(())
        })
        .await;
        assert_eq!(result.unwrap_err()["code"], "INTEGRATION_CONFIG");
    }

    #[test]
    fn a_listener_that_did_not_start_says_which_kind_of_failure_and_leaks_no_value() {
        use std::io::{Error, ErrorKind};
        let causes = [
            Error::new(
                ErrorKind::InvalidInput,
                "local extension configuration: extension `stats` needs stats.mjs in the worker directory, which does not exist",
            ),
            Error::new(ErrorKind::AddrInUse, "Address already in use (os error 48) 127.0.0.1:4310"),
            Error::new(ErrorKind::NotFound, "No such file or directory /opt/secret-runtime/node"),
            Error::new(ErrorKind::PermissionDenied, "Permission denied /var/run/secret-socket"),
            Error::other("worker runtime probe exited with exit status: 1; stdout=\"\"; stderr=\"token=abc123\""),
        ];
        let mut sentences = std::collections::HashSet::new();
        for cause in &causes {
            let value = server_start_error(cause);
            assert_eq!(value["code"], "SERVER_CONFIG");
            let message = value["message"].as_str().unwrap().to_owned();
            assert!(!message.contains("secret") && !message.contains("abc123") && !message.contains("4310"), "{message}");
            sentences.insert(message);
        }
        assert_eq!(sentences.len(), causes.len(), "each cause has its own sentence");
        let validation = server_start_error(&causes[0])["message"].as_str().unwrap().to_owned();
        assert!(validation.contains("stats.mjs"), "{validation}");
    }
}
