use serde_json::json;
use spike_v4_worker::{CapabilityPrincipal, MongoCapability, MongoDriverCapability, RedisCapability, RedisDriverCapability};

use std::{collections::HashSet, time::Duration};

fn endpoint(name: &str, expected_port: &str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is required; start tests/fixtures/native-stores/compose.yaml"));
    let parsed = url::Url::parse(&value).expect("fixture endpoint URL");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"), "fixture must bind loopback only");
    assert_eq!(parsed.port().map(|p| p.to_string()).as_deref(), Some(expected_port), "fixture must use its dedicated high port");
    value
}

fn principal(actor_id: Option<i64>, grants: &[&str]) -> CapabilityPrincipal {
    CapabilityPrincipal { actor_id, actor_uuid: None, grants: grants.iter().map(|v| (*v).to_string()).collect::<HashSet<_>>() }
}

#[tokio::test]
async fn mongo_driver_reads_fixture_and_enforces_principal_and_input_bounds() {
    let uri = endpoint("AIP_TEST_MONGO_URL", "15441");
    let database = "aip_test_slice13a";
    let direct = mongodb::Client::with_uri_str(&uri).await.expect("dedicated Mongo fixture");
    let collection = direct.database(database).collection::<mongodb::bson::Document>("youtube_watching_data");
    collection
        .delete_many(mongodb::bson::doc! {"user_id":{"$in":[7_i64,8_i64]}, "video_id":{"$in":[1_i64,2_i64,3_i64,4_i64]}})
        .await
        .expect("clear dedicated fixture rows");
    collection
        .insert_many([
            mongodb::bson::doc! {"user_id":7_i64, "video_id":1_i64},
            mongodb::bson::doc! {"user_id":8_i64, "video_id":2_i64},
            mongodb::bson::doc! {"user_id":7_i64, "video_id":4_i64, "emotion_percentages":{"happy":0.75_f64,"sad":0.25_f64}, "client_info":"private"},
            mongodb::bson::doc! {"user_id":7_i64, "video_id":3_i64, "emotion_score_timeline":"x".repeat(1_100_000)},
        ])
        .await
        .expect("seed dedicated fixture rows");
    let driver = MongoDriverCapability::connect(&uri, database, 2, Duration::from_secs(2)).await.expect("driver readiness");
    let owner = principal(Some(7), &["mongo.youtube_watching_data"]);
    let result = driver
        .call(
            &owner,
            "youtube_watching_data",
            &json!({
                "command":"find", "filter":{"video_id":1}, "projection":{"user_id":1,"video_id":1}
            }),
        )
        .await
        .expect("owner read");
    let rows = result["docs"].as_array().expect("driver response");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["user_id"], 7);
    assert_eq!(rows[0]["video_id"], 1);
    // The recommendation profile reads only the per-session emotion shares; fields outside the allowlist stay unreachable.
    let shares = driver
        .call(&owner, "youtube_watching_data", &json!({"command":"find","filter":{"video_id":4},"projection":{"video_id":1,"emotion_percentages":1}}))
        .await
        .expect("emotion percentages are projectable");
    assert_eq!(shares["docs"][0]["emotion_percentages"], json!({"happy": 0.75, "sad": 0.25}));
    assert!(shares["docs"][0].get("client_info").is_none());
    assert_eq!(
        driver
            .call(&owner, "youtube_watching_data", &json!({"command":"find","filter":{"video_id":4},"projection":{"client_info":1}}))
            .await
            .unwrap_err()
            .code,
        "WIRE.BAD_VALUE"
    );
    assert_eq!(
        driver.call(&owner, "youtube_watching_data", &json!({"command":"find","filter":{"user_id":8}})).await.unwrap_err().code,
        "WIRE.ACCESS_DENIED"
    );
    assert_eq!(
        driver
            .call(&principal(None, &["mongo.youtube_watching_data"]), "youtube_watching_data", &json!({"command":"find","filter":{}}))
            .await
            .unwrap_err()
            .code,
        "WIRE.ACCESS_DENIED"
    );
    assert_eq!(
        driver.call(&owner, "youtube_watching_data", &json!({"command":"find","filter":{"$where":"true"}})).await.unwrap_err().code,
        "WIRE.BAD_VALUE"
    );
    assert_eq!(
        driver.call(&owner, "youtube_watching_data", &json!({"command":"find","filter":{},"limit":-1})).await.unwrap_err().code,
        "WIRE.BAD_VALUE"
    );
    assert_eq!(
        driver.call(&owner, "youtube_watching_data", &json!({"command":"find","filter":{},"projection":{"video_id":2}})).await.unwrap_err().code,
        "WIRE.BAD_VALUE"
    );
    assert_eq!(
        driver.call(&owner, "youtube_watching_data", &json!({"command":"find","filter":{},"sort":{"video_id":"desc"}})).await.unwrap_err().code,
        "WIRE.BAD_VALUE"
    );
    let oversized = driver
        .call(&owner, "youtube_watching_data", &json!({"command":"find","filter":{"video_id":3},"projection":{"emotion_score_timeline":1}}))
        .await
        .unwrap_err();
    assert_eq!(oversized.code, "WIRE.BAD_VALUE");
    assert_eq!(
        driver.call(&owner, "youtube_watching_data", &json!({"command":"find","filter":{},"limit":101})).await.unwrap_err().code,
        "WIRE.BAD_VALUE"
    );
    assert_eq!(
        driver
            .call(&principal(Some(7), &["mongo.youtube_watching_data", "mongo.admin"]), "admin", &json!({"command":"find","filter":{}}))
            .await
            .unwrap_err()
            .code,
        "WIRE.CAPABILITY_DENIED"
    );
    collection
        .delete_many(mongodb::bson::doc! {"user_id":{"$in":[7_i64,8_i64]}, "video_id":{"$in":[1_i64,2_i64,3_i64,4_i64]}})
        .await
        .expect("clear dedicated fixture rows");
}

#[tokio::test]
async fn timeline_collection_aggregates_all_viewers_without_exposing_sessions() {
    let uri = endpoint("AIP_TEST_MONGO_URL", "15441");
    let database = "aip_test_slice13a";
    let direct = mongodb::Client::with_uri_str(&uri).await.expect("dedicated Mongo fixture");
    let collection = direct.database(database).collection::<mongodb::bson::Document>("youtube_watching_data");
    let video = "timeline-slice-7-2-5";
    let other = "timeline-slice-7-2-5-other";
    collection.delete_many(mongodb::bson::doc! {"video_id":{"$in":[video, other]}}).await.expect("clear dedicated fixture rows");
    collection
        .insert_many([
            // two viewers share key "0"; viewer 7 also has "5"; one malformed array and one non-object timeline are skipped
            mongodb::bson::doc! {"user_id":7_i64, "video_id":video, "emotion_score_timeline":{"0":[10.0_f64,20.0,0.0,0.0,70.0],"5":[50_i32,0,0,0,50], "9":[1.0_f64,2.0]}},
            mongodb::bson::doc! {"user_id":8_i64, "video_id":video, "emotion_score_timeline":{"0":[30.0_f64,0.0,0.0,0.0,70.0]}},
            mongodb::bson::doc! {"user_id":9_i64, "video_id":video, "emotion_score_timeline":"broken"},
            mongodb::bson::doc! {"user_id":9_i64, "video_id":video},
            mongodb::bson::doc! {"user_id":8_i64, "video_id":other, "emotion_score_timeline":{"0":[99.0_f64,0.0,0.0,0.0,1.0]}},
        ])
        .await
        .expect("seed dedicated fixture rows");
    let driver = MongoDriverCapability::connect(&uri, database, 2, Duration::from_secs(2)).await.expect("driver readiness");
    // The caller (user 11) owns none of these sessions: the aggregate must still answer, and must not be reachable without its grant.
    let reader = principal(Some(11), &["mongo.video_emotion_timeline"]);
    let answer = driver
        .call(&reader, "video_emotion_timeline", &json!({"command":"timeline","video_id":video,"duration":1}))
        .await
        .expect("aggregate over every viewer");
    let timeline = &answer["timeline"];
    // key "0": neutral (10 + 30) / 2 = 20, angry 70; key "5": neutral 50, angry 50. Duration 1 s: bucket width 1 cs.
    assert_eq!(timeline["neutral"][0], json!({"x":1,"y":20.0}));
    assert_eq!(timeline["angry"][0], json!({"x":1,"y":70.0}));
    assert_eq!(timeline["happy"][0], json!({"x":1,"y":10.0}));
    assert_eq!(timeline["neutral"][5], json!({"x":6,"y":50.0}));
    assert_eq!(timeline["neutral"][9], json!({"x":10,"y":0.0}), "the two-element array is not a usable key");
    let serialized = answer.to_string();
    for private in ["user_id", "video_view_log_id", "client_info"] {
        assert!(!serialized.contains(private), "aggregate must not carry `{private}`");
    }
    let empty = driver
        .call(&reader, "video_emotion_timeline", &json!({"command":"timeline","video_id":"timeline-slice-7-2-5-none"}))
        .await
        .expect("no sessions");
    assert!(empty["timeline"].is_null(), "no usable key means the caller falls back to the default timeline");
    // Without the grant, with raw-document commands, or with extra inputs the aggregate stays closed.
    assert_eq!(
        driver
            .call(&principal(Some(11), &["mongo.youtube_watching_data"]), "video_emotion_timeline", &json!({"command":"timeline","video_id":video}))
            .await
            .unwrap_err()
            .code,
        "WIRE.ACCESS_NOT_DECLARED"
    );
    for bad in [
        json!({"command":"find","filter":{}}),
        json!({"command":"timeline"}),
        json!({"command":"timeline","video_id":video,"filter":{"user_id":7}}),
        json!({"command":"timeline","video_id":video,"duration":-1}),
    ] {
        assert_eq!(driver.call(&reader, "video_emotion_timeline", &bad).await.unwrap_err().code, "WIRE.BAD_VALUE", "{bad}");
    }
    // The grant to the aggregate does not open the raw collection.
    assert_eq!(
        driver.call(&reader, "youtube_watching_data", &json!({"command":"find","filter":{}})).await.unwrap_err().code,
        "WIRE.ACCESS_NOT_DECLARED"
    );
    collection.delete_many(mongodb::bson::doc! {"video_id":{"$in":[video, other]}}).await.expect("clear dedicated fixture rows");
}

#[tokio::test]
async fn redis_driver_reads_every_mget_key_and_rejects_mixed_scope() {
    let uri = endpoint("AIP_TEST_REDIS_URL", "15442");
    let client = redis::Client::open(uri.as_str()).expect("dedicated Redis fixture");
    let mut con = client.get_multiplexed_async_connection().await.expect("Redis fixture connection");
    redis::cmd("SET").arg("facereview:reco:slice13a:first").arg("one").exec_async(&mut con).await.expect("seed string");
    redis::cmd("SET").arg("facereview:reco:slice13a:second").arg("two").exec_async(&mut con).await.expect("seed string");
    redis::cmd("HSET").arg("facereview:reco:slice13a:hash").arg("field").arg("value").exec_async(&mut con).await.expect("seed hash");
    redis::cmd("SET")
        .arg("facereview:reco:slice13a:oversize")
        .arg("x".repeat(1_048_577))
        .exec_async(&mut con)
        .await
        .expect("seed bounded response case");
    let driver = RedisDriverCapability::connect(&uri, vec!["facereview:reco:".into()], Duration::from_secs(2)).await.expect("driver readiness");
    let owner = principal(Some(7), &["redis.facereview:reco:*"]);
    let result = driver
        .call(
            &owner,
            "facereview:reco:slice13a:first",
            &json!({
                "command":"mget", "keys":["facereview:reco:slice13a:first", "facereview:reco:slice13a:second"]
            }),
        )
        .await
        .expect("MGET from Redis");
    assert_eq!(result["values"], json!(["one", "two"]));
    let hash = driver.call(&owner, "facereview:reco:slice13a:hash", &json!({"command":"hgetall"})).await.expect("HGETALL from Redis");
    assert_eq!(hash["value"]["field"], "value");
    let exists = driver.call(&owner, "facereview:reco:slice13a:first", &json!({"command":"exists"})).await.expect("EXISTS from Redis");
    assert_eq!(exists["exists"], true);
    let oversized = driver.call(&owner, "facereview:reco:slice13a:oversize", &json!({"command":"get"})).await.unwrap_err();
    assert_eq!(oversized.code, "WIRE.BAD_VALUE");
    let mixed_scope = principal(Some(7), &["redis.facereview:reco:*", "redis.facereview:session:*"]);
    assert_eq!(
        driver
            .call(
                &mixed_scope,
                "facereview:reco:slice13a:first",
                &json!({
                    "command":"mget", "keys":["facereview:reco:slice13a:first", "facereview:session:other"]
                })
            )
            .await
            .unwrap_err()
            .code,
        "WIRE.CAPABILITY_DENIED"
    );
    for key in
        ["facereview:reco:slice13a:first", "facereview:reco:slice13a:second", "facereview:reco:slice13a:hash", "facereview:reco:slice13a:oversize"]
    {
        let _: () = redis::cmd("DEL").arg(key).query_async(&mut con).await.expect("clear dedicated fixture key");
    }
}

#[tokio::test]
async fn missing_driver_endpoints_fail_instead_of_returning_empty_data() {
    let mongo = MongoDriverCapability::connect("mongodb://127.0.0.1:0/aip_test_slice13a", "aip_test_slice13a", 1, Duration::from_millis(100)).await;
    assert_eq!(mongo.err().unwrap().code, "WIRE.ADAPTER_UNAVAILABLE");
    let redis = RedisDriverCapability::connect("redis://127.0.0.1:0/", vec!["facereview:reco:".into()], Duration::from_millis(100)).await;
    assert_eq!(redis.err().unwrap().code, "WIRE.ADAPTER_UNAVAILABLE");
}

const WRITE_DATABASE: &str = "aip_test_slice725";

async fn write_driver(collections: &[&str]) -> (MongoDriverCapability, mongodb::Database) {
    let uri = endpoint("AIP_TEST_MONGO_URL", "15441");
    let direct = mongodb::Client::with_uri_str(&uri).await.expect("dedicated Mongo fixture");
    let database = direct.database(WRITE_DATABASE);
    database.drop().await.expect("drop the run-owned database");
    // Index creation runs against the shared CI fixture while the full native-store
    // integration suite is active; give this setup operation room to finish on a
    // loaded runner without changing the driver's production timeout policy.
    let driver = MongoDriverCapability::connect(&uri, WRITE_DATABASE, 2, Duration::from_secs(10))
        .await
        .expect("driver readiness")
        .enable_writes(collections)
        .expect("writes");
    driver.ensure_write_indexes().await.expect("indexes");
    (driver, database)
}

const SESSION: &str = "youtube_watching_data";

#[tokio::test]
async fn mongo_writes_are_bounded_private_and_undoable() {
    let (driver, database) = write_driver(&[SESSION, "video_distribution"]).await;
    let sessions = database.collection::<mongodb::bson::Document>(SESSION);
    let alice = principal(Some(7), &["mongo.youtube_watching_data", "mongo.video_distribution"]);
    let bob = principal(Some(8), &["mongo.youtube_watching_data"]);
    let read_only = principal(Some(7), &["mongo.youtube_watching_data"]);

    // The original's first frame: upsert with a centisecond key, $inc sums and $setOnInsert defaults, read back after the write.
    let frame = |scores: [f64; 5]| {
        json!({
            "command": "find_one_and_update", "upsert": true,
            "filter": {"video_view_log_id": "log-1"},
            "update": {
                "$set": {"emotion_score_timeline.2029": scores, "most_emotion_timeline.2029": "happy", "updated_at": {"$date": "2026-10-22T00:00:00Z"}},
                "$setOnInsert": {"video_id": "v1", "created_at": {"$date": "2026-10-22T00:00:00Z"}, "completion_rate": 0.0},
                "$inc": {"frame_count": 1, "emotion_sum.neutral": scores[0], "emotion_sum.happy": scores[1]},
            },
        })
    };
    let first = driver.write(&alice, SESSION, &frame([10.0, 90.0, 0.0, 0.0, 0.0])).await.expect("first frame");
    assert_eq!(first.value["doc"]["frame_count"], 1);
    assert_eq!(first.value["doc"]["user_id"], 7, "the principal owns the new session");
    assert_eq!(first.value["doc"]["emotion_score_timeline"]["2029"], json!([10.0, 90.0, 0.0, 0.0, 0.0]));
    assert!(first.compensation.is_none(), "no compensation unless asked for");
    // The same frame replayed counts twice: the dedupe is the caller's (Redis SET NX), not the store's.
    let again = driver.write(&alice, SESSION, &frame([10.0, 90.0, 0.0, 0.0, 0.0])).await.expect("replay");
    assert_eq!(again.value["doc"]["frame_count"], 2);
    let stored = sessions.find_one(mongodb::bson::doc! {"video_view_log_id": "log-1"}).await.unwrap().expect("stored");
    assert!(matches!(stored.get("created_at"), Some(mongodb::bson::Bson::DateTime(_))), "$date arrives as a BSON date: {stored:?}");
    assert!(matches!(stored.get("frame_count"), Some(mongodb::bson::Bson::Int32(2))));

    // Another member cannot touch that session: the filter is forced to the caller, so it matches nothing and upserts their own row.
    let foreign = driver
        .write(
            &bob,
            SESSION,
            &json!({"command":"update_one","filter":{"video_view_log_id":"log-1","user_id":7},"update":{"$set":{"completion_rate":1.0}}}),
        )
        .await;
    assert_eq!(foreign.unwrap_err().code, "WIRE.ACCESS_DENIED");
    let blocked = driver
        .write(&bob, SESSION, &json!({"command":"update_one","filter":{"video_view_log_id":"log-1"},"update":{"$set":{"completion_rate":1.0}}}))
        .await
        .expect("own filter");
    assert_eq!(blocked.value["matched"], 0, "bob's forced user_id filter does not match alice's session");
    assert_eq!(
        sessions.find_one(mongodb::bson::doc! {"video_view_log_id": "log-1"}).await.unwrap().unwrap().get_f64("completion_rate").unwrap(),
        0.0
    );
    let spoof = driver.write(&bob, SESSION, &json!({"command":"insert_one","document":{"video_view_log_id":"x","user_id":7}})).await;
    assert_eq!(spoof.unwrap_err().code, "WIRE.ACCESS_DENIED");

    // Shapes the extension may not send.
    for (label, input) in [
        ("operator in filter", json!({"command":"update_one","filter":{"video_view_log_id":{"$ne":"x"}},"update":{"$set":{"completion_rate":1}}})),
        ("unknown update operator", json!({"command":"update_one","filter":{"video_view_log_id":"log-1"},"update":{"$unset":{"client_info":1}}})),
        ("field outside the model", json!({"command":"update_one","filter":{"video_view_log_id":"log-1"},"update":{"$set":{"admin":true}}})),
        ("deep path", json!({"command":"update_one","filter":{"video_view_log_id":"log-1"},"update":{"$set":{"emotion_sum.a.b.c.d":1}}})),
        ("$inc of text", json!({"command":"update_one","filter":{"video_view_log_id":"log-1"},"update":{"$inc":{"frame_count":"1"}}})),
        ("operator key in a value", json!({"command":"insert_one","document":{"video_view_log_id":"y","client_info":{"$where":"1"}}})),
        ("empty filter", json!({"command":"delete_one","filter":{}})),
        ("extra input", json!({"command":"delete_one","filter":{"video_view_log_id":"log-1"},"limit":5})),
        ("document too large", json!({"command":"insert_one","document":{"video_view_log_id":"big","client_info":{"x":"y".repeat(1_100_000)}}})),
        ("compensate not boolean", json!({"command":"delete_one","filter":{"video_view_log_id":"log-1"},"compensate":"yes"})),
    ] {
        assert_eq!(driver.write(&alice, SESSION, &input).await.err().map(|e| e.code), Some("WIRE.BAD_VALUE"), "{label}");
    }
    assert_eq!(driver.write(&alice, SESSION, &json!({"command":"drop"})).await.err().map(|e| e.code), Some("WIRE.CAPABILITY_DENIED"));
    assert_eq!(
        driver.write(&alice, "saga_transaction_log", &json!({"command":"delete_one","filter":{"transaction_id":"t"}})).await.err().map(|e| e.code),
        Some("WIRE.ACCESS_NOT_DECLARED"),
        "the journal collection has no grant"
    );
    assert_eq!(
        driver
            .write(
                &principal(Some(7), &["mongo.saga_transaction_log"]),
                "saga_transaction_log",
                &json!({"command":"delete_one","filter":{"transaction_id":"t"}})
            )
            .await
            .err()
            .map(|e| e.code),
        Some("WIRE.CAPABILITY_DENIED"),
        "even a forged grant does not open the journal collection"
    );
    assert_eq!(
        driver
            .write(&principal(Some(7), &[]), SESSION, &json!({"command":"delete_one","filter":{"video_view_log_id":"log-1"}}))
            .await
            .err()
            .map(|e| e.code),
        Some("WIRE.ACCESS_NOT_DECLARED")
    );
    assert_eq!(
        driver
            .write(
                &principal(None, &["mongo.youtube_watching_data"]),
                SESSION,
                &json!({"command":"delete_one","filter":{"video_view_log_id":"log-1"}})
            )
            .await
            .err()
            .map(|e| e.code),
        Some("WIRE.ACCESS_DENIED"),
        "a guest owns no private session"
    );

    // The unique index the original creates makes a second insert of one session a conflict.
    driver.write(&alice, SESSION, &json!({"command":"insert_one","document":{"video_view_log_id":"log-2","video_id":"v1"}})).await.expect("insert");
    let duplicate = driver.write(&alice, SESSION, &json!({"command":"insert_one","document":{"video_view_log_id":"log-2","video_id":"v1"}})).await;
    assert_eq!(duplicate.err().map(|e| e.code), Some("WIRE.CONFLICT"));

    // Compensation: each way back restores the state before the call, and replaying it is harmless.
    let count = |filter: mongodb::bson::Document| {
        let sessions = sessions.clone();
        async move { sessions.count_documents(filter).await.unwrap() }
    };
    let inserted = driver
        .write(&alice, SESSION, &json!({"command":"insert_one","document":{"video_view_log_id":"log-3","video_id":"v3"},"compensate":true}))
        .await
        .expect("insert with a way back");
    assert_eq!(count(mongodb::bson::doc! {"video_view_log_id":"log-3"}).await, 1);
    let undo = inserted.compensation.expect("insert is undone by deleting it");
    driver.compensate(SESSION, &undo).await.expect("undo insert");
    driver.compensate(SESSION, &undo).await.expect("undo insert twice");
    assert_eq!(count(mongodb::bson::doc! {"video_view_log_id":"log-3"}).await, 0);

    let fresh = driver
        .write(&alice, SESSION, &json!({"command":"update_one","upsert":true,"compensate":true,"filter":{"video_view_log_id":"log-4"},"update":{"$set":{"video_id":"v4"}}}))
        .await
        .expect("upsert creating a row");
    assert_eq!(count(mongodb::bson::doc! {"video_view_log_id":"log-4"}).await, 1);
    driver.compensate(SESSION, &fresh.compensation.expect("created row is deleted")).await.expect("undo upsert");
    assert_eq!(count(mongodb::bson::doc! {"video_view_log_id":"log-4"}).await, 0);

    let before = sessions.find_one(mongodb::bson::doc! {"video_view_log_id":"log-1"}).await.unwrap().unwrap();
    let changed = driver
        .write(
            &alice,
            SESSION,
            &json!({"command":"update_one","compensate":true,"filter":{"video_view_log_id":"log-1"},"update":{"$set":{"dominant_emotion":"sad"},"$inc":{"frame_count":5}}}),
        )
        .await
        .expect("update an existing row");
    assert_eq!(sessions.find_one(mongodb::bson::doc! {"video_view_log_id":"log-1"}).await.unwrap().unwrap().get_i32("frame_count").unwrap(), 7);
    driver.compensate(SESSION, &changed.compensation.expect("existing row is restored")).await.expect("undo update");
    let after = sessions.find_one(mongodb::bson::doc! {"video_view_log_id":"log-1"}).await.unwrap().unwrap();
    assert_eq!(after, before, "the document is exactly as before, including the date and Int32 types");

    let removed = driver
        .write(&alice, SESSION, &json!({"command":"delete_one","compensate":true,"filter":{"video_view_log_id":"log-1"}}))
        .await
        .expect("delete with a way back");
    assert_eq!(removed.value["deleted"], 1);
    assert_eq!(count(mongodb::bson::doc! {"video_view_log_id":"log-1"}).await, 0);
    let restore = removed.compensation.expect("deleted row is inserted back");
    driver.compensate(SESSION, &restore).await.expect("undo delete");
    driver.compensate(SESSION, &restore).await.expect("undo delete twice");
    assert_eq!(sessions.find_one(mongodb::bson::doc! {"video_view_log_id":"log-1"}).await.unwrap().unwrap(), before);

    // A collection that was not opened for writes, and a driver without writes, refuse.
    let (narrow, _) = write_driver(&["video_distribution"]).await;
    assert_eq!(
        narrow.write(&read_only, SESSION, &json!({"command":"delete_one","filter":{"video_view_log_id":"log-1"}})).await.err().map(|e| e.code),
        Some("WIRE.CAPABILITY_DENIED")
    );
    assert!(MongoDriverCapability::connect(&endpoint("AIP_TEST_MONGO_URL", "15441"), WRITE_DATABASE, 1, Duration::from_secs(2))
        .await
        .unwrap()
        .enable_writes(&["admin"])
        .is_err());

    // The journal keeps one document per transaction and rewrites it as the saga advances.
    driver
        .journal(&json!({"transaction_id":"t-1","status":"in_progress","steps":[],"created_at":{"$date":"2026-10-22T00:00:00Z"}}))
        .await
        .expect("journal");
    driver
        .journal(&json!({"transaction_id":"t-1","status":"completed","steps":[{"name":"a"}],"created_at":{"$date":"2026-10-22T00:00:00Z"}}))
        .await
        .expect("journal again");
    let log = database.collection::<mongodb::bson::Document>("saga_transaction_log");
    assert_eq!(log.count_documents(mongodb::bson::doc! {"transaction_id":"t-1"}).await.unwrap(), 1);
    assert_eq!(log.find_one(mongodb::bson::doc! {"transaction_id":"t-1"}).await.unwrap().unwrap().get_str("status").unwrap(), "completed");
    database.drop().await.expect("drop the run-owned database");
}

async fn pttl_of(connection: &redis::aio::MultiplexedConnection, key: &str) -> i64 {
    let mut connection = connection.clone();
    redis::cmd("PTTL").arg(key).query_async::<i64>(&mut connection).await.unwrap()
}

#[tokio::test]
async fn redis_writes_expire_dedupe_and_restore() {
    let uri = endpoint("AIP_TEST_REDIS_URL", "15442");
    let client = redis::Client::open(uri.as_str()).expect("dedicated Redis fixture");
    let mut con = client.get_multiplexed_async_connection().await.expect("Redis fixture connection");
    let driver = RedisDriverCapability::connect(
        &uri,
        vec!["facereview:blacklist:".into(), "facereview:dedupe:".into(), "facereview:metrics:".into()],
        Duration::from_secs(2),
    )
    .await
    .expect("driver readiness");
    let grants = principal(Some(7), &["redis.facereview:blacklist:*", "redis.facereview:dedupe:*", "redis.facereview:metrics:requests:1h"]);
    let key = "facereview:blacklist:t-725";
    let probe = con.clone();
    let pttl = |key: &'static str| {
        let mut con = probe.clone();
        async move { redis::cmd("PTTL").arg(key).query_async::<i64>(&mut con).await.unwrap() }
    };
    let _: () = redis::cmd("DEL").arg(key).query_async(&mut con).await.unwrap();

    // TTL: a refresh-token blacklist entry lives for its token's remaining life and then disappears.
    driver.write(&grants, key, &json!({"command":"set_ex","value":"1","ttl_seconds":1})).await.expect("blacklist");
    assert_eq!(driver.call(&grants, key, &json!({"command":"get"})).await.unwrap()["value"], "1");
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert_eq!(driver.call(&grants, key, &json!({"command":"exists"})).await.unwrap()["exists"], false, "the entry expired");

    // set_nx_ex is the original's frame dedupe: the first caller wins, the replay is refused, the entry then expires.
    let dedupe = "facereview:dedupe:log-1:2029";
    let _: () = redis::cmd("DEL").arg(dedupe).query_async(&mut con).await.unwrap();
    let won = driver.write(&grants, dedupe, &json!({"command":"set_nx_ex","value":"1","ttl_seconds":1,"compensate":true})).await.unwrap();
    assert_eq!(won.value["set"], true);
    let lost = driver.write(&grants, dedupe, &json!({"command":"set_nx_ex","value":"1","ttl_seconds":1,"compensate":true})).await.unwrap();
    assert_eq!(lost.value["set"], false);
    assert!(lost.compensation.is_none(), "a refused dedupe changed nothing, so it has nothing to undo");
    driver.compensate(dedupe, &won.compensation.expect("the winner can be undone")).await.expect("undo dedupe");
    assert_eq!(driver.call(&grants, dedupe, &json!({"command":"exists"})).await.unwrap()["exists"], false);
    let rewon = driver.write(&grants, dedupe, &json!({"command":"set_nx_ex","value":"1","ttl_seconds":1})).await.unwrap();
    assert_eq!(rewon.value["set"], true, "after the undo the same frame can be recorded again");
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert_eq!(
        driver.write(&grants, dedupe, &json!({"command":"set_nx_ex","value":"1","ttl_seconds":5})).await.unwrap().value["set"],
        true,
        "expired entry no longer blocks"
    );

    // del / getdel / set_ex with a way back restore the value and the remaining life.
    let _: () = redis::cmd("SET").arg(key).arg("code-1").arg("EX").arg(100).query_async(&mut con).await.unwrap();
    let deleted = driver.write(&grants, key, &json!({"command":"del","compensate":true})).await.unwrap();
    assert_eq!(deleted.value["deleted"], 1);
    assert_eq!(driver.call(&grants, key, &json!({"command":"exists"})).await.unwrap()["exists"], false);
    driver.compensate(key, &deleted.compensation.expect("del is undone")).await.expect("undo del");
    assert_eq!(driver.call(&grants, key, &json!({"command":"get"})).await.unwrap()["value"], "code-1");
    let left = pttl("facereview:blacklist:t-725").await;
    assert!((90_000..=100_000).contains(&left), "the remaining life came back, not a fresh or infinite one: {left}");
    let taken = driver.write(&grants, key, &json!({"command":"getdel","compensate":true})).await.unwrap();
    assert_eq!(taken.value["value"], "code-1", "the verification code is consumed once");
    assert_eq!(driver.write(&grants, key, &json!({"command":"getdel"})).await.unwrap().value["value"], serde_json::Value::Null);
    driver.compensate(key, &taken.compensation.unwrap()).await.expect("undo getdel");
    assert_eq!(driver.call(&grants, key, &json!({"command":"get"})).await.unwrap()["value"], "code-1");
    let replaced = driver.write(&grants, key, &json!({"command":"set_ex","value":"code-2","ttl_seconds":30,"compensate":true})).await.unwrap();
    driver.compensate(key, &replaced.compensation.unwrap()).await.expect("undo set_ex");
    assert_eq!(driver.call(&grants, key, &json!({"command":"get"})).await.unwrap()["value"], "code-1");

    // Metrics: a counter with its window and a bounded list, neither needing a way back.
    let counter = "facereview:metrics:requests:1h";
    let _: () = redis::cmd("DEL").arg(counter).query_async(&mut con).await.unwrap();
    assert_eq!(driver.write(&grants, counter, &json!({"command":"incr_expire","ttl_seconds":3600})).await.unwrap().value["value"], 1);
    assert_eq!(driver.write(&grants, counter, &json!({"command":"incr_expire","ttl_seconds":3600})).await.unwrap().value["value"], 2);
    assert!((3_500_000..=3_600_000).contains(&pttl("facereview:metrics:requests:1h").await));
    let list = "facereview:metrics:response_times";
    let _: () = redis::cmd("DEL").arg(list).query_async(&mut con).await.unwrap();
    let times = principal(Some(7), &["redis.facereview:metrics:*"]);
    for n in 0..5 {
        driver.write(&times, list, &json!({"command":"push_trim_expire","value":n.to_string(),"max_len":3,"ttl_seconds":3600})).await.unwrap();
    }
    let kept: Vec<String> = redis::cmd("LRANGE").arg(list).arg(0).arg(-1).query_async(&mut con).await.unwrap();
    assert_eq!(kept, ["4", "3", "2"], "the list keeps the newest three");

    // The recommendation pool cache of the original: a JSON text far larger than 64KB and a category hash replaced as a whole.
    let pool = "facereview:metrics:pool";
    let big = "x".repeat(900_000);
    driver.write(&times, pool, &json!({"command":"set_ex","value":big,"ttl_seconds":30})).await.expect("a 900KB value");
    assert_eq!(driver.call(&times, pool, &json!({"command":"get"})).await.unwrap()["value"].as_str().map(str::len), Some(900_000));
    let hash = "facereview:metrics:categories";
    driver.write(&times, hash, &json!({"command":"replace_hash_ex","fields":{"drama":"[1]","cook":"[2]"},"ttl_seconds":30})).await.unwrap();
    driver.write(&times, hash, &json!({"command":"replace_hash_ex","fields":{"vlog":"[3]"},"ttl_seconds":30})).await.unwrap();
    let categories = driver.call(&times, hash, &json!({"command":"hgetall"})).await.unwrap();
    assert_eq!(categories["value"], json!({"vlog":"[3]"}), "the old categories are gone, not merged");
    assert!((20_000..=30_000).contains(&pttl_of(&probe, hash).await));
    for (label, input) in [
        ("no fields", json!({"command":"replace_hash_ex","fields":{},"ttl_seconds":30})),
        ("non-text field", json!({"command":"replace_hash_ex","fields":{"a":1},"ttl_seconds":30})),
        ("no ttl", json!({"command":"replace_hash_ex","fields":{"a":"1"}})),
    ] {
        assert_eq!(driver.write(&times, hash, &input).await.err().map(|e| e.code), Some("WIRE.BAD_VALUE"), "{label}");
    }
    // lrange reads the metrics list the original's admin status reads, within a window.
    assert_eq!(driver.call(&times, list, &json!({"command":"lrange","start":0,"stop":-1})).await.unwrap()["values"], json!(["4", "3", "2"]));
    assert_eq!(driver.call(&times, list, &json!({"command":"lrange","start":1,"stop":1})).await.unwrap()["values"], json!(["3"]));
    assert_eq!(driver.call(&times, list, &json!({"command":"lrange","start":-1})).await.err().map(|e| e.code), Some("WIRE.BAD_VALUE"));
    assert_eq!(driver.call(&times, list, &json!({"command":"get","start":0})).await.err().map(|e| e.code), Some("WIRE.BAD_VALUE"));

    // Bounds and scope.
    for (label, input) in [
        ("zero ttl", json!({"command":"set_ex","value":"1","ttl_seconds":0})),
        ("ttl over 30 days", json!({"command":"set_ex","value":"1","ttl_seconds":2_592_001})),
        ("no ttl", json!({"command":"set_ex","value":"1"})),
        ("value over 1MB", json!({"command":"set_ex","value":"x".repeat(1_048_577),"ttl_seconds":5})),
        ("non-text value", json!({"command":"set_ex","value":1,"ttl_seconds":5})),
        ("extra field", json!({"command":"del","ttl_seconds":5})),
        ("list too long", json!({"command":"push_trim_expire","value":"1","max_len":10_001,"ttl_seconds":5})),
    ] {
        assert_eq!(driver.write(&grants, key, &input).await.err().map(|e| e.code), Some("WIRE.BAD_VALUE"), "{label}");
    }
    assert_eq!(driver.write(&grants, key, &json!({"command":"flushall"})).await.err().map(|e| e.code), Some("WIRE.CAPABILITY_DENIED"));
    assert_eq!(
        driver.write(&grants, "facereview:other:x", &json!({"command":"set_ex","value":"1","ttl_seconds":5})).await.err().map(|e| e.code),
        Some("WIRE.ACCESS_NOT_DECLARED"),
        "no grant for the key"
    );
    assert_eq!(
        driver
            .write(&principal(Some(7), &["redis.facereview:other:*"]), "facereview:other:x", &json!({"command":"set_ex","value":"1","ttl_seconds":5}))
            .await
            .err()
            .map(|e| e.code),
        Some("WIRE.CAPABILITY_DENIED"),
        "a grant outside the configured namespaces does not open it"
    );
    assert_eq!(
        driver.write(&principal(Some(7), &["redis.facereview:blacklist:t-1"]), key, &json!({"command":"del"})).await.err().map(|e| e.code),
        Some("WIRE.ACCESS_NOT_DECLARED"),
        "an exact grant covers only its own key"
    );
    for k in [key, dedupe, counter, list, pool, hash] {
        let _: () = redis::cmd("DEL").arg(k).query_async(&mut con).await.unwrap();
    }
}
