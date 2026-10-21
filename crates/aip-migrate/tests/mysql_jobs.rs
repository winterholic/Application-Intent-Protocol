#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.1.3: the durable job store over a real MySQL 8.4: idempotent enqueue, claim by `SKIP LOCKED`, leases with fencing epochs,
//! retry and deadlines, cancel, payload expiry, operator list and redrive.
//! Run with `-- --ignored` against a run-owned container (`scripts/mysql-live-fixture.sh`).
use aip_execution::{Error, NewRun, Redrive};
use aip_migrate::mysql_driver::{MysqlDriver, mysql_async::prelude::Queryable};
use aip_migrate::mysql_jobs::MysqlJobStore;
use aip_migrate::mysql_outbox::{MysqlOutboxRelay, RelayError};
use serde_json::json;

const META: &str = "aip_t_jobs_meta";
const SOURCE: &str = "aip_t_jobs_src";
const FACTS: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn run(key: &str) -> NewRun {
    NewRun {
        owner: "owner-1".into(),
        actor: 7,
        grant: json!({"issuer": "https://issuer.example", "subject": "s"}),
        operation: "Order.settle".into(),
        contract: "c".repeat(64),
        deployment: "d".repeat(64),
        implementation: "i".repeat(64),
        input: json!({"order": 1, "note": "한글"}),
        key: key.into(),
        timeout_ms: 5_000,
        max_attempts: 3,
        ttl_ms: 60_000,
        delay_ms: 0,
        trace_id: Some("0af7651916cd43dd8448eb211c80319c".into()),
    }
}

/// The tests share one schema pair, so they must not run at the same time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Harness {
    driver: MysqlDriver,
    store: MysqlJobStore,
    _serial: tokio::sync::MutexGuard<'static, ()>,
    is_mariadb: bool,
}

impl Harness {
    async fn new() -> Self {
        let serial = SERIAL.lock().await;
        // The job store runs on MySQL 8.4 and on MariaDB 11.4: `AIP_TEST_JOBS_URL` picks the engine, MySQL by default.
        let var = if std::env::var("AIP_TEST_JOBS_URL").is_ok() { "AIP_TEST_JOBS_URL" } else { "AIP_TEST_MYSQL_URL" };
        let url = std::env::var(var).unwrap_or_else(|_| panic!("set {var} to a run-owned MySQL 8.4 or MariaDB 11.4 container"));
        aip_test_support::validate_mysql_admin_url(&url).unwrap_or_else(|reason| panic!("{var} is unsafe: {reason}"));
        let driver = MysqlDriver::connect(&url).await.expect("admin connection");
        aip_migrate::mysql_backend::check_job_store_profile(&driver).await.expect("the server runs the job store");
        let mut setup = driver.pool().get_conn().await.unwrap();
        for schema in [META, SOURCE] {
            setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
        }
        setup.query_drop(format!("CREATE DATABASE `{SOURCE}`")).await.unwrap();
        setup.query_drop(format!("CREATE TABLE `{SOURCE}`.`t` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
        drop(setup);
        let store = MysqlJobStore::new(META).unwrap();
        driver.ensure_meta_schema_bootstrap(META, SOURCE, FACTS).await.expect("meta bootstrap creates the job tables");
        Self {
            driver,
            store: if url.starts_with("mariadb://") { store.for_mariadb() } else { store },
            _serial: serial,
            is_mariadb: url.starts_with("mariadb://"),
        }
    }

    async fn conn(&self) -> aip_migrate::mysql_driver::mysql_async::Conn {
        self.driver.pool().get_conn().await.unwrap()
    }

    async fn sql(&self, sql: String) {
        self.conn().await.query_drop(sql).await.unwrap();
    }

    async fn cell(&self, sql: String) -> Option<String> {
        self.conn().await.query_first::<Option<String>, _>(sql).await.unwrap().flatten()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_job_store_enqueue_claim_lease_fencing_and_lifecycle() {
    let h = Harness::new().await;
    let mut c = h.conn().await;
    let jobs = format!("`{META}`.`aip_jobs`");

    // Idempotent enqueue: one run per key; a different request under the key is a conflict; retry policy is not part of the request.
    let first = h.store.enqueue(&mut c, run("k1")).await.unwrap();
    assert_eq!((first.state.as_str(), first.attempt), ("QUEUED", 0));
    assert_eq!(h.store.enqueue(&mut c, run("k1")).await.unwrap().id, first.id, "same key, same run");
    let mut policy = run("k1");
    policy.max_attempts = 5;
    assert_eq!(h.store.enqueue(&mut c, policy).await.unwrap().id, first.id, "max_attempts is server policy, not request");
    let mut other_input = run("k1");
    other_input.input = json!({"order": 2});
    assert_eq!(h.store.enqueue(&mut c, other_input).await.unwrap_err(), Error::IdempotencyConflict);
    let mut other_owner = run("k1");
    other_owner.owner = "owner-2".into();
    assert_ne!(h.store.enqueue(&mut c, other_owner).await.unwrap().id, first.id, "the key is per owner");
    for bad in [
        NewRun { timeout_ms: 0, ..run("bad") },
        NewRun { max_attempts: 11, ..run("bad") },
        NewRun { actor: 0, ..run("bad") },
        NewRun { key: String::new(), ..run("bad") },
    ] {
        assert_eq!(h.store.enqueue(&mut c, bad).await.unwrap_err(), Error::InvalidRun);
    }

    // Claim: a lease with a fencing epoch; the payload, grant and trace id come back intact. Nothing else is runnable now.
    let lease = h.store.claim(&mut c, "worker-a").await.unwrap().expect("a queued run");
    assert_eq!((lease.epoch, lease.attempt, lease.worker.as_str(), lease.owner.as_str()), (1, 1, "worker-a", "owner-1"));
    assert_eq!(lease.input, json!({"order": 1, "note": "한글"}));
    assert_eq!(lease.grant["subject"], "s");
    assert_eq!(lease.trace_id.as_deref(), Some("0af7651916cd43dd8448eb211c80319c"));
    assert_eq!((lease.operation.as_str(), lease.actor), ("Order.settle", 7));
    assert!(h.store.heartbeat(&mut c, &lease).await.unwrap() && h.store.check_lease(&mut c, &lease).await.unwrap());
    // The other owner's run is the only one left; claim it and put it back to prove "nothing else" below.
    let other = h.store.claim(&mut c, "worker-b").await.unwrap().expect("owner-2's run");
    assert!(h.store.finish(&mut c, &other, json!({})).await.unwrap());
    assert!(h.store.claim(&mut c, "worker-c").await.unwrap().is_none(), "nothing runnable while the lease is live");

    // The lease expires and another worker takes the run over; the first worker is fenced out by the epoch.
    h.sql(format!("UPDATE {jobs} SET lease_until = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND) WHERE id = '{}'", first.id)).await;
    let taken = h.store.claim(&mut c, "worker-b").await.unwrap().expect("the expired lease is reclaimed");
    assert_eq!((taken.run_id, taken.epoch, taken.attempt, taken.worker.as_str()), (lease.run_id, 2, 2, "worker-b"));
    assert!(!h.store.heartbeat(&mut c, &lease).await.unwrap(), "stale epoch cannot heartbeat");
    assert!(!h.store.finish(&mut c, &lease, json!({"late": true})).await.unwrap(), "stale epoch cannot finish");
    assert!(!h.store.retry(&mut c, &lease, "late").await.unwrap() && !h.store.fail(&mut c, &lease, "late").await.unwrap());
    assert!(!h.store.check_lease(&mut c, &lease).await.unwrap());
    assert!(h.store.finish(&mut c, &taken, json!({"settled": 3})).await.unwrap());
    let done = h.store.status(&mut c, &first.id, "owner-1").await.unwrap().unwrap();
    assert_eq!((done.state.as_str(), done.attempt, done.output.clone()), ("SUCCEEDED", 2, Some(json!({"settled": 3}))));
    assert_eq!(h.store.result(&mut c, &first.id, "owner-1").await.unwrap().unwrap().output, json!({"settled": 3}));
    assert!(h.store.status(&mut c, &first.id, "owner-2").await.unwrap().is_none(), "another owner sees nothing");
    assert!(h.store.result(&mut c, &first.id, "owner-2").await.unwrap().is_none());

    // Retry with backoff, then the attempt limit ends the run.
    let flaky = h.store.enqueue(&mut c, NewRun { max_attempts: 2, ..run("flaky") }).await.unwrap();
    let a = h.store.claim(&mut c, "w").await.unwrap().unwrap();
    assert!(h.store.retry(&mut c, &a, "boom").await.unwrap());
    assert_eq!(h.store.status(&mut c, &flaky.id, "owner-1").await.unwrap().unwrap().state, "QUEUED");
    assert!(h.store.claim(&mut c, "w").await.unwrap().is_none(), "the retry waits out its backoff");
    h.sql(format!("UPDATE {jobs} SET next_run_at = UTC_TIMESTAMP(6) WHERE id = '{}'", flaky.id)).await;
    let b = h.store.claim(&mut c, "w").await.unwrap().unwrap();
    assert_eq!((b.attempt, b.epoch), (2, 2));
    assert!(h.store.retry(&mut c, &b, "boom again").await.unwrap());
    let ended = h.store.status(&mut c, &flaky.id, "owner-1").await.unwrap().unwrap();
    assert_eq!((ended.state.as_str(), ended.error.as_deref()), ("FAILED", Some("boom again")), "attempts exhausted");

    // A wait the extension asks for replaces the default backoff and extends the run's deadline by the difference.
    let waiting = h.store.enqueue(&mut c, run("waiting")).await.unwrap();
    let w = h.store.claim(&mut c, "w").await.unwrap().unwrap();
    let before = h
        .cell(format!("SELECT TIMESTAMPDIFF(MICROSECOND, UTC_TIMESTAMP(6), run_deadline) / 1000 FROM {jobs} WHERE id = '{}'", waiting.id))
        .await
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!(h.store.retry_after(&mut c, &w, "provider asked", Some(120_000)).await.unwrap());
    let wait_ms = h
        .cell(format!("SELECT TIMESTAMPDIFF(MICROSECOND, UTC_TIMESTAMP(6), next_run_at) / 1000 FROM {jobs} WHERE id = '{}'", waiting.id))
        .await
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!((115_000.0..=120_000.0).contains(&wait_ms), "requested wait of 120s: {wait_ms}");
    let after = h
        .cell(format!("SELECT TIMESTAMPDIFF(MICROSECOND, UTC_TIMESTAMP(6), run_deadline) / 1000 FROM {jobs} WHERE id = '{}'", waiting.id))
        .await
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!(after > before + 100_000.0, "the deadline grew by what the wait added over the default: {before} -> {after}");

    // A run past its deadline ends as `deadline_exceeded` the next time anyone claims; a delayed run is not claimable early.
    let late = h.store.enqueue(&mut c, run("late")).await.unwrap();
    h.sql(format!("UPDATE {jobs} SET run_deadline = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND) WHERE id = '{}'", late.id)).await;
    let delayed = h.store.enqueue(&mut c, NewRun { delay_ms: 60_000, ..run("delayed") }).await.unwrap();
    assert!(h.store.claim(&mut c, "w").await.unwrap().is_none());
    let late_status = h.store.status(&mut c, &late.id, "owner-1").await.unwrap().unwrap();
    assert_eq!((late_status.state.as_str(), late_status.error.as_deref()), ("FAILED", Some("deadline_exceeded")));
    assert_eq!(h.store.status(&mut c, &delayed.id, "owner-1").await.unwrap().unwrap().state, "QUEUED");

    // Cancel: queued cancels at once, running is asked to stop and acknowledges, a stranger cannot cancel.
    let queued = h.store.enqueue(&mut c, run("cancel-queued")).await.unwrap();
    assert!(h.store.cancel(&mut c, &queued.id, "owner-2").await.unwrap().is_none(), "not the owner");
    assert_eq!(h.store.cancel(&mut c, &queued.id, "owner-1").await.unwrap().unwrap().state, "CANCELLED");
    let running = h.store.enqueue(&mut c, run("cancel-running")).await.unwrap();
    h.sql(format!("UPDATE {jobs} SET next_run_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 1 DAY) WHERE id = '{}'", delayed.id)).await;
    let r = h.store.claim(&mut c, "w").await.unwrap().unwrap();
    assert_eq!(r.run_id.to_string(), running.id);
    assert_eq!(h.store.cancel(&mut c, &running.id, "owner-1").await.unwrap().unwrap().state, "CANCEL_REQUESTED");
    assert!(!h.store.finish(&mut c, &r, json!({})).await.unwrap(), "a cancelled run cannot succeed");
    assert!(h.store.acknowledge_cancel(&mut c, &r).await.unwrap());
    assert_eq!(h.store.status(&mut c, &running.id, "owner-1").await.unwrap().unwrap().state, "CANCELLED");

    // Redrive: a FAILED run whose payload is retained goes back to the queue as the same run, under the same deployment only.
    assert_eq!(h.store.redrive(&mut c, &queued.id, &"d".repeat(64)).await.unwrap(), Redrive::NotFailed);
    assert_eq!(h.store.redrive(&mut c, &flaky.id, &"x".repeat(64)).await.unwrap(), Redrive::DeploymentChanged);
    assert_eq!(h.store.redrive(&mut c, "00000000-0000-4000-8000-000000000000", &"d".repeat(64)).await.unwrap(), Redrive::NotFound);
    assert_eq!(h.store.redrive(&mut c, &flaky.id, &"d".repeat(64)).await.unwrap(), Redrive::Queued);
    let again = h.store.claim(&mut c, "w").await.unwrap().unwrap();
    assert_eq!((again.run_id.to_string(), again.attempt, again.epoch), (flaky.id.clone(), 1, 3), "same run, the epoch keeps growing");
    assert!(h.store.fail(&mut c, &again, "gave up").await.unwrap());

    // Operator view: no payloads, newest first, filterable; payloads are dropped after the result window.
    let failed = h.store.list(&mut c, Some("FAILED"), None, 50).await.unwrap();
    assert!(failed.iter().all(|r| r.state == "FAILED") && failed.iter().any(|r| r.id == flaky.id && r.redrivable));
    assert!(h.store.list(&mut c, None, Some("No.such"), 50).await.unwrap().is_empty());
    assert_eq!(h.store.list(&mut c, Some("NOPE"), None, 50).await.unwrap_err(), Error::InvalidRun);
    assert_eq!(h.store.list(&mut c, None, None, 501).await.unwrap_err(), Error::InvalidRun);
    h.sql(format!(
        "UPDATE {jobs} SET result_expires_at = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND) WHERE id IN ('{}', '{}')",
        first.id, flaky.id
    ))
    .await;
    let expired = h.store.expire_payloads(&mut c).await.unwrap();
    assert!(expired.contains(&first.id) && expired.contains(&flaky.id), "{expired:?}");
    assert!(h.store.result(&mut c, &first.id, "owner-1").await.unwrap().is_none(), "the result window ended");
    assert_eq!(h.cell(format!("SELECT CAST(input AS CHAR) FROM {jobs} WHERE id = '{}'", first.id)).await.as_deref(), Some("{}"));
    assert_eq!(h.store.redrive(&mut c, &flaky.id, &"d".repeat(64)).await.unwrap(), Redrive::Expired);
    // The key and the input digest survive the payload: a late redelivery converges on the same run instead of creating a new one.
    let redelivered = h.store.enqueue(&mut c, run("k1")).await.unwrap();
    assert_eq!((redelivered.id.as_str(), redelivered.output), (first.id.as_str(), None));
}

/// Claim under contention: forty runs, eight workers on separate connections, every run claimed exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_concurrent_claims_never_hand_a_run_to_two_workers() {
    let h = std::sync::Arc::new(Harness::new().await);
    let mut c = h.conn().await;
    for n in 0..40 {
        h.store.enqueue(&mut c, run(&format!("job-{n}"))).await.unwrap();
    }
    drop(c);
    let mut joins = vec![];
    for worker in 0..8 {
        let h = h.clone();
        joins.push(tokio::spawn(async move {
            let mut conn = h.conn().await;
            let mut claimed = vec![];
            while let Some(lease) = h.store.claim(&mut conn, &format!("worker-{worker}")).await.unwrap() {
                claimed.push((lease.run_id, lease.epoch));
                assert!(h.store.finish(&mut conn, &lease, json!({"by": worker})).await.unwrap());
            }
            claimed
        }));
    }
    let mut all = vec![];
    for join in joins {
        all.extend(join.await.unwrap());
    }
    let unique: std::collections::BTreeSet<_> = all.iter().map(|(id, _)| *id).collect();
    assert_eq!((all.len(), unique.len()), (40, 40), "each run was claimed once");
    assert!(all.iter().all(|(_, epoch)| *epoch == 1));
    let mut c = h.conn().await;
    let done = c.query_first::<i64, _>(format!("SELECT COUNT(*) FROM `{META}`.`aip_jobs` WHERE state = 'SUCCEEDED'")).await.unwrap().unwrap();
    assert_eq!(done, 40);
}

/// Outbox rows become jobs exactly once: a failing batch leaves nothing behind, concurrent relays never share a row, and a row that
/// was dispatched is never dispatched again.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_outbox_relay_dispatches_each_row_once_and_rolls_back_a_failed_batch() {
    let h = std::sync::Arc::new(Harness::new().await);
    for n in 0..30 {
        h.sql(format!(
            "INSERT INTO `{META}`.`aip_outbox` (topic, recipient_id, source, source_id) VALUES ('note.edited', '{n}', 'Note', '{}')",
            100 + n
        ))
        .await;
    }
    h.sql(format!("INSERT INTO `{META}`.`aip_outbox` (topic, recipient_id, source, source_id) VALUES ('other.topic', '1', 'Note', '1')")).await;
    let relay = MysqlOutboxRelay::new(META).unwrap();
    let relay = if h.is_mariadb { relay.for_mariadb() } else { relay };
    let count = |sql: &'static str| {
        let h = h.clone();
        async move { h.cell(sql.replace("{META}", META)).await.unwrap().parse::<i64>().unwrap() }
    };
    let build = |event: &aip_migrate::mysql_outbox::OutboxEvent| -> Result<NewRun, String> {
        let mut new = run(&event.job_key());
        new.input =
            json!({"event": {"id": event.id.to_string(), "topic": event.topic, "sourceId": event.source_id, "recipientId": event.recipient_id}});
        Ok(new)
    };

    // A batch the caller refuses halfway undoes everything: no job, no receipt, every row still pending.
    let mut conn = h.conn().await;
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let refused = relay
        .relay_once(&mut conn, "note.edited", 10, |event| {
            if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 3 { Err("provider unavailable".to_string()) } else { build(event) }
        })
        .await;
    assert_eq!(refused, Err(RelayError::Rejected("provider unavailable".to_string())));
    assert_eq!((count("SELECT COUNT(*) FROM `{META}`.`aip_jobs`").await, count("SELECT COUNT(*) FROM `{META}`.`aip_outbox_dispatch`").await), (0, 0));
    drop(conn);

    // Four relays at once, small batches: every row is dispatched by exactly one of them.
    let mut joins = vec![];
    for _ in 0..4 {
        let (h, relay) = (h.clone(), relay.clone());
        joins.push(tokio::spawn(async move {
            let mut conn = h.conn().await;
            let mut total = 0;
            loop {
                let n = relay
                    .relay_once(&mut conn, "note.edited", 5, |event| -> Result<NewRun, String> {
                        let mut new = run(&event.job_key());
                        new.input = json!({"event": {"id": event.id.to_string(), "topic": event.topic}});
                        Ok(new)
                    })
                    .await
                    .unwrap();
                if n == 0 {
                    return total;
                }
                total += n;
            }
        }));
    }
    let mut dispatched = 0;
    for join in joins {
        dispatched += join.await.unwrap();
    }
    assert_eq!(dispatched, 30);
    assert_eq!(count("SELECT COUNT(*) FROM `{META}`.`aip_jobs`").await, 30);
    assert_eq!(count("SELECT COUNT(*) FROM `{META}`.`aip_outbox_dispatch`").await, 30);
    assert_eq!(count("SELECT COUNT(DISTINCT job_id) FROM `{META}`.`aip_outbox_dispatch`").await, 30, "one job per row");
    assert_eq!(count("SELECT COUNT(*) FROM `{META}`.`aip_outbox` o WHERE o.topic = 'other.topic' AND NOT EXISTS (SELECT 1 FROM `{META}`.`aip_outbox_dispatch` d WHERE d.outbox_id = o.id)").await, 1, "other topics are untouched");

    // Nothing is dispatched twice, and the trace and key of a job come from its row.
    let mut conn = h.conn().await;
    assert_eq!(relay.relay_once(&mut conn, "note.edited", 50, build).await.unwrap(), 0);
    assert_eq!(count("SELECT COUNT(*) FROM `{META}`.`aip_jobs`").await, 30);
    let key = h.cell(format!("SELECT idempotency_key FROM `{META}`.`aip_jobs` ORDER BY created_at LIMIT 1")).await.unwrap();
    assert!(key.starts_with("outbox:"), "{key}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_event_streams_publish_once_replay_in_order_and_expire_explicitly() {
    use aip_execution::events::Error as EventError;
    use aip_migrate::mysql_driver::mysql_async::TxOpts;
    use aip_migrate::mysql_events::MysqlEventStore;
    let harness = Harness::new().await;
    let store = if harness.is_mariadb { MysqlEventStore::new(META).unwrap().for_mariadb() } else { MysqlEventStore::new(META).unwrap() };
    let mut conn = harness.conn().await;

    // Idempotent publish: the same key and payload converges on one cursor, another payload under the key is a conflict.
    assert_eq!(store.publish_in(&mut conn, "owner-a", "audit", "k1", &json!({"n": 1, "text": "한글"})).await.unwrap(), "1");
    assert_eq!(store.publish_in(&mut conn, "owner-a", "audit", "k1", &json!({"n": 1, "text": "한글"})).await.unwrap(), "1");
    assert_eq!(store.publish_in(&mut conn, "owner-a", "audit", "k1", &json!({"n": 2})).await.unwrap_err(), EventError::IdempotencyConflict);
    assert_eq!(store.publish_in(&mut conn, "owner-a", "audit", "k2", &json!({"n": 2})).await.unwrap(), "2");
    // Streams are scoped by owner and name.
    assert_eq!(store.publish_in(&mut conn, "owner-b", "audit", "k1", &json!({"n": 9})).await.unwrap(), "1");
    assert_eq!(store.publish_in(&mut conn, "owner-a", "other", "k1", &json!({"n": 9})).await.unwrap(), "1");
    assert_eq!(store.publish_in(&mut conn, "owner-a", "bad stream", "k", &json!({})).await.unwrap_err(), EventError::InvalidEvent);
    assert_eq!(
        store.publish_in(&mut conn, "owner-a", "audit", "big", &json!({"blob": "x".repeat(70_000)})).await.unwrap_err(),
        EventError::InvalidEvent
    );

    // Replay is ordered and bounded; the next cursor resumes where the page ended.
    let page = store.scan(&mut conn, "owner-a", "audit", "0", 1).await.unwrap();
    assert_eq!((page.events.len(), page.next_cursor.as_str(), page.events[0].payload.clone()), (1, "1", json!({"n": 1, "text": "한글"})));
    let page = store.scan(&mut conn, "owner-a", "audit", &page.next_cursor, 10).await.unwrap();
    assert_eq!((page.events.len(), page.next_cursor.as_str(), page.events[0].payload.clone()), (1, "2", json!({"n": 2})));
    let page = store.scan(&mut conn, "owner-a", "audit", "2", 10).await.unwrap();
    assert_eq!((page.events.len(), page.next_cursor.as_str()), (0, "2"));
    assert_eq!(store.scan(&mut conn, "owner-a", "audit", "7", 10).await.unwrap_err(), EventError::InvalidEvent, "a cursor beyond the head");
    assert_eq!(store.scan(&mut conn, "owner-a", "missing", "0", 10).await.unwrap().events.len(), 0);
    assert_eq!(store.scan(&mut conn, "owner-b", "audit", "0", 10).await.unwrap().events.len(), 1, "another owner's stream is its own");

    // A pruned window is an explicit error, never a silent gap; the idempotency ledger outlives the payloads.
    store.prune(&mut conn, "owner-a", "audit", "1").await.unwrap();
    assert_eq!(store.scan(&mut conn, "owner-a", "audit", "0", 10).await.unwrap_err(), EventError::CursorExpired);
    assert_eq!(store.scan(&mut conn, "owner-a", "audit", "1", 10).await.unwrap().events.len(), 1);
    assert_eq!(store.publish_in(&mut conn, "owner-a", "audit", "k1", &json!({"n": 1, "text": "한글"})).await.unwrap(), "1");
    drop(conn);

    // Concurrent publishers are serialized by the stream row: cursors are unique and gapless, and a key sent by every task once
    // gets one cursor.
    let mut tasks = vec![];
    for task in 0..8 {
        let (driver, store) = (harness.driver.clone(), store.clone());
        tasks.push(tokio::spawn(async move {
            let mut cursors = vec![];
            for n in 0..10 {
                let mut conn = driver.pool().get_conn().await.unwrap();
                let mut tx = conn.start_transaction(TxOpts::default()).await.unwrap();
                cursors.push(store.publish_in(&mut tx, "owner-c", "load", &format!("t{task}-{n}"), &json!({"task": task, "n": n})).await.unwrap());
                let shared = store.publish_in(&mut tx, "owner-c", "load", "shared", &json!({"shared": true})).await.unwrap();
                tx.commit().await.unwrap();
                cursors.push(shared);
            }
            cursors
        }));
    }
    let mut own = std::collections::BTreeSet::new();
    let mut shared = std::collections::BTreeSet::new();
    for task in tasks {
        let cursors = task.await.unwrap();
        for (index, cursor) in cursors.into_iter().enumerate() {
            if index % 2 == 0 {
                own.insert(cursor.parse::<i64>().unwrap());
            } else {
                shared.insert(cursor.parse::<i64>().unwrap());
            }
        }
    }
    assert_eq!(own.len(), 80, "every distinct key got its own cursor");
    assert_eq!(shared.len(), 1, "a repeated key converges on one cursor");
    let mut conn = harness.conn().await;
    let page = store.scan(&mut conn, "owner-c", "load", "0", 1000).await.unwrap();
    let cursors: Vec<i64> = page.events.iter().map(|event| event.cursor.parse().unwrap()).collect();
    assert_eq!(cursors, (1..=81).collect::<Vec<i64>>(), "80 distinct events plus the shared one, in order and without a gap");
}

fn message(owner: &str, queue: &str, key: &str, payload: serde_json::Value) -> aip_execution::messages::NewMessage {
    aip_execution::messages::NewMessage {
        owner: owner.into(),
        queue: queue.into(),
        key: key.into(),
        payload,
        max_deliveries: 2,
        ttl_ms: 60_000,
        delay_ms: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_message_queue_leases_retries_dead_letters_and_expires() {
    use aip_execution::messages::Error as MessageError;
    use aip_migrate::mysql_messages::MysqlMessageStore;
    let harness = Harness::new().await;
    let store = if harness.is_mariadb { MysqlMessageStore::new(META).unwrap().for_mariadb() } else { MysqlMessageStore::new(META).unwrap() };
    let mut conn = harness.conn().await;

    // Idempotent enqueue: the same key and payload is the same message, another payload under the key is a conflict.
    let first = store.enqueue(&mut conn, message("owner-a", "orders", "k1", json!({"n": 1, "text": "한글"}))).await.unwrap();
    assert_eq!((first.state.as_str(), first.deliveries, first.payload.clone()), ("READY", 0, json!({"n": 1, "text": "한글"})));
    let again = store.enqueue(&mut conn, message("owner-a", "orders", "k1", json!({"n": 1, "text": "한글"}))).await.unwrap();
    assert_eq!(again.id, first.id);
    assert_eq!(store.enqueue(&mut conn, message("owner-a", "orders", "k1", json!({"n": 2}))).await.unwrap_err(), MessageError::IdempotencyConflict);
    // Queues and owners are separate scopes.
    let other_queue = store.enqueue(&mut conn, message("owner-a", "billing", "k1", json!({"n": 9}))).await.unwrap();
    let other_owner = store.enqueue(&mut conn, message("owner-b", "orders", "k1", json!({"n": 9}))).await.unwrap();
    assert!(other_queue.id != first.id && other_owner.id != first.id);
    assert_eq!(store.enqueue(&mut conn, message("owner-a", "bad queue", "k", json!({}))).await.unwrap_err(), MessageError::InvalidMessage);
    assert_eq!(
        store.enqueue(&mut conn, message("owner-a", "orders", "big", json!({"blob": "x".repeat(600_000)}))).await.unwrap_err(),
        MessageError::PayloadTooLarge
    );
    let mut delayed = message("owner-a", "orders", "later", json!({"later": true}));
    delayed.delay_ms = 60_000;
    store.enqueue(&mut conn, delayed).await.unwrap();

    // A claim takes the oldest ready message and nothing else; the delayed one is not ready yet.
    let lease = store.claim(&mut conn, "owner-a", "orders", "worker-1", 30_000).await.unwrap().unwrap();
    assert_eq!((lease.id.as_str(), lease.deliveries, lease.epoch, lease.worker.as_str()), (first.id.as_str(), 1, 1, "worker-1"));
    assert!(store.claim(&mut conn, "owner-a", "orders", "worker-2", 30_000).await.unwrap().is_none());
    // Only the live lease commits: a forged token or another worker cannot ack.
    let mut forged = lease.clone();
    forged.token = uuid::Uuid::new_v4().to_string();
    assert!(!store.ack(&mut conn, &forged).await.unwrap());
    let mut stranger = lease.clone();
    stranger.worker = "worker-2".into();
    assert!(!store.ack(&mut conn, &stranger).await.unwrap());
    assert!(store.ack(&mut conn, &lease).await.unwrap());
    assert!(!store.ack(&mut conn, &lease).await.unwrap(), "a message is acknowledged once");

    // A nack retries until the delivery budget is spent, then the message is dead and listed.
    store.enqueue(&mut conn, message("owner-a", "retry", "r1", json!({"r": 1}))).await.unwrap();
    let lease = store.claim(&mut conn, "owner-a", "retry", "worker-1", 30_000).await.unwrap().unwrap();
    assert!(store.nack_after(&mut conn, &lease, "TEMPORARY", 0).await.unwrap());
    let lease = store.claim(&mut conn, "owner-a", "retry", "worker-1", 30_000).await.unwrap().unwrap();
    assert_eq!((lease.deliveries, lease.epoch), (2, 2));
    assert!(store.nack_after(&mut conn, &lease, "TEMPORARY", 0).await.unwrap());
    assert!(store.claim(&mut conn, "owner-a", "retry", "worker-1", 30_000).await.unwrap().is_none());
    let dead = store.dead_letters_after(&mut conn, "owner-a", "retry", None, 10).await.unwrap();
    assert_eq!((dead.items.len(), dead.items[0].last_error.clone(), dead.next_cursor.clone()), (1, Some("TEMPORARY".into()), None));
    assert!(store.dead_letters_after(&mut conn, "owner-b", "retry", None, 10).await.unwrap().items.is_empty(), "the dead-letter set is owner-scoped");
    // Redrive reopens it with a fresh budget and the original payload.
    assert!(store.redrive(&mut conn, "owner-a", "retry", &dead.items[0].id, 3).await.unwrap());
    assert!(!store.redrive(&mut conn, "owner-a", "retry", &dead.items[0].id, 3).await.unwrap(), "only a dead message can be redriven");
    let lease = store.claim(&mut conn, "owner-a", "retry", "worker-1", 30_000).await.unwrap().unwrap();
    assert_eq!((lease.deliveries, lease.max_deliveries, lease.payload.clone()), (1, 3, json!({"r": 1})));

    // An expired lease returns the message to the queue, or kills it when the budget is gone; the old lease cannot ack.
    store.enqueue(&mut conn, message("owner-a", "lapse", "l1", json!({"l": 1}))).await.unwrap();
    let old = store.claim(&mut conn, "owner-a", "lapse", "worker-1", 100).await.unwrap().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(!store.ack(&mut conn, &old).await.unwrap(), "an expired lease cannot commit");
    let fresh = store.claim(&mut conn, "owner-a", "lapse", "worker-2", 30_000).await.unwrap().unwrap();
    assert_eq!((fresh.deliveries, fresh.epoch), (2, 2));
    assert!(!store.ack(&mut conn, &old).await.unwrap(), "a stale epoch cannot commit after the takeover");
    harness
        .sql(format!("UPDATE `{META}`.aip_message_queue SET lease_until = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND) WHERE id = '{}'", fresh.id))
        .await;
    assert!(store.claim(&mut conn, "owner-a", "lapse", "worker-3", 30_000).await.unwrap().is_none(), "the budget is spent, so it goes dead");
    let dead = store.dead_letters_after(&mut conn, "owner-a", "lapse", None, 10).await.unwrap();
    assert_eq!(dead.items[0].last_error.as_deref(), Some("LEASE_EXPIRED"));

    // Dead letters page by id.
    for n in 0..5 {
        let key = format!("d{n}");
        let mut dying = message("owner-a", "pages", &key, json!({"n": n}));
        dying.max_deliveries = 1;
        store.enqueue(&mut conn, dying).await.unwrap();
        let lease = store.claim(&mut conn, "owner-a", "pages", "w", 30_000).await.unwrap().unwrap();
        assert!(store.nack_after(&mut conn, &lease, "X", 0).await.unwrap());
    }
    let page = store.dead_letters_after(&mut conn, "owner-a", "pages", None, 2).await.unwrap();
    assert_eq!(page.items.len(), 2);
    let cursor = page.next_cursor.clone().expect("more pages");
    let rest = store.dead_letters_after(&mut conn, "owner-a", "pages", Some(&cursor), 10).await.unwrap();
    assert_eq!((rest.items.len(), rest.next_cursor), (3, None));
    assert!(rest.items.iter().all(|item| item.id > cursor));

    // Expired rows are deleted in bounded batches.
    harness
        .sql(format!("UPDATE `{META}`.aip_message_queue SET expires_at = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND) WHERE queue = 'pages'"))
        .await;
    assert_eq!(store.cleanup_expired(&mut conn, 3).await.unwrap().len(), 3);
    assert_eq!(store.cleanup_expired(&mut conn, 10).await.unwrap().len(), 2);
    assert!(store.cleanup_expired(&mut conn, 10).await.unwrap().is_empty());

    // Concurrent claimers never take one message twice and together take all of them.
    for n in 0..40 {
        store.enqueue(&mut conn, message("owner-c", "load", &format!("m{n}"), json!({"n": n}))).await.unwrap();
    }
    drop(conn);
    let mut tasks = vec![];
    for task in 0..8 {
        let (driver, store) = (harness.driver.clone(), store.clone());
        tasks.push(tokio::spawn(async move {
            let mut conn = driver.pool().get_conn().await.unwrap();
            let mut taken = vec![];
            while let Some(lease) = store.claim(&mut conn, "owner-c", "load", &format!("worker-{task}"), 30_000).await.unwrap() {
                assert_eq!(lease.deliveries, 1);
                assert!(store.ack(&mut conn, &lease).await.unwrap());
                taken.push(lease.id);
            }
            taken
        }));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut total = 0;
    for task in tasks {
        for id in task.await.unwrap() {
            total += 1;
            assert!(seen.insert(id), "a message was handed to two consumers");
        }
    }
    assert_eq!((total, seen.len()), (40, 40));
}
