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
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const FIXTURE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

type QueryRecord = (String, Vec<Option<String>>);
type QueryStore = Arc<Mutex<Vec<QueryRecord>>>;
type RowsStore = Arc<Mutex<Option<Vec<Value>>>>;

struct MockRelationalBackend {
    executed_queries: QueryStore,
    rows_override: RowsStore,
}

impl MockRelationalBackend {
    fn new() -> (Self, QueryStore, RowsStore) {
        let executed_queries = Arc::new(Mutex::new(Vec::new()));
        let rows_override = Arc::new(Mutex::new(None));
        (Self { executed_queries: executed_queries.clone(), rows_override: rows_override.clone() }, executed_queries, rows_override)
    }
}

impl RelationalBackend for MockRelationalBackend {
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
        Box::pin(async { Ok("2026-10-18T00:00:00Z".into()) })
    }

    fn read<'a>(&'a self, plan: &'a Plan, _wire: IdWire) -> BackendFuture<'a, Result<Vec<Value>, Reject>> {
        self.executed_queries.lock().unwrap().push((plan.sql.clone(), plan.params.clone()));

        let rows = if let Some(custom) = self.rows_override.lock().unwrap().as_ref() {
            custom.clone()
        } else {
            // Default mock row matching Recruitment resource expose read { select id, title }
            vec![json!({ "id": 10, "title": "Soccer Club" })]
        };

        Box::pin(async move { Ok(rows) })
    }
}

fn setup_facts() -> Value {
    let mut facts = load_str(FIXTURE, Form::A).expect("valid fixture").execution;
    let extensions = facts["resources"]["Recruitment"]["extensions"].as_object_mut().unwrap();
    extensions.clear();

    let base_input = json!([["clubId", "Id<Club>"]]);
    let base_output = json!([["clubName", "Text"], ["distCount", "Int"], ["cachedReco", "Text"]]);

    let make_ext = |impl_name: &str| {
        json!({
            "kind": "read",
            "effect": "none",
            "deadlineMs": 3000,
            "input": base_input,
            "output": base_output,
            "access": {
                "mongo.video_distribution": {},
                "redis.facereview:reco:*": {}
            },
            "implementation": impl_name,
        })
    };

    extensions.insert("feed".into(), make_ext("slice13a.feed"));
    extensions.insert("readRedis".into(), make_ext("slice13a.readRedisOnly"));
    extensions.insert("unauthMongoCollection".into(), make_ext("slice13a.unauthMongoCollection"));
    extensions.insert("unauthMongoWrite".into(), make_ext("slice13a.unauthMongoWrite"));
    extensions.insert("unauthRedisKey".into(), make_ext("slice13a.unauthRedisKey"));
    extensions.insert("unauthApply".into(), make_ext("slice13a.unauthApply"));
    extensions.insert("policyOwner".into(), make_ext("slice13a.policyOwnerCheck"));

    facts
}

async fn post(address: SocketAddr, token: Option<&str>, contract: Option<&str>, body: &Value) -> Result<Value, String> {
    let mut socket = TcpStream::connect(address).await.map_err(|error| format!("connect: {:?}", error.kind()))?;
    let body = body.to_string();
    let authorization = token.map(|token| format!("authorization: Bearer {token}\r\n")).unwrap_or_default();
    let contract = contract.map(|fingerprint| format!("x-aip-contract: {fingerprint}\r\n")).unwrap_or_default();
    let request = format!("POST /extension HTTP/1.1\r\nhost: 127.0.0.1\r\n{authorization}{contract}content-length: {}\r\n\r\n{body}", body.len());
    socket.write_all(request.as_bytes()).await.map_err(|error| format!("write: {:?}", error.kind()))?;
    let mut response = Vec::new();
    timeout(Duration::from_secs(6), socket.read_to_end(&mut response))
        .await
        .map_err(|_| "HTTP response timeout".to_string())?
        .map_err(|error| format!("read: {:?}", error.kind()))?;
    let response = String::from_utf8(response).map_err(|_| "HTTP response is not UTF-8".to_string())?;
    let (_, response_body) = response.split_once("\r\n\r\n").ok_or_else(|| format!("HTTP response has no body: {response}"))?;
    serde_json::from_str(response_body).map_err(|error| format!("invalid JSON response: {error}"))
}

#[tokio::test]
async fn test_relational_worker_mysql_read_and_mongo_redis_combined() {
    let facts = setup_facts();
    let (backend, executed_queries, _) = MockRelationalBackend::new();

    let mongo = MemoryMongoCapability::new(&["video_distribution"], &["find", "find_one", "count_documents"]);
    mongo.seed(
        "video_distribution",
        vec![json!({ "clubId": "10", "views": 100, "region": "KR" }), json!({ "clubId": "10", "views": 250, "region": "US" })],
    );

    let redis = MemoryRedisCapability::new(&["facereview:reco:"], &["get", "hgetall", "exists", "mget"]);
    redis.seed("facereview:reco:10", "recommended-item-alpha");

    let infra = Infrastructure { mongo: Some(Arc::new(mongo)), redis: Some(Arc::new(redis)), ..Infrastructure::default() };

    let ext_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/slice13a"));
    let read_extensions = ReadExtensions { lang: WorkerLang::Python, dir: ext_dir };

    let server = listen_relational_with_extensions(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(backend),
        Some(read_extensions),
        ServerOptions::default(),
        infra,
        None,
    )
    .await
    .expect("server must start");

    let contract = spike_v5_sdk::contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);
    let token = server.keys.issue(1, 600);

    // Case 1: Combined read (MySQL read callback + Mongo find + Redis get)
    let body = json!({
        "extension": "Recruitment.feed",
        "input": { "clubId": 10 }
    });

    let resp = post(server.address, Some(&token), Some(&contract), &body).await.expect("valid response");
    assert_eq!(resp["ok"], true, "Response must be ok: {resp}");
    assert_eq!(resp["output"]["clubName"], "Soccer Club", "MySQL read callback delivered title");
    assert_eq!(resp["output"]["distCount"], 2, "Mongo capability delivered 2 seeded records");
    assert_eq!(resp["output"]["cachedReco"], "recommended-item-alpha", "Redis capability delivered cached value");

    // Verify backend received MySQL dialect query for Recruitment resource
    {
        let queries = executed_queries.lock().unwrap();
        assert!(!queries.is_empty(), "RelationalBackend must have executed at least 1 plan");
        assert!(queries[0].0.to_lowercase().contains("recruitment"), "Plan SQL must reference recruitment table: {}", queries[0].0);
    }

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_relational_worker_redis_cache_read() {
    let facts = setup_facts();
    let (backend, _, _) = MockRelationalBackend::new();

    let redis = MemoryRedisCapability::new(&["facereview:reco:"], &["get", "hgetall", "exists", "mget"]);
    redis.seed("facereview:reco:10", "cached-reco-xyz");

    let infra = Infrastructure { redis: Some(Arc::new(redis)), ..Infrastructure::default() };

    let ext_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/slice13a"));
    let server = listen_relational_with_extensions(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(backend),
        Some(ReadExtensions { lang: WorkerLang::Python, dir: ext_dir }),
        ServerOptions::default(),
        infra,
        None,
    )
    .await
    .unwrap();

    let contract = spike_v5_sdk::contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);
    let token = server.keys.issue(1, 600);

    // Case 2: Redis cache get
    let body = json!({
        "extension": "Recruitment.readRedis",
        "input": { "clubId": 10 }
    });
    let resp = post(server.address, Some(&token), Some(&contract), &body).await.unwrap();
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["output"]["cachedReco"], "cached-reco-xyz");

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_relational_worker_rejects_unauthorized_mongo_collection() {
    let facts = setup_facts();
    let (backend, _, _) = MockRelationalBackend::new();

    let mongo = MemoryMongoCapability::new(&["video_distribution"], &["find", "find_one", "count_documents"]);
    let infra = Infrastructure { mongo: Some(Arc::new(mongo)), ..Infrastructure::default() };

    let ext_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/slice13a"));
    let server = listen_relational_with_extensions(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(backend),
        Some(ReadExtensions { lang: WorkerLang::Python, dir: ext_dir }),
        ServerOptions::default(),
        infra,
        None,
    )
    .await
    .unwrap();

    let contract = spike_v5_sdk::contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);
    let token = server.keys.issue(1, 600);

    // Case 3: Mongo collection 'admin_secrets' is not in the extension's `access` list. The per-extension grant is checked
    // before the server allowlist, so this is ACCESS_NOT_DECLARED (spec/wire-errors.md); CAPABILITY_DENIED is for declared grants the server refuses.
    let body = json!({
        "extension": "Recruitment.unauthMongoCollection",
        "input": { "clubId": 10 }
    });
    let resp = post(server.address, Some(&token), Some(&contract), &body).await.unwrap();
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["output"]["clubName"], "WIRE.ACCESS_NOT_DECLARED", "Undeclared mongo collection must be denied with WIRE.ACCESS_NOT_DECLARED");

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_relational_worker_rejects_unauthorized_mongo_write() {
    let facts = setup_facts();
    let (backend, _, _) = MockRelationalBackend::new();

    let mongo = MemoryMongoCapability::new(&["video_distribution"], &["find", "find_one", "count_documents"]);
    let infra = Infrastructure { mongo: Some(Arc::new(mongo)), ..Infrastructure::default() };

    let ext_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/slice13a"));
    let server = listen_relational_with_extensions(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(backend),
        Some(ReadExtensions { lang: WorkerLang::Python, dir: ext_dir }),
        ServerOptions::default(),
        infra,
        None,
    )
    .await
    .unwrap();

    let contract = spike_v5_sdk::contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);
    let token = server.keys.issue(1, 600);

    // Case 4: Mongo write operation 'insert'
    let body = json!({
        "extension": "Recruitment.unauthMongoWrite",
        "input": { "clubId": 10 }
    });
    let resp = post(server.address, Some(&token), Some(&contract), &body).await.unwrap();
    assert_eq!(resp["ok"], true);
    assert_eq!(
        resp["output"]["clubName"], "WIRE.CAPABILITY_DENIED",
        "Write operation on Mongo capability must be denied with WIRE.CAPABILITY_DENIED"
    );

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_relational_worker_rejects_unauthorized_redis_key() {
    let facts = setup_facts();
    let (backend, _, _) = MockRelationalBackend::new();

    let redis = MemoryRedisCapability::new(&["facereview:reco:"], &["get", "hgetall", "exists", "mget"]);
    let infra = Infrastructure { redis: Some(Arc::new(redis)), ..Infrastructure::default() };

    let ext_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/slice13a"));
    let server = listen_relational_with_extensions(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(backend),
        Some(ReadExtensions { lang: WorkerLang::Python, dir: ext_dir }),
        ServerOptions::default(),
        infra,
        None,
    )
    .await
    .unwrap();

    let contract = spike_v5_sdk::contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);
    let token = server.keys.issue(1, 600);

    // Case 5: Redis key 'other_app:secret' matches no declared `redis.<prefix>*` grant, so the grant check rejects it first.
    let body = json!({
        "extension": "Recruitment.unauthRedisKey",
        "input": { "clubId": 10 }
    });
    let resp = post(server.address, Some(&token), Some(&contract), &body).await.unwrap();
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["output"]["clubName"], "WIRE.ACCESS_NOT_DECLARED", "Undeclared redis key must be denied with WIRE.ACCESS_NOT_DECLARED");

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_relational_worker_rejects_unauthorized_apply_in_read_extension() {
    let facts = setup_facts();
    let (backend, _, _) = MockRelationalBackend::new();

    let ext_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/slice13a"));
    let server = listen_relational_with_extensions(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(backend),
        Some(ReadExtensions { lang: WorkerLang::Python, dir: ext_dir }),
        ServerOptions::default(),
        Infrastructure::default(),
        None,
    )
    .await
    .unwrap();

    let contract = spike_v5_sdk::contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);
    let token = server.keys.issue(1, 600);

    // Case 6: ctx.data.apply call in read extension
    let body = json!({
        "extension": "Recruitment.unauthApply",
        "input": { "clubId": 10 }
    });
    let resp = post(server.address, Some(&token), Some(&contract), &body).await.unwrap();
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["output"]["clubName"], "WIRE.ACCESS_DENIED", "Calling apply in read extension must be rejected with ACCESS_DENIED");

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_relational_worker_principal_policy_guest_vs_owner() {
    let facts = setup_facts();
    let (backend, executed_queries, _) = MockRelationalBackend::new();

    let ext_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/slice13a"));
    let server = listen_relational_with_extensions(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(backend),
        Some(ReadExtensions { lang: WorkerLang::Python, dir: ext_dir }),
        ServerOptions::default(),
        Infrastructure::default(),
        None,
    )
    .await
    .unwrap();

    let contract = spike_v5_sdk::contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);

    let body = json!({
        "extension": "Recruitment.policyOwner",
        "input": { "clubId": 10 }
    });

    // 1. Guest request (without token): must be immediately rejected at transport boundary
    let guest_resp = post(server.address, None, Some(&contract), &body).await.unwrap();
    assert_eq!(guest_resp["ok"], false);
    assert_eq!(guest_resp["code"], "WIRE.UNAUTHENTICATED", "Guest request without signed session must be rejected with UNAUTHENTICATED");

    // 2. Actor 1 request (with signed token for actor 1)
    let token1 = server.keys.issue(1, 600);
    let resp1 = post(server.address, Some(&token1), Some(&contract), &body).await.unwrap();
    assert_eq!(resp1["ok"], true);

    // 3. Actor 42 request (with signed token for actor 42)
    let token42 = server.keys.issue(42, 600);
    let resp42 = post(server.address, Some(&token42), Some(&contract), &body).await.unwrap();
    assert_eq!(resp42["ok"], true);

    let all_queries = executed_queries.lock().unwrap().clone();
    assert_eq!(all_queries.len(), 2, "Exactly 2 authenticated queries should have been executed");

    let query1 = &all_queries[0];
    let query42 = &all_queries[1];

    // Both plans were planned under their respective caller principals
    // The actor_id is bound securely by the server's Grant, never spoofable by worker
    assert!(
        query1.1 != query42.1 || query1.0 != query42.0 || query42.1.iter().any(|p| p.as_deref() == Some("42")),
        "Different actor tokens must result in distinct principal evaluation in planned query"
    );

    server.shutdown().await.unwrap();
}
