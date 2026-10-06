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
const REMOTE_TIMEOUT: Duration = Duration::from_secs(2);
const DB_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub issuer: String,
    pub audience: String,
    pub jwks: JwksSource,
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

#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedClaims {
    pub issuer: String,
    pub subject: String,
    pub issued_at: i64,
    pub expires_at: i64,
}

struct KeyState {
    keys: HashMap<String, Arc<DecodingKey>>,
    loaded: Instant,
    last_attempt: Option<Instant>,
    failed_until: Option<Instant>,
}

pub struct AuthenticatorImpl {
    config: Config,
    db_url: String,
    schema: String,
    http: reqwest::Client,
    keys: Mutex<KeyState>,
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
        if modulus.first() == Some(&0) || !(2048..=4096).contains(&bits) || exponent.is_empty() || exponent.len() > 8 {
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

impl AuthenticatorImpl {
    pub async fn new(config: Config, db_url: String, schema: String) -> Result<Self, AuthError> {
        if !valid_identifier(&schema) || config.audience.is_empty() || config.audience.len() > 256 || db_url.is_empty() {
            return Err(AuthError::Invalid);
        }
        let issuer = reqwest::Url::parse(&config.issuer).map_err(|_| AuthError::Invalid)?;
        if issuer.scheme() != "https"
            || issuer.host_str().is_none()
            || !issuer.username().is_empty()
            || issuer.password().is_some()
            || issuer.fragment().is_some()
            || issuer.query().is_some()
        {
            return Err(AuthError::Invalid);
        }
        match &config.jwks {
            JwksSource::File { path } if path.as_os_str().is_empty() => return Err(AuthError::Invalid),
            JwksSource::Https { url } => {
                let parsed = reqwest::Url::parse(url).map_err(|_| AuthError::Invalid)?;
                if parsed.scheme() != "https"
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err(AuthError::Invalid);
                }
            }
            _ => {}
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REMOTE_TIMEOUT)
            .build()
            .map_err(|_| AuthError::Unavailable)?;
        let keys = parse_jwks(&read_jwks(&config.jwks, &http).await?)?;
        let now = Instant::now();
        Ok(Self { config, db_url, schema, http, keys: Mutex::new(KeyState { keys, loaded: now, last_attempt: None, failed_until: None }) })
    }

    async fn key_for(&self, kid: &str) -> Result<Arc<DecodingKey>, AuthError> {
        let mut state = self.keys.lock().await;
        let file = matches!(self.config.jwks, JwksSource::File { .. });
        let expired = state.loaded.elapsed() >= KEY_TTL;
        let unknown = !state.keys.contains_key(kid);
        if !file && state.failed_until.is_some_and(|until| Instant::now() < until) {
            return Err(AuthError::Unavailable);
        }
        let refresh_unknown = unknown && state.last_attempt.is_none_or(|at| at.elapsed() >= UNKNOWN_KID_REFRESH);
        if file || expired || refresh_unknown {
            state.last_attempt = Some(Instant::now());
            match read_jwks(&self.config.jwks, &self.http).await.and_then(|bytes| parse_jwks(&bytes)) {
                Ok(fresh) => {
                    state.keys = fresh;
                    state.loaded = Instant::now();
                    state.failed_until = None;
                }
                Err(error) => {
                    if !file {
                        state.failed_until = Some(Instant::now() + UNKNOWN_KID_REFRESH);
                    }
                    return Err(error);
                }
            }
        }
        state.keys.get(kid).cloned().ok_or(AuthError::Invalid)
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
        validation.set_required_spec_claims(&["iss", "aud", "sub", "exp", "nbf"]);
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
            || claims.nbf < 0
            || claims.exp < 0
            || claims.iat > now
            || claims.nbf > now
            || claims.exp <= now
            || !matches!(claims.exp.checked_sub(claims.iat), Some(1..=900))
        {
            return Err(if claims.exp <= now { AuthError::Expired } else { AuthError::Invalid });
        }
        Ok(VerifiedClaims { issuer: claims.iss, subject: claims.sub, issued_at: claims.iat, expires_at: claims.exp })
    }

    pub async fn session_for(&self, token: &str) -> Result<spike_v6_transport::auth::Session, AuthError> {
        let claims = self.verify_token(token).await?;
        let db = tokio::time::timeout(DB_TIMEOUT, spike_v2_read::connect_owned_with_url(&self.db_url))
            .await
            .map_err(|_| AuthError::Unavailable)?
            .map_err(|_| AuthError::Unavailable)?;
        let sql = format!("SELECT actor_id, enabled, min_iat FROM \"{}\".aip_principals WHERE issuer=$1 AND subject=$2", self.schema);
        let row = tokio::time::timeout(DB_TIMEOUT, db.query_opt(&sql, &[&claims.issuer, &claims.subject]))
            .await
            .map_err(|_| AuthError::Unavailable)?
            .map_err(|_| AuthError::Unavailable)?;
        let row = row.ok_or(AuthError::Invalid)?;
        let actor_id: i64 = row.try_get(0).map_err(|_| AuthError::Unavailable)?;
        let enabled: bool = row.try_get(1).map_err(|_| AuthError::Unavailable)?;
        let min_iat: i64 = row.try_get(2).map_err(|_| AuthError::Unavailable)?;
        if !enabled || claims.issued_at < min_iat {
            return Err(AuthError::Invalid);
        }
        spike_v6_transport::auth::Session::new(actor_id, claims.expires_at).map_err(|error| match error {
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
    nbf: i64,
    exp: i64,
}

impl spike_v6_transport::auth::Authenticator for AuthenticatorImpl {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn local_tls_response() -> (u16, std::thread::JoinHandle<bool>) {
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
            let body = br#"{"keys":[]}"#;
            let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            tls.write_all(reply.as_bytes()).expect("write TLS response headers");
            tls.write_all(body).expect("write TLS JWKS body");
            tls.flush().expect("flush TLS JWKS response");
            true
        });
        (port, worker)
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
}
