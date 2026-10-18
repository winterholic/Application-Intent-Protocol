//! Slice13-D through the real HTTP server (`POST /extension`) with the real Python worker, the real extension code and
//! the real `facereview.aip`. Compare with spikes/spike-v4-worker/tests/slice13d_home_native.rs, which uses real drivers
//! below the server but no HTTP layer.
//!
//! EVIDENCE SCOPE
//! - real: HTTP server, contract fingerprint check, bearer token, worker process, `slice13d_home.py`, request planning against
//!   the compiled FaceReview definition.
//! - scripted: MySQL rows (chosen by the resource the plan reads) and MEMORY Mongo/Redis capabilities. No real database or
//!   driver is involved, so nothing here is Mongo/Redis driver evidence and nothing says the SQL is right.
//! `gap_*` tests pin today's blocked behaviour of the shared core; they must fail when the dependency is closed.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    backend::{BackendFuture, RelationalBackend},
    id_wire::IdWire,
    plan::{Plan, Reject},
    sqlgen::Dialect,
};
use spike_v4_worker::{MemoryMongoCapability, MemoryRedisCapability};
use spike_v6_transport::{
    infrastructure::Infrastructure,
    server::{listen_relational_with_extensions, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const DEFINITION: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/facereview.aip");
const UUID_USER: &str = "0b9c1d22-4e5f-4a61-8c7d-1234567890ab";
const VIDEO_IDS: [&str; 3] = ["11111111-1111-4111-8111-111111111111", "22222222-2222-4222-8222-222222222222", "33333333-3333-4333-8333-333333333333"];

struct ScriptedBackend {
    bookmarks: Vec<&'static str>,
}

impl RelationalBackend for ScriptedBackend {
    fn dialect(&self) -> Dialect {
        Dialect::MySql
    }
    fn adapter(&self) -> &'static str {
        "scripted"
    }
    fn ready(&self) -> BackendFuture<'_, Result<(), &'static str>> {
        Box::pin(async { Ok(()) })
    }
    fn now(&self) -> BackendFuture<'_, Result<String, &'static str>> {
        Box::pin(async { Ok("2026-10-18T12:00:00Z".into()) })
    }
    fn read<'a>(&'a self, plan: &'a Plan, _wire: IdWire) -> BackendFuture<'a, Result<Vec<Value>, Reject>> {
        let has = |name: &str| plan.deps.iter().any(|d| d == name);
        let rows: Vec<Value> = if has("VideoBookmark") {
            self.bookmarks.iter().map(|v| json!({"video": v})).collect()
        } else if has("VideoViewLog") || has("UserFavoriteGenre") {
            Vec::new()
        } else {
            vec![json!({"id": UUID_USER})]
        };
        Box::pin(async move { Ok(rows) })
    }
}

fn facts() -> Value {
    let mut facts = load_str(DEFINITION, Form::A).expect("FaceReview definition").execution;
    // Grants cannot be written in the definition language yet (dependency D1); only they are attached here.
    let grants: [(&str, &[&str]); 2] = [
        ("homeCategory", &["read.VideoBookmark", "redis.facereview:reco:*"]),
        (
            "homePersonalized",
            &["read.User", "read.UserFavoriteGenre", "read.VideoViewLog", "read.VideoBookmark", "mongo.youtube_watching_data", "redis.facereview:reco:*"],
        ),
    ];
    for (name, list) in grants {
        let ext = &mut facts["resources"]["Video"]["extensions"][name];
        assert_eq!(ext["access"], json!({}));
        ext["access"] = list.iter().map(|g| ((*g).to_string(), json!({}))).collect::<serde_json::Map<_, _>>().into();
    }
    facts
}

/// Over HTTP the extension's own error code never reaches the client: every failure is the same generic body, so
/// ADAPTER_UNAVAILABLE, SLICE13D.*_CACHE_MISS and ACCESS_DENIED cannot be told apart (reported as D6, HTTP form).
fn assert_hidden_failure(resp: &Value) {
    assert_eq!(resp["ok"], false, "{resp}");
    assert_eq!(resp["code"], "WIRE.EXTENSION_ERROR", "{resp}");
    assert_eq!(resp["retry"], "never", "{resp}");
    assert!(!resp.to_string().contains("SLICE13D") && !resp.to_string().contains("ADAPTER_UNAVAILABLE"), "inner code leaked: {resp}");
}

fn pool_entry(id: &str, category: &str) -> Value {
    json!({"video_id": id, "youtube_url": "u", "title": "t", "category": category, "dominant_emotion": "happy",
           "dominant_emotion_per": 60.0, "emotion_distribution": {"happy": 0.6, "neutral": 0.4}, "base_score": 50.0})
}

async fn post(address: SocketAddr, token: &str, contract: &str, body: &Value) -> Value {
    let mut socket = TcpStream::connect(address).await.expect("connect");
    let body = body.to_string();
    let request = format!(
        "POST /extension HTTP/1.1\r\nhost: 127.0.0.1\r\nauthorization: Bearer {token}\r\nx-aip-contract: {contract}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(request.as_bytes()).await.expect("write");
    let mut response = Vec::new();
    timeout(Duration::from_secs(8), socket.read_to_end(&mut response)).await.expect("response timeout").expect("read");
    let response = String::from_utf8(response).expect("utf8");
    let (_, response_body) = response.split_once("\r\n\r\n").expect("body");
    serde_json::from_str(response_body).unwrap_or_else(|e| panic!("response is not JSON ({e}): {response_body}"))
}

async fn serve(
    facts: &Value,
    backend: ScriptedBackend,
    mongo: Option<MemoryMongoCapability>,
    redis: Option<MemoryRedisCapability>,
) -> (spike_v6_transport::server::Server, String, String) {
    let infra = Infrastructure {
        mongo: mongo.map(|m| Arc::new(m) as Arc<dyn spike_v4_worker::MongoCapability>),
        redis: redis.map(|r| Arc::new(r) as Arc<dyn spike_v4_worker::RedisCapability>),
        ..Infrastructure::default()
    };
    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../product/tests/fixtures/facereview-mysql/extensions"));
    let server = listen_relational_with_extensions(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(backend),
        Some(ReadExtensions { lang: WorkerLang::Python, dir }),
        ServerOptions::default(),
        infra,
        None,
    )
    .await
    .expect("server must start");
    let contract = spike_v5_sdk::contract_fingerprint_with_extensions(facts, IdWire::SafeNumber);
    let token = server.keys.issue(1, 600);
    (server, contract, token)
}

fn redis_with(seed: impl FnOnce(&MemoryRedisCapability)) -> MemoryRedisCapability {
    let redis = MemoryRedisCapability::new(&["facereview:reco:"], &["get", "hgetall", "exists", "mget"]);
    seed(&redis);
    redis
}

#[tokio::test]
async fn category_over_http_returns_cached_groups_and_marks_bookmarks() {
    let facts = facts();
    let redis = redis_with(|r| {
        let entries = |ids: &[&str], c: &str| json!(ids.iter().map(|i| pool_entry(i, c)).collect::<Vec<_>>()).to_string();
        r.set_hash(
            "facereview:reco:category",
            [("music".to_string(), entries(&VIDEO_IDS[..2], "music")), ("game".to_string(), entries(&VIDEO_IDS[2..], "game"))].into_iter().collect(),
        );
    });
    let (server, contract, token) = serve(&facts, ScriptedBackend { bookmarks: vec![VIDEO_IDS[1]] }, None, Some(redis)).await;
    let resp = post(server.address, &token, &contract, &json!({"extension": "Video.homeCategory", "input": {}})).await;
    assert_eq!(resp["ok"], true, "{resp}");
    let groups: Value = serde_json::from_str(resp["output"]["video_data"].as_str().expect("JSON text envelope")).unwrap();
    let mut flat: Vec<(String, bool)> = groups
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["videos"].as_array().unwrap().iter().map(|v| (v["video_id"].as_str().unwrap().to_string(), v["is_bookmarked"].as_bool().unwrap())))
        .collect();
    flat.sort();
    assert_eq!(flat, VIDEO_IDS.iter().map(|id| (id.to_string(), *id == VIDEO_IDS[1])).collect::<Vec<_>>());
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn category_without_redis_adapter_or_with_empty_hash_fails_the_request_instead_of_returning_data() {
    let facts = facts();
    let (server, contract, token) = serve(&facts, ScriptedBackend { bookmarks: vec![] }, None, None).await;
    let absent = post(server.address, &token, &contract, &json!({"extension": "Video.homeCategory", "input": {}})).await;
    assert_hidden_failure(&absent);
    server.shutdown().await.unwrap();

    let (server, contract, token) = serve(&facts, ScriptedBackend { bookmarks: vec![] }, None, Some(redis_with(|_| {}))).await;
    let miss = post(server.address, &token, &contract, &json!({"extension": "Video.homeCategory", "input": {}})).await;
    assert_hidden_failure(&miss);
    assert_eq!(absent["msg"], miss["msg"], "absent adapter and cache miss are indistinguishable to the client");
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn gap_uuid_user_request_fails_through_http_with_the_cause_hidden() {
    let facts = facts();
    let redis = redis_with(|r| r.set_str("facereview:reco:pool", &json!([pool_entry(VIDEO_IDS[0], "music")]).to_string()));
    let mongo = MemoryMongoCapability::new(&["youtube_watching_data"], &["find", "find_one", "count_documents"]);
    let (server, contract, token) = serve(&facts, ScriptedBackend { bookmarks: vec![] }, Some(mongo), Some(redis)).await;
    let resp = post(server.address, &token, &contract, &json!({"extension": "Video.homePersonalized", "input": {}})).await;
    // The cause (ACCESS_DENIED on the private collection) is proven by the native test; HTTP shows only the generic failure.
    assert_hidden_failure(&resp);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn gap_undeclared_read_is_allowed_through_the_http_server() {
    let mut facts = facts();
    facts["resources"]["Video"]["extensions"]["probe"] = json!({
        "kind": "read", "effect": "none", "deadlineMs": 5000, "input": [], "output": [["result", "Text", null]],
        "access": {}, "implementation": "slice13d_probe.undeclared_read"
    });
    let (server, contract, token) = serve(&facts, ScriptedBackend { bookmarks: vec![] }, None, None).await;
    let resp = post(server.address, &token, &contract, &json!({"extension": "Video.probe", "input": {}})).await;
    assert_eq!(resp["ok"], true, "{resp}");
    assert_eq!(resp["output"]["result"], "READ_OK rows=1", "{resp}");
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_rejects_a_request_whose_contract_fingerprint_differs() {
    let facts = facts();
    let (server, _, token) = serve(&facts, ScriptedBackend { bookmarks: vec![] }, None, Some(redis_with(|_| {}))).await;
    let resp = post(server.address, &token, "not-the-fingerprint", &json!({"extension": "Video.homeCategory", "input": {}})).await;
    assert_ne!(resp["ok"], true, "{resp}");
    server.shutdown().await.unwrap();
}
