use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};

const DB: &str = "host=localhost dbname=postgres";
const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
limit maxItems on Item = atMost 2 where active = true
resource Item {
  fields { id: Id; owner: Member?; active: Bool; readable: Bool }
  rows read when readable = true
  transition activate { allow true; from active = false; to active = true }
  transition moveOwner { allow true; from active = true; to owner = actor }
  expose apply activate { target id; bulk maxRows 1 }
  expose apply moveOwner { target id; bulk maxRows 1 }
  invariant maxItems per owner
}
"#;

fn facts() -> Value {
    load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}
fn who(id: i64) -> Caller {
    Caller { actor_id: Some(id), now: "2026-10-07T00:00:00Z".into() }
}
fn request(id: i64) -> Value {
    json!({"apply":"Item.activate","target":{"ids":[id.to_string()]}})
}
fn move_request(id: i64) -> Value {
    json!({"apply":"Item.moveOwner","target":{"ids":[id.to_string()]}})
}
fn code(result: Result<spike_v3_write::Applied, spike_v2_read::plan::Reject>) -> &'static str {
    match result {
        Ok(_) => "OK",
        Err(error) => error.code,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_concurrent_activations_cannot_exceed_the_per_owner_capacity() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let schema = format!("aip_capacity_v3_{unique}");
    sqlgen::set_schema(&schema);
    let facts = facts();
    let mut first = connect_with_url(DB).await.unwrap();
    let mut second = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        first.batch_execute(&statement).await.unwrap();
    }
    first.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2),(3); INSERT INTO {schema}.item(id,owner_id,active,readable) VALUES(11,1,true,true),(12,1,false,true),(13,1,false,true),(21,2,true,false),(22,2,true,false),(23,2,false,true),(31,3,true,false),(32,3,true,false),(24,NULL,false,true),(25,NULL,false,true)"
    )).await.unwrap();
    first.batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION ISOLATION LEVEL REPEATABLE READ").await.unwrap();
    second.batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION ISOLATION LEVEL REPEATABLE READ").await.unwrap();

    let knobs = Knobs { pause_after_lock_ms: 100, ..Knobs::default() };
    let request_a = request(12);
    let request_b = request(13);
    let caller_a = who(1);
    let caller_b = who(1);
    let (a, b) = tokio::join!(apply(&mut first, &facts, &request_a, &caller_a, &knobs), apply(&mut second, &facts, &request_b, &caller_b, &knobs),);
    let mut outcomes = [code(a), code(b)];
    outcomes.sort();
    assert_eq!(outcomes, ["INVARIANT_VIOLATED", "OK"]);
    let active: i64 = first.query_one(&format!("SELECT count(*) FROM {schema}.item WHERE owner_id=1 AND active"), &[]).await.unwrap().get(0);
    assert_eq!(active, 2);
    let hidden = apply(&mut first, &facts, &request(23), &who(1), &Knobs::default()).await;
    let violation = hidden.unwrap_err();
    assert_eq!(violation.code, "INVARIANT_VIOLATED");
    assert_eq!(violation.msg, "`Item.maxItems` 정원 제약 위반");
    let nullable_a = apply(&mut first, &facts, &request(24), &who(1), &Knobs::default()).await;
    let nullable_b = apply(&mut first, &facts, &request(25), &who(1), &Knobs::default()).await;
    assert!(nullable_a.is_ok() && nullable_b.is_ok(), "nullable groups should not be counted");
    let moved_into_full_group = apply(&mut first, &facts, &move_request(11), &who(3), &Knobs::default()).await;
    assert_eq!(moved_into_full_group.unwrap_err().code, "INVARIANT_VIOLATED");
    first.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
