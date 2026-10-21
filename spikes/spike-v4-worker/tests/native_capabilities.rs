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
