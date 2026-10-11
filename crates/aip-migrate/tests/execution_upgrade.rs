use aip_migrate::{apply, init_with_wire, plan, preflight};
use serde_json::json;
use spike_v1_fixture::{Form, load_str};
use std::sync::atomic::{AtomicU64, Ordering};

const DB: &str = "host=localhost dbname=postgres";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct OwnedSchema(String);
impl OwnedSchema {
    fn new() -> Self {
        Self(format!("aip_exec_up_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
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

fn operation_facts(kind: &str) -> serde_json::Value {
    let source =
        include_str!("../../../spikes/spike-v6-transport/tests/fixtures/operations.aip").replace("operation read", &format!("operation {kind}"));
    load_str(&source, Form::A).unwrap().execution
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upgrading_sync_operations_to_jobs_installs_storage_and_preserves_managed_rows() {
    let schema = OwnedSchema::new();
    let sync = operation_facts("read");
    let jobs = operation_facts("job");
    init_with_wire(DB, &schema.0, &sync, "safe").await.unwrap();

    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    db.execute(&format!("INSERT INTO {}.aip_actors(id) VALUES(42)", schema.0), &[]).await.unwrap();
    db.execute(
        &format!("INSERT INTO {}.aip_principals(issuer,subject,actor_id,min_iat) VALUES('https://issuer','subject-42',42,123)", schema.0),
        &[],
    )
    .await
    .unwrap();
    db.execute(
        &format!("INSERT INTO {}.aip_idem(principal,key,request,result) VALUES('actor:42:ids:safe-number-v13','key-1','request-1',$1)", schema.0),
        &[&json!({"kept":true})],
    )
    .await
    .unwrap();

    let report = plan(DB, &schema.0, &jobs, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    assert_eq!(report["requiresReview"], true, "{report}");
    assert!(report["steps"].as_array().is_some_and(|steps| !steps.is_empty()), "{report}");
    apply(DB, &schema.0, &jobs, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema.0, &jobs).await.unwrap();

    let tables = db.query("SELECT tablename FROM pg_tables WHERE schemaname=$1", &[&schema.0]).await.unwrap();
    let names: Vec<String> = tables.iter().map(|row| row.get(0)).collect();
    for table in ["aip_jobs", "aip_files", "aip_events", "aip_effects"] {
        assert!(names.iter().any(|name| name == table), "missing {table}: {names:?}");
    }
    let principal = db.query_one(&format!("SELECT issuer,subject,actor_id,min_iat FROM {}.aip_principals", schema.0), &[]).await.unwrap();
    assert_eq!(principal.get::<_, String>(0), "https://issuer");
    assert_eq!(principal.get::<_, String>(1), "subject-42");
    assert_eq!(principal.get::<_, i64>(2), 42);
    assert_eq!(principal.get::<_, i64>(3), 123);
    let idem: serde_json::Value = db.query_one(&format!("SELECT result FROM {}.aip_idem WHERE key='key-1'", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(idem, json!({"kept":true}));
    let actor: i64 = db.query_one(&format!("SELECT id FROM {}.aip_actors", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(actor, 42);

    db.execute(
        &format!("INSERT INTO {}.aip_jobs(id,owner,actor_id,grant_data,operation,contract_digest,deployment_digest,implementation_digest,input,input_digest,idempotency_key,state,max_attempts,timeout_ms,run_deadline,result_ttl_ms,initial_delay_ms) VALUES('00000000-0000-4000-8000-000000000001','https://issuer|subject-42',42,'{{}}','length','contract','deployment','implementation','{{}}','input-digest','job-key','QUEUED',3,1000,clock_timestamp()+interval '1 hour',86400000,0)", schema.0),
        &[],
    ).await.unwrap();
    let downgrade = plan(DB, &schema.0, &sync, &None).await.unwrap();
    assert_eq!(downgrade["blocked"], true, "{downgrade}");
    assert!(downgrade["steps"].as_array().unwrap().iter().all(|step| !step["sql"].as_str().unwrap_or("").starts_with("DROP TABLE")), "{downgrade}");
    assert_eq!(apply(DB, &schema.0, &sync, &None, downgrade["digest"].as_str().unwrap()).await.unwrap_err()["code"], "MIGRATION_BLOCKED");
    let retained: i64 = db
        .query_one(&format!("SELECT count(*) FROM {}.aip_jobs WHERE id='00000000-0000-4000-8000-000000000001'", schema.0), &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(retained, 1);
}
