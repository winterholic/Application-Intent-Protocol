pub mod id_wire;
pub mod plan;
pub mod scalar;
pub mod sqlgen;

use plan::{Plan, Reject};
use std::{
    error::Error as StdError,
    fmt,
    io::Cursor,
    net::IpAddr,
    str::FromStr,
    sync::{Arc, OnceLock},
};
use tokio_postgres::{
    config::{Config, Host, SslMode},
    types::ToSql,
    Client, NoTls,
};

pub const DB_URL: &str = "host=localhost dbname=postgres";
const MAX_CA_BUNDLE_BYTES: usize = 64 * 1024;
static DATABASE_CA_BUNDLE: OnceLock<Vec<u8>> = OnceLock::new();

#[derive(Debug)]
pub enum ConnectError {
    Policy(&'static str),
    Postgres(tokio_postgres::Error),
    TlsConfig(String),
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy(message) => f.write_str(message),
            Self::Postgres(error) => error.fmt(f),
            Self::TlsConfig(error) => write!(f, "TLS configuration failed: {error}"),
        }
    }
}

impl StdError for ConnectError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Policy(_) => None,
            Self::Postgres(error) => Some(error),
            Self::TlsConfig(_) => None,
        }
    }
}

fn is_loopback_name(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("localhost.")
        || host.parse::<IpAddr>().is_ok_and(|address| address.is_loopback())
}

fn is_local_target(config: &Config) -> bool {
    let hosts = config.get_hosts();
    if hosts.is_empty() {
        return false;
    }

    let hostaddrs = config.get_hostaddrs();
    hosts.iter().enumerate().all(|(index, host)| {
        let named_host_is_local = match host {
            Host::Tcp(name) => is_loopback_name(name),
            Host::Unix(_) => true,
        };
        let address_is_local = hostaddrs.get(index).is_none_or(IpAddr::is_loopback);
        named_host_is_local && address_is_local
    })
}

fn parse_ca_bundle(pem: &[u8]) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, ConnectError> {
    if pem.len() > MAX_CA_BUNDLE_BYTES {
        return Err(ConnectError::Policy("database CA bundle exceeds 64 KiB"));
    }
    let mut reader = Cursor::new(pem);
    let mut certificates = Vec::new();
    while let Some(item) = rustls_pemfile::read_one(&mut reader).map_err(|error| ConnectError::TlsConfig(format!("invalid CA PEM: {error}")))? {
        match item {
            rustls_pemfile::Item::X509Certificate(certificate) => certificates.push(certificate),
            _ => return Err(ConnectError::TlsConfig("CA PEM may contain certificates only".into())),
        }
    }
    if certificates.is_empty() {
        return Err(ConnectError::TlsConfig("CA PEM contains no certificates".into()));
    }
    Ok(certificates)
}

/// Install one process-wide additive CA bundle before opening database connections.
/// Public web PKI roots and rustls hostname verification remain enabled.
pub fn install_ca_bundle(pem: &[u8]) -> Result<(), ConnectError> {
    if let Some(installed) = DATABASE_CA_BUNDLE.get() {
        return if installed.as_slice() == pem { Ok(()) } else { Err(ConnectError::Policy("database CA bundle is already configured differently")) };
    }

    let certificates = parse_ca_bundle(pem)?;
    let mut validation_roots = rustls::RootCertStore::empty();
    validation_roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for certificate in certificates {
        validation_roots.add(certificate).map_err(|error| ConnectError::TlsConfig(error.to_string()))?;
    }

    match DATABASE_CA_BUNDLE.set(pem.to_vec()) {
        Ok(()) => Ok(()),
        Err(attempted) if DATABASE_CA_BUNDLE.get().is_some_and(|installed| installed == &attempted) => Ok(()),
        Err(_) => Err(ConnectError::Policy("database CA bundle is already configured differently")),
    }
}

fn tls_connector(ca_cert_pem: Option<&[u8]>) -> Result<tokio_postgres_rustls::MakeRustlsConnect, ConnectError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let protocol_builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| ConnectError::TlsConfig(error.to_string()))?;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = ca_cert_pem {
        for certificate in parse_ca_bundle(pem)? {
            roots.add(certificate).map_err(|error| ConnectError::TlsConfig(error.to_string()))?;
        }
    }
    let config = protocol_builder.with_root_certificates(roots).with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}

fn spawn_driver(connection: impl std::future::Future<Output = Result<(), tokio_postgres::Error>> + Send + 'static) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if connection.await.is_err() {
            eprintln!("AIP DB connection closed with an error");
        }
    })
}

async fn open_connection(url: &str, ca_cert_pem: Option<&[u8]>) -> Result<(Client, tokio::task::JoinHandle<()>), ConnectError> {
    let config = Config::from_str(url).map_err(ConnectError::Postgres)?;
    let local = is_local_target(&config);
    let require_tls = config.get_ssl_mode() == SslMode::Require;
    if !local && !require_tls {
        return Err(ConnectError::Policy("remote PostgreSQL connection requires sslmode=require"));
    }

    let (client, driver) = if local && !require_tls {
        let (client, connection) = config.connect(NoTls).await.map_err(ConnectError::Postgres)?;
        (client, spawn_driver(connection))
    } else {
        let (client, connection) = config.connect(tls_connector(ca_cert_pem)?).await.map_err(ConnectError::Postgres)?;
        (client, spawn_driver(connection))
    };
    Ok((client, driver))
}

pub async fn connect_with_url(url: &str) -> Result<Client, ConnectError> {
    let (client, _driver) = open_connection(url, DATABASE_CA_BUNDLE.get().map(Vec::as_slice)).await?;
    Ok(client)
}

/// Cancelling a query does not drain a silent PostgreSQL response. Keep its driver owned.
pub struct OwnedConnection {
    client: Client,
    driver: tokio::task::JoinHandle<()>,
}

impl std::ops::Deref for OwnedConnection {
    type Target = Client;
    fn deref(&self) -> &Client {
        &self.client
    }
}

impl std::ops::DerefMut for OwnedConnection {
    fn deref_mut(&mut self) -> &mut Client {
        &mut self.client
    }
}

impl Drop for OwnedConnection {
    fn drop(&mut self) {
        // Abort schedules local socket cleanup; it does not establish a database commit outcome.
        self.driver.abort();
    }
}

pub async fn connect_owned_with_url(url: &str) -> Result<OwnedConnection, ConnectError> {
    let (client, driver) = open_connection(url, DATABASE_CA_BUNDLE.get().map(Vec::as_slice)).await?;
    Ok(OwnedConnection { client, driver })
}

/// Connect with a per-call PEM CA bundle in addition to public roots. For a process-wide
/// database CA, install it once with [`install_ca_bundle`] before opening connections.
pub async fn connect_owned_with_url_and_ca(url: &str, ca_cert_pem: &[u8]) -> Result<OwnedConnection, ConnectError> {
    let (client, driver) = open_connection(url, Some(ca_cert_pem)).await?;
    Ok(OwnedConnection { client, driver })
}

pub async fn connect() -> Client {
    connect_with_url(DB_URL).await.expect("로컬 PostgreSQL 연결")
}

/// 읽기 전용 트랜잭션 + statement_timeout으로 실행한다. DB 오류 원문은 호출자에게 보내지 않는다.
pub async fn execute(client: &mut Client, plan: &Plan) -> Result<Vec<serde_json::Value>, Reject> {
    let tx = client.transaction().await.map_err(|_| Reject { code: "INTERNAL", msg: "트랜잭션 시작 실패".into() })?;
    tx.batch_execute(&format!("SET TRANSACTION READ ONLY; SET LOCAL statement_timeout = '{}ms'; SET LOCAL TimeZone = 'UTC'", plan.deadline_ms))
        .await
        .map_err(|_| Reject { code: "INTERNAL", msg: "세션 설정 실패".into() })?;
    let params: Vec<&(dyn ToSql + Sync)> = plan.params.iter().map(|p| p as &(dyn ToSql + Sync)).collect();
    let rows = match tx.query(plan.sql.as_str(), &params).await {
        Ok(r) => r,
        Err(e) => {
            let code = e.code().map(|c| c.code().to_string()).unwrap_or_default();
            return Err(match code.as_str() {
                "57014" => Reject { code: "DEADLINE_EXCEEDED", msg: format!("{}ms 안에 끝나지 않음", plan.deadline_ms) },
                c if c.starts_with("22") => Reject { code: "BAD_VALUE", msg: "값 변환 실패".into() },
                _ => Reject { code: "INTERNAL", msg: format!("실행 실패(sqlstate {code})") },
            });
        }
    };
    tx.commit().await.ok();
    let mut bytes = 0usize;
    let mut out = vec![];
    for r in &rows {
        match r.get::<_, Option<String>>(0) {
            Some(s) => {
                bytes += s.len();
                if bytes > plan::MAX_OUTPUT_BYTES {
                    return Err(Reject { code: "OUTPUT_TOO_LARGE", msg: format!("응답이 {} bytes 상한을 넘음", plan::MAX_OUTPUT_BYTES) });
                }
                out.push(serde_json::from_str(&s).unwrap());
            }
            None if plan.null_is_denied => return Err(Reject { code: "ACCESS_DENIED", msg: "집계 접근 권한 없음".into() }),
            None => out.push(serde_json::Value::Null),
        }
    }
    Ok(out)
}
