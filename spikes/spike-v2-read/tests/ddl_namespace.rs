use serde_json::json;

#[test]
fn deployment_namespace_does_not_mutate_request_namespace() {
    let facts = json!({"resources":{"Member":{"fields":{"id":{"ty":"Id<Member>"}},"invariants":{}}}});
    let before = spike_v2_read::sqlgen::schema().to_owned();
    let ddl = spike_v2_read::sqlgen::create_ddl_in("aip_deployed", &facts).unwrap();
    assert_eq!(ddl[0], "CREATE SCHEMA aip_deployed");
    assert!(ddl.iter().any(|sql| sql.starts_with("CREATE TABLE aip_deployed.member ")));
    assert!(ddl.iter().any(|sql| sql.starts_with("CREATE TABLE aip_deployed.aip_outbox ")));
    assert_eq!(spike_v2_read::sqlgen::schema(), before);
    assert!(spike_v2_read::sqlgen::create_ddl_in("x; DROP SCHEMA public", &facts).is_err());
}
