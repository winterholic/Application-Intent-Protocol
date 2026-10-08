use aip_migrate::{apply, init, plan, preflight};
use serde_json::json;
use spike_v1_fixture::{Form, load_str};

const DB: &str = "host=localhost dbname=postgres";

fn facts(source: &str) -> serde_json::Value {
    load_str(source, Form::A).unwrap().execution
}

fn schema(label: &str) -> String {
    format!("aip_ref_{label}_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
}

async fn connect() -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

#[tokio::test]
async fn existing_table_can_reference_new_resource_chain() {
    let before = facts("resource Member { fields { id: Id } } actor Member");
    let after = facts(
        "resource Member { fields { id: Id; team: Team? } } actor Member \
         resource Team { fields { id: Id; organization: Organization? } } \
         resource Organization { fields { id: Id } }",
    );
    let schema = schema("chain");
    init(DB, &schema, &before).await.unwrap();
    let db = connect().await;
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1)")).await.unwrap();

    let report = plan(DB, &schema, &after, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    apply(DB, &schema, &after, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema, &after).await.unwrap();

    let old_value: Option<i64> = db.query_one(&format!("SELECT team_id FROM {schema}.member WHERE id=1"), &[]).await.unwrap().get(0);
    assert_eq!(old_value, None);
    db.batch_execute(&format!(
        "INSERT INTO {schema}.organization(id) VALUES(10); INSERT INTO {schema}.team(id,organization_id) VALUES(20,10); \
         UPDATE {schema}.member SET team_id=20 WHERE id=1"
    ))
    .await
    .unwrap();
    let value: i64 = db.query_one(&format!("SELECT team_id FROM {schema}.member WHERE id=1"), &[]).await.unwrap().get(0);
    assert_eq!(value, 20);
    let missing = db.execute(&format!("UPDATE {schema}.member SET team_id=999 WHERE id=1"), &[]).await.unwrap_err();
    assert_eq!(missing.as_db_error().unwrap().code().code(), "23503");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}

#[tokio::test]
async fn ref_conversion_waits_for_new_resource_table() {
    let before = facts("resource Member { fields { id: Id; team: Member.Id? } } actor Member");
    let after = facts("resource Member { fields { id: Id; team: Team? } } actor Member resource Team { fields { id: Id } }");
    let schema = schema("convert");
    init(DB, &schema, &before).await.unwrap();
    let db = connect().await;
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1)")).await.unwrap();

    let report = plan(DB, &schema, &after, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    apply(DB, &schema, &after, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema, &after).await.unwrap();
    db.batch_execute(&format!("INSERT INTO {schema}.team(id) VALUES(7); UPDATE {schema}.member SET team_id=7 WHERE id=1")).await.unwrap();
    let missing = db.execute(&format!("UPDATE {schema}.member SET team_id=8 WHERE id=1"), &[]).await.unwrap_err();
    assert_eq!(missing.as_db_error().unwrap().code().code(), "23503");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}

#[tokio::test]
async fn failed_backfill_rolls_back_new_table_and_column() {
    let before = facts("resource Member { fields { id: Id } } actor Member");
    let after = facts("resource Member { fields { id: Id; team: Team? } } actor Member resource Team { fields { id: Id } }");
    let migration = Some(json!({"backfill":[{"resource":"Member","field":"team","value":999}]}));
    let schema = schema("rollback");
    init(DB, &schema, &before).await.unwrap();
    let db = connect().await;
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1)")).await.unwrap();

    let report = plan(DB, &schema, &after, &migration).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    let error = apply(DB, &schema, &after, &migration, report["digest"].as_str().unwrap()).await.unwrap_err();
    assert_eq!(error["code"], "MIGRATION_FAILED");
    preflight(DB, &schema, &before).await.unwrap();
    let team: Option<String> = db.query_one("SELECT to_regclass($1)::text", &[&format!("{schema}.team")]).await.unwrap().get(0);
    assert_eq!(team, None);
    let column_count: i64 = db
        .query_one(
            "SELECT count(*) FROM information_schema.columns WHERE table_schema=$1 AND table_name='member' AND column_name='team_id'",
            &[&schema],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(column_count, 0);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
