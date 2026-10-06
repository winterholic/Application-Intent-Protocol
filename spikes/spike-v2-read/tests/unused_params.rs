//! 정의 식에 쓰이지 않은 매개변수(예: 상수 guard `= true`)가 42P18로 실행 실패하던 문제.
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v2_read::{connect, execute, sqlgen};

const DEF: &str = "
enum S { DRAFT, PUBLISHED }
resource Member { fields { id: Id } rows read when true }
actor Member
access anyone(a: Member) = true
resource Post {
  fields { id: Id; st: S; author: Member }
  rows read when st = PUBLISHED
  aggregate publishedTotal: Int { sourceAccess anyone(actor) where st = PUBLISHED callerFilter none rowOutput none release count }
  expose aggregate publishedTotal
}
";

#[tokio::test]
async fn constant_guard_aggregate_executes() {
    sqlgen::set_schema(&format!("aip_v2_unused_{}", std::process::id()));
    let f = load_str(DEF, Form::A).unwrap().execution;
    let mut db = connect().await;
    for stmt in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&stmt).await.unwrap_or_else(|e| panic!("DDL {stmt}: {e}"));
    }
    let s = sqlgen::schema();
    db.batch_execute(&format!(
        "INSERT INTO {s}.member (id) VALUES (1); INSERT INTO {s}.post (id, st, author_id) VALUES (1, 'PUBLISHED', 1), (2, 'DRAFT', 1), (3, 'PUBLISHED', 1)"
    ))
    .await
    .unwrap();
    let plan =
        plan_read(&f, &json!({ "aggregate": "Post.publishedTotal", "input": {} }), &Caller { actor_id: Some(1), now: "2026-10-07T00:00:00Z".into() })
            .unwrap_or_else(|e| panic!("{} {}", e.code, e.msg));
    let out = execute(&mut db, &plan).await.unwrap_or_else(|e| panic!("{} {}", e.code, e.msg));
    assert_eq!(out, vec![json!(2)]);
    db.batch_execute(&format!("DROP SCHEMA {s} CASCADE")).await.unwrap();
}
