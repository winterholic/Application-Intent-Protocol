use aip_execution::{Error, NewRun, Redrive, Store, ddl};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_postgres::{Client, NoTls};

const DB: &str = "host=localhost dbname=postgres";
const OWNER: &str = "https://issuer.example|subject-1";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Db {
    client: Client,
    schema: String,
}

impl Db {
    async fn new() -> Self {
        let schema = format!("aip_execution_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let (client, connection) = tokio_postgres::connect(DB, NoTls).await.expect("connect to local test PostgreSQL");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
        client.batch_execute(&ddl(&schema).unwrap()).await.unwrap();
        Self { client, schema }
    }

    fn store(&self) -> Store {
        Store::new(&self.schema).unwrap()
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        let schema = self.schema.clone();
        std::thread::spawn(move || {
            let _ =
                std::process::Command::new("psql").args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")]).output();
        })
        .join()
        .unwrap();
    }
}

fn new_run(key: &str, input: serde_json::Value) -> NewRun {
    NewRun {
        owner: "https://issuer.example|subject-1".into(),
        actor: 17,
        grant: json!({"scope":"operation:normalize"}),
        operation: "normalize".into(),
        contract: "contract-a".into(),
        deployment: "deployment-a".into(),
        implementation: "sha256:impl-a".into(),
        input,
        key: key.into(),
        timeout_ms: 30_000,
        max_attempts: 3,
        ttl_ms: 60_000,
        delay_ms: 0,
    }
}

#[tokio::test]
async fn enqueue_reuses_same_idempotency_key_and_rejects_changed_input() {
    let db = Db::new().await;
    let store = db.store();
    let first = store.enqueue(&db.client, new_run("k1", json!({"x":1}))).await.unwrap();
    let mut refreshed = new_run("k1", json!({"x":1}));
    refreshed.grant = json!({"scope":"refreshed-grant","issuedAt":2});
    let replay = store.enqueue(&db.client, refreshed).await.unwrap();
    assert_eq!(first.id, replay.id);
    let lease = store.claim(&db.client, "worker-a").await.unwrap().unwrap();
    assert_eq!(lease.grant, json!({"scope":"operation:normalize"}));
    assert_eq!(store.enqueue(&db.client, new_run("k1", json!({"x":2}))).await.unwrap_err(), Error::IdempotencyConflict);
}

#[tokio::test]
async fn concurrent_claims_skip_locked_rows_and_stale_epoch_cannot_finish() {
    let db = Db::new().await;
    let store = db.store();
    store.enqueue(&db.client, new_run("k1", json!({"x":1}))).await.unwrap();
    store.enqueue(&db.client, new_run("k2", json!({"x":2}))).await.unwrap();

    let (one, two) = tokio::join!(store.claim(&db.client, "worker-a"), store.claim(&db.client, "worker-b"));
    let (one, two) = (one.unwrap().unwrap(), two.unwrap().unwrap());
    assert_ne!(one.run_id, two.run_id);

    db.client
        .execute(&format!("UPDATE {}.aip_jobs SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1", db.schema), &[&one.run_id])
        .await
        .unwrap();
    let replacement = store.claim(&db.client, "worker-c").await.unwrap().unwrap();
    assert_eq!(one.run_id, replacement.run_id);
    assert!(replacement.epoch > one.epoch);
    assert!(!store.finish(&db.client, &one, json!({"stale":true})).await.unwrap());
    assert!(store.finish(&db.client, &replacement, json!({"ok":true})).await.unwrap());
}

#[tokio::test]
async fn owner_checks_hide_run_and_result_and_cancel_from_other_owner() {
    let db = Db::new().await;
    let store = db.store();
    let owner = "https://issuer.example|subject-1";
    let run = store.enqueue(&db.client, new_run("k1", json!({"x":1}))).await.unwrap();
    assert_eq!(store.status(&db.client, &run.id, "other-owner").await.unwrap(), None);
    assert_eq!(store.result(&db.client, &run.id, "other-owner").await.unwrap(), None);
    assert_eq!(store.cancel(&db.client, &run.id, "other-owner").await.unwrap(), None);
    assert_eq!(store.status(&db.client, &run.id, owner).await.unwrap().unwrap().state, "QUEUED");
}

#[tokio::test]
async fn retry_limit_and_run_deadline_make_run_terminal() {
    let db = Db::new().await;
    let store = db.store();
    let mut input = new_run("k1", json!({"x":1}));
    input.max_attempts = 1;
    let run = store.enqueue(&db.client, input).await.unwrap();
    let lease = store.claim(&db.client, "worker-a").await.unwrap().unwrap();
    store.retry(&db.client, &lease, "worker crashed").await.unwrap();
    assert_eq!(store.status(&db.client, &run.id, "https://issuer.example|subject-1").await.unwrap().unwrap().state, "FAILED");
}

#[tokio::test]
async fn expired_result_payload_is_hidden_while_status_tombstone_remains() {
    let db = Db::new().await;
    let store = db.store();
    let mut input = new_run("k1", json!({"x":1}));
    input.ttl_ms = 25;
    let run = store.enqueue(&db.client, input).await.unwrap();
    let lease = store.claim(&db.client, "worker-a").await.unwrap().unwrap();
    assert!(store.finish(&db.client, &lease, json!({"secret":"result"})).await.unwrap());
    let result = store.result(&db.client, &run.id, "https://issuer.example|subject-1").await.unwrap().unwrap();
    assert_eq!(result.operation, "normalize");
    assert_eq!(result.output, json!({"secret":"result"}));
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    assert_eq!(store.result(&db.client, &run.id, "https://issuer.example|subject-1").await.unwrap(), None);
    assert_eq!(store.status(&db.client, &run.id, "https://issuer.example|subject-1").await.unwrap().unwrap().state, "SUCCEEDED");
    let cancelled = store.cancel(&db.client, &run.id, "https://issuer.example|subject-1").await.unwrap().unwrap();
    assert_eq!(cancelled.output, None);
}

#[tokio::test]
async fn payload_expiry_scrubs_secrets_and_keeps_idempotency_tombstone() {
    let db = Db::new().await;
    let store = db.store();
    let run = store.enqueue(&db.client, new_run("scrub-me", json!({"secret":"input"}))).await.unwrap();
    let lease = store.claim(&db.client, "worker-a").await.unwrap().unwrap();
    assert!(store.finish(&db.client, &lease, json!({"secret":"output"})).await.unwrap());
    assert!(store.result(&db.client, &run.id, OWNER).await.unwrap().is_some());

    let mut active = new_run("active-payload", json!({"secret":"active-input"}));
    active.grant = json!({"secret":"active-grant"});
    let active_run = store.enqueue(&db.client, active).await.unwrap();
    db.client
        .execute(
            &format!("UPDATE {}.aip_jobs SET result_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1", db.schema),
            &[&uuid::Uuid::parse_str(&run.id).unwrap()],
        )
        .await
        .unwrap();

    assert_eq!(store.result(&db.client, &run.id, OWNER).await.unwrap(), None);
    let expired = store.expire_payloads(&db.client).await.unwrap();
    assert_eq!(expired, vec![uuid::Uuid::parse_str(&run.id).unwrap()]);
    assert_eq!(store.result(&db.client, &run.id, OWNER).await.unwrap(), None);
    let status = store.status(&db.client, &run.id, OWNER).await.unwrap().unwrap();
    assert_eq!(status.state, "SUCCEEDED");
    assert_eq!(status.operation, "normalize");
    assert_eq!(status.output, None);

    let scrubbed = db
        .client
        .query_one(
            &format!("SELECT input,grant_data,output,owner,operation,idempotency_key,input_digest,state FROM {}.aip_jobs WHERE id=$1", db.schema),
            &[&uuid::Uuid::parse_str(&run.id).unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(scrubbed.get::<_, serde_json::Value>(0), json!({}));
    assert_eq!(scrubbed.get::<_, serde_json::Value>(1), json!({}));
    assert_eq!(scrubbed.get::<_, Option<serde_json::Value>>(2), None);
    assert_eq!(scrubbed.get::<_, String>(3), OWNER);
    assert_eq!(scrubbed.get::<_, String>(4), "normalize");
    assert_eq!(scrubbed.get::<_, String>(5), "scrub-me");
    assert_ne!(scrubbed.get::<_, String>(6), "");
    assert_eq!(scrubbed.get::<_, String>(7), "SUCCEEDED");

    let mut replay = new_run("scrub-me", json!({"secret":"input"}));
    replay.grant = json!({"refreshed":true});
    assert_eq!(store.enqueue(&db.client, replay).await.unwrap().id, run.id);
    let active_payload = db
        .client
        .query_one(
            &format!("SELECT input,grant_data,output FROM {}.aip_jobs WHERE id=$1", db.schema),
            &[&uuid::Uuid::parse_str(&active_run.id).unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(active_payload.get::<_, serde_json::Value>(0), json!({"secret":"active-input"}));
    assert_eq!(active_payload.get::<_, serde_json::Value>(1), json!({"secret":"active-grant"}));
    assert_eq!(active_payload.get::<_, Option<serde_json::Value>>(2), None);
    assert!(store.expire_payloads(&db.client).await.unwrap().is_empty());

    let missing_output = store.enqueue(&db.client, new_run("null-output", json!({"x":3}))).await.unwrap();
    let missing_output_lease = store.claim(&db.client, "worker-b").await.unwrap().unwrap();
    assert!(store.finish(&db.client, &missing_output_lease, json!({"ok":true})).await.unwrap());
    db.client
        .execute(&format!("UPDATE {}.aip_jobs SET output=NULL WHERE id=$1", db.schema), &[&uuid::Uuid::parse_str(&missing_output.id).unwrap()])
        .await
        .unwrap();
    assert_eq!(store.result(&db.client, &missing_output.id, OWNER).await.unwrap(), None);
}

#[tokio::test]
async fn expired_run_deadline_rejects_completion() {
    let db = Db::new().await;
    let store = db.store();
    let run = store.enqueue(&db.client, new_run("k1", json!({"x":1}))).await.unwrap();
    let lease = store.claim(&db.client, "worker-a").await.unwrap().unwrap();
    db.client
        .execute(&format!("UPDATE {}.aip_jobs SET run_deadline=clock_timestamp()-interval '1 second' WHERE id=$1", db.schema), &[&lease.run_id])
        .await
        .unwrap();
    assert!(!store.heartbeat(&db.client, &lease).await.unwrap());
    assert!(!store.finish(&db.client, &lease, json!({"tooLate":true})).await.unwrap());
    assert_eq!(store.status(&db.client, &run.id, "https://issuer.example|subject-1").await.unwrap().unwrap().state, "RUNNING");
    assert_eq!(store.claim(&db.client, "worker-b").await.unwrap(), None);
    let status = store.status(&db.client, &run.id, "https://issuer.example|subject-1").await.unwrap().unwrap();
    assert_eq!(status.state, "FAILED");
    assert_eq!(status.error.as_deref(), Some("deadline_exceeded"));
}

#[tokio::test]
async fn running_cancel_is_a_request_until_worker_acknowledges_exit() {
    let db = Db::new().await;
    let store = db.store();
    let run = store.enqueue(&db.client, new_run("k1", json!({"x":1}))).await.unwrap();
    let lease = store.claim(&db.client, "worker-a").await.unwrap().unwrap();
    let requested = store.cancel(&db.client, &run.id, "https://issuer.example|subject-1").await.unwrap().unwrap();
    assert_eq!(requested.state, "CANCEL_REQUESTED");
    assert!(!store.finish(&db.client, &lease, json!({"lostRace":true})).await.unwrap());
    let mut stale = lease.clone();
    stale.epoch += 1;
    assert!(!store.acknowledge_cancel(&db.client, &stale).await.unwrap());
    assert!(store.acknowledge_cancel(&db.client, &lease).await.unwrap());
    assert_eq!(store.status(&db.client, &run.id, "https://issuer.example|subject-1").await.unwrap().unwrap().state, "CANCELLED");
}

#[tokio::test]
async fn uncertain_external_cancel_is_terminal_only_for_the_current_lease() {
    let db = Db::new().await;
    let store = db.store();
    let run = store.enqueue(&db.client, new_run("uncertain-cancel", json!({"x":1}))).await.unwrap();
    let lease = store.claim(&db.client, "worker-a").await.unwrap().unwrap();
    assert_eq!(store.cancel(&db.client, &run.id, OWNER).await.unwrap().unwrap().state, "CANCEL_REQUESTED");

    let mut stale = lease.clone();
    stale.epoch += 1;
    assert!(!store.reject_cancel(&db.client, &stale, "CANCELLATION_UNCONFIRMED").await.unwrap());
    assert_eq!(store.status(&db.client, &run.id, OWNER).await.unwrap().unwrap().state, "CANCEL_REQUESTED");

    assert!(store.reject_cancel(&db.client, &lease, "CANCELLATION_UNCONFIRMED").await.unwrap());
    let status = store.status(&db.client, &run.id, OWNER).await.unwrap().unwrap();
    assert_eq!(status.state, "FAILED");
    assert_eq!(status.error.as_deref(), Some("CANCELLATION_UNCONFIRMED"));
    assert!(!store.check_lease(&db.client, &lease).await.unwrap());
    assert_eq!(store.claim(&db.client, "worker-b").await.unwrap(), None);
}

#[tokio::test]
async fn execution_store_methods_accept_the_callers_transaction() {
    let mut db = Db::new().await;
    let store = db.store();
    let tx = db.client.transaction().await.unwrap();
    let run = store.enqueue(&tx, new_run("tx-success", json!({"x":1}))).await.unwrap();
    let lease = store.claim(&tx, "worker-tx").await.unwrap().unwrap();
    assert!(store.check_lease(&tx, &lease).await.unwrap());
    assert!(store.heartbeat(&tx, &lease).await.unwrap());
    assert!(store.finish(&tx, &lease, json!({"ok":true})).await.unwrap());
    assert_eq!(store.status(&tx, &run.id, OWNER).await.unwrap().unwrap().state, "SUCCEEDED");
    assert_eq!(store.result(&tx, &run.id, OWNER).await.unwrap().unwrap().output, json!({"ok":true}));

    let failed_run = store.enqueue(&tx, new_run("tx-fail", json!({"x":2}))).await.unwrap();
    let failed_lease = store.claim(&tx, "worker-tx").await.unwrap().unwrap();
    assert!(store.fail(&tx, &failed_lease, "FAILED_TEST").await.unwrap());
    assert_eq!(store.status(&tx, &failed_run.id, OWNER).await.unwrap().unwrap().state, "FAILED");

    let retry_run = store.enqueue(&tx, new_run("tx-retry", json!({"x":3}))).await.unwrap();
    let retry_lease = store.claim(&tx, "worker-tx").await.unwrap().unwrap();
    assert!(store.retry(&tx, &retry_lease, "TEMPORARY_TEST").await.unwrap());
    assert_eq!(store.status(&tx, &retry_run.id, OWNER).await.unwrap().unwrap().state, "QUEUED");
    assert!(store.cancel(&tx, &retry_run.id, OWNER).await.unwrap().is_some());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn expired_cancel_lease_fails_as_unconfirmed_and_is_never_reclaimed() {
    let db = Db::new().await;
    let store = db.store();
    let run = store.enqueue(&db.client, new_run("cancel-crash", json!({"x":1}))).await.unwrap();
    let lease = store.claim(&db.client, "worker-a").await.unwrap().unwrap();
    assert_eq!(store.cancel(&db.client, &run.id, OWNER).await.unwrap().unwrap().state, "CANCEL_REQUESTED");
    db.client
        .execute(&format!("UPDATE {}.aip_jobs SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1", db.schema), &[&lease.run_id])
        .await
        .unwrap();

    assert_eq!(store.claim(&db.client, "worker-b").await.unwrap(), None);
    let state = store.status(&db.client, &run.id, OWNER).await.unwrap().unwrap();
    assert_eq!(state.state, "FAILED");
    assert_eq!(state.error.as_deref(), Some("CANCELLATION_UNCONFIRMED"));
    assert!(!store.acknowledge_cancel(&db.client, &lease).await.unwrap());
}

#[tokio::test]
async fn initial_delay_defers_first_claim() {
    let db = Db::new().await;
    let store = db.store();
    let mut run = new_run("delayed", json!({"x":1}));
    run.delay_ms = 200;
    store.enqueue(&db.client, run).await.unwrap();
    assert_eq!(store.claim(&db.client, "worker-a").await.unwrap(), None);
    tokio::time::sleep(std::time::Duration::from_millis(220)).await;
    assert!(store.claim(&db.client, "worker-a").await.unwrap().is_some());
}

#[tokio::test]
async fn permanent_failure_is_a_terminal_lease_cas() {
    let db = Db::new().await;
    let store = db.store();
    let run = store.enqueue(&db.client, new_run("permanent", json!({"x":1}))).await.unwrap();
    let lease = store.claim(&db.client, "worker-a").await.unwrap().unwrap();
    assert!(store.check_lease(&db.client, &lease).await.unwrap());
    assert!(store.fail(&db.client, &lease, "OUTPUT_INVALID").await.unwrap());
    assert!(!store.check_lease(&db.client, &lease).await.unwrap());
    let status = store.status(&db.client, &run.id, "https://issuer.example|subject-1").await.unwrap().unwrap();
    assert_eq!(status.state, "FAILED");
    assert_eq!(status.operation, "normalize");
    assert_eq!(status.error.as_deref(), Some("OUTPUT_INVALID"));
}

#[tokio::test]
async fn payload_and_budget_limits_are_enforced() {
    let db = Db::new().await;
    let store = db.store();
    let oversized = new_run("large", json!({"data":"x".repeat((1 << 20) + 1)}));
    assert_eq!(store.enqueue(&db.client, oversized).await.unwrap_err(), Error::PayloadTooLarge);

    let mut invalid_budget = new_run("budget", json!({"x":1}));
    invalid_budget.timeout_ms = 24 * 60 * 60 * 1000 + 1;
    assert_eq!(store.enqueue(&db.client, invalid_budget).await.unwrap_err(), Error::InvalidRun);
}

#[tokio::test]
async fn oversized_result_is_rejected_without_finishing_lease() {
    let db = Db::new().await;
    let store = db.store();
    let run = store.enqueue(&db.client, new_run("large-result", json!({"x":1}))).await.unwrap();
    let lease = store.claim(&db.client, "worker-a").await.unwrap().unwrap();
    let output = json!({"data":"x".repeat((1 << 20) + 1)});
    assert_eq!(store.finish(&db.client, &lease, output).await.unwrap_err(), Error::PayloadTooLarge);
    assert!(store.check_lease(&db.client, &lease).await.unwrap());
    assert_eq!(store.status(&db.client, &run.id, "https://issuer.example|subject-1").await.unwrap().unwrap().state, "RUNNING");
}

#[tokio::test]
async fn operator_lists_without_payloads_and_redrives_a_failed_run_as_the_same_fenced_run() {
    let db = Db::new().await;
    let store = db.store();
    let mut input = new_run("dead", json!({"secret":"payload"}));
    input.max_attempts = 1;
    let run = store.enqueue(&db.client, input).await.unwrap();
    store.enqueue(&db.client, new_run("alive", json!({"x":1}))).await.unwrap();
    let first = store.claim(&db.client, "worker-a").await.unwrap().unwrap();
    assert_eq!(first.run_id.to_string(), run.id);
    assert!(store.fail(&db.client, &first, "EXTENSION_ERROR").await.unwrap());

    let failed = store.list(&db.client, Some("FAILED"), None, 50).await.unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!((failed[0].id.as_str(), failed[0].error.as_deref(), failed[0].redrivable), (run.id.as_str(), Some("EXTENSION_ERROR"), true));
    assert!(!serde_json::to_string(&failed).unwrap().contains("payload"), "operator listing must not expose job input");
    assert_eq!(store.list(&db.client, None, Some("normalize"), 50).await.unwrap().len(), 2);
    assert!(store.list(&db.client, Some("DONE"), None, 50).await.is_err());

    assert_eq!(store.redrive(&db.client, &run.id, "deployment-b").await.unwrap(), Redrive::DeploymentChanged);
    assert_eq!(store.redrive(&db.client, &uuid_like(), "deployment-a").await.unwrap(), Redrive::NotFound);
    assert_eq!(store.redrive(&db.client, &run.id, "deployment-a").await.unwrap(), Redrive::Queued);
    assert_eq!(store.redrive(&db.client, &run.id, "deployment-a").await.unwrap(), Redrive::NotFailed);
    let status = store.status(&db.client, &run.id, OWNER).await.unwrap().unwrap();
    assert_eq!((status.state.as_str(), status.attempt, status.error), ("QUEUED", 0, None));

    // The redriven run is claimable again with a newer epoch, so the earlier lease can no longer settle it.
    let mut again = None;
    for _ in 0..2 {
        let lease = store.claim(&db.client, "worker-b").await.unwrap().unwrap();
        if lease.run_id == first.run_id {
            again = Some(lease);
            break;
        }
    }
    let again = again.expect("redriven run claimed");
    assert!(again.epoch > first.epoch);
    assert_eq!(again.input, json!({"secret":"payload"}));
    assert!(!store.finish(&db.client, &first, json!({"stale":true})).await.unwrap());
    assert!(store.finish(&db.client, &again, json!({"ok":true})).await.unwrap());

    // Once the payload is collected, the run is a tombstone and cannot be redriven.
    let mut gone = new_run("gone", json!({"x":2}));
    gone.max_attempts = 1;
    gone.ttl_ms = 1;
    let gone = store.enqueue(&db.client, gone).await.unwrap();
    let lease = store.claim(&db.client, "worker-c").await.unwrap().unwrap();
    assert_eq!(lease.run_id.to_string(), gone.id);
    assert!(store.fail(&db.client, &lease, "EXTENSION_ERROR").await.unwrap());
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    store.expire_payloads(&db.client).await.unwrap();
    assert_eq!(store.redrive(&db.client, &gone.id, "deployment-a").await.unwrap(), Redrive::Expired);
}

fn uuid_like() -> String {
    "00000000-0000-4000-8000-000000000000".into()
}
