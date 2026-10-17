//! Executed attack check for write concurrency (security audit section 6, "write concurrency").
//!
//! Real PostgreSQL, one connection per concurrent caller. The attacker's tools are racing the same request from many
//! identities, racing a transition against the one that revokes its precondition, overlapping bulk targets in opposite
//! orders, and racing a capacity limit. Each scenario repeats so a lucky interleaving does not pass it.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

const DB: &str = aip_test_support::test_database_url!();
const SPEC: &str = r#"
enum Status { OPEN, CLAIMED }
resource Member { fields { id: Id } }
actor Member
resource Team { fields { id: Id } }
limit maxClaimed on Task = atMost 3 where status = CLAIMED
resource Task {
  fields { id: Id; team: Team; owner: Member?; status: Status; attempts: Int }
  rows read when true
  transition claim { from status = OPEN; to status = CLAIMED, owner = actor; allow true }
  transition bump { from status = CLAIMED; to attempts = attempts + 1; allow owner = actor }
  transition release { from status = CLAIMED; to status = OPEN; allow owner = actor }
  expose apply claim { target id; bulk maxRows 5 }
  expose apply bump { target id; bulk maxRows 1 }
  expose apply release { target id; bulk maxRows 1 }
  invariant maxClaimed per team
}
"#;

/// Error codes may carry a `WIRE.` namespace; the scenarios care about the condition, not the spelling.
fn plain(code: &'static str) -> &'static str {
    code.strip_prefix("WIRE.").unwrap_or(code)
}

fn caller(actor: i64) -> Caller {
    Caller { actor_id: Some(actor), now: "2026-10-13T00:00:00Z".into() }
}

fn request(transition: &str, ids: &[i64]) -> Value {
    json!({"apply":transition,"target":{"ids":ids.iter().map(i64::to_string).collect::<Vec<_>>()}})
}

/// One request per spawned task, each on its own connection, all released together.
async fn race(facts: &Arc<Value>, calls: Vec<(i64, Value)>) -> Vec<(i64, Result<spike_v3_write::Applied, &'static str>)> {
    let gate = Arc::new(tokio::sync::Barrier::new(calls.len()));
    let mut tasks = Vec::new();
    for (actor, body) in calls {
        let (facts, gate) = (facts.clone(), gate.clone());
        tasks.push(tokio::spawn(async move {
            let mut db = connect_with_url(DB).await.unwrap();
            gate.wait().await;
            let outcome = tokio::time::timeout(Duration::from_secs(20), apply(&mut db, &facts, &body, &caller(actor), &Knobs::default()))
                .await
                .expect("a racing request must finish, not hang")
                .map_err(|reject| plain(reject.code));
            (actor, outcome)
        }));
    }
    let mut out = Vec::new();
    for task in tasks {
        out.push(task.await.unwrap());
    }
    out
}

fn tally(outcomes: &[(i64, Result<spike_v3_write::Applied, &'static str>)]) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for (_, outcome) in outcomes {
        *counts.entry(outcome.as_ref().map_or_else(|code| *code, |_| "OK")).or_insert(0) += 1;
    }
    counts
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn racing_writers_cannot_double_claim_lose_updates_exceed_capacity_or_hang() {
    let schema =
        format!("aip_adv_write_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let facts = Arc::new(load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution);
    let db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    let members: Vec<String> = (1..=24).map(|id| format!("({id})")).collect();
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES {}; INSERT INTO {schema}.team(id) VALUES (1),(2),(3)", members.join(",")))
        .await
        .unwrap();
    let mut observed: BTreeMap<String, BTreeMap<&'static str, usize>> = BTreeMap::new();

    for round in 0..3 {
        db.batch_execute(&format!("DELETE FROM {schema}.task")).await.unwrap();
        db.batch_execute(&format!(
            "INSERT INTO {schema}.task(id,team_id,owner_id,status,attempts) VALUES
               (1,1,NULL,'OPEN',0),
               (2,1,1,'CLAIMED',0),
               (30,2,NULL,'OPEN',0),(31,2,NULL,'OPEN',0),(32,2,NULL,'OPEN',0),
               (40,3,1,'CLAIMED',0)"
        ))
        .await
        .unwrap();
        // Capacity race rows: team 3 already holds task 40, so only two more claims fit.
        db.batch_execute(&format!(
            "INSERT INTO {schema}.task(id,team_id,owner_id,status,attempts) SELECT 100+i,3,NULL,'OPEN',0 FROM generate_series(1,12) i"
        ))
        .await
        .unwrap();

        // 1. Twenty-four identities claim the same task: exactly one wins and the row names that winner.
        let outcomes = race(&facts, (1..=24).map(|actor| (actor, request("Task.claim", &[1]))).collect()).await;
        let winners: Vec<i64> = outcomes.iter().filter(|(_, outcome)| outcome.is_ok()).map(|(actor, _)| *actor).collect();
        assert_eq!(winners.len(), 1, "round {round}: {:?}", tally(&outcomes));
        let row = db.query_one(&format!("SELECT owner_id,status,attempts FROM {schema}.task WHERE id=1"), &[]).await.unwrap();
        assert_eq!((row.get::<_, Option<i64>>(0), row.get::<_, String>(1), row.get::<_, i64>(2)), (Some(winners[0]), "CLAIMED".into(), 0));
        for (_, outcome) in outcomes.iter().filter(|(_, outcome)| outcome.is_err()) {
            assert!(matches!(outcome, Err("MISSING_TARGET" | "INVALID_STATE" | "CONFLICT")), "round {round}: {outcome:?}");
        }
        for (code, count) in tally(&outcomes) {
            *observed.entry("single-winner claim x24".into()).or_default().entry(code).or_insert(0) += count;
        }

        // 2. Sixty increments of one counter from a single owner: no update is lost.
        let outcomes = race(&facts, (0..60).map(|_| (1, request("Task.bump", &[2]))).collect()).await;
        let applied = outcomes.iter().filter(|(_, outcome)| outcome.is_ok()).count() as i64;
        let attempts: i64 = db.query_one(&format!("SELECT attempts FROM {schema}.task WHERE id=2"), &[]).await.unwrap().get(0);
        assert_eq!(attempts, applied, "round {round}: a bump was lost or applied twice: {:?}", tally(&outcomes));
        assert!(applied >= 1);
        for (code, count) in tally(&outcomes) {
            *observed.entry("counter bump x60".into()).or_default().entry(code).or_insert(0) += count;
        }

        // 3. Capacity: twelve different tasks of one team race for the last two slots.
        let calls = (1..=12).map(|i| (1 + i % 5, request("Task.claim", &[100 + i]))).collect();
        let outcomes = race(&facts, calls).await;
        let claimed: i64 =
            db.query_one(&format!("SELECT count(*) FROM {schema}.task WHERE team_id=3 AND status='CLAIMED'"), &[]).await.unwrap().get(0);
        assert!(claimed <= 3, "round {round}: the per-team limit of 3 was exceeded: {claimed} ({:?})", tally(&outcomes));
        assert_eq!(claimed, 3, "round {round}: slots left unused: {:?}", tally(&outcomes));
        assert_eq!(outcomes.iter().filter(|(_, outcome)| outcome.is_ok()).count(), 2, "round {round}: {:?}", tally(&outcomes));
        for (code, count) in tally(&outcomes) {
            *observed.entry("capacity race x12".into()).or_default().entry(code).or_insert(0) += count;
        }

        // 4. Bulk requests over the same rows in opposite orders: no deadlock outcome other than a clean refusal, one winner,
        //    and the three rows end up claimed together by the same identity (a bulk apply is all or nothing).
        let mut calls = Vec::new();
        for actor in 1..=16 {
            let ids: &[i64] = if actor % 2 == 0 { &[30, 31, 32] } else { &[32, 31, 30] };
            calls.push((actor, request("Task.claim", ids)));
        }
        let outcomes = race(&facts, calls).await;
        let winners: Vec<i64> = outcomes.iter().filter(|(_, outcome)| outcome.is_ok()).map(|(actor, _)| *actor).collect();
        assert_eq!(winners.len(), 1, "round {round}: {:?}", tally(&outcomes));
        let owners: Vec<Option<i64>> = db
            .query(&format!("SELECT owner_id FROM {schema}.task WHERE id IN (30,31,32) ORDER BY id"), &[])
            .await
            .unwrap()
            .iter()
            .map(|row| row.get(0))
            .collect();
        assert_eq!(owners, vec![Some(winners[0]); 3], "round {round}: a bulk claim was split between callers");
        for (_, outcome) in outcomes.iter().filter(|(_, outcome)| outcome.is_err()) {
            assert!(matches!(outcome, Err("MISSING_TARGET" | "INVALID_STATE" | "CONFLICT" | "INVARIANT_VIOLATED")), "round {round}: {outcome:?}");
        }
        for (code, count) in tally(&outcomes) {
            *observed.entry("bulk overlap x16".into()).or_default().entry(code).or_insert(0) += count;
        }

        // 5. A transition races the one that removes its precondition: whatever the interleaving, the counter equals the
        //    bumps that were reported applied, and nothing is applied once the claim was released.
        let mut calls: Vec<(i64, Value)> = (0..20).map(|_| (1, request("Task.bump", &[40]))).collect();
        calls.push((1, request("Task.release", &[40])));
        let outcomes = race(&facts, calls).await;
        let row = db.query_one(&format!("SELECT status,attempts FROM {schema}.task WHERE id=40"), &[]).await.unwrap();
        let applied_bumps = outcomes[..20].iter().filter(|(_, outcome)| outcome.is_ok()).count() as i64;
        assert_eq!(row.get::<_, i64>(1), applied_bumps, "round {round}: a bump was reported without being stored, or stored without a report");
        assert_eq!(row.get::<_, String>(0), "OPEN", "round {round}: the release must have taken effect");
        assert!(outcomes[20].1.is_ok(), "round {round}: {:?}", outcomes[20].1);
        for (code, count) in tally(&outcomes) {
            *observed.entry("bump vs release x21".into()).or_default().entry(code).or_insert(0) += count;
        }
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    for (scenario, counts) in &observed {
        eprintln!("{scenario}: {counts:?}");
    }
}
