//! sum/min/max는 값을 내보내므로 원본 행 정책을 따른다. count는 sourceAccess 선언대로 전체를 센다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v2_read::{connect, execute, sqlgen};

const DEF: &str = "
resource Member { fields { id: Id } }
actor Member
access totalOfVisibleOrder = totalOfVisible(Order)
resource Order {
  fields { id: Id }
  rows read when true
  expose read { select id, itemTotal, cheapest, itemCount; sort id; budget { rows 10; depth 1; deadline 2s; cost 1000 } }
  aggregate itemTotal: Int { source Item; sourceAccess totalOfVisibleOrder; groupKey order; callerFilter none; rowOutput none; release sum(price) }
  aggregate cheapest: Int? { source Item; sourceAccess totalOfVisibleOrder; groupKey order; callerFilter none; rowOutput none; release min(price) }
  aggregate itemCount: Int { source Item; sourceAccess totalOfVisibleOrder; groupKey order; callerFilter none; rowOutput none; release count }
}
resource Item {
  fields { id: Id; order: Order; seller: Member; price: Int }
  rows read when seller = actor
}
";

async fn row(db: &mut tokio_postgres::Client, f: &Value, actor: Option<i64>) -> Value {
    let req = json!({ "read": "Order", "select": ["id", "itemTotal", "cheapest", "itemCount"], "sort": [{ "field": "id" }] });
    let plan = plan_read(f, &req, &Caller { actor_id: actor, now: "2026-10-07T00:00:00Z".into() }).unwrap();
    execute(db, &plan).await.unwrap().remove(0)
}

#[tokio::test]
async fn value_aggregates_skip_rows_the_caller_cannot_read() {
    sqlgen::set_schema(&format!("aip_v2_aggpol_{}", std::process::id()));
    let f = load_str(DEF, Form::A).unwrap().execution;
    let mut db = connect().await;
    for stmt in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&stmt).await.unwrap_or_else(|e| panic!("DDL {stmt}: {e}"));
    }
    let s = sqlgen::schema();
    db.batch_execute(&format!(
        "INSERT INTO {s}.member (id) VALUES (1), (2); INSERT INTO {s}.order (id) VALUES (1);
         INSERT INTO {s}.item (id, order_id, seller_id, price) VALUES (1, 1, 1, 100), (2, 1, 2, 3), (3, 1, 2, 7777)"
    ))
    .await
    .unwrap();
    let mine = row(&mut db, &f, Some(1)).await;
    assert_eq!((mine["itemTotal"].clone(), mine["cheapest"].clone()), (json!(100), json!(100)), "{mine}");
    let anonymous = row(&mut db, &f, None).await;
    assert_eq!((anonymous["itemTotal"].clone(), anonymous["cheapest"].clone()), (json!(0), Value::Null), "{anonymous}");
    // count는 선언(totalOfVisible)대로 원본 행 정책 없이 센다.
    assert_eq!(anonymous["itemCount"], json!(3));
    db.batch_execute(&format!("DROP SCHEMA {s} CASCADE")).await.unwrap();
}
