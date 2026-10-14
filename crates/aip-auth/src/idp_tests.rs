//! Local mock identity provider: OIDC discovery document, HTTPS JWKS endpoint and key rotation.
//! It stands in for Auth0/Keycloak/Entra ID, which cannot be reached without an account
//! (see `product/IDP-RUNBOOK.md`). The mock speaks HTTPS with the repository's test CA so the
//! same `reqwest` path as production (TLS, no redirects, size limit) is exercised.
use super::*;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rand::thread_rng;
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::json;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const ISSUER: &str = "https://issuer.example";
const AUDIENCE: &str = "aip-api";

struct Idp {
    port: u16,
    jwks: Arc<std::sync::Mutex<String>>,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    failing: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Idp {
    fn start(initial_jwks: String) -> Self {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};
        let cert = CertificateDer::from(include_bytes!("../../../spikes/spike-v2-read/tests/certs/server.der").to_vec());
        let key = PrivateKeyDer::try_from(include_bytes!("../../../spikes/spike-v2-read/tests/certs/server-key.der").to_vec()).expect("test key");
        let config = Arc::new(rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(vec![cert], key).expect("TLS config"));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock IdP");
        let port = listener.local_addr().expect("address").port();
        let jwks = Arc::new(std::sync::Mutex::new(initial_jwks));
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let failing = Arc::new(AtomicBool::new(false));
        let (shared, counter, flag, down) = (jwks.clone(), hits.clone(), stop.clone(), failing.clone());
        let thread = std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                stream.set_read_timeout(Some(REMOTE_TIMEOUT)).ok();
                stream.set_write_timeout(Some(REMOTE_TIMEOUT)).ok();
                let Ok(connection) = rustls::ServerConnection::new(config.clone()) else { continue };
                let mut tls = rustls::StreamOwned::new(connection, stream);
                let mut request = [0_u8; 4096];
                let Ok(read) = tls.read(&mut request) else { continue };
                let head = String::from_utf8_lossy(&request[..read]).into_owned();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let body = match path.as_str() {
                    "/.well-known/openid-configuration" => json!({
                        "issuer": ISSUER,
                        "jwks_uri": format!("https://localhost:{port}/jwks"),
                        "id_token_signing_alg_values_supported": ["RS256"],
                    })
                    .to_string(),
                    "/jwks" if down.load(Ordering::SeqCst) => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        let _ = tls.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        let _ = tls.flush();
                        continue;
                    }
                    "/jwks" => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        shared.lock().expect("mock IdP state").clone()
                    }
                    _ => String::new(),
                };
                let status = if body.is_empty() { "404 Not Found" } else { "200 OK" };
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(reply.as_bytes());
                let _ = tls.flush();
            }
        });
        Self { port, jwks, hits, stop, failing, thread: Some(thread) }
    }

    fn publish(&self, jwks: String) {
        *self.jwks.lock().expect("mock IdP state") = jwks;
    }

    fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    fn fetches(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    fn client(&self) -> reqwest::Client {
        let root = reqwest::Certificate::from_pem(include_bytes!("../../../spikes/spike-v2-read/tests/certs/test-ca.pem")).expect("test CA");
        reqwest::Client::builder()
            .add_root_certificate(root)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REMOTE_TIMEOUT)
            .no_proxy()
            .resolve("localhost", ([127, 0, 0, 1], self.port).into())
            .build()
            .expect("TLS test client")
    }

    /// Builds the authenticator exactly as `new_explained` would, but trusting the test CA.
    async fn authenticator(&self, jwks_url: &str) -> AuthenticatorImpl {
        let config = Config { issuer: ISSUER.into(), audience: AUDIENCE.into(), jwks: JwksSource::Https { url: jwks_url.into() } };
        let http = self.client();
        let document = read_jwks(&config.jwks, &http).await.expect("initial JWKS fetch");
        let keys = parse_jwks(&document).expect("initial JWKS parses");
        AuthenticatorImpl {
            actor_resource_sql: None,
            config,
            db_url: "host=unused".into(),
            schema: "aip_app".into(),
            http,
            keys: Mutex::new(KeyState { keys, loaded: Instant::now(), last_attempt: None, failed_until: None, file_stamp: None }),
            refresh: Mutex::new(()),
            lookups: tokio::sync::Semaphore::new(MAX_LOOKUP_CONNECTIONS),
            pool: std::sync::OnceLock::new(),
        }
    }
}

impl Drop for Idp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct SigningKey {
    kid: &'static str,
    private: RsaPrivateKey,
}

impl SigningKey {
    fn new(kid: &'static str) -> Self {
        Self { kid, private: RsaPrivateKey::new(&mut thread_rng(), 2048).expect("RSA key") }
    }

    fn jwk(&self) -> Value {
        let public = self.private.to_public_key();
        json!({"kty":"RSA","kid":self.kid,"alg":"RS256","use":"sig",
            "n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())})
    }

    fn token(&self) -> String {
        let now = now_secs().expect("clock");
        let claims = json!({"iss":ISSUER,"aud":AUDIENCE,"sub":"user-1","iat":now,"nbf":now,"exp":now + 300});
        let mut header = Header::new(Algorithm::RS256);
        header.typ = Some("at+jwt".into());
        header.kid = Some(self.kid.into());
        let der = self.private.to_pkcs1_der().expect("DER");
        encode(&header, &claims, &EncodingKey::from_rsa_der(der.as_bytes())).expect("sign")
    }
}

fn jwks(keys: &[&SigningKey]) -> String {
    json!({"keys": keys.iter().map(|key| key.jwk()).collect::<Vec<_>>()}).to_string()
}

#[tokio::test]
async fn discovery_document_yields_the_jwks_url_and_the_issuer_the_service_enforces() {
    let a = SigningKey::new("idp-2026-10");
    let idp = Idp::start(jwks(&[&a]));
    let response = idp.client().get(format!("https://localhost:{}/.well-known/openid-configuration", idp.port)).send().await.expect("discovery");
    assert!(response.status().is_success());
    let document: Value = response.json().await.expect("discovery JSON");
    // The runbook tells operators to copy these two fields into `auth.issuer` and `auth.jwks.url`.
    assert_eq!(document["issuer"], ISSUER);
    let jwks_url = document["jwks_uri"].as_str().expect("jwks_uri");
    let auth = idp.authenticator(jwks_url).await;
    assert_eq!(auth.verify_token(&a.token()).await.expect("token verifies").subject, "user-1");
}

#[tokio::test]
async fn a_new_signing_key_is_picked_up_without_a_restart_and_the_retired_key_stops_working() {
    let (a, b) = (SigningKey::new("key-a"), SigningKey::new("key-b"));
    let idp = Idp::start(jwks(&[&a]));
    let auth = idp.authenticator(&format!("https://localhost:{}/jwks", idp.port)).await;
    assert!(auth.verify_token(&a.token()).await.is_ok(), "the current key signs valid tokens");
    let before = idp.fetches();

    // Overlap phase of a rotation: both keys are published, so tokens signed by either still verify.
    idp.publish(jwks(&[&a, &b]));
    assert!(auth.verify_token(&b.token()).await.is_ok(), "an unknown kid triggers one refresh and then verifies");
    assert_eq!(idp.fetches(), before + 1, "exactly one JWKS fetch for the new kid");
    assert!(auth.verify_token(&a.token()).await.is_ok(), "the old key is still accepted during the overlap");
    assert!(auth.verify_token(&b.token()).await.is_ok(), "the new key is served from the cache");
    assert_eq!(idp.fetches(), before + 1, "cached keys do not cause more fetches");

    // Retirement: the IdP drops key-a. The cache learns it on the next refresh (TTL or an unknown kid).
    idp.publish(jwks(&[&b]));
    let c = SigningKey::new("key-c");
    idp.publish(jwks(&[&b, &c]));
    {
        // The unknown-kid refresh is rate limited; age the last attempt the way waiting 5s would.
        auth.keys.lock().await.last_attempt = Instant::now().checked_sub(UNKNOWN_KID_REFRESH);
    }
    assert!(auth.verify_token(&c.token()).await.is_ok(), "key-c arrives with the refresh that also drops key-a");
    assert_eq!(auth.verify_token(&a.token()).await, Err(AuthError::Invalid), "a retired kid is rejected after the refresh");
}

#[tokio::test]
async fn a_forged_token_for_an_unknown_kid_is_rejected_and_never_accepts_the_attacker_key() {
    let (a, attacker) = (SigningKey::new("key-a"), SigningKey::new("key-x"));
    let idp = Idp::start(jwks(&[&a]));
    let auth = idp.authenticator(&format!("https://localhost:{}/jwks", idp.port)).await;
    // The attacker signs with their own key under a kid the IdP never published.
    assert_eq!(auth.verify_token(&attacker.token()).await, Err(AuthError::Invalid));
    // Reusing a published kid with the wrong key must fail the signature check, not select the attacker key.
    let impostor = SigningKey { kid: "key-a", private: attacker.private.clone() };
    assert_eq!(auth.verify_token(&impostor.token()).await, Err(AuthError::Invalid));
    assert!(auth.verify_token(&a.token()).await.is_ok(), "legitimate tokens are unaffected");
}

#[tokio::test]
async fn a_failed_refresh_for_an_unknown_kid_does_not_block_tokens_for_cached_keys() {
    let (a, stranger) = (SigningKey::new("key-a"), SigningKey::new("key-unknown"));
    let idp = Idp::start(jwks(&[&a]));
    let auth = idp.authenticator(&format!("https://localhost:{}/jwks", idp.port)).await;
    idp.set_failing(true);
    // An unauthenticated sender picks an unknown kid, which makes the service try to refresh while the IdP is down.
    assert_eq!(auth.verify_token(&stranger.token()).await, Err(AuthError::Unavailable));
    let after_failure = idp.fetches();
    // Tokens signed by a key that is already cached must keep working; they need no refresh.
    assert!(auth.verify_token(&a.token()).await.is_ok(), "a cached key was blocked by someone else's failed refresh");
    assert_eq!(idp.fetches(), after_failure, "a cached key must not trigger another fetch");
}

#[tokio::test]
async fn a_known_key_is_still_served_for_a_while_when_the_idp_is_down_after_the_cache_expired() {
    let a = SigningKey::new("key-a");
    let idp = Idp::start(jwks(&[&a]));
    let auth = idp.authenticator(&format!("https://localhost:{}/jwks", idp.port)).await;
    idp.set_failing(true);
    auth.keys.lock().await.loaded = Instant::now().checked_sub(KEY_TTL + Duration::from_secs(1)).expect("clock");
    assert!(auth.verify_token(&a.token()).await.is_ok(), "an IdP outage right after the TTL must not log everyone out");
    // The grace is bounded: a key older than the stale limit is refused instead of trusted forever.
    auth.keys.lock().await.loaded = Instant::now().checked_sub(MAX_STALE_KEYS + Duration::from_secs(1)).expect("clock");
    auth.keys.lock().await.failed_until = None;
    assert_eq!(auth.verify_token(&a.token()).await, Err(AuthError::Unavailable));
}

async fn file_auth(path: &std::path::Path, document: &[u8]) -> AuthenticatorImpl {
    let keys = parse_jwks(document).expect("initial JWKS parses");
    AuthenticatorImpl {
        actor_resource_sql: None,
        config: Config { issuer: ISSUER.into(), audience: AUDIENCE.into(), jwks: JwksSource::File { path: path.to_owned() } },
        db_url: "host=unused".into(),
        schema: "aip_app".into(),
        http: reqwest::Client::new(),
        refresh: Mutex::new(()),
        keys: Mutex::new(KeyState { keys, loaded: Instant::now(), last_attempt: None, failed_until: None, file_stamp: file_stamp(path) }),
        lookups: tokio::sync::Semaphore::new(MAX_LOOKUP_CONNECTIONS),
        pool: std::sync::OnceLock::new(),
    }
}

#[tokio::test]
async fn file_jwks_is_parsed_once_until_the_file_changes() {
    let (a, b) = (SigningKey::new("key-a"), SigningKey::new("key-b"));
    let (mut doc_a, mut doc_b) = (jwks(&[&a]).into_bytes(), jwks(&[&b]).into_bytes());
    let width = doc_a.len().max(doc_b.len());
    doc_a.resize(width, b' ');
    doc_b.resize(width, b' ');
    let dir = std::env::temp_dir().join(format!("aip-jwks-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("jwks.json");
    std::fs::write(&path, &doc_a).expect("write JWKS");
    let auth = file_auth(&path, &doc_a).await;
    let original = std::fs::metadata(&path).expect("metadata").modified().expect("mtime");
    assert!(auth.verify_token(&a.token()).await.is_ok());

    // Same size and same mtime: the file is considered unchanged, so the cached keys are used and the file is not parsed again.
    std::fs::write(&path, &doc_b).expect("rewrite JWKS");
    std::fs::File::options().write(true).open(&path).expect("open").set_modified(original).expect("restore mtime");
    assert_eq!(auth.verify_token(&b.token()).await, Err(AuthError::Invalid), "an unchanged file stamp must not trigger a re-read");
    assert!(auth.verify_token(&a.token()).await.is_ok(), "the cached key is still in use");

    // A new mtime is a change: the next token reloads the file.
    std::fs::File::options().write(true).open(&path).expect("open").set_modified(original + Duration::from_secs(5)).expect("bump mtime");
    assert!(auth.verify_token(&b.token()).await.is_ok(), "a changed file is picked up");
    assert_eq!(auth.verify_token(&a.token()).await, Err(AuthError::Invalid), "and the removed key stops working");
    let _ = std::fs::remove_dir_all(&dir);
}
