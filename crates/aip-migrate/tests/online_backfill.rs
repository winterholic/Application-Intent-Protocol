//! A constant backfill must not rewrite or update existing rows, must quote hostile values, and a migration
//! that cannot get its lock must fail within the lock timeout and be safe to retry.
use aip_migrate::{apply, init, plan};
use serde_json::json;
use spike_v1_fixture::{Form, load_str};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const DB: &str = "host=localhost dbname=postgres";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct OwnedSchema(String);
impl OwnedSchema {
    fn new() -> Self {
        Self(format!("aip_online_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
    }
}
impl Drop for OwnedSchema {
    fn drop(&mut self) {
        let name = self.0.clone();
        std::thread::spawn(move || {
            let _ = std::process::Command::new("psql").args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA IF EXISTS {name} CASCADE")]).output();
        })
        .join()
        .ok();
    }
}

fn facts() -> serde_json::Value {
    load_str(include_str!("../../../prototype/example/app.aip"), Form::A).unwrap().execution
}

async fn client() -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

async fn filenode(db: &tokio_postgres::Client, schema: &str) -> i64 {
    db.query_one(&format!("SELECT pg_relation_filenode('{schema}.member'::regclass)::bigint"), &[]).await.unwrap().get(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn required_backfill_adds_the_column_without_rewriting_or_updating_rows() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let db = client().await;
    db.execute(&format!("INSERT INTO {}.member(id) SELECT g FROM generate_series(1,1000) g", schema.0), &[]).await.unwrap();
    let node_before = filenode(&db, &schema.0).await;
    let positions_before: String =
        db.query_one(&format!("SELECT string_agg(ctid::text, ',' ORDER BY id) FROM {}.member", schema.0), &[]).await.unwrap().get(0);
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["score"] = json!({"ty":"Int"});
    let migration = Some(json!({"backfill":[{"resource":"Member","field":"score","value":7}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    let sql: Vec<&str> = report["steps"].as_array().unwrap().iter().map(|step| step["sql"].as_str().unwrap()).collect();
    assert!(sql.iter().all(|sql| !sql.starts_with("UPDATE")), "no row-by-row UPDATE is planned: {sql:?}");
    apply(DB, &schema.0, &after, &migration, report["digest"].as_str().unwrap()).await.unwrap();

    assert_eq!(filenode(&db, &schema.0).await, node_before, "the table file was not rewritten");
    let positions_after: String =
        db.query_one(&format!("SELECT string_agg(ctid::text, ',' ORDER BY id) FROM {}.member", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(positions_after, positions_before, "no existing row was updated (an UPDATE would move its ctid)");
    let sevens: i64 = db.query_one(&format!("SELECT count(*) FROM {}.member WHERE score=7", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(sevens, 1000, "every existing row reads the backfilled value");
    let default_left: i64 = db
        .query_one(
            "SELECT count(*) FROM pg_attrdef d JOIN pg_attribute a ON a.attrelid=d.adrelid AND a.attnum=d.adnum WHERE d.adrelid=($1||'.member')::regclass AND a.attname='score'",
            &[&schema.0],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(default_left, 0, "the backfill default is removed so new rows must state the value");
    assert!(
        db.execute(&format!("INSERT INTO {}.member(id) VALUES(5000)", schema.0), &[]).await.is_err(),
        "a required column still rejects a row without a value"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backfill_values_with_quotes_and_backslashes_are_stored_verbatim() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let db = client().await;
    db.execute(&format!("INSERT INTO {}.member(id) VALUES(1)", schema.0), &[]).await.unwrap();
    let hostile = r"it's \n $1 ''; DROP TABLE member; --";
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["label"] = json!({"ty":"Text"});
    let migration = Some(json!({"backfill":[{"resource":"Member","field":"label","value":hostile}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    apply(DB, &schema.0, &after, &migration, report["digest"].as_str().unwrap()).await.unwrap();
    let stored: String = db.query_one(&format!("SELECT label FROM {}.member WHERE id=1", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(stored, hostile);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocked_migration_gives_up_at_the_lock_timeout_changes_nothing_and_can_be_retried() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let db = client().await;
    db.execute(&format!("INSERT INTO {}.member(id) VALUES(1)", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["score"] = json!({"ty":"Int"});
    let migration = Some(json!({"backfill":[{"resource":"Member","field":"score","value":7}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    let digest = report["digest"].as_str().unwrap().to_string();

    // A long-running reader holds ACCESS SHARE on the table, which an ACCESS EXCLUSIVE lock must wait for.
    let reader = client().await;
    reader.batch_execute(&format!("BEGIN; SELECT count(*) FROM {}.member", schema.0)).await.unwrap();
    let started = Instant::now();
    let error = apply(DB, &schema.0, &after, &migration, &digest).await.unwrap_err();
    assert_eq!(error["code"], "DB_LOCK", "{error}");
    assert!(started.elapsed() < Duration::from_secs(6), "the lock wait is bounded, took {:?}", started.elapsed());
    let columns: i64 = db
        .query_one(
            "SELECT count(*) FROM information_schema.columns WHERE table_schema=$1 AND table_name='member' AND column_name='score'",
            &[&schema.0],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(columns, 0, "the failed attempt left no partial change");

    reader.batch_execute("COMMIT").await.unwrap();
    apply(DB, &schema.0, &after, &migration, &digest).await.unwrap();
    let value: i64 = db.query_one(&format!("SELECT score FROM {}.member WHERE id=1", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(value, 7, "the same acknowledged plan applies once the lock is free");
}
