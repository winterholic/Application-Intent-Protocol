//! Delegated decision CD-5 (2026-10-13, security audit O-26): a policy that depends on the absence of other rows
//! (`allow not exists Booking ... status = CONFIRMED`) cannot stop two requests that change two different rows at the
//! same time. The scenario is the audit's double booking. Transitions whose `allow`/`from`/`check` read other rows run
//! in SERIALIZABLE and the server retries a serialization failure (40001) or a deadlock (40P01) up to 3 times.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};

const DB: &str = "host=localhost dbname=postgres";
const SPEC: &str = r#"
enum BookingStatus { REQUESTED, CONFIRMED }
resource Member { fields { id: Id } }
actor Member
resource Room { fields { id: Id; name: Text } rows read when true }
predicate roomFree(r: Room) = not exists Booking where room = r and status = CONFIRMED
resource Booking {
  fields { id: Id; room: Room; guest: Member; status: BookingStatus }
  rows read when true
  transition confirm { allow roomFree(this.room); from status = REQUESTED; to status = CONFIRMED }
  transition touch { allow true; from status = REQUESTED; to status = REQUESTED }
  expose apply confirm { target id; bulk maxRows 1 }
  expose apply touch { target id; bulk maxRows 1 }
}
"#;

fn facts() -> Value {
    load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}

fn who(id: i64) -> Caller {
    Caller { actor_id: Some(id), now: "2026-10-13T00:00:00Z".into() }
}

fn confirm(id: i64) -> Value {
    json!({"apply":"Booking.confirm","target":{"ids":[id.to_string()]}})
}

async fn setup() -> (Value, tokio_postgres::Client, tokio_postgres::Client, String) {
    let schema =
        format!("aip_absence_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let facts = facts();
    let first = connect_with_url(DB).await.unwrap();
    let second = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        first.batch_execute(&statement).await.unwrap();
    }
    // two guests request the same room 1 (bookings 11 and 12); rooms 2 and 3 each have one request (21, 31)
    first
        .batch_execute(&format!(
            "INSERT INTO {schema}.member(id) VALUES (1),(2); INSERT INTO {schema}.room(id,name) VALUES (1,'a'),(2,'b'),(3,'c'); \
             INSERT INTO {schema}.booking(id,room_id,guest_id,status) VALUES (11,1,1,'REQUESTED'),(12,1,2,'REQUESTED'),(21,2,1,'REQUESTED'),(31,3,1,'REQUESTED')"
        ))
        .await
        .unwrap();
    (facts, first, second, schema)
}

fn code(result: Result<spike_v3_write::Applied, spike_v2_read::plan::Reject>) -> &'static str {
    match result {
        Ok(_) => "OK",
        Err(error) => error.code,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_confirmations_of_one_room_cannot_both_succeed_while_other_rooms_proceed() {
    let (facts, mut first, mut second, schema) = setup().await;
    // Both requests pass `allow` and then wait, so each judges before the other commits: the audit's interleaving.
    let knobs = Knobs { pause_after_lock_ms: 300, ..Knobs::default() };
    let (request_a, request_b, caller_a, caller_b) = (confirm(11), confirm(12), who(1), who(2));
    let (a, b) = tokio::join!(apply(&mut first, &facts, &request_a, &caller_a, &knobs), apply(&mut second, &facts, &request_b, &caller_b, &knobs));
    let mut outcomes = [code(a), code(b)];
    outcomes.sort();
    let confirmed: i64 =
        first.query_one(&format!("SELECT count(*) FROM {schema}.booking WHERE room_id=1 AND status='CONFIRMED'"), &[]).await.unwrap().get(0);
    assert_eq!((outcomes, confirmed), (["OK", "WIRE.FORBIDDEN"], 1), "exactly one booking per room: {outcomes:?}, confirmed={confirmed}");

    // different rooms never conflict, even with the same overlap
    let (request_a, request_b, caller) = (confirm(21), confirm(31), who(1));
    let (a, b) = tokio::join!(apply(&mut first, &facts, &request_a, &caller, &knobs), apply(&mut second, &facts, &request_b, &caller, &knobs));
    assert_eq!((code(a), code(b)), ("OK", "OK"), "different rooms never conflict");
    // the compile step marks only the transition that reads other rows; `touch` stays on the current isolation level
    assert_eq!(facts["resources"]["Booking"]["transitions"]["touch"]["isolation"], Value::Null);
    assert_eq!(facts["resources"]["Booking"]["transitions"]["confirm"]["isolation"], json!("serializable"));
    first.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
