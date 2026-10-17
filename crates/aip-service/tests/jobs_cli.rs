//! `aip service jobs` lets an operator see failed runs across owners and redrive one as the same run.
use aip_execution::{NewRun, Store};
use aip_service::{Command, Jobs};
use serde_json::{Value, json};

const DB: &str = aip_test_support::test_database_url!();
const DB_ENV: &str = "AIP_JOBS_CLI_TEST_DATABASE";

struct Cleanup(String, std::path::PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::process::Command::new("psql").args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA IF EXISTS {} CASCADE", self.0)]).output();
        let _ = std::fs::remove_file(&self.1);
    }
}

fn run(name: &str, deployment: &str, key: &str) -> NewRun {
    NewRun {
        owner: "test|svc".into(),
        actor: 1,
        grant: json!({"issuer":"test","subject":"svc"}),
        operation: name.into(),
        contract: "contract".into(),
        deployment: deployment.into(),
        implementation: "impl".into(),
        input: json!({"secret":"business payload"}),
        key: key.into(),
        timeout_ms: 10_000,
        max_attempts: 1,
        ttl_ms: 86_400_000,
        delay_ms: 0,
        trace_id: None,
    }
}

async fn cli(config: &std::path::Path, command: Jobs) -> Result<Value, Value> {
    aip_service::run(Command::Jobs { config: config.to_path_buf(), command }).await
}

#[tokio::test]
async fn operator_lists_failed_jobs_without_payloads_and_redrives_only_safe_ones() {
    let schema = format!("aip_jobs_cli_{}", std::process::id());
    let config = std::env::temp_dir().join(format!("{schema}.json"));
    let _cleanup = Cleanup(schema.clone(), config.clone());
    let facts =
        spike_v1_fixture::load_str(include_str!("../../../spikes/spike-v6-transport/tests/fixtures/job-retry.aip"), spike_v1_fixture::Form::A)
            .unwrap()
            .execution;
    aip_migrate::init_with_wire(DB, &schema, &facts, "safe").await.unwrap();
    // SAFETY: this test binary sets one variable no other test reads, before any thread reads it.
    unsafe { std::env::set_var(DB_ENV, DB) };
    std::fs::write(
        &config,
        json!({"schema":schema,"database_url_env":DB_ENV,"auth":{"issuer":"test","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}}})
            .to_string(),
    )
    .unwrap();

    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let store = Store::new(&schema).unwrap();
    let deployment = spike_v1_fixture::digest(&facts);
    let mut ids = Vec::new();
    for (key, deployment) in [("current", deployment.as_str()), ("older", "older-deployment"), ("uncertain", deployment.as_str())] {
        let accepted = store.enqueue(&*db, run("boom", deployment, key)).await.unwrap();
        let lease = store.claim(&*db, "worker").await.unwrap().unwrap();
        assert!(store.fail(&*db, &lease, "WIRE.EXTENSION_ERROR").await.unwrap());
        ids.push(accepted.id);
    }
    let effect_columns: Vec<String> = db
        .query("SELECT column_name::text FROM information_schema.columns WHERE table_schema=$1 AND table_name='aip_effects' AND is_nullable='NO' AND column_default IS NULL", &[&schema])
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert!(
        effect_columns.iter().all(|c| ["run_id", "effect_key", "kind", "state", "request_digest", "scope"].contains(&c.as_str())),
        "{effect_columns:?}"
    );
    db.execute(
        &format!(
            "INSERT INTO {schema}.aip_effects({}) SELECT {} ",
            effect_columns.join(","),
            effect_columns
                .iter()
                .map(|c| match c.as_str() {
                    "run_id" => "$1::text::uuid".to_string(),
                    "state" => "'UNKNOWN'".to_string(),
                    "kind" => "'http'".to_string(),
                    _ => format!("'{c}'"),
                })
                .collect::<Vec<_>>()
                .join(",")
        ),
        &[&ids[2]],
    )
    .await
    .unwrap();

    let listed = cli(&config, Jobs::List { state: Some("FAILED".into()), operation: None, limit: 50 }).await.unwrap();
    let jobs = listed["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 3, "{listed}");
    assert!(!listed.to_string().contains("business payload"), "listing must not print job input");
    let uncertain = jobs.iter().find(|job| job["id"] == ids[2]).unwrap();
    assert_eq!(uncertain["unsettledEffects"], 1, "{listed}");
    assert!(cli(&config, Jobs::List { state: Some("DONE".into()), operation: None, limit: 50 }).await.is_err());

    let refused = cli(&config, Jobs::Retry { id: ids[1].clone() }).await.unwrap_err();
    assert_eq!(refused["code"], "WIRE.DEPLOYMENT_CHANGED", "{refused}");
    let refused = cli(&config, Jobs::Retry { id: ids[2].clone() }).await.unwrap_err();
    assert_eq!(refused["code"], "WIRE.EFFECT_UNKNOWN", "{refused}");
    let refused = cli(&config, Jobs::Retry { id: "not-a-uuid".into() }).await.unwrap_err();
    assert_eq!(refused["code"], "BAD_JOB_ID", "{refused}");

    let queued = cli(&config, Jobs::Retry { id: ids[0].clone() }).await.unwrap();
    assert_eq!(queued["state"], "QUEUED", "{queued}");
    let again = cli(&config, Jobs::Retry { id: ids[0].clone() }).await.unwrap_err();
    assert_eq!(again["code"], "JOB_NOT_FAILED", "{again}");
    let state: String = db.query_one(&format!("SELECT state FROM {schema}.aip_jobs WHERE id=$1::text::uuid"), &[&ids[0]]).await.unwrap().get(0);
    assert_eq!(state, "QUEUED");
}
