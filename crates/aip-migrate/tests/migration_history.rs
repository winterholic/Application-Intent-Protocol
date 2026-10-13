//! O-24: a deployment may go A -> B -> A -> B (a rollback followed by a second roll-out), and a Time <-> Date
//! conversion must not depend on the TimeZone of whichever session happens to run the migration.
use aip_migrate::{apply, init, plan};
use serde_json::{Value, json};
use spike_v1_fixture::{Form, load_str};
use std::sync::atomic::{AtomicU64, Ordering};

const DB: &str = "host=localhost dbname=postgres";
// The migration runs in a session whose TimeZone is far from UTC.
const DB_SEOUL: &str = "host=localhost dbname=postgres options='-c TimeZone=Asia/Seoul'";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct OwnedSchema(String);
impl OwnedSchema {
    fn new() -> Self {
        Self(format!("aip_history_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
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

fn facts() -> Value {
    load_str(include_str!("../../../prototype/example/app.aip"), Form::A).unwrap().execution
}

async fn client() -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

async fn step(db_url: &str, schema: &str, facts: &Value, migration: &Option<Value>) -> Result<Value, Value> {
    let report = plan(db_url, schema, facts, migration).await?;
    apply(db_url, schema, facts, migration, report["digest"].as_str().unwrap()).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rollback_followed_by_the_same_rollout_applies_again() {
    let schema = OwnedSchema::new();
    let a = facts();
    let mut b = a.clone();
    b["resources"]["Member"]["fields"]["nickname"] = json!({"ty":"Text?"});
    init(DB, &schema.0, &a).await.unwrap();

    step(DB, &schema.0, &b, &None).await.expect("A -> B");
    step(DB, &schema.0, &a, &None).await.expect("B -> A (the column is dropped again)");
    let again = step(DB, &schema.0, &b, &None).await;
    assert!(again.is_ok(), "the second A -> B has the same plan digest as the first and must still apply: {again:?}");

    let db = client().await;
    let journal: i64 = db.query_one(&format!("SELECT count(*) FROM {}.aip_migrate_journal", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(journal, 4, "init plus three applied plans, each recorded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn time_to_date_conversion_uses_utc_whatever_the_session_time_zone() {
    let schema = OwnedSchema::new();
    let mut before = facts();
    before["resources"]["Member"]["fields"]["seen"] = json!({"ty":"Time?"});
    init(DB, &schema.0, &before).await.unwrap();
    let db = client().await;
    // 20:00 UTC on the 13th is already the 14th in Seoul.
    db.execute(&format!("INSERT INTO {}.member(id,seen) VALUES(1,'2026-10-13T20:00:00Z')", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["seen"] = json!({"ty":"Date?"});
    let conversion = Some(json!({"convert":[{"resource":"Member","field":"seen","to":"Date?"}]}));
    step(DB_SEOUL, &schema.0, &after, &conversion).await.expect("Time -> Date");
    let date: String = db.query_one(&format!("SELECT seen::text FROM {}.member WHERE id=1", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(date, "2026-10-13", "the date of the instant in UTC, not in the migrating session's zone");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn date_to_time_conversion_is_midnight_utc_whatever_the_session_time_zone() {
    let schema = OwnedSchema::new();
    let mut before = facts();
    before["resources"]["Member"]["fields"]["since"] = json!({"ty":"Date?"});
    init(DB, &schema.0, &before).await.unwrap();
    let db = client().await;
    db.execute(&format!("INSERT INTO {}.member(id,since) VALUES(1,'2026-10-13')", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["since"] = json!({"ty":"Time?"});
    let conversion = Some(json!({"convert":[{"resource":"Member","field":"since","to":"Time?"}]}));
    step(DB_SEOUL, &schema.0, &after, &conversion).await.expect("Date -> Time");
    let instant: String = db
        .query_one(&format!("SELECT to_char(since AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS') FROM {}.member WHERE id=1", schema.0), &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(instant, "2026-10-13T00:00:00");
}
