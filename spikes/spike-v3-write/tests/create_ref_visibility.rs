//! Founder decision 2026-10-13 (tenant isolation, create path): a Ref value the caller supplies on create must point
//! at a row the caller can read (the target resource's `rows read`). A reference to the actor's own row is the one
//! exception, because an actor resource often has no `rows read` at all (`author = actor`).
//! Compose creates take Ref values from server-read rows (`VALUE_SOURCE_NOT_ALLOWED` for constants), so they are
//! covered by the existing compose tests and are not touched by this rule.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{bundle, Knobs};

const SOURCE: &str = r#"
resource Member { fields { id: Id; org: Int } }
actor Member
resource Task {
  fields { id: Id; org: Int; title: Text }
  rows read when org = actor.org
}
resource Comment {
  fields { id: Id; task: Task; author: Member; body: Text }
  expose create { allow actor != null; fields task, author, body }
}
"#;

fn caller(id: i64) -> Caller {
    Caller { actor_id: Some(id), now: "2026-10-13T00:00:00Z".into() }
}

fn comment(task: &str, author: &str) -> Value {
    json!({"atomic":[{"create":"Comment","values":{"task":task,"author":author,"body":"hi"}}]})
}

#[tokio::test]
async fn a_created_row_may_only_reference_rows_the_caller_can_read() {
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema =
        format!("aip_create_ref_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url("host=localhost dbname=postgres").await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    // members 1 and 3 belong to org 1, member 2 to org 2. Task 10 is org 1, task 20 is org 2.
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id,org) VALUES (1,1),(2,2),(3,1); \
         INSERT INTO {schema}.task(id,org,title) VALUES (10,1,'mine'),(20,2,'theirs')"
    ))
    .await
    .unwrap();
    let knobs = Knobs::default();
    let comments = format!("SELECT count(*) FROM {schema}.comment");

    // the tenant-crossing create: member 1 (org 1) attaches a comment to org 2's task
    let crossing = bundle(&mut db, &facts, &comment("20", "1"), &caller(1), &knobs).await;
    assert_eq!(crossing.as_ref().err().map(|e| e.code), Some("MISSING_TARGET"), "{crossing:?}");
    let none: i64 = db.query_one(comments.as_str(), &[]).await.unwrap().get(0);
    assert_eq!(none, 0, "a rejected create leaves no row");

    // the caller's own tenant and their own actor row are allowed
    let ok = bundle(&mut db, &facts, &comment("10", "1"), &caller(1), &knobs).await.unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(ok.created, 1);

    // another member's row is not readable (Member has no `rows read`); only the actor's own row is excepted
    let other = bundle(&mut db, &facts, &comment("10", "3"), &caller(1), &knobs).await;
    assert_eq!(other.as_ref().err().map(|e| e.code), Some("MISSING_TARGET"), "{other:?}");
    let one: i64 = db.query_one(comments.as_str(), &[]).await.unwrap().get(0);
    assert_eq!(one, 1);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
