//! Audit B (2026-11-16): the same idempotency key raced from twelve connections through the `/apply` handler, on real PostgreSQL.
//! Coverage the suite lacked: `commit_unknown_wire` and `v14_apply` replay a key one request at a time. Exactly one request may
//! execute, the other eleven must replay its stored result, in READ COMMITTED and in SERIALIZABLE (isolation marker from `not exists`).
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, plan::Caller, sqlgen};
use std::{sync::Arc, time::Duration};

const DB: &str = aip_test_support::test_database_url!();
const SPEC: &str = r#"
enum BookingStatus { REQUESTED, CONFIRMED }
resource Member { fields { id: Id } }
actor Member
resource Room { fields { id: Id; name: Text } rows read when true }
predicate roomFree(r: Room) = not exists Booking where room = r and status = CONFIRMED
resource Counter {
  fields { id: Id; owner: Member; hits: Int(0..1000000) }
  rows read when true
  transition bump { from true; to hits = hits + 1; allow owner = actor }
  expose apply bump { target id; bulk maxRows 1 }
}
resource Booking {
  fields { id: Id; room: Room; guest: Member; status: BookingStatus }
  rows read when true
  transition confirm { allow roomFree(this.room); from status = REQUESTED; to status = CONFIRMED }
  expose apply confirm { target id; bulk maxRows 1 }
}
"#;

async fn race(facts: &Arc<Value>, request: Value, key: &'static str) -> Vec<Value> {
    let gate = Arc::new(tokio::sync::Barrier::new(12));
    let mut tasks = vec![];
    for _ in 0..12 {
        let (facts, gate, request) = (facts.clone(), gate.clone(), request.clone());
        tasks.push(tokio::spawn(async move {
            let mut db = connect_with_url(DB).await.unwrap();
            gate.wait().await;
            let body = json!({"key":key,"request":request});
            let caller = Caller { actor_id: Some(1), now: "2026-11-16T00:00:00Z".into() };
            tokio::time::timeout(Duration::from_secs(10), spike_v6_transport::handle(&mut db, &facts, "/apply", &body, &caller, IdWire::Legacy))
                .await
                .expect("a racing request must finish, not hang")
        }));
    }
    let mut out = vec![];
    for task in tasks {
        out.push(task.await.unwrap());
    }
    out
}

fn assert_one_execution_and_eleven_replays(label: &str, outcomes: &[Value]) {
    let executed = outcomes.iter().filter(|v| v["ok"] == true && v["replayed"].is_null()).count();
    let replayed = outcomes.iter().filter(|v| v["ok"] == true && v["replayed"] == true).count();
    assert_eq!((executed, replayed), (1, 11), "{label}: {outcomes:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn twelve_concurrent_requests_with_one_key_execute_once_and_replay_eleven_times() {
    let schema = format!("aip_bd_idem_race_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let facts = Arc::new(load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution);
    assert_eq!(facts["resources"]["Booking"]["transitions"]["confirm"]["isolation"], "serializable", "fixture must exercise the SERIALIZABLE path");
    assert!(facts["resources"]["Counter"]["transitions"]["bump"]["isolation"].is_null(), "fixture must exercise the READ COMMITTED path");
    let db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1); INSERT INTO {schema}.room(id,name) VALUES (1,'r');
         INSERT INTO {schema}.counter(id,owner_id,hits) VALUES (7,1,0);
         INSERT INTO {schema}.booking(id,room_id,guest_id,status) VALUES (7,1,1,'REQUESTED')"
    ))
    .await
    .unwrap();

    let outcomes = race(&facts, json!({"apply":"Counter.bump","target":{"ids":["7"]}}), "counter-key").await;
    assert_one_execution_and_eleven_replays("read committed", &outcomes);
    let hits: i64 = db.query_one(&format!("SELECT hits FROM {schema}.counter WHERE id=7"), &[]).await.unwrap().get(0);
    assert_eq!(hits, 1, "the transition ran more than once under one key");

    let outcomes = race(&facts, json!({"apply":"Booking.confirm","target":{"ids":["7"]}}), "booking-key").await;
    assert_one_execution_and_eleven_replays("serializable", &outcomes);

    let stored: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"), &[]).await.unwrap().get(0);
    assert_eq!(stored, 2, "one stored result per distinct request under the one key space");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
