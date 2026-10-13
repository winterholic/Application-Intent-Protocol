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
        let (shared, counter, flag) = (jwks.clone(), hits.clone(), stop.clone());
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
        Self { port, jwks, hits, stop, thread: Some(thread) }
    }

    fn publish(&self, jwks: String) {
        *self.jwks.lock().expect("mock IdP state") = jwks;
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
            config,
            db_url: "host=unused".into(),
            schema: "aip_app".into(),
            http,
            keys: Mutex::new(KeyState { keys, loaded: Instant::now(), last_attempt: None, failed_until: None }),
            lookups: tokio::sync::Semaphore::new(MAX_LOOKUP_CONNECTIONS),
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
