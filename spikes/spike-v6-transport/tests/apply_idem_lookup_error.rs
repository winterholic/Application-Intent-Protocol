//! Concurrency hardening (2026-11-21): `/apply` must not discard an error from its idempotency key lookup.
//! The lookup used `if let Ok(Some(row))`, so a serialization failure or deadlock there fell through to the write path on a
//! transaction PostgreSQL had already aborted. The caller saw a retry-less `WIRE.INTERNAL` instead of the retryable
//! `WIRE.CONFLICT`. The journal is replaced by a view whose filter raises a chosen SQLSTATE, which injects the failure at
//! the lookup and nowhere else (a superuser would bypass row-level security, so a policy cannot be used here).
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, plan::Caller, sqlgen};

const DB: &str = aip_test_support::test_database_url!();
const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Counter {
  fields { id: Id; owner: Member; hits: Int(0..1000000) }
  rows read when true
  transition bump { from true; to hits = hits + 1; allow owner = actor }
  expose apply bump { target id; bulk maxRows 1 }
}
"#;

async fn hits(db: &tokio_postgres::Client, schema: &str) -> i64 {
    db.query_one(format!("SELECT hits FROM {schema}.counter WHERE id=7").as_str(), &[]).await.unwrap().get(0)
}

async fn journal(db: &tokio_postgres::Client, schema: &str) -> i64 {
    db.query_one(format!("SELECT count(*) FROM {schema}.aip_idem_journal").as_str(), &[]).await.unwrap().get(0)
}

async fn apply(db: &mut tokio_postgres::Client, facts: &Value, key: &str) -> Value {
    let body = json!({"key":key,"request":{"apply":"Counter.bump","target":{"ids":["7"]}}});
    let caller = Caller { actor_id: Some(1), now: "2026-11-21T00:00:00Z".into() };
    tokio::time::timeout(std::time::Duration::from_secs(10), spike_v6_transport::handle(db, facts, "/apply", &body, &caller, IdWire::Legacy))
        .await
        .expect("/apply must answer, not hang")
}

#[tokio::test]
async fn idempotency_lookup_errors_are_retried_or_reported_and_never_run_the_write() {
    let schema = format!("aip_idem_lookup_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let facts = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert!(facts["resources"]["Counter"]["transitions"]["bump"]["isolation"].is_null(), "fixture must exercise the READ COMMITTED path");
    let setup = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        setup.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&setup).await;
    // The sequence survives the rollback of the failing transaction, so "fail the first N lookups" is deterministic. STABLE with no
    // column argument makes the filter a one-time gate that runs even when the journal is empty (a VOLATILE one runs per row).
    setup
        .batch_execute(&format!(
            "INSERT INTO {schema}.member(id) VALUES (1);
             INSERT INTO {schema}.counter(id,owner_id,hits) VALUES (7,1,0);
             ALTER TABLE {schema}.aip_idem RENAME TO aip_idem_journal;
             CREATE SEQUENCE {schema}.lookup_calls;
             CREATE TABLE {schema}.lookup_failure(sqlstate text NOT NULL, failing_calls bigint NOT NULL);
             CREATE FUNCTION {schema}.lookup_gate() RETURNS boolean LANGUAGE plpgsql STABLE AS $$
               DECLARE f record;
               BEGIN
                 SELECT * INTO f FROM {schema}.lookup_failure;
                 IF f.sqlstate IS NOT NULL AND nextval('{schema}.lookup_calls') <= f.failing_calls THEN
                   RAISE EXCEPTION 'injected idempotency lookup failure' USING ERRCODE = f.sqlstate;
                 END IF;
                 RETURN true;
               END $$;
             CREATE VIEW {schema}.aip_idem AS SELECT * FROM {schema}.aip_idem_journal WHERE {schema}.lookup_gate()"
        ))
        .await
        .unwrap();
    let inject = |state: &'static str, calls: i64| {
        format!("TRUNCATE {schema}.lookup_failure; INSERT INTO {schema}.lookup_failure VALUES ('{state}', {calls}); SELECT setval('{schema}.lookup_calls', 1, false)")
    };
    let mut db = connect_with_url(DB).await.unwrap();

    // 1. Transient failures: the first two lookups fail. The request is rolled back and retried by the server, then runs once.
    for state in ["40001", "40P01"] {
        let key = format!("transient-{state}");
        setup.batch_execute(&inject(state, 2)).await.unwrap();
        let before = hits(&setup, &schema).await;
        let outcome = apply(&mut db, &facts, &key).await;
        let after = hits(&setup, &schema).await;
        assert!(after <= before + 1, "{state}: the write ran more than once ({before} -> {after})");
        assert_eq!(outcome["ok"], true, "{state}: a transient lookup failure must be retried, not reported as {outcome} (hits {before} -> {after})");
        assert!(outcome["replayed"].is_null(), "{state}: first success must be an execution, not a replay: {outcome}");
        assert_eq!(hits(&setup, &schema).await, before + 1, "{state}: the write must run exactly once");
        let replay = apply(&mut db, &facts, &key).await;
        assert_eq!((replay["ok"].clone(), replay["replayed"].clone()), (json!(true), json!(true)), "{state}: same key must replay: {replay}");
        assert_eq!(hits(&setup, &schema).await, before + 1, "{state}: the replay must not run the write again");
    }

    // 2. A lookup that keeps failing exhausts the retries and says so with the retryable code. Nothing is written or journaled.
    for state in ["40001", "40P01"] {
        let key = format!("persistent-{state}");
        setup.batch_execute(&inject(state, 1_000)).await.unwrap();
        let (before, stored) = (hits(&setup, &schema).await, journal(&setup, &schema).await);
        let outcome = apply(&mut db, &facts, &key).await;
        assert_eq!(outcome["code"], "WIRE.CONFLICT", "{state}: the failed lookup must surface as the retryable conflict, got {outcome}");
        assert_eq!(
            (hits(&setup, &schema).await, journal(&setup, &schema).await),
            (before, stored),
            "{state}: a failed lookup must not write or journal"
        );
        // Once the failure clears, the same key runs once and then replays.
        setup.batch_execute(&format!("TRUNCATE {schema}.lookup_failure")).await.unwrap();
        let retry = apply(&mut db, &facts, &key).await;
        assert!(retry["ok"] == true && retry["replayed"].is_null(), "{state}: retry after recovery: {retry}");
        assert_eq!(hits(&setup, &schema).await, before + 1, "{state}: the recovered request must run exactly once");
        let replay = apply(&mut db, &facts, &key).await;
        assert_eq!(replay["replayed"], true, "{state}: {replay}");
        assert_eq!(hits(&setup, &schema).await, before + 1);
    }

    // 3. A lookup failure with no retry meaning (permission denied) is still an error, never a silent write.
    setup.batch_execute(&inject("42501", 1_000)).await.unwrap();
    let before = hits(&setup, &schema).await;
    let outcome = apply(&mut db, &facts, "denied").await;
    assert_eq!(outcome["ok"], false, "{outcome}");
    assert_eq!(hits(&setup, &schema).await, before, "a failed lookup must not run the write");

    setup.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
