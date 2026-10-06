//! Ref 필터는 traverse와 같게 대상 행 정책을 따른다. 안 보이는 대상은 null로 취급한다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v2_read::{connect, execute, sqlgen};

const DEF: &str = "
resource Member { fields { id: Id } }
actor Member
resource Team {
  fields { id: Id; owner: Member }
  rows read when owner = actor
}
resource Task {
  fields { id: Id; team: Team? }
  rows read when true
  expose read { select id; filter team.eq, team.in, team.isNull; sort id; budget { rows 10; depth 2; deadline 2s; cost 1000 } }
}
";

async fn ids(db: &mut tokio_postgres::Client, f: &Value, actor: Option<i64>, op: &str, value: Value) -> Vec<i64> {
    let req = json!({ "read": "Task", "select": ["id"], "filter": [{ "field": "team", "op": op, "value": value }], "sort": [{ "field": "id" }] });
    let plan = plan_read(f, &req, &Caller { actor_id: actor, now: "2026-10-07T00:00:00Z".into() }).unwrap();
    execute(db, &plan).await.unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect()
}

#[tokio::test]
async fn hidden_targets_behave_as_null() {
    sqlgen::set_schema(&format!("aip_v2_refvis_{}", std::process::id()));
    let f = load_str(DEF, Form::A).unwrap().execution;
    let mut db = connect().await;
    for stmt in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&stmt).await.unwrap_or_else(|e| panic!("DDL {stmt}: {e}"));
    }
    let s = sqlgen::schema();
    db.batch_execute(&format!(
        "INSERT INTO {s}.member (id) VALUES (1), (2);
         INSERT INTO {s}.team (id, owner_id) VALUES (10, 1), (11, 2);
         INSERT INTO {s}.task (id, team_id) VALUES (1, 10), (2, 11), (3, NULL)"
    ))
    .await
    .unwrap();
    // 1은 팀 10만 볼 수 있다. 팀 11을 eq/in으로 찍어도 존재가 드러나지 않는다.
    assert_eq!(ids(&mut db, &f, Some(1), "eq", json!(10)).await, vec![1]);
    assert_eq!(ids(&mut db, &f, Some(1), "eq", json!(11)).await, Vec::<i64>::new());
    assert_eq!(ids(&mut db, &f, Some(1), "in", json!([10, 11])).await, vec![1]);
    // 안 보이는 팀을 가리키는 task 2는 traverse처럼 null 쪽에 속한다.
    assert_eq!(ids(&mut db, &f, Some(1), "isNull", json!(true)).await, vec![2, 3]);
    assert_eq!(ids(&mut db, &f, Some(1), "isNull", json!(false)).await, vec![1]);
    // 익명은 어떤 팀도 볼 수 없다.
    assert_eq!(ids(&mut db, &f, None, "in", json!([10, 11])).await, Vec::<i64>::new());
    db.batch_execute(&format!("DROP SCHEMA {s} CASCADE")).await.unwrap();
}
