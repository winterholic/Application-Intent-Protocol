use aip_migrate::{apply, init, plan, preflight};
use spike_v1_fixture::{Form, load_str};

const DB: &str = "host=localhost dbname=postgres";

fn facts(source: &str) -> serde_json::Value {
    load_str(source, Form::A).unwrap_or_else(|errors| panic!("V1: {errors:?}")).execution
}

fn schema(label: &str) -> String {
    format!("aip_cycle_{label}_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
}

async fn connect() -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

async fn migrate(schema: &str, next: &serde_json::Value) {
    let report = plan(DB, schema, next, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    apply(DB, schema, next, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, schema, next).await.unwrap();
}

#[tokio::test]
async fn initial_cyclic_schema_has_two_immediate_foreign_keys() {
    let source = "resource Member { fields { id: Id; dept: Dept? } } actor Member resource Dept { fields { id: Id; lead: Member? } }";
    let facts = facts(source);
    let schema = schema("init");
    init(DB, &schema, &facts).await.unwrap();
    preflight(DB, &schema, &facts).await.unwrap();
    let db = connect().await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(10); INSERT INTO {schema}.dept(id) VALUES(1); \
         UPDATE {schema}.member SET dept_id=1 WHERE id=10; UPDATE {schema}.dept SET lead_id=10 WHERE id=1"
    ))
    .await
    .unwrap();
    let bad = db.execute(&format!("UPDATE {schema}.dept SET lead_id=999 WHERE id=1"), &[]).await.unwrap_err();
    assert_eq!(bad.as_db_error().unwrap().code().code(), "23503");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}

#[tokio::test]
async fn adding_a_cycle_does_not_rebuild_an_unchanged_tail_fk() {
    let before = facts(
        "resource Alpha { fields { id: Id; beta: Beta? } } actor Alpha \
         resource Beta { fields { id: Id; gamma: Gamma? } } \
         resource Gamma { fields { id: Id } }",
    );
    let after = facts(
        "resource Alpha { fields { id: Id; beta: Beta? } } actor Alpha \
         resource Beta { fields { id: Id; gamma: Gamma? } } \
         resource Gamma { fields { id: Id; beta: Beta? } }",
    );
    let schema = schema("tail");
    init(DB, &schema, &before).await.unwrap();
    let db = connect().await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.gamma(id) VALUES(1); \
         INSERT INTO {schema}.beta(id,gamma_id) VALUES(2,1); \
         INSERT INTO {schema}.alpha(id,beta_id) VALUES(3,2)"
    ))
    .await
    .unwrap();
    let report = plan(DB, &schema, &after, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    assert!(
        !report["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| { step["sql"].as_str().is_some_and(|sql| sql.starts_with(&format!("ALTER TABLE {schema}.alpha "))) }),
        "unchanged Alpha FK should not be rebuilt: {report}"
    );
    apply(DB, &schema, &after, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema, &after).await.unwrap();
    let beta: Option<i64> = db.query_one(&format!("SELECT beta_id FROM {schema}.alpha WHERE id=3"), &[]).await.unwrap().get(0);
    assert_eq!(beta, Some(2));
    let missing = db.execute(&format!("UPDATE {schema}.alpha SET beta_id=999 WHERE id=3"), &[]).await.unwrap_err();
    assert_eq!(missing.as_db_error().unwrap().code().code(), "23503");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}

#[tokio::test]
async fn cyclic_fk_can_be_added_retargeted_made_inline_and_removed_with_existing_rows() {
    let base = facts(
        "resource Member { fields { id: Id } } actor Member \
         resource Dept { fields { id: Id; lead: Member? } } \
         resource Sponsor { fields { id: Id; dept: Dept? } }",
    );
    let cycle_member = facts(
        "resource Member { fields { id: Id; dept: Dept? } } actor Member \
         resource Dept { fields { id: Id; lead: Member? } } \
         resource Sponsor { fields { id: Id; dept: Dept? } }",
    );
    let cycle_sponsor = facts(
        "resource Member { fields { id: Id; dept: Dept? } } actor Member \
         resource Dept { fields { id: Id; lead: Sponsor? } } \
         resource Sponsor { fields { id: Id; dept: Dept? } }",
    );
    let acyclic = facts(
        "resource Member { fields { id: Id; dept: Dept? } } actor Member \
         resource Dept { fields { id: Id; lead: Sponsor? } } \
         resource Sponsor { fields { id: Id } }",
    );
    let removed = facts("resource Member { fields { id: Id; dept: Dept? } } actor Member resource Dept { fields { id: Id } }");
    let schema = schema("evolve");
    init(DB, &schema, &base).await.unwrap();
    let db = connect().await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(10); \
         INSERT INTO {schema}.dept(id,lead_id) VALUES(1,10); \
         INSERT INTO {schema}.sponsor(id,dept_id) VALUES(10,1)"
    ))
    .await
    .unwrap();

    migrate(&schema, &cycle_member).await;
    let preserved: Option<i64> = db.query_one(&format!("SELECT lead_id FROM {schema}.dept WHERE id=1"), &[]).await.unwrap().get(0);
    assert_eq!(preserved, Some(10));
    db.batch_execute(&format!("UPDATE {schema}.member SET dept_id=1 WHERE id=10")).await.unwrap();
    migrate(&schema, &cycle_sponsor).await;
    let lead: Option<i64> = db.query_one(&format!("SELECT lead_id FROM {schema}.dept WHERE id=1"), &[]).await.unwrap().get(0);
    assert_eq!(lead, Some(10));
    let invalid = db.execute(&format!("UPDATE {schema}.dept SET lead_id=999 WHERE id=1"), &[]).await.unwrap_err();
    assert_eq!(invalid.as_db_error().unwrap().code().code(), "23503");

    migrate(&schema, &acyclic).await;
    let lead: Option<i64> = db.query_one(&format!("SELECT lead_id FROM {schema}.dept WHERE id=1"), &[]).await.unwrap().get(0);
    assert_eq!(lead, Some(10));
    let invalid = db.execute(&format!("UPDATE {schema}.dept SET lead_id=999 WHERE id=1"), &[]).await.unwrap_err();
    assert_eq!(invalid.as_db_error().unwrap().code().code(), "23503");

    migrate(&schema, &removed).await;
    let member_dept: Option<i64> = db.query_one(&format!("SELECT dept_id FROM {schema}.member WHERE id=10"), &[]).await.unwrap().get(0);
    assert_eq!(member_dept, Some(1));
    let sponsor: Option<String> = db.query_one("SELECT to_regclass($1)::text", &[&format!("{schema}.sponsor")]).await.unwrap().get(0);
    assert_eq!(sponsor, None);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
