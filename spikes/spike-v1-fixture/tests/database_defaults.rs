use spike_v1_fixture::{load_str, Form};
#[test]
fn database_defaults_make_create_fields_optional_without_changing_read_nullability() {
    let source="actor principal\nresource Note {fields {id: Uuid; title: Text; createdAt: Time default database} rows read when false expose create {fields title allow true}}";
    let facts = load_str(source, Form::A).unwrap().execution;
    assert_eq!(facts["resources"]["Note"]["fields"]["createdAt"]["databaseDefault"], true);
    assert_eq!(facts["resources"]["Note"]["fields"]["createdAt"]["ty"], "Time");
}
