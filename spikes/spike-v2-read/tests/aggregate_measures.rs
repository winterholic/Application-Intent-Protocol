//! sum/min/max 집계. 값을 드러내는 집계는 정책 필드를 거부하고, 빈 집합 규칙(sum 0, min/max null)을 지킨다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v2_read::{connect, execute, sqlgen};

const DEF: &str = "
resource Member { fields { id: Id } }
actor Member
access totalOfVisibleOrder = totalOfVisible(Order)
resource Order {
  fields { id: Id; title: Text }
  rows read when true
  expose read { select id, itemTotal, cheapest; sort id; budget { rows 10; depth 1; deadline 2s; cost 1000 } }
  aggregate itemTotal: Int { source Item; sourceAccess totalOfVisibleOrder; groupKey order; callerFilter none; rowOutput none; release sum(price) }
  aggregate cheapest: Int? { source Item; sourceAccess totalOfVisibleOrder; groupKey order; callerFilter none; rowOutput none; release min(price) }
}
resource Item {
  fields { id: Id; order: Order; price: Int; cost: Int }
  rows read when true
  field cost read when actor != null
}
";

fn load(src: &str) -> Result<Value, String> {
    load_str(src, Form::A).map(|l| l.execution).map_err(|e| format!("{e:?}"))
}

#[test]
fn value_revealing_aggregates_are_checked_at_definition_time() {
    let cases = [
        ("release sum(price) }", "release sum(cost) }", "POLICY_FIELD_NOT_AGGREGATABLE"),
        ("release sum(price) }", "release sum(title) }", "UNKNOWN_FIELD"),
        ("release sum(price) }", "release avg(price) }", "UNSUPPORTED"),
        ("cheapest: Int?", "cheapest: Int", "UNSUPPORTED"),
    ];
    for (old, new, code) in cases {
        let src = DEF.replacen(old, new, 1);
        let got = load(&src).unwrap_err();
        assert!(got.contains(code), "{new}: {got}");
    }
    let f = load(DEF).unwrap();
    assert_eq!(f["resources"]["Order"]["aggregates"]["itemTotal"]["release"], "sum");
    assert_eq!(f["resources"]["Order"]["aggregates"]["itemTotal"]["releaseField"], "price");
}

#[tokio::test]
async fn sum_and_min_follow_empty_set_rules() {
    sqlgen::set_schema(&format!("aip_v2_measure_{}", std::process::id()));
    let f = load(DEF).unwrap();
    let mut db = connect().await;
    for stmt in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&stmt).await.unwrap_or_else(|e| panic!("DDL {stmt}: {e}"));
    }
    let s = sqlgen::schema();
    db.batch_execute(&format!(
        "INSERT INTO {s}.order (id, title) VALUES (1, 'a'), (2, 'empty');
         INSERT INTO {s}.item (id, order_id, price, cost) VALUES (1, 1, 300, 1), (2, 1, 120, 1), (3, 1, 580, 1)"
    ))
    .await
    .unwrap();
    let plan = plan_read(
        &f,
        &json!({ "read": "Order", "select": ["id", "itemTotal", "cheapest"], "sort": [{ "field": "id" }] }),
        &Caller { actor_id: None, now: "2026-10-07T00:00:00Z".into() },
    )
    .unwrap();
    let rows = execute(&mut db, &plan).await.unwrap();
    assert_eq!(rows[0]["itemTotal"], json!(1000), "{rows:?}");
    assert_eq!(rows[0]["cheapest"], json!(120), "{rows:?}");
    assert_eq!(rows[1]["itemTotal"], json!(0), "빈 집합 sum은 0: {rows:?}");
    assert_eq!(rows[1]["cheapest"], Value::Null, "빈 집합 min은 null: {rows:?}");
    db.batch_execute(&format!("DROP SCHEMA {s} CASCADE")).await.unwrap();
}
