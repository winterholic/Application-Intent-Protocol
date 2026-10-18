use serde_json::json;
use spike_v2_read::sqlgen;

#[test]
fn shared_primary_key_reference_uses_the_resource_id_column() {
    let facts = json!({"resources": {"UserEmotionDna": {"fields": {"id": {"ty": "Ref<User>"}, "user": {"ty": "Ref<User>"}}}}});
    assert_eq!(sqlgen::column(&facts, "UserEmotionDna", "id").as_deref(), Some("id"));
    assert_eq!(sqlgen::column(&facts, "UserEmotionDna", "user").as_deref(), Some("user_id"));
}
