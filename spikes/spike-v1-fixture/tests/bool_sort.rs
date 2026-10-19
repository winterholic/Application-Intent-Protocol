use spike_v1_fixture::{load_str, Form};

const SOURCE: &str = "actor principal resource Topic { fields { id: Id; important: Bool; archived: Bool? } \
    rows read when actor != null expose read { select id, important, archived; sort important, archived, id; \
    budget { rows 10; depth 1; deadline 1s; cost 100 } } }";

#[test]
fn boolean_fields_can_be_sorted_and_nonnull_boolean_keys_support_cursors() {
    let facts = load_str(SOURCE, Form::A).expect("boolean sort").execution;
    assert_eq!(facts["resources"]["Topic"]["exposeRead"]["sort"], serde_json::json!(["archived", "id", "important"]));
    let cursor = SOURCE.replace("sort important, archived, id", "sort important, id").replace("cost 100", "cost 100; cursor");
    assert!(load_str(&cursor, Form::A).is_ok());
    let nullable_cursor = SOURCE.replace("cost 100", "cost 100; cursor");
    let errors = load_str(&nullable_cursor, Form::A).err().expect("nullable cursor must fail");
    assert!(errors.iter().any(|e| e.code == "DEF.CURSOR_NULLABLE_SORT"));
}

#[test]
fn boolean_sort_keeps_field_policy_and_json_restrictions() {
    let guarded = SOURCE.replace("rows read", "field important read when actor != null rows read");
    let errors = load_str(&guarded, Form::A).err().expect("guarded sort must fail");
    assert!(errors.iter().any(|e| e.code == "DEF.POLICY_FIELD_NOT_FILTERABLE"));
    let errors = load_str(&SOURCE.replace("important: Bool", "important: Json"), Form::A).err().expect("Json sort must fail");
    assert!(errors.iter().any(|e| e.code == "DEF.OP_TYPE_MISMATCH"));
}

#[test]
fn bounded_child_traversals_can_sort_boolean_fields() {
    let source = "actor principal resource Forum { fields { id: Id } rows read when true \
        expose read { select id; traverse topics via Topic.forum { select id, important; sort important desc; limit 3 } \
        budget { rows 10; depth 2; deadline 1s; cost 100 } } } \
        resource Topic { fields { id: Id; forum: Forum; important: Bool } rows read when true expose read { select id, important } }";
    assert!(load_str(source, Form::A).is_ok());
}
