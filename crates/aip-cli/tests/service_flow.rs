#![allow(clippy::unwrap_used, clippy::expect_used)]

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::FutureExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use std::{
    panic::AssertUnwindSafe,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

const DB: &str = aip_test_support::test_database_url!();
const APP: &str = include_str!("../../../prototype/example/app.aip");
const SHARED_SEMANTIC_FIXTURE: &str = include_str!("../../../conformance/semantic-v1.json");
const SERVICE_POLICY_APP: &str = r#"
resource Member { fields { id: Id; org: Text } rows read when true }
actor Member
resource Record {
  fields { id: Id; org: Text; owner: Member; title: Text; secret: Text; checked: Bool }
  // `org` is an ordinary user-defined field and row predicate, not a service tenant primitive.
  rows read when org = actor.org
  field secret read when owner = actor
  transition edit { allow owner = actor; from true; patch title; to checked = checked }
  expose apply edit { target id; bulk maxRows 1 }
  expose read { select id, org, owner, title, secret, checked; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
resource Locked {
  fields { id: Id; value: Text; checked: Bool }
  transition edit { allow true; from true; patch value; to checked = checked }
  expose apply edit { target id; bulk maxRows 1 }
  expose read { select id, value, checked; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
"#;
const SERVICE_AGGREGATE_APP: &str = r#"
resource Member {
  fields { id: Id; org: Text; email: Text }
  rows read when org = actor.org
  field email read when id = actor
  expose read { select id, org, email; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
actor Member
access callerVotes(a: Member) = true
access publicVotes(a: Member) = true
resource Record {
  fields { id: Id; org: Text; owner: Member; title: Text; secret: Text; checked: Bool }
  rows read when org = actor.org
  field secret read when owner = actor
  expose read {
    select id, org, owner, title, secret, checked, ownVoteCount, publicVoteCount, minOwnVote, maxOwnVote
    traverse owner { select id, email }
    traverse votes via Vote.record { select id, owner, amount; sort id; limit 10 }
    sort id
    budget { rows 20; depth 2; deadline 2s; cost 1000; offset 100; cursor }
  }
  aggregate ownVoteCount: Int { source Vote; sourceAccess callerVotes(actor); groupKey record; where owner = actor; callerFilter none; rowOutput none; release count }
  aggregate filteredOwnVoteCount: Int { source Vote; sourceAccess callerVotes(actor); groupKey record; where owner = actor; callerFilter read; rowOutput none; release count }
  aggregate publicVoteCount: Int { source Vote; sourceAccess publicVotes(actor); groupKey record; callerFilter none; rowOutput none; release count }
  aggregate minOwnVote: Int? { source Vote; sourceAccess callerVotes(actor); groupKey record; callerFilter none; rowOutput none; release min(amount) }
  aggregate maxOwnVote: Int? { source Vote; sourceAccess callerVotes(actor); groupKey record; callerFilter none; rowOutput none; release max(amount) }
  expose aggregate ownVoteCount
  expose aggregate filteredOwnVoteCount
  expose aggregate publicVoteCount
  expose aggregate minOwnVote
  expose aggregate maxOwnVote
}
resource Vote {
  fields { id: Id; record: Record; owner: Member; amount: Int }
  rows read when owner = actor
  expose read { select id, record, owner, amount; filter amount.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000; offset 100; cursor } }
}
"#;
const SEMANTIC_DATA_APP: &str = r#"
enum Phase { READY, DONE }
resource Member {
  fields { id: Id; name: Text }
  rows read when true
  expose read { select id, name }
}
actor Member
access visibleVotes(a: Member) = true
access visibleMetrics(a: Member) = true
resource Event {
  fields { id: Id; member: Member; title: Text; note: Text?; amount: Decimal(10,2)?; phase: Phase; score: Decimal(10,2); checked: Bool }
  rows read when member = actor
  field phase read when note = "kept"
  transition edit { allow member = actor; from true; patch title, note, amount; to checked = checked }
  expose apply edit { target id; bulk maxRows 1 }
  expose read {
    select id, member, title, note, amount, phase, score, checked, voteCount, emptySum, emptyMin, emptyMax
    filter note.eq
    traverse member { select id, name }
    sort score
    budget { rows 20; depth 2; deadline 2s; cost 1000 }
  }
  aggregate voteCount: Int { source Vote; sourceAccess visibleVotes(actor); groupKey event; callerFilter none; rowOutput none; release count }
  aggregate emptySum: Int { source Metric; sourceAccess visibleMetrics(actor); groupKey event; callerFilter none; rowOutput none; release sum(amount) }
  aggregate emptyMin: Int? { source Metric; sourceAccess visibleMetrics(actor); groupKey event; callerFilter none; rowOutput none; release min(amount) }
  aggregate emptyMax: Int? { source Metric; sourceAccess visibleMetrics(actor); groupKey event; callerFilter none; rowOutput none; release max(amount) }
}
resource Vote { fields { id: Id; event: Event } rows read when true }
resource Metric { fields { id: Id; event: Event; amount: Int } rows read when true }
"#;

async fn command(config: &Path, args: &[&str]) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aip"));
    child.arg("service");
    if args.first() == Some(&"principal") {
        child.arg("principal").arg("--config").arg(config).args(&args[1..]);
    } else {
        child.args(args).arg("--config").arg(config);
    }
    let output = child.env("AIP_TEST_DATABASE", DB).output().await.unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}
fn rejection_body_without_request_id(mut body: Value) -> Value {
    if let Some(object) = body.as_object_mut() {
        object.remove("requestId");
    }
    body
}

async fn post(address: &str, path: &str, token: &str, contract: &str, body: Value) -> Value {
    let text = body.to_string();
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let extra = if contract.is_empty() { String::new() } else { format!("x-aip-contract: {contract}\r\n") };
    socket
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nAuthorization: Bearer {token}\r\n{extra}Origin: https://app.example.com\r\nContent-Length: {}\r\n\r\n{text}",
                text.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut result = String::new();
    tokio::time::timeout(Duration::from_secs(8), socket.read_to_string(&mut result)).await.unwrap().unwrap();
    assert!(result.contains("access-control-allow-origin: https://app.example.com"), "{result}");
    serde_json::from_str(result.split_once("\r\n\r\n").unwrap().1).unwrap()
}
async fn start(config: &Path, file: &Path) -> (tokio::process::Child, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aip"))
        .args(["service", "serve"])
        .arg(file)
        .arg("--config")
        .arg(config)
        .env("AIP_TEST_DATABASE", DB)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let count = tokio::time::timeout(Duration::from_secs(8), reader.read_line(&mut line)).await.unwrap().unwrap();
    if count == 0 {
        let output = child.wait_with_output().await.unwrap();
        panic!("product startup failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["event"], "listening");
    assert!(value.get("devToken").is_none());
    (child, value)
}
async fn stop(mut child: tokio::process::Child) {
    let id = child.id().unwrap();
    let status = std::process::Command::new("kill").args(["-TERM", &id.to_string()]).status().unwrap();
    assert!(status.success());
    assert!(tokio::time::timeout(Duration::from_secs(8), child.wait()).await.unwrap().unwrap().success());
}

struct PolicyHttpFixture {
    _dir: tempfile::TempDir,
    schema: String,
    child: Option<tokio::process::Child>,
    address: String,
    fingerprint: String,
    token: String,
    peer_token: String,
}

impl PolicyHttpFixture {
    async fn stop_server(&mut self) {
        if let Some(child) = self.child.take() {
            stop(child).await;
        }
    }

    async fn cleanup(mut self) {
        self.stop_server().await;
        let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
        db.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema)).await.unwrap();
    }
}

async fn policy_db_execute(schema: &str, statement: &str) {
    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    db.batch_execute(&format!("SET search_path TO {schema}; {statement}")).await.unwrap();
}

async fn policy_vote_metrics(schema: &str) -> (i64, i64, i64, f64) {
    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let row = db
        .query_one(&format!("SELECT count(*), min(amount), max(amount), avg(amount)::float8 FROM {schema}.vote WHERE record_id=1"), &[])
        .await
        .unwrap();
    (row.get(0), row.get(1), row.get(2), row.get(3))
}

async fn policy_standalone_vote_values(fixture: &PolicyHttpFixture) -> Vec<Value> {
    let mut values = Vec::new();
    for name in ["ownVoteCount", "minOwnVote", "maxOwnVote"] {
        let response =
            post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"aggregate":format!("Record.{name}"),"input":{}}}))
                .await;
        assert_eq!(response["ok"], true, "standalone {name}: {response}");
        values.push(response["rows"][0].clone());
    }
    values
}

async fn service_policy_http_fixture() -> PolicyHttpFixture {
    let dir = tempfile::tempdir().unwrap();
    let schema = format!("aip_policyagg_{}", uuid::Uuid::new_v4().simple());
    let config = dir.path().join("service.json");
    let file = dir.path().join("policy.aip");
    std::fs::write(&file, SERVICE_AGGREGATE_APP).unwrap();
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = private.to_public_key();
    std::fs::write(
        dir.path().join("jwks.json"),
        json!({"keys":[{"kty":"RSA","kid":"current","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]}).to_string(),
    )
    .unwrap();
    std::fs::write(
        &config,
        json!({"schema":schema,"database_url_env":"AIP_TEST_DATABASE","listen":"127.0.0.1:0","wire":"decimal","allowed_origins":["https://app.example.com"],"auth":{"issuer":"https://issuer.example","audience":"aip-api","jwks":{"kind":"file","path":"jwks.json"}}}).to_string(),
    )
    .unwrap();
    command(&config, &["init", file.to_str().unwrap()]).await;
    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id,org,email) VALUES (42,'red','owner@example.com'),(99,'red','peer@example.com'),(100,'blue','other@example.com'),(101,'blue','hidden@example.com'); \
         INSERT INTO {schema}.record(id,org,owner_id,title,secret,checked) VALUES \
         (1,'red',42,'mine','owner-secret',false),(2,'blue',100,'hidden-middle','hidden-middle-secret',false),(3,'red',99,'peer-record','peer-secret','false'),(4,'red',101,'hidden-owner','hidden-owner-secret',false),(5,'blue',99,'foreign','hidden-secret',false); \
         INSERT INTO {schema}.vote(id,record_id,owner_id,amount) VALUES \
         (1,1,42,10),(2,1,42,20),(3,1,99,-900),(4,3,99,7),(5,5,99,-700)"
    )).await.unwrap();
    command(&config, &["principal", "bind", "--subject", "policy-user", "--actor", "42"]).await;
    command(&config, &["principal", "bind", "--subject", "policy-peer", "--actor", "99"]).await;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("at+jwt".into());
    header.kid = Some("current".into());
    let key = EncodingKey::from_rsa_der(private.to_pkcs1_der().unwrap().as_bytes());
    let token = encode(&header, &json!({"iss":"https://issuer.example","aud":"aip-api","sub":"policy-user","iat":now,"nbf":now,"exp":now+300}), &key)
        .unwrap();
    let peer_token =
        encode(&header, &json!({"iss":"https://issuer.example","aud":"aip-api","sub":"policy-peer","iat":now,"nbf":now,"exp":now+300}), &key)
            .unwrap();
    let (child, listening) = start(&config, &file).await;
    PolicyHttpFixture {
        _dir: dir,
        schema,
        child: Some(child),
        address: listening["address"].as_str().unwrap().to_owned(),
        fingerprint: listening["fingerprint"].as_str().unwrap().to_owned(),
        token,
        peer_token,
    }
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operation_only_service_preserves_principal_contract_revocation_and_deployment_fences() {
    let root = tempfile::tempdir().unwrap();
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = private.to_public_key();
    let jwks = json!({"keys":[{"kty":"RSA","kid":"current","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]});
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("at+jwt".into());
    header.kid = Some("current".into());
    let der = private.to_pkcs1_der().unwrap();
    let key = EncodingKey::from_rsa_der(der.as_bytes());
    for wire in ["safe", "decimal"] {
        for lang in ["node", "python"] {
            let directory = root.path().join(format!("{wire}-{lang}"));
            std::fs::create_dir(&directory).unwrap();
            let schema = format!("aip_operations_{}", uuid::Uuid::new_v4().simple());
            let config = directory.join("service.json");
            let file = directory.join("app.aip");
            let source = format!(
                "{}\noperation read waiting {{ input {{}} output {{ length: Int }} allow actor != null effect none deadline 2s implementation \"compute.waiting\" }}",
                include_str!("../../../spikes/spike-v6-transport/tests/fixtures/operations.aip")
            );
            let workers = directory.join("workers");
            std::fs::create_dir(&workers).unwrap();
            let marker = directory.join("worker-started");
            let marker_literal = serde_json::to_string(marker.to_str().unwrap()).unwrap();
            std::fs::write(workers.join("compute.mjs"), format!("{}\nimport {{ writeFileSync }} from 'node:fs';\nexport async function waiting() {{ writeFileSync({marker_literal}, 'started'); await new Promise(resolve => setTimeout(resolve, 800)); return {{length:1}}; }}", include_str!("../../../spikes/spike-v6-transport/tests/fixtures/operations/compute.mjs"))).unwrap();
            std::fs::write(workers.join("compute.py"), format!("{}\nfrom pathlib import Path\nasync def waiting(input, ctx):\n    Path({marker_literal}).write_text('started')\n    await asyncio.sleep(0.8)\n    return {{'length': 1}}\n", include_str!("../../../spikes/spike-v6-transport/tests/fixtures/operations/compute.py"))).unwrap();
            std::fs::write(&file, &source).unwrap();
            std::fs::write(directory.join("jwks.json"), jwks.to_string()).unwrap();
            std::fs::write(&config, json!({"schema":schema,"database_url_env":"AIP_TEST_DATABASE","listen":"127.0.0.1:0","wire":wire,"allowed_origins":["https://app.example.com"],"auth":{"issuer":"https://issuer.example","audience":"aip-api","jwks":{"kind":"file","path":"jwks.json"}},"workers":{"directory":workers,"language":lang,"isolation":if cfg!(target_os = "macos") { "mac_net_deny" } else { "auto" }}}).to_string()).unwrap();
            command(&config, &["init", file.to_str().unwrap()]).await;
            let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
            let outcome = AssertUnwindSafe(async {
                if wire == "safe" {
                    let mismatched_config = directory.join("decimal.json");
                    let mut mismatched: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
                    mismatched["wire"] = json!("decimal");
                    std::fs::write(&mismatched_config, mismatched.to_string()).unwrap();
                    let output = Command::new(env!("CARGO_BIN_EXE_aip")).args(["service","principal","--config"]).arg(&mismatched_config).args(["bind","--subject","unsafe","--actor","9007199254740992"]).env("AIP_TEST_DATABASE",DB).output().await.unwrap();
                    assert!(!output.status.success(), "mismatched deployment wire must reject principal registration");
                    assert_eq!(serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"], "WIRE_MODE_MISMATCH");
                }
                command(&config, &["principal","bind","--subject","user-42","--actor","42"]).await;
                let changed = Command::new(env!("CARGO_BIN_EXE_aip")).args(["service","principal","--config"]).arg(&config).args(["bind","--subject","user-42","--actor","99"]).env("AIP_TEST_DATABASE",DB).output().await.unwrap();
                assert!(!changed.status.success());
                assert_eq!(serde_json::from_slice::<Value>(&changed.stderr).unwrap()["code"], "PRINCIPAL_CONFLICT");
                let extra: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_actors WHERE id=99"),&[]).await.unwrap().get(0);
                assert_eq!(extra, 0, "a failed bind must roll back actor registration");
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                let token = encode(&header,&json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-42","iat":now,"nbf":now,"exp":now+300,"actorId":99}),&key).unwrap();
                let (child, listening) = start(&config,&file).await;
                let address = listening["address"].as_str().unwrap();
                let fingerprint = listening["fingerprint"].as_str().unwrap();
                let owner = if wire == "safe" { json!(42) } else { json!("42") };
                let body = json!({"operation":"length","input":{"text":"가😀A","owner":owner,"optional":null}});
                let result = post(address,"/operation",&token,fingerprint,body.clone()).await;
                assert_eq!(result["output"],json!({"length":3}),"{lang}/{wire}: {result}");
                let waiting = {
                    let address = address.to_string();
                    let token = token.clone();
                    let fingerprint = fingerprint.to_string();
                    tokio::spawn(async move { post(&address,"/operation",&token,&fingerprint,json!({"operation":"waiting","input":{}})).await })
                };
                tokio::time::timeout(Duration::from_secs(3), async {
                    while !marker.exists() { tokio::time::sleep(Duration::from_millis(10)).await; }
                }).await.expect("worker must have started before revoke");
                assert!(!waiting.is_finished());
                command(&config,&["principal","revoke","--subject","user-42"]).await;
                assert_eq!(waiting.await.unwrap()["code"], "WIRE.UNAUTHENTICATED", "revoked principal must not receive in-flight output");
                assert_eq!(post(address,"/operation",&token,fingerprint,body.clone()).await["code"],"WIRE.UNAUTHENTICATED");
                stop(child).await;
                command(&config,&["principal","bind","--subject","user-42","--actor","42"]).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                let token = encode(&header,&json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-42","iat":now,"nbf":now,"exp":now+300}),&key).unwrap();
                let (child,listening) = start(&config,&file).await;
                let address = listening["address"].as_str().unwrap();
                assert_eq!(post(address,"/operation",&token,fingerprint,body.clone()).await["ok"],true);
                std::fs::write(&file,source.replace("allow actor != null and actor.id = input.owner", "allow false")).unwrap();
                let plan = command(&config,&["migrate",file.to_str().unwrap()]).await;
                assert_eq!(plan["blocked"],false,"{plan}");
                command(&config,&["migrate",file.to_str().unwrap(),"--apply",plan["digest"].as_str().unwrap()]).await;
                assert_eq!(post(address,"/operation",&token,fingerprint,body.clone()).await["code"],"WIRE.DEPLOYMENT_CHANGED");
                stop(child).await;
                let (child,listening) = start(&config,&file).await;
                assert_eq!(post(listening["address"].as_str().unwrap(),"/operation",&token,listening["fingerprint"].as_str().unwrap(),body).await["code"],"WIRE.ACCESS_DENIED");
                stop(child).await;
            }).catch_unwind().await;
            db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
            if let Err(panic) = outcome {
                std::panic::resume_unwind(panic);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn product_identity_migration_and_restart_use_the_same_contract() {
    let dir = tempfile::tempdir().unwrap();
    let schema = format!("aip_product_{}", uuid::Uuid::new_v4().simple());
    let config = dir.path().join("service.json");
    let file = dir.path().join("app.aip");
    std::fs::write(&file, APP).unwrap();
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = private.to_public_key();
    std::fs::write(dir.path().join("jwks.json"),json!({"keys":[{"kty":"RSA","kid":"current","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]}).to_string()).unwrap();
    std::fs::write(&config,json!({"schema":schema,"database_url_env":"AIP_TEST_DATABASE","listen":"127.0.0.1:0","wire":"decimal","allowed_origins":["https://app.example.com"],"auth":{"issuer":"https://issuer.example","audience":"aip-api","jwks":{"kind":"file","path":"jwks.json"}}}).to_string()).unwrap();
    command(&config, &["init", file.to_str().unwrap()]).await;
    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let outcome=AssertUnwindSafe(async{
        db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(42),(99); INSERT INTO {schema}.event(id,member_id,checked,title,link,phase,date) VALUES(11,42,false,'owned','https://example.com','READY','2026-10-05Z'),(12,99,false,'other','https://example.com','READY','2026-10-05Z')")).await.unwrap();
        command(&config,&["principal","bind","--subject","user-42","--actor","42"]).await;
        let changed=Command::new(env!("CARGO_BIN_EXE_aip")).args(["service","principal","--config"]).arg(&config).args(["bind","--subject","user-42","--actor","99"]).env("AIP_TEST_DATABASE",DB).output().await.unwrap();
        assert!(!changed.status.success(),"existing identity cannot be reassigned");
        let denied:Value=serde_json::from_slice(&changed.stderr).unwrap();
        assert_eq!(denied["code"],"PRINCIPAL_CONFLICT");
        let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let mut header=Header::new(Algorithm::RS256);header.typ=Some("at+jwt".into());header.kid=Some("current".into());
        let der=private.to_pkcs1_der().unwrap();let key=EncodingKey::from_rsa_der(der.as_bytes());
        let token=encode(&header,&json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-42","iat":now,"nbf":now,"exp":now+300,"actorId":99}),&key).unwrap();
        let (child,listening)=start(&config,&file).await;let address=listening["address"].as_str().unwrap();let fingerprint=listening["fingerprint"].as_str().unwrap();
        assert_eq!(post(address,"/session",&token,"",json!({})).await["principal"]["actorId"],"42");
        let read=post(address,"/read",&token,fingerprint,json!({"query":{"read":"Event","select":["id","title"]}})).await;
        assert_eq!(read["rows"],json!([{"id":"11","title":"owned"}]));
        let apply=json!({"key":"product-mark","request":{"apply":"Event.mark","target":{"ids":["11"]}}});
        let written=post(address,"/apply",&token,fingerprint,apply.clone()).await;assert_eq!(written["ok"],true,"{written}");
        stop(child).await;
        let (child,listening)=start(&config,&file).await;let address=listening["address"].as_str().unwrap();
        assert_eq!(post(address,"/session",&token,"",json!({})).await["principal"]["actorId"],"42","restarting must preserve identity-provider token validity");
        assert_eq!(written["changed"], json!(["11"]), "{written}");
        let mut replayed=post(address,"/apply",&token,fingerprint,apply).await;
        assert_eq!(replayed.as_object_mut().unwrap().remove("replayed"),Some(json!(true)));
        assert_eq!(replayed, written, "replay must preserve the complete committed result");
        let missing=Command::new(env!("CARGO_BIN_EXE_aip")).args(["service","principal","--config"]).arg(&config).args(["revoke","--subject","missing-user"]).env("AIP_TEST_DATABASE",DB).output().await.unwrap();
        assert!(!missing.status.success(), "revoking an unknown principal must report the missing mapping");
        let denied:Value=serde_json::from_slice(&missing.stderr).unwrap();
        assert_eq!(denied["code"],"PRINCIPAL_NOT_FOUND");
        command(&config,&["principal","revoke","--subject","user-42"]).await;
        assert_eq!(post(address,"/read",&token,fingerprint,json!({"query":{"read":"Event","select":["id"]}})).await["code"],"WIRE.UNAUTHENTICATED");
        stop(child).await;
        let new_source=APP.replace("date: Time }","date: Time; note: Text? }");assert_ne!(new_source,APP);std::fs::write(&file,new_source).unwrap();
        let plan=command(&config,&["migrate",file.to_str().unwrap()]).await;assert_eq!(plan["blocked"],false,"{plan}");
        let applied=command(&config,&["migrate",file.to_str().unwrap(),"--apply",plan["digest"].as_str().unwrap()]).await;assert_eq!(applied["ok"],true);
        let row=db.query_one(&format!("SELECT checked,note FROM {schema}.event WHERE id=11"),&[]).await.unwrap();assert!(row.get::<_,bool>(0));assert_eq!(row.get::<_,Option<String>>(1),None);
        command(&config,&["principal","bind","--subject","user-42","--actor","42"]).await;
        let (child,listening)=start(&config,&file).await;
        let old_address=listening["address"].as_str().unwrap();
        let old_fingerprint=listening["fingerprint"].as_str().unwrap();
        assert_eq!(post(old_address,"/session",&token,"",json!({})).await["code"],"WIRE.UNAUTHENTICATED","rebind must not revive a token issued before revoke");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let token=encode(&header,&json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-42","iat":now,"nbf":now,"exp":now+300}),&key).unwrap();
        assert_eq!(post(old_address,"/read",&token,old_fingerprint,json!({"query":{"read":"Event","select":["id"]}})).await["rows"],json!([{"id":"11"}]));
        let restricted=std::fs::read_to_string(&file).unwrap().replace("rows read when member = actor","rows read when member = actor and checked = false");
        std::fs::write(&file,restricted).unwrap();
        let plan=command(&config,&["migrate",file.to_str().unwrap()]).await;
        command(&config,&["migrate",file.to_str().unwrap(),"--apply",plan["digest"].as_str().unwrap()]).await;
        let fenced=post(old_address,"/read",&token,old_fingerprint,json!({"query":{"read":"Event","select":["id"]}})).await;
        assert_eq!(fenced["code"],"WIRE.DEPLOYMENT_CHANGED","live old policy must stop authorizing reads: {fenced}");
        stop(child).await;
        let (child,listening)=start(&config,&file).await;
        assert_eq!(post(listening["address"].as_str().unwrap(),"/read",&token,listening["fingerprint"].as_str().unwrap(),json!({"query":{"read":"Event","select":["id"]}})).await["rows"],json!([]));
        stop(child).await;
    }).catch_unwind().await;
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn service_data_semantics_hold_across_http_and_database_state() {
    let conformance: Value = serde_json::from_str(SHARED_SEMANTIC_FIXTURE).unwrap();
    let expected = &conformance["expected"];
    let cases = conformance["cases"].as_array().unwrap();
    let event_values = cases
        .iter()
        .map(|row| {
            let note = if row["note"].is_null() { "NULL".to_owned() } else { format!("'{}'", row["note"].as_str().unwrap().replace('\'', "''")) };
            let amount = row["amount"].as_str().map(|value| format!("'{value}'")).unwrap_or_else(|| "NULL".into());
            format!(
                "({},42,'{}',{note},{amount},'{}',{}, {})",
                row["id"].as_str().unwrap(),
                row["title"].as_str().unwrap(),
                row["phase"].as_str().unwrap(),
                row["score"].as_str().unwrap(),
                row["checked"]
            )
        })
        .collect::<Vec<_>>();
    let vote_values = cases
        .iter()
        .flat_map(|row| {
            row["votes"].as_array().unwrap().iter().map(move |vote| format!("({},{})", vote.as_i64().unwrap(), row["id"].as_str().unwrap()))
        })
        .collect::<Vec<_>>();
    let dir = tempfile::tempdir().unwrap();
    let schema = format!("aip_semantic_data_{}", uuid::Uuid::new_v4().simple());
    let undeclared_schema = format!("aip_semantic_negative_{}", uuid::Uuid::new_v4().simple());
    let config = dir.path().join("service.json");
    let file = dir.path().join("data.aip");
    std::fs::write(&file, SEMANTIC_DATA_APP).unwrap();
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = private.to_public_key();
    std::fs::write(dir.path().join("jwks.json"), json!({"keys":[{"kty":"RSA","kid":"current","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]}).to_string()).unwrap();
    std::fs::write(&config, json!({"schema":schema,"database_url_env":"AIP_TEST_DATABASE","listen":"127.0.0.1:0","wire":"decimal","allowed_origins":["https://app.example.com"],"auth":{"issuer":"https://issuer.example","audience":"aip-api","jwks":{"kind":"file","path":"jwks.json"}}}).to_string()).unwrap();
    command(&config, &["init", file.to_str().unwrap()]).await;
    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let outcome = AssertUnwindSafe(async {
        db.batch_execute(&format!("INSERT INTO {schema}.member(id,name) VALUES(42,'Ada'); INSERT INTO {schema}.event(id,member_id,title,note,amount,phase,score,checked) VALUES {}; INSERT INTO {schema}.vote(id,event_id) VALUES {}", event_values.join(","), vote_values.join(","))).await.unwrap();
        command(&config, &["principal","bind","--subject","semantic-user","--actor","42"]).await;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let mut header = Header::new(Algorithm::RS256); header.typ = Some("at+jwt".into()); header.kid = Some("current".into());
        let key = EncodingKey::from_rsa_der(private.to_pkcs1_der().unwrap().as_bytes());
        let token = encode(&header, &json!({"iss":"https://issuer.example","aud":"aip-api","sub":"semantic-user","iat":now,"nbf":now,"exp":now+300}), &key).unwrap();
        let (child, listening) = start(&config, &file).await;
        let address = listening["address"].as_str().unwrap();
        let fingerprint = listening["fingerprint"].as_str().unwrap();
        let query = json!({"query":{"read":"Event","select":["id","title","note","amount","phase","score","voteCount","emptySum","emptyMin","emptyMax",{"member":{"select":["id","name"]}}],"sort":[{"field":"score","dir":"asc"}]}});
        let first = post(address,"/read",&token,fingerprint,query.clone()).await;
        assert_eq!(first["ok"],true,"{first}");
        assert_eq!(first["rows"].as_array().map(Vec::len),Some(3),"all three authorized rows are returned");
        assert_eq!(first["rows"][0]["id"],"11","equal sort values use stable id ordering");
        assert_eq!(first["rows"].as_array().unwrap().iter().map(|row| row["id"].clone()).collect::<Vec<_>>(),expected["sortIds"].as_array().unwrap().iter().map(|id| id.as_str().map(|s| json!(s)).unwrap()).collect::<Vec<_>>(),"sorted result follows shared fixture");
        assert_eq!(first["rows"][0]["note"],Value::Null,"stored nullable value is JSON null");
        assert_eq!(first["rows"][0]["note"],expected["null"],"fixture null case is preserved");
        assert_eq!(first["rows"][1]["note"],expected["value"],"fixture present value is preserved");
        assert_eq!(first["rows"][0]["amount"],expected["decimalExact"],"declared Decimal(10,2) upper boundary remains exact");
        assert_eq!(first["rows"][0]["emptySum"],expected["emptySum"],"empty sum follows shared fixture");
        assert_eq!(first["rows"][0]["emptyMin"],expected["emptyMin"],"empty min follows shared fixture");
        assert_eq!(first["rows"][0]["emptyMax"],expected["emptyMax"],"empty max follows shared fixture");
        assert_eq!(first["rows"][0]["voteCount"],1);
        assert_eq!(first["rows"][1]["voteCount"],expected["emptyCount"],"empty relation count follows shared fixture");
        assert_eq!(first["rows"][0]["phase"],expected["policyNull"],"note = 'kept' is UNKNOWN for SQL NULL and field policy fails closed");
        assert_eq!(first["rows"][1]["phase"],expected["policyVisible"],"non-null note equality is TRUE");
        let filtered = post(
            address,
            "/read",
            &token,
            fingerprint,
            json!({"query":{"read":"Event","select":["id"],"filter":[{"field":"note","op":"eq","value":"kept"}]}}),
        )
        .await;
        assert_eq!(filtered["ok"],true,"{filtered}");
        let filtered_ids = filtered["rows"].as_array().expect("filtered rows").iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>();
        assert_eq!(filtered_ids.contains(&"11"),expected["nullEqualityMatches"],"HTTP filter excludes the NULL note row");
        assert_eq!(filtered_ids.contains(&"12"),expected["nonNullEqualityMatches"],"HTTP filter includes the matching non-null note row");
        assert_eq!(first["rows"][0]["member"],json!({"id":"42","name":"Ada"}));
        assert!(expected["missingFields"].as_array().unwrap().iter().all(|field| first["rows"][0].get(field.as_str().unwrap()).is_none()),"fixture fields omitted from selection stay absent");
        let patch = json!({"key":"semantic-partial","request":{"apply":"Event.edit","target":{"ids":["12"]},"values":{"title":"changed"}}});
        let applied = post(address,"/apply",&token,fingerprint,patch).await;
        assert_eq!(applied["ok"],true,"{applied}");
        let row = db.query_one(&format!("SELECT title,note,amount::text,checked FROM {schema}.event WHERE id=12"),&[]).await.unwrap();
        assert_eq!(row.get::<_,String>(0),"changed");
        assert_eq!(row.get::<_,Option<String>>(1).as_deref(),Some("kept"),"omitted patch fields are unchanged");
        assert_eq!(row.get::<_,Option<String>>(2).as_deref(),Some("1.20"));
        assert!(!row.get::<_,bool>(3));

        let explicit_null = json!({"key":"semantic-null","request":{"apply":"Event.edit","target":{"ids":["12"]},"values":{"note":null,"amount":"99999999.99"}}});
        let cleared = post(address,"/apply",&token,fingerprint,explicit_null).await;
        assert_eq!(cleared["ok"],true,"{cleared}");
        let row = db.query_one(&format!("SELECT note,amount::text FROM {schema}.event WHERE id=12"),&[]).await.unwrap();
        assert_eq!(row.get::<_,Option<String>>(0),None,"explicit null clears a nullable field");
        assert_eq!(row.get::<_,Option<String>>(1).as_deref(),Some("99999999.99"),"upper Decimal boundary survives write and storage");

        // The same rows inserted in the opposite order must yield the identical sorted response.
        db.batch_execute(&format!("DELETE FROM {schema}.vote; DELETE FROM {schema}.event; INSERT INTO {schema}.event(id,member_id,title,note,amount,phase,score,checked) VALUES {}; INSERT INTO {schema}.vote(id,event_id) VALUES {}", event_values.iter().rev().cloned().collect::<Vec<_>>().join(","), vote_values.iter().rev().cloned().collect::<Vec<_>>().join(","))).await.unwrap();
        let permuted = post(address,"/read",&token,fingerprint,query.clone()).await;
        assert_eq!(permuted["ok"],true,"{permuted}");
        assert_eq!(permuted["rows"],first["rows"],"input insertion order does not change the semantic result");
        stop(child).await;

        // The old fixture's omitted Event traverse declaration remains a negative control.
        let undeclared_source = SEMANTIC_DATA_APP.replace("    traverse member { select id, name }\n", "");
        assert_ne!(undeclared_source, SEMANTIC_DATA_APP, "negative control must remove the Event traverse declaration");
        let undeclared_file = dir.path().join("undeclared-traverse.aip");
        std::fs::write(&undeclared_file, undeclared_source).unwrap();
        let undeclared_config = dir.path().join("undeclared-service.json");
        let mut undeclared_config_value: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        undeclared_config_value["schema"] = json!(undeclared_schema);
        std::fs::write(&undeclared_config, undeclared_config_value.to_string()).unwrap();
        command(&undeclared_config, &["init", undeclared_file.to_str().unwrap()]).await;
        db.batch_execute(&format!("INSERT INTO {undeclared_schema}.member(id,name) VALUES(42,'Ada'); INSERT INTO {undeclared_schema}.event(id,member_id,title,note,amount,phase,score,checked) VALUES(11,42,'first',NULL,'99999999.99','DONE',10.50,false)")).await.unwrap();
        command(&undeclared_config, &["principal","bind","--subject","semantic-user","--actor","42"]).await;
        let (child, undeclared_listening) = start(&undeclared_config, &undeclared_file).await;
        let denied = post(
            undeclared_listening["address"].as_str().unwrap(),
            "/read",
            &token,
            undeclared_listening["fingerprint"].as_str().unwrap(),
            query,
        )
        .await;
        assert_eq!(denied["code"],"WIRE.TRAVERSE_NOT_ALLOWED","undeclared relation traversal remains default-deny: {denied}");
        stop(child).await;
    }).catch_unwind().await;
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {undeclared_schema} CASCADE")).await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn service_policy_default_deny_redaction_and_hidden_target_invariants() {
    let dir = tempfile::tempdir().unwrap();
    let schema = format!("aip_policy_{}", uuid::Uuid::new_v4().simple());
    let config = dir.path().join("service.json");
    let file = dir.path().join("policy.aip");
    std::fs::write(&file, SERVICE_POLICY_APP).unwrap();
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = private.to_public_key();
    std::fs::write(
        dir.path().join("jwks.json"),
        json!({"keys":[{"kty":"RSA","kid":"current","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]}).to_string(),
    )
    .unwrap();
    std::fs::write(
        &config,
        json!({"schema":schema,"database_url_env":"AIP_TEST_DATABASE","listen":"127.0.0.1:0","wire":"decimal","allowed_origins":["https://app.example.com"],"auth":{"issuer":"https://issuer.example","audience":"aip-api","jwks":{"kind":"file","path":"jwks.json"}}}).to_string(),
    )
    .unwrap();
    command(&config, &["init", file.to_str().unwrap()]).await;
    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let outcome = AssertUnwindSafe(async {
        db.batch_execute(&format!(
            "INSERT INTO {schema}.member(id,org) VALUES (42,'red'),(99,'red'),(100,'blue'); \
             INSERT INTO {schema}.record(id,org,owner_id,title,secret,checked) VALUES \
             (1,'red',42,'mine','owner-secret',false),(2,'red',99,'peer-secret','redacted-secret',false),(3,'blue',99,'foreign','hidden-secret',false); \
             INSERT INTO {schema}.locked(id,value,checked) VALUES (7,'locked-seed',false)"
        ))
        .await
        .unwrap();
        command(&config, &["principal", "bind", "--subject", "policy-user", "--actor", "42"]).await;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let mut header = Header::new(Algorithm::RS256);
        header.typ = Some("at+jwt".into());
        header.kid = Some("current".into());
        let key = EncodingKey::from_rsa_der(private.to_pkcs1_der().unwrap().as_bytes());
        let token = encode(
            &header,
            &json!({"iss":"https://issuer.example","aud":"aip-api","sub":"policy-user","iat":now,"nbf":now,"exp":now+300}),
            &key,
        )
        .unwrap();
        let (child, listening) = start(&config, &file).await;
        let address = listening["address"].as_str().unwrap();
        let fingerprint = listening["fingerprint"].as_str().unwrap();

        let locked_read = post(address, "/read", &token, fingerprint, json!({"query":{"read":"Locked","select":["id","value"]}})).await;
        assert_eq!(locked_read["rows"], json!([]), "omitted rows read declaration defaults to no visible rows");
        let locked_apply = post(
            address,
            "/apply",
            &token,
            fingerprint,
            json!({"key":"locked-default-deny","request":{"apply":"Locked.edit","target":{"ids":["7"]},"values":{"value":"changed"}}}),
        )
        .await;
        assert_eq!(locked_apply["code"], "WIRE.MISSING_TARGET", "a target without rows read is unavailable to apply");
        let locked_value: String = db.query_one(&format!("SELECT value FROM {schema}.locked WHERE id=7"), &[]).await.unwrap().get(0);
        assert_eq!(locked_value, "locked-seed", "default-denied apply leaves the row unchanged");

        let query = json!({"query":{"read":"Record","select":["id","title","secret"],"sort":[{"field":"id","dir":"asc"}]}});
        let visible = post(address, "/read", &token, fingerprint, query).await;
        assert_eq!(visible["rows"].as_array().unwrap().iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>(), vec!["1", "2"], "organization row policy filters the other organization's row");
        assert_eq!(visible["rows"][0]["secret"], "owner-secret", "owner can read the protected field");
        assert_eq!(visible["rows"][1]["secret"], Value::Null, "field policy redacts a same-organization peer's secret");

        let visible_denied = post(
            address,
            "/apply",
            &token,
            fingerprint,
            json!({"key":"visible-action-denied","request":{"apply":"Record.edit","target":{"ids":["2"]},"values":{"title":"forged"}}}),
        )
        .await;
        assert_eq!(visible_denied["code"], "WIRE.FORBIDDEN", "visible same-organization row still requires transition authorization");
        let hidden = post(
            address,
            "/apply",
            &token,
            fingerprint,
            json!({"key":"hidden-target","request":{"apply":"Record.edit","target":{"ids":["3"]},"values":{"title":"forged"}}}),
        )
        .await;
        let missing = post(
            address,
            "/apply",
            &token,
            fingerprint,
            json!({"key":"missing-target","request":{"apply":"Record.edit","target":{"ids":["404"]},"values":{"title":"forged"}}}),
        )
        .await;
        assert_eq!(hidden["code"], "WIRE.MISSING_TARGET", "row-read-hidden targets are reported as unavailable");
        assert_eq!(hidden["code"], missing["code"], "hidden and nonexistent targets have the same wire error");
        let titles = db.query(&format!("SELECT id,title FROM {schema}.record WHERE id IN (2,3) ORDER BY id"), &[]).await.unwrap();
        assert_eq!(titles.iter().map(|row| row.get::<_, String>(1)).collect::<Vec<_>>(), vec!["peer-secret", "foreign"], "denied visible and hidden applies leave both target rows unchanged");

        db.execute(&format!("UPDATE {schema}.member SET org='blue' WHERE id=42"), &[]).await.unwrap();
        let after_policy_change = post(address, "/read", &token, fingerprint, json!({"query":{"read":"Record","select":["id"],"sort":[{"field":"id","dir":"asc"}]}})).await;
        assert_eq!(after_policy_change["rows"], json!([{"id":"3"}]), "the same JWT sees the updated actor policy on the next request");
        stop(child).await;
    })
    .catch_unwind()
    .await;
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn service_policy_aggregate_noninterference_http_postgres() {
    let mut fixture = service_policy_http_fixture().await;
    let outcome = AssertUnwindSafe(async {
        let query = json!({"query":{"read":"Record","select":["id","owner","secret","ownVoteCount","publicVoteCount","minOwnVote","maxOwnVote",{"votes":{"select":["id","owner","amount"]}}],"sort":[{"field":"id","dir":"asc"}]}});
        let before = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, query.clone()).await;
        assert_eq!(before["ok"], true, "{before}");
        assert_eq!(before["rows"].as_array().unwrap().iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>(), vec!["1", "3", "4"], "row policy hides another organization");
        assert_eq!(before["rows"][0]["owner"], json!("42"), "to-one reference remains visible under target row policy");
        assert_eq!(before["rows"][0]["secret"], "owner-secret");
        assert_eq!(before["rows"][1]["secret"], Value::Null, "field policy redacts peer data");
        assert_eq!(before["rows"][0]["ownVoteCount"], 2, "caller-scoped count includes only caller rows");
        assert_eq!(before["rows"][0]["publicVoteCount"], 3, "explicitly public count includes every source row");
        assert_eq!(before["rows"][0]["minOwnVote"], 10, "min has no where but measure() re-applies Vote rowRead, so the hidden -900 is excluded (count does not)");
        assert_eq!(before["rows"][0]["maxOwnVote"], 20, "max has no where but measure() re-applies Vote rowRead, so hidden rows are excluded (count does not)");
        assert_eq!(before["rows"][1]["ownVoteCount"], 0);
        assert_eq!(before["rows"][1]["publicVoteCount"], 1);
        assert_eq!(before["rows"][1]["minOwnVote"], Value::Null);
        assert_eq!(before["rows"][1]["maxOwnVote"], Value::Null);
        assert_eq!(before["rows"][0]["votes"], json!([{"id":"1","owner":"42","amount":10},{"id":"2","owner":"42","amount":20}]), "to-many traversal applies child row policy before its limit");
        assert_eq!(before["rows"][1]["votes"], json!([]), "a child collection with only hidden rows is empty");
        let caller_aggregate = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"aggregate":"Record.ownVoteCount","input":{}}})).await;
        assert_eq!(caller_aggregate["rows"][0], 2, "standalone caller-scoped aggregate preserves its where contract");
        let public_aggregate = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"aggregate":"Record.publicVoteCount","input":{}}})).await;
        assert_eq!(public_aggregate["rows"][0], 5, "count without where does not inherit Vote rowRead and counts every source row");
        let min_aggregate = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"aggregate":"Record.minOwnVote","input":{}}})).await;
        let max_aggregate = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"aggregate":"Record.maxOwnVote","input":{}}})).await;
        assert_eq!(min_aggregate["ok"], true, "{min_aggregate}");
        assert_eq!(max_aggregate["ok"], true, "{max_aggregate}");
        assert_eq!(min_aggregate["rows"][0], 10);
        assert_eq!(max_aggregate["rows"][0], 20);
        let peer_view = post(&fixture.address, "/read", &fixture.peer_token, &fixture.fingerprint, query.clone()).await;
        assert_eq!(peer_view["rows"].as_array().unwrap().iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>(), vec!["1", "3", "4"], "same-organization caller sees the row set through their own identity");
        assert_eq!(peer_view["rows"][0]["secret"], Value::Null, "another user cannot read the owner's field");
        assert_eq!(peer_view["rows"][0]["ownVoteCount"], 1, "peer owns vote 3 on record 1; the first caller's votes 1 and 2 are not counted");
        assert_eq!(peer_view["rows"][0]["minOwnVote"], -900, "peer min sees only its own vote, not the first caller's 10 and 20");
        assert_eq!(peer_view["rows"][0]["maxOwnVote"], -900);
        assert_eq!(peer_view["rows"][1]["ownVoteCount"], 1, "peer owns vote 4 on record 3");
        assert_eq!(peer_view["rows"][2]["ownVoteCount"], 0);
        assert_eq!(peer_view["rows"][0]["votes"], json!([{"id":"3","owner":"99","amount":-900}]), "to-many traversal is independently filtered for the second user");

        let before_metrics = policy_vote_metrics(&fixture.schema).await;
        assert_eq!(before_metrics, (3, -900, 20, -870.0 / 3.0), "DB baseline includes hidden same-organization source rows");
        let standalone_vote_baseline = policy_standalone_vote_values(&fixture).await;
        assert_eq!(standalone_vote_baseline, vec![json!(2), json!(10), json!(20)]);
        // A vote owned by a different organization also belongs to the public count, while rowRead hides it
        // from this caller's source-value aggregates and to-many relation.
        policy_db_execute(&fixture.schema, "INSERT INTO vote(id,record_id,owner_id,amount) VALUES (6,1,100,-5000)").await;
        let after_cross_org_insert = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, query.clone()).await;
        let mut expected_cross_org_insert = before["rows"].clone();
        expected_cross_org_insert[0]["publicVoteCount"] = json!(4);
        assert_eq!(after_cross_org_insert["rows"], expected_cross_org_insert, "cross-organization hidden insert changes only the explicitly public count across the full row and votes response");
        assert_eq!(after_cross_org_insert["rows"][0]["ownVoteCount"], before["rows"][0]["ownVoteCount"]);
        assert_eq!(after_cross_org_insert["rows"][0]["minOwnVote"], before["rows"][0]["minOwnVote"]);
        assert_eq!(after_cross_org_insert["rows"][0]["maxOwnVote"], before["rows"][0]["maxOwnVote"]);
        assert_eq!(after_cross_org_insert["rows"][0]["votes"], before["rows"][0]["votes"], "hidden cross-organization vote does not change the child relation");
        assert_eq!(policy_standalone_vote_values(&fixture).await, standalone_vote_baseline, "standalone caller count/min/max ignore hidden cross-organization vote changes");
        policy_db_execute(&fixture.schema, "UPDATE vote SET amount=-6000 WHERE id=6").await;
        let after_cross_org_update = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, query.clone()).await;
        assert_eq!(after_cross_org_update["rows"], after_cross_org_insert["rows"], "cross-organization hidden update leaves caller-visible rows and votes unchanged");
        assert_eq!(after_cross_org_update["rows"][0]["minOwnVote"], before["rows"][0]["minOwnVote"]);
        assert_eq!(after_cross_org_update["rows"][0]["maxOwnVote"], before["rows"][0]["maxOwnVote"]);
        assert_eq!(after_cross_org_update["rows"][0]["votes"], before["rows"][0]["votes"]);
        assert_eq!(policy_standalone_vote_values(&fixture).await, standalone_vote_baseline, "standalone caller count/min/max ignore hidden cross-organization vote changes");
        policy_db_execute(&fixture.schema, "DELETE FROM vote WHERE id=6").await;
        let after_cross_org_delete = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, query.clone()).await;
        assert_eq!(after_cross_org_delete["rows"], before["rows"], "cross-organization hidden delete restores the full baseline response");
        assert_eq!(after_cross_org_delete["rows"][0]["votes"], before["rows"][0]["votes"]);
        assert_eq!(policy_standalone_vote_values(&fixture).await, standalone_vote_baseline, "standalone caller count/min/max ignore hidden cross-organization vote changes");

        policy_db_execute(&fixture.schema, "INSERT INTO vote(id,record_id,owner_id,amount) VALUES (6,1,99,-5000)").await;
        let after_insert = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, query.clone()).await;
        assert_eq!(after_insert["rows"][0]["ownVoteCount"], before["rows"][0]["ownVoteCount"]);
        assert_eq!(after_insert["rows"][0]["minOwnVote"], before["rows"][0]["minOwnVote"]);
        assert_eq!(after_insert["rows"][0]["maxOwnVote"], before["rows"][0]["maxOwnVote"]);
        let mut expected_after_insert = before["rows"].clone();
        expected_after_insert[0]["publicVoteCount"] = json!(4);
        assert_eq!(after_insert["rows"], expected_after_insert, "the explicitly public count is the only full-row response difference after hidden insert");
        assert_eq!(after_insert["rows"][0]["votes"], before["rows"][0]["votes"], "hidden insert leaves to-many votes unchanged");
        let inserted_public_aggregate = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"aggregate":"Record.publicVoteCount","input":{}}})).await;
        assert_eq!(inserted_public_aggregate["rows"][0], 6);
        let inserted_caller_aggregate = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"aggregate":"Record.ownVoteCount","input":{}}})).await;
        assert_eq!(inserted_caller_aggregate["rows"][0], 2);
        // count(no where) 는 rowRead 를 상속하지 않아 은닉 삽입에 반응하지만, min/max 는 measure() 가 rowRead 를 FILTER 로 걸어 반응하지 않는다.
        let inserted_min = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"aggregate":"Record.minOwnVote","input":{}}})).await;
        let inserted_max = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"aggregate":"Record.maxOwnVote","input":{}}})).await;
        assert_eq!(inserted_min["rows"][0], 10, "standalone min ignores the hidden -5000 row through source rowRead");
        assert_eq!(inserted_max["rows"][0], 20, "standalone max ignores hidden rows through source rowRead");
        let insert_metrics = policy_vote_metrics(&fixture.schema).await;
        assert_eq!(insert_metrics, (4, -5000, 20, (-5000.0 + 10.0 + 20.0 - 900.0) / 4.0));
        policy_db_execute(&fixture.schema, "UPDATE vote SET amount=-6000 WHERE id=6").await;
        let update_metrics = policy_vote_metrics(&fixture.schema).await;
        assert_eq!(update_metrics, (4, -6000, 20, (-6000.0 + 10.0 + 20.0 - 900.0) / 4.0));
        let after_update = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, query.clone()).await;
        assert_eq!(after_update["rows"][0]["ownVoteCount"], before["rows"][0]["ownVoteCount"]);
        assert_eq!(after_update["rows"][0]["publicVoteCount"], 4);
        assert_eq!(after_update["rows"][0]["minOwnVote"], before["rows"][0]["minOwnVote"]);
        assert_eq!(after_update["rows"][0]["maxOwnVote"], before["rows"][0]["maxOwnVote"]);
        assert_eq!(after_update["rows"], after_insert["rows"], "hidden-only update does not alter caller output or public row count");
        assert_eq!(post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"aggregate":"Record.publicVoteCount","input":{}}})).await["rows"][0], 6);
        policy_db_execute(&fixture.schema, "DELETE FROM vote WHERE id=6").await;
        let delete_metrics = policy_vote_metrics(&fixture.schema).await;
        assert_eq!(delete_metrics, before_metrics);
        let after_delete = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, query.clone()).await;
        assert_eq!(after_delete["rows"], before["rows"], "hidden-only delete restores the caller response");
        assert_eq!(post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"aggregate":"Record.publicVoteCount","input":{}}})).await["rows"][0], 5);

        let bad_cursor = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"read":"Record","select":["id"],"sort":[{"field":"id","dir":"asc"}],"after":"not-a-cursor"}})).await;
        assert_eq!(bad_cursor["code"], "WIRE.BAD_CURSOR", "malformed cursor is rejected");
        let votes = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, json!({"query":{"read":"Vote","select":["id","record","owner","amount"],"sort":[{"field":"id","dir":"asc"}]}})).await;
        assert_eq!(votes["rows"], json!([{"id":"1","record":"1","owner":"42","amount":10},{"id":"2","record":"1","owner":"42","amount":20}]), "child collection read excludes other owners and retains to-one references");
        policy_db_execute(&fixture.schema, "UPDATE member SET org='blue' WHERE id=42").await;
        let changed_policy = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, query).await;
        assert_eq!(changed_policy["rows"], json!([{"id":"2","owner":"100","secret":null,"ownVoteCount":0,"publicVoteCount":0,"minOwnVote":null,"maxOwnVote":null,"votes":[]},{"id":"5","owner":null,"secret":null,"ownVoteCount":0,"publicVoteCount":1,"minOwnVote":null,"maxOwnVote":null,"votes":[]}]), "same identity is re-evaluated against current row and reference policy");

        let unsupported_avg = SERVICE_AGGREGATE_APP.replace("release max(amount)", "release avg(amount)");
        assert_ne!(unsupported_avg, SERVICE_AGGREGATE_APP);
        let avg_file = fixture._dir.path().join("unsupported-avg.aip");
        std::fs::write(&avg_file, unsupported_avg).unwrap();
        fixture.stop_server().await;
        let output = Command::new(env!("CARGO_BIN_EXE_aip")).arg("service").arg("init").arg(&avg_file).arg("--config").arg(fixture._dir.path().join("service.json")).env("AIP_TEST_DATABASE", DB).output().await.unwrap();
        assert!(!output.status.success(), "avg is currently unsupported and must not be treated as an HTTP aggregate assertion");
    }).catch_unwind().await;
    fixture.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// 감사 2026-11-21 §6.2: 원본 Vote에 `rows read`가 있고 where 없는 `callerFilter read` count를 두면, 수정 전에는 기본 check가 통과했고
/// 다른 사용자의 숨은 행 값을 `filter amount eq`로 한 값씩 셀 수 있었다. 지금은 정의 검사가 거부한다. DB는 쓰지 않는다.
#[tokio::test]
async fn service_check_rejects_the_caller_filter_count_oracle_of_the_audit_repro() {
    const ORACLE: &str = "  aggregate oracleCount: Int { source Vote; sourceAccess publicVotes(actor); groupKey record; callerFilter read; rowOutput none; release count }\n  expose aggregate oracleCount\n";
    let dir = std::env::temp_dir().join(format!("aip-oracle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let check = |name: &str, source: String| {
        let file = dir.join(name);
        std::fs::write(&file, source).unwrap();
        let file = file.to_string_lossy().into_owned();
        async move {
            let output = Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "check", &file]).output().await.unwrap();
            let body = if output.status.success() { &output.stdout } else { &output.stderr };
            (output.status.success(), serde_json::from_slice::<Value>(body).unwrap())
        }
    };
    let with_line =
        |line: &str| SERVICE_AGGREGATE_APP.replacen("  expose aggregate ownVoteCount\n", &format!("{line}  expose aggregate ownVoteCount\n"), 1);

    // 변경 전 정의: 오류 없이 통과하고 where 없는 공개 count 경고는 publicVoteCount 하나뿐이다(--dev-checks 없이).
    let (ok, base) = check("base.aip", SERVICE_AGGREGATE_APP.to_string()).await;
    assert!(ok, "{base}");
    let advisories: Vec<_> =
        base["advisories"].as_array().unwrap().iter().map(|a| (a["code"].as_str().unwrap(), a["message"].as_str().unwrap())).collect();
    assert_eq!(advisories.len(), 1, "{base}");
    assert_eq!(advisories[0].0, "DEF.ADVISORY_PUBLIC_COUNT");
    assert!(advisories[0].1.contains("publicVoteCount"), "{base}");

    let (ok, rejected) = check("oracle.aip", with_line(ORACLE)).await;
    assert!(!ok, "오라클 정의는 기본 check에서 거부되어야 함: {rejected}");
    let diagnostics = rejected["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1, "{rejected}");
    assert_eq!(diagnostics[0]["code"], "DEF.CALLER_FILTER_POLICY_REQUIRED", "{rejected}");
    assert!(diagnostics[0]["message"].as_str().unwrap().contains("oracleCount"), "{rejected}");

    // Vote의 정책(`owner = actor`)을 where에 적으면 같은 정의가 통과하고 경고도 없다.
    let guarded = ORACLE.replace("groupKey record;", "groupKey record; where owner = actor;");
    let (ok, accepted) = check("guarded.aip", with_line(&guarded)).await;
    assert!(ok, "{accepted}");
    assert_eq!(accepted["advisories"].as_array().unwrap().len(), 1, "oracleCount는 where가 있어 경고 대상이 아님: {accepted}");
    std::fs::remove_dir_all(&dir).ok();
}

async fn read_policy_pages(fixture: &PolicyHttpFixture) -> (Vec<Value>, Vec<Value>) {
    let mut offsets = Vec::new();
    for offset in 0..=3 {
        offsets.push(
            post(
                &fixture.address,
                "/read",
                &fixture.token,
                &fixture.fingerprint,
                json!({"query":{"read":"Record","select":["id"],"sort":[{"field":"id","dir":"asc"}],"limit":1,"offset":offset}}),
            )
            .await,
        );
    }
    let mut cursors = Vec::new();
    for after in ["1", "3", "4"] {
        cursors.push(
            post(
                &fixture.address,
                "/read",
                &fixture.token,
                &fixture.fingerprint,
                json!({"query":{"read":"Record","select":["id"],"sort":[{"field":"id","dir":"asc"}],"limit":1,"after":{"id":after}}}),
            )
            .await,
        );
    }
    (offsets, cursors)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn service_policy_http_pagination_ignores_hidden_middle_rows() {
    let fixture = service_policy_http_fixture().await;
    let outcome = AssertUnwindSafe(async {
        let (baseline_offsets, baseline_cursors) = read_policy_pages(&fixture).await;
        assert_eq!(
            baseline_offsets.iter().map(|page| page["rows"].clone()).collect::<Vec<_>>(),
            vec![json!([{"id":"1"}]), json!([{"id":"3"}]), json!([{"id":"4"}]), json!([])]
        );
        assert_eq!(
            baseline_cursors.iter().map(|page| page["rows"].clone()).collect::<Vec<_>>(),
            vec![json!([{"id":"3"}]), json!([{"id":"4"}]), json!([])]
        );
        assert!(
            baseline_offsets.iter().chain(&baseline_cursors).all(|page| page["ok"] == true),
            "every offset and cursor response must be a successful read"
        );

        policy_db_execute(&fixture.schema, "UPDATE record SET title='changed-hidden-middle', secret='changed-hidden-secret' WHERE id=2").await;
        let (changed_offsets, changed_cursors) = read_policy_pages(&fixture).await;
        assert_eq!(
            (changed_offsets, changed_cursors),
            (baseline_offsets.clone(), baseline_cursors.clone()),
            "changing an unauthorized middle row preserves pages and terminal responses"
        );

        policy_db_execute(&fixture.schema, "DELETE FROM record WHERE id=2").await;
        let (deleted_offsets, deleted_cursors) = read_policy_pages(&fixture).await;
        assert_eq!(
            (deleted_offsets, deleted_cursors),
            (baseline_offsets.clone(), baseline_cursors.clone()),
            "deleting an unauthorized middle row preserves pages and terminal responses"
        );

        policy_db_execute(
            &fixture.schema,
            "INSERT INTO record(id,org,owner_id,title,secret,checked) VALUES (2,'blue',100,'inserted-hidden-middle','inserted-hidden-secret',false)",
        )
        .await;
        let (inserted_offsets, inserted_cursors) = read_policy_pages(&fixture).await;
        assert_eq!(
            (inserted_offsets, inserted_cursors),
            (baseline_offsets, baseline_cursors),
            "inserting an unauthorized middle row preserves pages and terminal responses"
        );
    })
    .catch_unwind()
    .await;
    fixture.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn service_policy_http_nested_to_one_hides_target_rows_and_fields() {
    let fixture = service_policy_http_fixture().await;
    let outcome = AssertUnwindSafe(async {
        let query = json!({"query":{"read":"Record","select":["id",{"owner":{"select":["id","email"]}}],"sort":[{"field":"id","dir":"asc"}]}});
        let before = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, query.clone()).await;
        assert_eq!(before["ok"], true, "{before}");
        assert_eq!(
            before["rows"],
            json!([
                {"id":"1","owner":{"id":"42","email":"owner@example.com"}},
                {"id":"3","owner":{"id":"99","email":null}},
                {"id":"4","owner":null}
            ]),
            "nested reference access must apply target row policy and target field policy"
        );

        policy_db_execute(&fixture.schema, "UPDATE member SET email='changed-peer@example.com' WHERE id=99").await;
        let after = post(&fixture.address, "/read", &fixture.token, &fixture.fingerprint, query).await;
        assert_eq!(after["rows"], before["rows"], "changing a hidden nested field does not alter the caller response");
    })
    .catch_unwind()
    .await;
    fixture.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn service_policy_caller_filter_read_and_none_boundaries() {
    let fixture = service_policy_http_fixture().await;
    let outcome = AssertUnwindSafe(async {
        let allowed = post(
            &fixture.address,
            "/read",
            &fixture.token,
            &fixture.fingerprint,
            json!({"query":{"aggregate":"Record.filteredOwnVoteCount","input":{},"filter":[{"field":"amount","op":"eq","value":20}]}}),
        )
        .await;
        assert_eq!(allowed["ok"], true, "callerFilter read accepts an allowed filter: {allowed}");
        assert_eq!(allowed["rows"], json!([1]), "callerFilter read filters inside the explicitly caller-scoped aggregate");

        let rejected = post(
            &fixture.address,
            "/read",
            &fixture.token,
            &fixture.fingerprint,
            json!({"query":{"aggregate":"Record.ownVoteCount","input":{},"filter":[]}}),
        )
        .await;
        assert_eq!(rejected["code"], "WIRE.FILTER_NOT_ALLOWED", "callerFilter none rejects even an empty caller filter");
        assert!(!rejected.to_string().contains("-900"), "callerFilter rejection must not include source row values: {rejected}");
        policy_db_execute(&fixture.schema, "UPDATE vote SET amount=-901 WHERE id=3").await;
        let changed_hidden_source = post(
            &fixture.address,
            "/read",
            &fixture.token,
            &fixture.fingerprint,
            json!({"query":{"aggregate":"Record.ownVoteCount","input":{},"filter":[]}}),
        )
        .await;
        assert_eq!(
            rejection_body_without_request_id(changed_hidden_source.clone()),
            rejection_body_without_request_id(rejected.clone()),
            "callerFilter rejection body, excluding the per-request correlation id, is identical after hidden source row value changes"
        );
        assert!(!changed_hidden_source.to_string().contains("-901"));
    })
    .catch_unwind()
    .await;
    fixture.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn service_policy_http_hidden_fields_cannot_filter_or_sort() {
    let fixture = service_policy_http_fixture().await;
    let outcome = AssertUnwindSafe(async {
        let hidden_filter = post(
            &fixture.address,
            "/read",
            &fixture.token,
            &fixture.fingerprint,
            json!({"query":{"read":"Record","select":["id"],"filter":[{"field":"secret","op":"eq","value":"peer-secret"}]}}),
        )
        .await;
        assert_eq!(hidden_filter["code"], "WIRE.FILTER_NOT_ALLOWED", "a field-hidden filter is not part of this read contract");
        assert!(
            !hidden_filter.to_string().contains("peer-secret"),
            "filter rejection must not return hidden field values or row data: {hidden_filter}"
        );
        let hidden_sort = post(
            &fixture.address,
            "/read",
            &fixture.token,
            &fixture.fingerprint,
            json!({"query":{"read":"Record","select":["id"],"sort":[{"field":"secret","dir":"asc"}]}}),
        )
        .await;
        assert_eq!(hidden_sort["code"], "WIRE.SORT_NOT_ALLOWED", "a field-hidden sort is not part of this read contract");
        assert!(!hidden_sort.to_string().contains("peer-secret"), "sort rejection must not return hidden field values or row data: {hidden_sort}");
        policy_db_execute(&fixture.schema, "UPDATE record SET secret='replacement-hidden-secret' WHERE id=2").await;
        let changed_value_filter = post(
            &fixture.address,
            "/read",
            &fixture.token,
            &fixture.fingerprint,
            json!({"query":{"read":"Record","select":["id"],"filter":[{"field":"secret","op":"eq","value":"replacement-hidden-secret"}]}}),
        )
        .await;
        assert_eq!(
            rejection_body_without_request_id(changed_value_filter.clone()),
            rejection_body_without_request_id(hidden_filter.clone()),
            "hidden filter denial, excluding the per-request correlation id, is identical across different underlying secret values"
        );
        assert!(!changed_value_filter.to_string().contains("replacement-hidden-secret"));
        let changed_hidden_sort = post(
            &fixture.address,
            "/read",
            &fixture.token,
            &fixture.fingerprint,
            json!({"query":{"read":"Record","select":["id"],"sort":[{"field":"secret","dir":"asc"}]}}),
        )
        .await;
        assert_eq!(
            rejection_body_without_request_id(changed_hidden_sort.clone()),
            rejection_body_without_request_id(hidden_sort.clone()),
            "hidden sort denial, excluding the per-request correlation id, is identical across different underlying secret values"
        );
        assert!(
            !changed_hidden_sort.to_string().contains("replacement-hidden-secret"),
            "sort rejection must not return changed hidden values: {changed_hidden_sort}"
        );
    })
    .catch_unwind()
    .await;
    fixture.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
