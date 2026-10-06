//! 식에 쓰이지 않은 매개변수(인자를 쓰지 않는 predicate)가 쓰기 SQL을 42P18로 깨뜨리지 않는다.
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect, sqlgen};
use spike_v3_write::{apply, Knobs};

const DEF: &str = "
resource Member { fields { id: Id } }
actor Member
predicate anyone(m: Member) = true
resource Note {
  fields { id: Id; done: Bool }
  rows read when true
  transition finish { allow anyone(actor); from done = false; to done = true }
  expose apply finish { target id; bulk maxRows 5 }
}
";

#[tokio::test]
async fn predicate_ignoring_its_argument_still_runs() {
    sqlgen::set_schema(&format!("aip_v3_unused_{}", std::process::id()));
    let f = load_str(DEF, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let mut db = connect().await;
    for s in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&s).await.unwrap_or_else(|e| panic!("DDL {s}: {e}"));
    }
    let s = sqlgen::schema();
    db.batch_execute(&format!("INSERT INTO {s}.member (id) VALUES (1); INSERT INTO {s}.note (id, done) VALUES (1, false)")).await.unwrap();
    let req = json!({ "apply": "Note.finish", "target": { "ids": ["1"] } });
    let r = apply(&mut db, &f, &req, &Caller { actor_id: Some(1), now: "2026-10-07T00:00:00Z".into() }, &Knobs::default()).await;
    assert!(r.is_ok(), "{:?}", r.err().map(|e| format!("{} {}", e.code, e.msg)));
    db.batch_execute(&format!("DROP SCHEMA {s} CASCADE")).await.unwrap();
}
