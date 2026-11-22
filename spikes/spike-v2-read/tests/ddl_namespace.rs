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

#[test]
fn postgres_ddl_quotes_reserved_field_names() {
    let words = ["grant", "order", "group", "limit", "check", "user", "table", "column", "default", "offset", "select", "desc", "from", "where"];
    let mut fields = serde_json::Map::new();
    fields.insert("name".into(), json!({"ty":"Text"}));
    fields.insert("owner".into(), json!({"ty":"Ref<Member>"}));
    for word in words {
        fields.insert(word.into(), json!({"ty":"Text"}));
    }
    let facts = json!({"resources":{
        "Member":{"fields":{"id":{"ty":"Id<Member>"}},"invariants":{}},
        "Item":{"fields":fields,"invariants":{}}
    }});
    let ddl = spike_v2_read::sqlgen::create_ddl_in("aip_reserved_fields", &facts).unwrap().join("\n");
    for word in words {
        assert!(ddl.contains(&format!("\"{word}\" text NOT NULL")), "{word}: {ddl}");
    }
    assert!(ddl.contains("\"name\" text NOT NULL"), "ordinary identifiers retain their spelling: {ddl}");
    assert!(
        ddl.contains("\"owner_id\" bigint REFERENCES") && ddl.contains("\"owner_id\" bigint REFERENCES aip_reserved_fields.member(id) NOT NULL"),
        "relationship columns use the same quoting contract: {ddl}"
    );
    assert_eq!(spike_v2_read::sqlgen::Dialect::Postgres.ident("grant"), "\"grant\"");
}

#[test]
fn postgres_read_sql_quotes_reserved_and_relationship_columns() {
    use spike_v1_fixture::{load_str, Form};
    use spike_v2_read::plan::{plan_read, Caller};

    let definition = r#"
resource Member { fields { id: Id } rows read when true }
actor Member
resource Item {
  fields { id: Id; name: Text; from: Text; where: Text; owner: Member }
  rows read when true
  expose read { select id, name, from, where, owner; filter from.eq; sort id; budget { rows 10; depth 1; deadline 1s; cost 100 } }
}
"#;
    let facts = load_str(definition, Form::A).expect("valid definition").execution;
    let plan = plan_read(
        &facts,
        &json!({"read":"Item", "select":["id", "name", "from", "where", "owner"], "filter":[{"field":"from", "op":"eq", "value":"x"}]}),
        &Caller { actor_id: Some(1), now: "2026-11-22T00:00:00Z".into() },
    )
    .expect("read plan");

    assert!(plan.sql.contains("\"from\""), "reserved field in query SQL: {}", plan.sql);
    assert!(plan.sql.contains("\"where\""), "reserved field in query SQL: {}", plan.sql);
    assert!(plan.sql.contains("\"owner_id\""), "relationship column in query SQL: {}", plan.sql);
    assert!(plan.sql.contains(".\"name\""), "ordinary field spelling remains unchanged: {}", plan.sql);
}
