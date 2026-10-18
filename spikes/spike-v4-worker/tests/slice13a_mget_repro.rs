use serde_json::json;
use spike_v4_worker::capability::{ManagedRedisCapability, RedisCapability};

#[tokio::test]
async fn allowed_mget_command_is_implemented_and_returns_each_key() {
    let redis = ManagedRedisCapability::default();
    redis.seed("facereview:reco:pool", "pool-value");
    redis.seed("facereview:reco:category", "category-value");
    let got = redis.call(
        "facereview:reco:pool",
        &json!({"command":"mget", "keys":["facereview:reco:pool", "facereview:reco:category"]}),
    ).await.expect("default capability advertises mget and should serve it");
    assert_eq!(got, json!({"values":["pool-value", "category-value"]}));
}
