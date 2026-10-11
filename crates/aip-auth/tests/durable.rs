use aip_auth::{AuthenticatorImpl, Config, JwksSource};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rand::thread_rng;
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::json;
use spike_v6_transport::auth::Authenticator;
use std::process::Command;

struct SchemaGuard(String);

impl Drop for SchemaGuard {
    fn drop(&mut self) {
        let _ = psql(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.0));
    }
}

fn psql(sql: &str) -> std::process::Output {
    Command::new("psql").args(["-X", "-q", "-d", "host=localhost dbname=postgres", "-Atc", sql]).output().expect("test setup")
}

fn mapped_schema() -> SchemaGuard {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("test setup").as_nanos();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let schema = format!("aip_durable_auth_{}_{}_{}", std::process::id(), stamp, sequence);
    let created = psql(&format!(
        "CREATE SCHEMA {schema}; CREATE TABLE {schema}.actor(id bigint PRIMARY KEY); INSERT INTO {schema}.actor VALUES(42), (43); CREATE TABLE {schema}.aip_principals(issuer text NOT NULL,subject text NOT NULL,actor_id bigint NOT NULL REFERENCES {schema}.actor(id),enabled boolean NOT NULL,min_iat bigint NOT NULL DEFAULT 0,PRIMARY KEY(issuer,subject)); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id,enabled) VALUES('https://issuer.example','user-1',42,true)"
    ));
    assert!(created.status.success(), "{}", String::from_utf8_lossy(&created.stderr));
    SchemaGuard(schema)
}

fn fixture() -> (tempfile::TempDir, Config, EncodingKey) {
    let dir = tempfile::tempdir().expect("test setup");
    let private = RsaPrivateKey::new(&mut thread_rng(), 2048).expect("test setup");
    let public = private.to_public_key();
    let jwks = json!({"keys":[{"kty":"RSA","kid":"current","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]});
    let path = dir.path().join("jwks.json");
    std::fs::write(&path, jwks.to_string()).expect("test setup");
    let key = EncodingKey::from_rsa_der(private.to_pkcs1_der().expect("test setup").as_bytes());
    (dir, Config { issuer: "https://issuer.example".into(), audience: "aip-api".into(), jwks: JwksSource::File { path } }, key)
}

#[tokio::test]
async fn durable_grant_survives_token_expiry_but_not_principal_revocation() {
    let schema = mapped_schema();
    let (_dir, config, key) = fixture();
    let auth = AuthenticatorImpl::new(config, "host=localhost dbname=postgres".into(), schema.0.clone()).await.expect("test setup");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("test setup").as_secs() as i64;
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("at+jwt".into());
    header.kid = Some("current".into());
    let jwt = encode(&header, &json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-1","iat":now,"nbf":now,"exp":now+2}), &key)
        .expect("test setup");
    let grant = auth.session_for(&jwt).await.expect("test setup").durable.expect("product grant");
    assert_eq!(grant.actor_id, 42);
    assert_eq!(grant.issued_at, now);
    assert_eq!(grant.min_iat, 0);
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert!(auth.session_for(&jwt).await.is_err());
    assert!(auth.verify_durable(&grant).await.is_ok());

    let revoked = psql(&format!("UPDATE {}.aip_principals SET enabled=false,min_iat={} WHERE subject='user-1'", schema.0, now + 1));
    assert!(revoked.status.success());
    assert!(auth.verify_durable(&grant).await.is_err());
    let rebound = psql(&format!("UPDATE {}.aip_principals SET enabled=true WHERE subject='user-1'", schema.0));
    assert!(rebound.status.success());
    assert!(auth.verify_durable(&grant).await.is_err());
}

#[tokio::test]
async fn durable_grant_is_bound_to_subject_actor_and_revocation_floor() {
    let schema = mapped_schema();
    let (_dir, config, key) = fixture();
    let auth = AuthenticatorImpl::new(config, "host=localhost dbname=postgres".into(), schema.0.clone()).await.expect("test setup");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("test setup").as_secs() as i64;
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("at+jwt".into());
    header.kid = Some("current".into());
    let jwt = encode(&header, &json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-1","iat":now,"nbf":now,"exp":now+60}), &key)
        .expect("test setup");
    let grant = auth.session_for(&jwt).await.expect("test setup").durable.expect("product grant");
    let mut forged = grant.clone();
    forged.subject = "someone-else".into();
    assert!(auth.verify_durable(&forged).await.is_err());
    forged = grant.clone();
    forged.actor_id = 43;
    assert!(auth.verify_durable(&forged).await.is_err());
    forged = grant.clone();
    forged.min_iat = 1;
    assert!(auth.verify_durable(&forged).await.is_err());
    forged = grant;
    forged.issued_at = -1;
    assert!(auth.verify_durable(&forged).await.is_err());
}
