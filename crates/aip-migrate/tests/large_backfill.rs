//! Large-table backfill measurements. They are `#[ignore]`d because they take minutes:
//! `AIP_BACKFILL_ROWS=3000000 cargo test -p aip-migrate --test large_backfill -- --ignored --nocapture`
use aip_migrate::{apply, init, plan};
use serde_json::json;
use spike_v1_fixture::{Form, load_str};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const DB: &str = "host=localhost dbname=postgres";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct OwnedSchema(String);
impl OwnedSchema {
    fn new() -> Self {
        Self(format!("aip_backfill_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
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

fn rows() -> i64 {
    std::env::var("AIP_BACKFILL_ROWS").ok().and_then(|value| value.parse().ok()).unwrap_or(2_000_000)
}

async fn client() -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "large synthetic table; run explicitly to measure"]
async fn measure_required_column_backfill_on_a_large_table() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let db = client().await;
    let n = rows();
    let started = Instant::now();
    db.execute(&format!("INSERT INTO {}.member(id) SELECT g FROM generate_series(1,{n}) g", schema.0), &[]).await.unwrap();
    println!("seeded {n} rows in {:?}", started.elapsed());
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["score"] = json!({"ty":"Int"});
    let migration = Some(json!({"backfill":[{"resource":"Member","field":"score","value":7}]}));
    let planned = Instant::now();
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    println!("plan took {:?}", planned.elapsed());

    // A second connection measures how long ordinary traffic is blocked while the migration runs.
    let probe_schema = schema.0.clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag = stop.clone();
    let probe = tokio::spawn(async move {
        let db = client().await;
        let mut worst = std::time::Duration::ZERO;
        let mut failures = 0u32;
        while !stop_flag.load(Ordering::SeqCst) {
            let tick = Instant::now();
            match db.query_one(&format!("SELECT 1 FROM {probe_schema}.member WHERE id=1"), &[]).await {
                Ok(_) => worst = worst.max(tick.elapsed()),
                Err(_) => failures += 1,
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        (worst, failures)
    });
    let applied = Instant::now();
    let result = apply(DB, &schema.0, &after, &migration, report["digest"].as_str().unwrap()).await;
    let elapsed = applied.elapsed();
    stop.store(true, Ordering::SeqCst);
    let (worst, failures) = probe.await.unwrap();
    println!("worst point-read latency during migration: {worst:?}, failed reads: {failures}");
    println!(
        "apply took {elapsed:?}: {}",
        match &result {
            Ok(_) => "ok".to_string(),
            Err(error) => error.to_string(),
        }
    );
    let remaining: i64 =
        db.query_one(&format!("SELECT count(*) FROM {}.member WHERE score IS NULL", schema.0), &[]).await.map(|row| row.get(0)).unwrap_or(-1);
    println!("rows still without score after apply: {remaining}");
    assert!(result.is_ok(), "a constant backfill must finish inside the 20s statement limit: {result:?}");
    assert_eq!(remaining, 0);
    assert!(elapsed < std::time::Duration::from_secs(10), "catalog-only backfill took {elapsed:?}");
    assert!(worst < std::time::Duration::from_secs(5), "readers were blocked for {worst:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "large synthetic table; run explicitly to measure"]
async fn measure_column_type_conversion_on_a_large_table_fails_safe() {
    let schema = OwnedSchema::new();
    let mut before = facts();
    before["resources"]["Member"]["fields"]["score"] = json!({"ty":"Text?"});
    init(DB, &schema.0, &before).await.unwrap();
    let db = client().await;
    let n = rows();
    db.execute(&format!("INSERT INTO {}.member(id,score) SELECT g, g::text FROM generate_series(1,{n}) g", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["score"] = json!({"ty":"Int?"});
    let conversion = Some(json!({"convert":[{"resource":"Member","field":"score","to":"Int?"}]}));
    let report = plan(DB, &schema.0, &after, &conversion).await.unwrap();
    let applied = Instant::now();
    let result = apply(DB, &schema.0, &after, &conversion, report["digest"].as_str().unwrap()).await;
    println!(
        "conversion of {n} rows took {:?}: {}",
        applied.elapsed(),
        match &result {
            Ok(_) => "ok".to_string(),
            Err(error) => error.to_string(),
        }
    );
    let intact: i64 =
        db.query_one(&format!("SELECT count(*) FROM {}.member WHERE score IS NOT NULL", schema.0), &[]).await.map(|row| row.get(0)).unwrap_or(-1);
    println!("rows intact after the attempt: {intact}");
}
