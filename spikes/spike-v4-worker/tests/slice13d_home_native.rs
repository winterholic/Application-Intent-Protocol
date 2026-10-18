//! Slice13-D: real Mongo/Redis drivers, real Python worker and the real extension code for GET /home/category and
//! GET /home/personalized.
//!
//! EVIDENCE SCOPE
//! - real: MongoDriverCapability + RedisDriverCapability against dedicated loopback containers, the Python worker process,
//!   `extensions/slice13d_home.py`, request planning against the FaceReview definition.
//! - scripted: the relational backend (MySQL). Rows for User/UserFavoriteGenre/VideoViewLog/VideoBookmark come from a script,
//!   so this is NOT MySQL evidence and says nothing about the SQL it would run.
//! - oracle: `expected_personalized` in the golden file is the output of the ORIGINAL rank_personalized (pinned revision),
//!   produced by product/tests/slice13d_home_golden.py.
//! Tests named `gap_*` pin today's blocked behaviour of the shared core. They must start failing when the core dependency
//! is closed, which is the signal to replace them with positive tests.
//!
//!   AIP_TEST_DATABASE_URL / AIP_TEST_ADMIN_URL (build guard only), AIP_TEST_MONGO_URL (port 35441), AIP_TEST_REDIS_URL (port 35442)
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    backend::{BackendFuture, RelationalBackend},
    id_wire::IdWire,
    plan::{Caller, Plan, Reject},
    sqlgen::Dialect,
};
use spike_v4_worker::{
    invoke_with_host, CapabilityPrincipal, Isolation, Lang, MongoCapability, MongoDriverCapability, RedisCapability, RedisDriverCapability,
    RelationalWorkerHost, Worker,
};
use std::{collections::HashSet, sync::Arc, time::Duration};

const DEFINITION: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/facereview.aip");
const GOLDEN: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13d_home_golden.json");
const EXT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../product/tests/fixtures/facereview-mysql/extensions");
const NOW: &str = "2026-10-18T12:00:00Z";
const DB: &str = "aip_test_slice13d";
const PREFIX_KEYS: [&str; 2] = ["facereview:reco:pool", "facereview:reco:category"];

fn endpoint(name: &str, port: &str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"));
    let parsed = url::Url::parse(&value).expect("fixture URL");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"), "fixture must bind loopback only");
    assert_eq!(parsed.port().map(|p| p.to_string()).as_deref(), Some(port), "fixture must use its dedicated high port");
    value
}

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden file")
}

/// Rows for each exposed read, chosen by the resource the plan actually reads (policy subqueries add User to every dep list).
struct ScriptedBackend {
    user_id: Value,
    favorites: Vec<String>,
    viewed: Vec<String>,
    bookmarks: Vec<String>,
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
        Box::pin(async { Ok(NOW.into()) })
    }
    fn read<'a>(&'a self, plan: &'a Plan, _wire: IdWire) -> BackendFuture<'a, Result<Vec<Value>, Reject>> {
        let has = |name: &str| plan.deps.iter().any(|d| d == name);
        let rows: Vec<Value> = if has("VideoBookmark") {
            self.bookmarks.iter().map(|v| json!({"video": v})).collect()
        } else if has("VideoViewLog") {
            self.viewed.iter().map(|v| json!({"video": v})).collect()
        } else if has("UserFavoriteGenre") {
            self.favorites.iter().map(|g| json!({"genre": g})).collect()
        } else {
            vec![json!({"id": self.user_id})]
        };
        Box::pin(async move { Ok(rows) })
    }
}

fn facts() -> Value {
    let mut facts = load_str(DEFINITION, Form::A).expect("FaceReview definition").execution;
    // The product definition language cannot declare read./mongo./redis. grants yet and extension outputs cannot be lists
    // (both reported dependencies), so the facts are injected like Slice13-A's bridge test and the list is a JSON Text value.
    let ext = |implementation: &str, output: &str, access: &[&str]| {
        json!({
            "kind": "read", "effect": "none", "deadlineMs": 5000, "input": [], "output": [[output, "Text"]],
            "access": access.iter().map(|a| ((*a).to_string(), json!({}))).collect::<serde_json::Map<_, _>>(),
            "implementation": implementation,
        })
    };
    facts["resources"]["Video"]["extensions"] = json!({
        "homeCategory": ext("slice13d_home.category", "video_data", &["read.VideoBookmark", "redis.facereview:reco:*"]),
        "homePersonalized": ext(
            "slice13d_home.personalized", "videos",
            &["read.User", "read.UserFavoriteGenre", "read.VideoViewLog", "read.VideoBookmark", "mongo.youtube_watching_data", "redis.facereview:reco:*"],
        ),
    });
    facts
}

async fn mongo_fixture() -> (String, mongodb::Collection<mongodb::bson::Document>) {
    let uri = endpoint("AIP_TEST_MONGO_URL", "35441");
    let client = mongodb::Client::with_uri_str(&uri).await.expect("dedicated Mongo");
    (uri, client.database(DB).collection("youtube_watching_data"))
}

async fn redis_conn() -> (String, redis::aio::MultiplexedConnection) {
    let uri = endpoint("AIP_TEST_REDIS_URL", "35442");
    let client = redis::Client::open(uri.as_str()).expect("dedicated Redis");
    let con = client.get_multiplexed_async_connection().await.expect("redis connection");
    (uri, con)
}

async fn clear_redis(con: &mut redis::aio::MultiplexedConnection) {
    for key in PREFIX_KEYS {
        let _: () = redis::cmd("DEL").arg(key).query_async(con).await.expect("clear key");
    }
}

fn isolation() -> Isolation {
    #[cfg(target_os = "macos")]
    {
        Isolation::MacNetDeny
    }
    #[cfg(not(target_os = "macos"))]
    {
        Isolation::None
    }
}

async fn run(
    name: &str,
    backend: &ScriptedBackend,
    mongo: Option<Arc<dyn MongoCapability>>,
    redis: Option<Arc<dyn RedisCapability>>,
    actor: i64,
) -> Result<Value, Reject> {
    let mut worker = Worker::start_with(Lang::Python, EXT_DIR, isolation()).await;
    let mut host = RelationalWorkerHost::with_capabilities(backend, mongo, redis);
    let result =
        invoke_with_host(&mut host, &mut worker, &facts(), name, &json!({}), &Caller { actor_id: Some(actor), now: NOW.into() }, IdWire::Legacy).await;
    worker.stop().await;
    result
}

fn backend_from(g: &Value, user_id: Value) -> ScriptedBackend {
    let strings = |k: &str| g[k].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect::<Vec<_>>();
    ScriptedBackend { user_id, favorites: strings("favorites"), viewed: strings("viewed"), bookmarks: strings("bookmarks") }
}

async fn seed_pool(con: &mut redis::aio::MultiplexedConnection, g: &Value) {
    clear_redis(con).await;
    let _: () = redis::cmd("SET").arg("facereview:reco:pool").arg(g["pool"].to_string()).query_async(con).await.expect("seed pool");
}

async fn seed_categories(con: &mut redis::aio::MultiplexedConnection, g: &Value) {
    clear_redis(con).await;
    for (cat, entries) in g["categories"].as_object().unwrap() {
        let _: () = redis::cmd("HSET").arg("facereview:reco:category").arg(cat).arg(entries.to_string()).query_async(con).await.expect("seed hash");
    }
}

async fn drivers() -> (Arc<dyn MongoCapability>, Arc<dyn RedisCapability>) {
    let mongo = MongoDriverCapability::connect(&endpoint("AIP_TEST_MONGO_URL", "35441"), DB, 2, Duration::from_secs(2)).await.expect("mongo driver");
    let redis =
        RedisDriverCapability::connect(&endpoint("AIP_TEST_REDIS_URL", "35442"), vec!["facereview:reco:".into()], Duration::from_secs(2))
            .await
            .expect("redis driver");
    (Arc::new(mongo), Arc::new(redis))
}

fn principal(actor: Option<i64>) -> CapabilityPrincipal {
    CapabilityPrincipal { actor_id: actor, grants: ["mongo.youtube_watching_data".to_string()].into_iter().collect::<HashSet<_>>() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn category_reads_redis_hash_and_marks_scripted_bookmarks() {
    let g = golden();
    let (_, mut con) = redis_conn().await;
    seed_categories(&mut con, &g).await;
    let (mongo, redis) = drivers().await;
    let out = run("Video.homeCategory", &backend_from(&g, json!(1)), Some(mongo), Some(redis), 1).await.expect("category");
    clear_redis(&mut con).await;

    let decoded: Value = serde_json::from_str(out["video_data"].as_str().expect("JSON text envelope")).expect("envelope JSON");
    let groups = decoded.as_array().expect("video_data");
    assert_eq!(groups.len(), 3, "{out}");
    let bookmarks: HashSet<&str> = g["bookmarks"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let mut seen = 0;
    for group in groups {
        let want = &g["categories"][group["category_name"].as_str().unwrap()];
        let videos = group["videos"].as_array().unwrap();
        assert_eq!(videos.len(), want.as_array().unwrap().len());
        for (video, entry) in videos.iter().zip(want.as_array().unwrap()) {
            for key in ["video_id", "youtube_url", "title", "dominant_emotion", "dominant_emotion_per"] {
                assert_eq!(video[key], entry[key], "{key}");
            }
            assert_eq!(video["is_bookmarked"], bookmarks.contains(entry["video_id"].as_str().unwrap()));
            seen += 1;
        }
    }
    assert_eq!(seen, 15);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn category_absent_adapter_and_cache_miss_are_explicit_errors() {
    let g = golden();
    let (_, mut con) = redis_conn().await;
    clear_redis(&mut con).await;
    let backend = backend_from(&g, json!(1));
    let absent = run("Video.homeCategory", &backend, None, None, 1).await.unwrap_err();
    assert_eq!(absent.code, "WIRE.EXTENSION_ERROR");
    assert!(absent.msg.contains("WIRE.ADAPTER_UNAVAILABLE"), "{}", absent.msg);
    let (mongo, redis) = drivers().await;
    let miss = run("Video.homeCategory", &backend, Some(mongo), Some(redis), 1).await.unwrap_err();
    assert_eq!(miss.code, "WIRE.EXTENSION_ERROR");
    assert!(miss.msg.contains("SLICE13D.CATEGORY_CACHE_MISS"), "{}", miss.msg);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn personalized_matches_original_rank_when_principal_and_document_ids_have_the_same_type() {
    // The golden documents use user_id = 1 (the numeric actor id). With a UUID user id this path is blocked: see gap_*.
    let g = golden();
    let (_, mut con) = redis_conn().await;
    seed_pool(&mut con, &g).await;
    let (_, collection) = mongo_fixture().await;
    collection.delete_many(mongodb::bson::doc! {"user_id": 1_i64}).await.expect("clear");
    let docs: Vec<mongodb::bson::Document> =
        g["summaries"].as_array().unwrap().iter().map(|s| mongodb::bson::serialize_to_document(s).expect("doc")).collect();
    collection.insert_many(docs).await.expect("seed summaries");
    let (mongo, redis) = drivers().await;

    let out = run("Video.homePersonalized", &backend_from(&g, json!(1)), Some(mongo), Some(redis), 1).await.expect("personalized");
    collection.delete_many(mongodb::bson::doc! {"user_id": 1_i64}).await.expect("cleanup");
    clear_redis(&mut con).await;

    let decoded: Value = serde_json::from_str(out["videos"].as_str().expect("JSON text envelope")).expect("envelope JSON");
    let got: Vec<&str> = decoded.as_array().expect("videos").iter().map(|v| v["video_id"].as_str().unwrap()).collect();
    let want: Vec<&str> = g["expected_personalized"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(got, want, "ranking must equal the original rank_personalized output");
    let viewed: HashSet<&str> = g["viewed"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(got.iter().all(|id| !viewed.contains(id)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn personalized_pool_cache_miss_is_an_explicit_error_not_an_empty_list() {
    let g = golden();
    let (_, mut con) = redis_conn().await;
    clear_redis(&mut con).await;
    let (_, collection) = mongo_fixture().await;
    collection.delete_many(mongodb::bson::doc! {"user_id": 1_i64}).await.expect("clear");
    let (mongo, redis) = drivers().await;
    let err = run("Video.homePersonalized", &backend_from(&g, json!(1)), Some(mongo), Some(redis), 1).await.unwrap_err();
    assert_eq!(err.code, "WIRE.EXTENSION_ERROR");
    assert!(err.msg.contains("SLICE13D.POOL_CACHE_MISS"), "{}", err.msg);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gap_uuid_user_cannot_read_own_private_summaries() {
    // FaceReview user ids are UUID strings; the server compares the filter with the numeric AIP actor id.
    let g = golden();
    let (_, mut con) = redis_conn().await;
    seed_pool(&mut con, &g).await;
    let (mongo, redis) = drivers().await;
    let uuid = "0b9c1d22-4e5f-4a61-8c7d-1234567890ab";
    let err = run("Video.homePersonalized", &backend_from(&g, json!(uuid)), Some(mongo), Some(redis), 1).await.unwrap_err();
    clear_redis(&mut con).await;
    assert_eq!(err.code, "WIRE.EXTENSION_ERROR");
    assert!(err.msg.contains("WIRE.ACCESS_DENIED"), "{}", err.msg);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gap_driver_field_allowlist_and_read_only_limits() {
    let (_, collection) = mongo_fixture().await;
    collection.delete_many(mongodb::bson::doc! {"user_id": {"$in": [1_i64, 2_i64]}}).await.expect("clear");
    collection
        .insert_many([
            mongodb::bson::doc! {"user_id": 1_i64, "video_id": "a", "emotion_percentages": {"happy": 1.0}, "created_at": 2_i64},
            mongodb::bson::doc! {"user_id": 2_i64, "video_id": "b", "most_emotion_timeline": "x".repeat(1_100_000), "created_at": 1_i64},
        ])
        .await
        .expect("seed");
    let driver = MongoDriverCapability::connect(&endpoint("AIP_TEST_MONGO_URL", "35441"), DB, 2, Duration::from_secs(2)).await.expect("driver");

    let full = driver.call(&principal(Some(1)), "youtube_watching_data", &json!({"command":"find","filter":{}})).await.expect("unprojected read");
    assert_eq!(full["docs"][0]["emotion_percentages"]["happy"], 1.0, "unprojected documents do carry emotion_percentages");
    let inclusion = driver
        .call(&principal(Some(1)), "youtube_watching_data", &json!({"command":"find","filter":{},"projection":{"emotion_percentages":1}}))
        .await
        .unwrap_err();
    assert_eq!(inclusion.code, "WIRE.BAD_VALUE", "emotion_percentages cannot be projected: {}", inclusion.msg);
    let timeline_only = driver
        .call(&principal(Some(2)), "youtube_watching_data", &json!({"command":"find","filter":{},"projection":{"emotion_score_timeline":0}}))
        .await
        .unwrap_err();
    assert_eq!(timeline_only.code, "WIRE.BAD_VALUE", "most_emotion_timeline cannot be excluded, so the 1 MiB cap trips: {}", timeline_only.msg);
    let exclude_other = driver
        .call(&principal(Some(2)), "youtube_watching_data", &json!({"command":"find","filter":{},"projection":{"most_emotion_timeline":0}}))
        .await
        .unwrap_err();
    assert_eq!(exclude_other.code, "WIRE.BAD_VALUE", "{}", exclude_other.msg);
    collection.delete_many(mongodb::bson::doc! {"user_id": {"$in": [1_i64, 2_i64]}}).await.expect("cleanup");

    let redis = RedisDriverCapability::connect(&endpoint("AIP_TEST_REDIS_URL", "35442"), vec!["facereview:reco:".into()], Duration::from_secs(2))
        .await
        .expect("redis driver");
    let reco = CapabilityPrincipal { actor_id: Some(1), grants: ["redis.facereview:reco:*".to_string()].into_iter().collect() };
    for command in ["set", "setex", "hset", "del", "expire"] {
        let err = redis.call(&reco, "facereview:reco:pool", &json!({"command": command})).await.unwrap_err();
        assert_eq!(err.code, "WIRE.CAPABILITY_DENIED", "{command}: {}", err.msg);
    }
}
