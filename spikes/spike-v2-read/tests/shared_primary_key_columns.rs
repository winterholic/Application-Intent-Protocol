use spike_v1_fixture::{load_str, Form};
use spike_v2_read::sqlgen;

#[test]
fn shared_primary_key_reference_uses_the_resource_id_column() {
    let source = r#"
        resource User { fields { id: Uuid } }
        actor User
        resource UserEmotionDna {
            fields { id: User; user: User }
            rows read when id = actor
            expose read { select id, user; budget { rows 1; depth 1; cost 100; deadline 1s } }
        }
    "#;
    let compiled = load_str(source, Form::A).expect("compiler accepts shared-primary-key resource");
    let facts = compiled.execution;
    assert_eq!(sqlgen::column(&facts, "UserEmotionDna", "id").as_deref(), Some("id"));
    assert_eq!(sqlgen::column(&facts, "UserEmotionDna", "user").as_deref(), Some("user_id"));
}
