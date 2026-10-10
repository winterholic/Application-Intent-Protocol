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

const DB: &str = "host=localhost dbname=postgres";
const APP: &str = include_str!("../../../prototype/example/app.aip");

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
            std::fs::write(&config, json!({"schema":schema,"database_url_env":"AIP_TEST_DATABASE","listen":"127.0.0.1:0","wire":wire,"allowed_origins":["https://app.example.com"],"auth":{"issuer":"https://issuer.example","audience":"aip-api","jwks":{"kind":"file","path":"jwks.json"}},"workers":{"directory":workers,"language":lang}}).to_string()).unwrap();
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
                assert_eq!(waiting.await.unwrap()["code"], "UNAUTHENTICATED", "revoked principal must not receive in-flight output");
                assert_eq!(post(address,"/operation",&token,fingerprint,body.clone()).await["code"],"UNAUTHENTICATED");
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
                assert_eq!(post(address,"/operation",&token,fingerprint,body.clone()).await["code"],"DEPLOYMENT_CHANGED");
                stop(child).await;
                let (child,listening) = start(&config,&file).await;
                assert_eq!(post(listening["address"].as_str().unwrap(),"/operation",&token,listening["fingerprint"].as_str().unwrap(),body).await["code"],"ACCESS_DENIED");
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
        assert_eq!(post(address,"/read",&token,fingerprint,json!({"query":{"read":"Event","select":["id"]}})).await["code"],"UNAUTHENTICATED");
        stop(child).await;
        let new_source=APP.replace("date: Time }","date: Time; note: Text? }");assert_ne!(new_source,APP);std::fs::write(&file,new_source).unwrap();
        let plan=command(&config,&["migrate",file.to_str().unwrap()]).await;assert_eq!(plan["blocked"],false,"{plan}");
        let applied=command(&config,&["migrate",file.to_str().unwrap(),"--apply",plan["digest"].as_str().unwrap()]).await;assert_eq!(applied["ok"],true);
        let row=db.query_one(&format!("SELECT checked,note FROM {schema}.event WHERE id=11"),&[]).await.unwrap();assert!(row.get::<_,bool>(0));assert_eq!(row.get::<_,Option<String>>(1),None);
        command(&config,&["principal","bind","--subject","user-42","--actor","42"]).await;
        let (child,listening)=start(&config,&file).await;
        let old_address=listening["address"].as_str().unwrap();
        let old_fingerprint=listening["fingerprint"].as_str().unwrap();
        assert_eq!(post(old_address,"/session",&token,"",json!({})).await["code"],"UNAUTHENTICATED","rebind must not revive a token issued before revoke");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let token=encode(&header,&json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-42","iat":now,"nbf":now,"exp":now+300}),&key).unwrap();
        assert_eq!(post(old_address,"/read",&token,old_fingerprint,json!({"query":{"read":"Event","select":["id"]}})).await["rows"],json!([{"id":"11"}]));
        let restricted=std::fs::read_to_string(&file).unwrap().replace("rows read when member = actor","rows read when member = actor and checked = false");
        std::fs::write(&file,restricted).unwrap();
        let plan=command(&config,&["migrate",file.to_str().unwrap()]).await;
        command(&config,&["migrate",file.to_str().unwrap(),"--apply",plan["digest"].as_str().unwrap()]).await;
        let fenced=post(old_address,"/read",&token,old_fingerprint,json!({"query":{"read":"Event","select":["id"]}})).await;
        assert_eq!(fenced["code"],"DEPLOYMENT_CHANGED","live old policy must stop authorizing reads: {fenced}");
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
