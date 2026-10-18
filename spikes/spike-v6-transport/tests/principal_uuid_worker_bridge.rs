//! 서버가 검증한 세션 공개 UUID가 worker ctx.mongo의 비공개 문서 소유자로 전달되는지 HTTP 경계에서 확인한다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    backend::{BackendFuture, RelationalBackend},
    id_wire::IdWire,
    plan::{Plan, Reject},
    sqlgen::Dialect,
};
use spike_v4_worker::MemoryMongoCapability;
use spike_v6_transport::{
    auth::{AuthError, Authenticator, Session},
    infrastructure::Infrastructure,
    server::{listen_relational_with_extensions, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{net::SocketAddr, path::PathBuf, pin::Pin, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const FIXTURE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const ALICE: &str = "0b1f4c7e-52a1-4d3e-9c1a-7f2b8d6e4a10";
const BOB: &str = "9d3c2b1a-8e7f-4a6b-b5c4-1e0f9a8b7c6d";

struct Backend;

impl RelationalBackend for Backend {
    fn dialect(&self) -> Dialect {
        Dialect::MySql
    }
    fn adapter(&self) -> &'static str {
        "mysql"
    }
    fn ready(&self) -> BackendFuture<'_, Result<(), &'static str>> {
        Box::pin(async { Ok(()) })
    }
    fn now(&self) -> BackendFuture<'_, Result<String, &'static str>> {
        Box::pin(async { Ok("2026-10-18T00:00:00Z".to_string()) })
    }
    fn read<'a>(&'a self, _plan: &'a Plan, _wire: IdWire) -> BackendFuture<'a, Result<Vec<Value>, Reject>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// 토큰 문자열로 세션 actor의 내부 handle과 서버가 검증한 공개 id를 고르는 테스트용 인증 출처.
struct SessionSource;

impl Authenticator for SessionSource {
    fn verify_session<'a>(&'a self, token: &'a str) -> Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let (actor, public) = match token {
                "alice" => (7, Some(ALICE.to_string())),
                "bob" => (8, Some(BOB.to_string())),
                "integer" => (7, Some("7".to_string())),
                "plain" => (7, None),
                _ => (7, Some("garbage".to_string())),
            };
            let expires = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64 + 600;
            let mut session = Session::new(actor, expires)?;
            session.actor_resource_id = public;
            Ok(session)
        })
    }
}

fn facts() -> Value {
    let mut facts = load_str(FIXTURE, Form::A).expect("valid fixture").execution;
    let extensions = facts["resources"]["Recruitment"]["extensions"].as_object_mut().unwrap();
    extensions.clear();
    extensions.insert(
        "watched".into(),
        json!({
            "kind": "read",
            "effect": "none",
            "deadlineMs": 3000,
            "input": [["clubId", "Id<Club>"]],
            "output": [["clubName", "Text"], ["distCount", "Int"], ["cachedReco", "Text"]],
            "access": { "mongo.youtube_watching_data": {} },
            "implementation": "principal_uuid.watched",
        }),
    );
    facts
}

async fn post(address: SocketAddr, token: &str, contract: &str) -> Value {
    let body = json!({"extension":"Recruitment.watched","input":{"clubId":10}}).to_string();
    let mut socket = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "POST /extension HTTP/1.1\r\nhost: 127.0.0.1\r\nauthorization: Bearer {token}\r\nx-aip-contract: {contract}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(6), socket.read_to_end(&mut response)).await.unwrap().unwrap();
    let response = String::from_utf8(response).unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn session_public_uuid_owns_private_mongo_documents_through_the_worker() {
    let mongo = MemoryMongoCapability::new(&["youtube_watching_data"], &["find", "find_one", "count_documents"]);
    mongo.seed(
        "youtube_watching_data",
        vec![
            json!({"user_id": ALICE, "video_id": 1}),
            json!({"user_id": BOB, "video_id": 2}),
            json!({"user_id": 7, "video_id": 3}),
            json!({"user_id": "7", "video_id": 4}),
        ],
    );
    let facts = facts();
    let server = listen_relational_with_extensions(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(Backend),
        Some(ReadExtensions { lang: WorkerLang::Python, dir: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/principal_uuid")) }),
        ServerOptions::default(),
        Infrastructure { mongo: Some(Arc::new(mongo)), ..Infrastructure::default() },
        Some(Arc::new(SessionSource)),
    )
    .await
    .expect("server must start");
    let contract = spike_v5_sdk::contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);

    // UUID actor는 내부 handle 7의 숫자·문자열 문서가 아니라 자기 UUID 문서만 본다.
    for (token, video) in [("alice", "1"), ("bob", "2")] {
        let response = post(server.address, token, &contract).await;
        assert_eq!(response["ok"], true, "{token}: {response}");
        assert_eq!((&response["output"]["distCount"], &response["output"]["cachedReco"]), (&json!(1), &json!(video)), "{token}");
    }
    // 정수 actor(공개 id가 자기 10진 id이거나 없음)는 기존대로 숫자 문서만 본다.
    for token in ["integer", "plain"] {
        let response = post(server.address, token, &contract).await;
        assert_eq!(response["ok"], true, "{token}: {response}");
        assert_eq!((&response["output"]["distCount"], &response["output"]["cachedReco"]), (&json!(1), &json!("3")), "{token}");
    }
    // 확인할 수 없는 공개 id는 숫자 handle로 대체되지 않고 거부된다.
    let response = post(server.address, "garbage", &contract).await;
    assert_eq!(response["ok"], false, "{response}");

    server.shutdown().await.unwrap();
}
