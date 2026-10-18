//! WBS 7.2.8: what the real `MongoDriverCapability` accepts and rejects for the emotion
//! summary and DNA reads of the FaceReview original (`mypage_service.get_emotion_summary`,
//! `get_emotion_dna`, `common/utils/emotion_summary.build_finalized_session_query`).
//!
//! The rejections are the platform gap, not test failures: if one starts to succeed the
//! gap report in product/tests/slice728_summary_oracle_test.py must be revisited.
//! Needs a run-owned Mongo on a dedicated loopback port: `AIP_TEST_MONGO_URL=mongodb://127.0.0.1:<port>`.
use serde_json::json;
use spike_v4_worker::{CapabilityPrincipal, MongoCapability, MongoDriverCapability};
use std::{collections::HashSet, time::Duration};

fn endpoint() -> String {
    let value = std::env::var("AIP_TEST_MONGO_URL").expect("AIP_TEST_MONGO_URL is required (run-owned loopback Mongo)");
    let parsed = url::Url::parse(&value).expect("fixture endpoint URL");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"), "fixture must bind loopback only");
    let port = parsed.port().expect("explicit port");
    assert!(port > 1024 && port != 27017, "fixture must use a dedicated high port");
    value
}

fn owner() -> CapabilityPrincipal {
    CapabilityPrincipal { actor_id: Some(7), grants: ["mongo.youtube_watching_data".to_string()].into_iter().collect::<HashSet<_>>() }
}

async fn read(driver: &MongoDriverCapability, input: serde_json::Value) -> Result<serde_json::Value, spike_v2_read::plan::Reject> {
    driver.call(&owner(), "youtube_watching_data", &input).await
}

async fn rejected(driver: &MongoDriverCapability, label: &str, input: serde_json::Value) -> (&'static str, String) {
    let error = read(driver, input).await.expect_err(label);
    println!("REJECTED {label}: {} {}", error.code, error.msg);
    (error.code, error.msg)
}

#[tokio::test]
async fn emotion_summary_and_dna_reads_the_driver_capability_cannot_express() {
    let uri = endpoint();
    let database = "aip_test_slice728_summary";
    let direct = mongodb::Client::with_uri_str(&uri).await.expect("dedicated Mongo fixture");
    let collection = direct.database(database).collection::<mongodb::bson::Document>("youtube_watching_data");
    collection.delete_many(mongodb::bson::doc! {"video_view_log_id":{"$in":["s728-a","s728-b","s728-c","s728-x"]}}).await.expect("clear own rows");
    let finalized = mongodb::bson::DateTime::from_millis(1_760_000_000_000);
    let session = |user: i64, id: &str| {
        mongodb::bson::doc! {
            "user_id": user, "video_id": 1_i64, "video_view_log_id": id, "finalized_at": finalized,
            "created_at": finalized, "emotion_percentages": {"happy": 0.5, "neutral": 0.5},
            "completion_rate": 0.9, "duration": 60_i64, "emotion_seconds": {"happy": 3_i64},
        }
    };
    collection.insert_many([session(7, "s728-a"), session(7, "s728-b"), session(7, "s728-c"), session(8, "s728-x")]).await.expect("seed own rows");
    let driver = MongoDriverCapability::connect(&uri, database, 2, Duration::from_secs(2)).await.expect("driver readiness");

    // Positive controls: the real driver is reached, the actor predicate is server-added.
    let counted = read(&driver, json!({"command":"count_documents","filter":{}})).await.expect("count own sessions");
    assert_eq!(counted["count"], 3, "{counted}");
    let ids = read(&driver, json!({"command":"find","filter":{},"projection":{"user_id":1,"video_id":1,"created_at":1}})).await.expect("allowed fields");
    assert_eq!(ids["docs"].as_array().map(Vec::len), Some(3), "{ids}");

    // first fold / DNA inputs sit outside the field allow-list
    for (label, projection) in [
        ("DNA/fold projection emotion_percentages+completion_rate", json!({"emotion_percentages":1,"completion_rate":1,"created_at":1,"video_id":1})),
        ("first-fold projection (original FIRST_FOLD)", json!({"most_emotion_timeline":1,"emotion_percentages":1,"duration":1,"completion_rate":1,"frame_count":1,"_id":0})),
        ("incremental exclusion projection (original SINCE)", json!({"emotion_score_timeline":0,"most_emotion_timeline":0,"client_info":0})),
        ("precomputed emotion_seconds projection", json!({"emotion_seconds":1})),
    ] {
        let (code, _) = rejected(&driver, label, json!({"command":"find","filter":{},"projection":projection})).await;
        assert_eq!(code, "WIRE.BAD_VALUE", "{label}");
    }

    // the watermark query: $or/$gt are not supported operators and finalized_at is not an allowed field
    let watermark = json!({"$or":[{"finalized_at":{"$gt":"2025-01-01T00:00:00Z"}},{"finalized_at":"2025-01-01T00:00:00Z","video_view_log_id":{"$gt":""}}]});
    let (code, msg) = rejected(&driver, "watermark $or/$gt filter (build_finalized_session_query)", json!({"command":"find","filter":watermark})).await;
    assert_eq!(code, "WIRE.BAD_VALUE");
    assert!(msg.contains("operator"), "{msg}");
    let (code, msg) = rejected(&driver, "filter on finalized_at", json!({"command":"find","filter":{"finalized_at":"x"}})).await;
    assert_eq!(code, "WIRE.BAD_VALUE");
    assert!(msg.contains("field"), "{msg}");
    let (code, _) = rejected(&driver, "$gt inside a plain field", json!({"command":"find","filter":{"created_at":{"$gt":"2025"}}})).await;
    assert_eq!(code, "WIRE.BAD_VALUE");
    let (code, _) = rejected(&driver, "cursor sort finalized_at, video_view_log_id", json!({"command":"find","filter":{},"sort":{"finalized_at":1,"video_view_log_id":1}})).await;
    assert_eq!(code, "WIRE.BAD_VALUE");

    // the first fold reads every session; one call returns at most 100 documents
    let (code, _) = rejected(&driver, "limit above 100", json!({"command":"find","filter":{},"limit":101})).await;
    assert_eq!(code, "WIRE.BAD_VALUE");

    // another actor's sessions stay unreachable even when the filter names them
    let error = read(&driver, json!({"command":"find","filter":{"user_id":8}})).await.expect_err("foreign actor");
    assert_eq!(error.code, "WIRE.ACCESS_DENIED");

    collection.delete_many(mongodb::bson::doc! {"video_view_log_id":{"$in":["s728-a","s728-b","s728-c","s728-x"]}}).await.expect("clear own rows");
}
