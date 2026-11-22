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
    // Existing deployments store the digest of the DDL their release generated and every start compares it with the current
    // generator. Identifiers that never needed quoting must therefore keep their original spelling.
    assert!(ddl.contains("(name text NOT NULL") || ddl.contains(", name text NOT NULL"), "ordinary identifiers retain their spelling: {ddl}");
    assert!(!ddl.contains("\"name\"") && !ddl.contains("\"owner_id\""), "only keywords are quoted: {ddl}");
    assert!(
        ddl.contains("owner_id bigint REFERENCES aip_reserved_fields.member(id) NOT NULL"),
        "relationship columns keep their original spelling: {ddl}"
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
    assert!(plan.sql.contains(".owner_id"), "relationship column keeps its spelling in query SQL: {}", plan.sql);
    assert!(plan.sql.contains(".name") && !plan.sql.contains("\"name\""), "ordinary field spelling remains unchanged: {}", plan.sql);
}

const DB: &str = aip_test_support::test_database_url!();

/// The generator quotes exactly the words PostgreSQL rejects as a bare column name (keyword categories R and T) and leaves every
/// other spelling alone, because stored DDL digests make unquoted names part of the deployment contract. The server's own keyword
/// catalog is the oracle, so a PostgreSQL release that reserves another word fails here instead of in a customer's `init`.
#[tokio::test]
async fn postgres_identifier_quoting_matches_the_server_keyword_catalog() {
    let db = spike_v2_read::connect_with_url(DB).await.unwrap();
    let keywords = db.query("SELECT word, catcode::text FROM pg_get_keywords() ORDER BY word", &[]).await.unwrap();
    assert!(keywords.len() > 400, "pg_get_keywords returned {} rows", keywords.len());
    let mut fields = serde_json::Map::new();
    for row in &keywords {
        let (word, category): (String, String) = (row.get(0), row.get(1));
        let quoted = spike_v2_read::sqlgen::Dialect::Postgres.ident(&word);
        let must_quote = matches!(category.as_str(), "R" | "T");
        assert_eq!(quoted == format!("\"{word}\""), must_quote, "{word} ({category}) rendered as {quoted}");
        assert!(must_quote || quoted == word, "{word} ({category}) must keep its spelling, got {quoted}");
        fields.insert(word, json!({"ty":"Text"}));
    }
    let facts = json!({"resources":{"Item":{"fields":fields,"invariants":{}}}});
    let schema = format!("aip_kw_{}", std::process::id());
    let ddl = spike_v2_read::sqlgen::create_ddl_in(&schema, &facts).unwrap();
    for statement in &ddl {
        db.batch_execute(statement).await.unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    let created: i64 = db
        .query_one("SELECT count(*) FROM information_schema.columns WHERE table_schema = $1 AND table_name = 'item'", &[&schema])
        .await
        .unwrap()
        .get(0);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert_eq!(created as usize, keywords.len(), "every keyword-named field becomes a real column");
}
