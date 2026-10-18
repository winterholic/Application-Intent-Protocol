use serde_json::json;
use spike_v4_worker::capability::{ManagedMongoCapability, MongoCapability, ManagedRedisCapability, RedisCapability};

#[tokio::test]
async fn default_managed_capabilities_report_empty_data_as_success() {
    let mongo = ManagedMongoCapability::default();
    let mongo_result = mongo.call("youtube_watching_data", &json!({"command":"find", "filter":{}})).await.unwrap();
    assert_eq!(mongo_result, json!({"docs":[]}));

    let redis = ManagedRedisCapability::default();
    let redis_result = redis.call("facereview:session:other-users-session:timeline", &json!({"command":"get"})).await.unwrap();
    assert_eq!(redis_result, json!({"value":null}));
}
