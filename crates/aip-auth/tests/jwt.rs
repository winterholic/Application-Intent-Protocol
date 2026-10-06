use aip_auth::{AuthenticatorImpl, Config, JwksSource};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rand::thread_rng;
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
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
    let schema = format!("aip_auth_{}_{}", std::process::id(), stamp);
    let created = psql(&format!(
        "CREATE SCHEMA {schema}; CREATE TABLE {schema}.actor(id bigint PRIMARY KEY); INSERT INTO {schema}.actor VALUES(42); CREATE TABLE {schema}.aip_principals(issuer text NOT NULL,subject text NOT NULL,actor_id bigint NOT NULL REFERENCES {schema}.actor(id),enabled boolean NOT NULL,min_iat bigint NOT NULL DEFAULT 0,PRIMARY KEY(issuer,subject)); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id,enabled) VALUES('https://issuer.example','user-1',42,true)"
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
    let der = private.to_pkcs1_der().expect("test setup");
    let key = EncodingKey::from_rsa_der(der.as_bytes());
    let config = Config { issuer: "https://issuer.example".into(), audience: "aip-api".into(), jwks: JwksSource::File { path } };
    (dir, config, key)
}

fn token(key: &EncodingKey, claims: Value, typ: &str, kid: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some(typ.into());
    header.kid = Some(kid.into());
    encode(&header, &claims, key).expect("test setup")
}

#[tokio::test]
async fn accepts_only_api_access_token_profile() {
    let (_dir, config, key) = fixture();
    let auth = AuthenticatorImpl::new(config, "host=localhost dbname=postgres".into(), "aip_example".into()).await.expect("test setup");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("test setup").as_secs();
    let claims = json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-1","iat":now,"nbf":now,"exp":now+300});
    let good = token(&key, claims.clone(), "at+jwt", "current");
    assert_eq!(auth.verify_token(&good).await.expect("test setup").subject, "user-1");
    assert!(auth.verify_token(&token(&key, claims.clone(), "JWT", "current")).await.is_err());
    assert!(auth.verify_token(&token(&key, claims.clone(), "at+jwt", "missing")).await.is_err());
    let mut wrong_audience = claims.clone();
    wrong_audience["aud"] = json!("other-api");
    assert!(auth.verify_token(&token(&key, wrong_audience, "at+jwt", "current")).await.is_err());
    let mut future = claims;
    future["nbf"] = json!(now + 60);
    assert!(auth.verify_token(&token(&key, future, "at+jwt", "current")).await.is_err());
}

#[tokio::test]
async fn extreme_signed_timestamps_are_rejected_without_panicking() {
    let (_dir, config, key) = fixture();
    let auth = AuthenticatorImpl::new(config, "host=localhost dbname=postgres".into(), "aip_example".into()).await.expect("test setup");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("test setup").as_secs();
    let claims = json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-1","iat":i64::MIN,"nbf":now,"exp":now+300});
    assert!(auth.verify_token(&token(&key, claims, "at+jwt", "current")).await.is_err());
}

#[tokio::test]
async fn negative_not_before_is_rejected() {
    let (_dir, config, key) = fixture();
    let auth = AuthenticatorImpl::new(config, "host=localhost dbname=postgres".into(), "aip_example".into()).await.expect("test setup");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("test setup").as_secs();
    let claims = json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-1","iat":now,"nbf":-1,"exp":now+300});
    assert!(auth.verify_token(&token(&key, claims, "at+jwt", "current")).await.is_err());
}

#[tokio::test]
async fn configured_issuer_cannot_include_a_query() {
    let (_dir, mut config, _key) = fixture();
    config.issuer = "https://issuer.example?tracking=1".into();
    assert!(AuthenticatorImpl::new(config, "host=localhost dbname=postgres".into(), "aip_example".into()).await.is_err());
}

#[tokio::test]
async fn maps_verified_subject_to_existing_actor_and_checks_revocation_each_call() {
    let schema = mapped_schema();
    let (_dir, config, key) = fixture();
    let auth = AuthenticatorImpl::new(config, "host=localhost dbname=postgres".into(), schema.0.clone()).await.expect("test setup");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("test setup").as_secs();
    let claims = json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-1","iat":now,"nbf":now,"exp":now+300});
    let token = token(&key, claims, "at+jwt", "current");
    assert_eq!(auth.session_for(&token).await.expect("test setup").actor_id, 42);
    let disabled = psql(&format!("UPDATE {}.aip_principals SET enabled=false", schema.0));
    assert!(disabled.status.success());
    assert!(auth.session_for(&token).await.is_err());
    let revoked = psql(&format!("UPDATE {}.aip_principals SET enabled=true,min_iat={}", schema.0, now + 1));
    assert!(revoked.status.success());
    assert!(auth.session_for(&token).await.is_err());
}

#[tokio::test]
async fn file_jwks_rotation_replaces_the_accepted_key() {
    let (first_dir, config, first_key) = fixture();
    let (second_dir, _, second_key) = fixture();
    let path = first_dir.path().join("jwks.json");
    let second = std::fs::read_to_string(second_dir.path().join("jwks.json")).expect("test setup").replace("current", "next");
    let auth = AuthenticatorImpl::new(config, "host=localhost dbname=postgres".into(), "aip_example".into()).await.expect("test setup");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("test setup").as_secs();
    let claims = json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-1","iat":now,"nbf":now,"exp":now+300});
    let old = token(&first_key, claims.clone(), "at+jwt", "current");
    assert!(auth.verify_token(&old).await.is_ok());
    std::fs::write(path, second).expect("test setup");
    assert_eq!(auth.verify_token(&token(&second_key, claims, "at+jwt", "next")).await.expect("test setup").subject, "user-1");
    assert!(auth.verify_token(&old).await.is_err());
}

#[tokio::test]
async fn signing_only_jwk_is_not_a_verification_key() {
    let (dir, config, _key) = fixture();
    let path = dir.path().join("jwks.json");
    let mut jwks: Value = serde_json::from_slice(&std::fs::read(&path).expect("test setup")).expect("test setup");
    jwks["keys"][0]["key_ops"] = json!(["sign"]);
    std::fs::write(path, jwks.to_string()).expect("test setup");
    assert!(AuthenticatorImpl::new(config, "host=localhost dbname=postgres".into(), "aip_example".into()).await.is_err());
}

#[tokio::test]
async fn rsa_modulus_below_2048_bits_cannot_be_padded_into_acceptance() {
    let (dir, config, _key) = fixture();
    let weak = RsaPrivateKey::new(&mut thread_rng(), 2040).expect("test setup").to_public_key();
    let mut padded = vec![0];
    padded.extend_from_slice(&weak.n().to_bytes_be());
    let path = dir.path().join("jwks.json");
    let mut jwks: Value = serde_json::from_slice(&std::fs::read(&path).expect("test setup")).expect("test setup");
    jwks["keys"][0]["n"] = json!(URL_SAFE_NO_PAD.encode(padded));
    jwks["keys"][0]["e"] = json!(URL_SAFE_NO_PAD.encode(weak.e().to_bytes_be()));
    std::fs::write(path, jwks.to_string()).expect("test setup");
    assert!(AuthenticatorImpl::new(config, "host=localhost dbname=postgres".into(), "aip_example".into()).await.is_err());
}
