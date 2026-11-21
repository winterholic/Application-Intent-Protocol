//! Decimal `sum` (P1 sema): exact total in `Decimal(p', s)`, `rows read` applied, `WIRE.OUTPUT_INVALID` when the total
//! does not fit, no rounding. Runs on a real PostgreSQL (`AIP_TEST_DATABASE_URL`).
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v2_read::{connect, execute, sqlgen};

const DEF: &str = "
resource Member { fields { id: Id } }
actor Member
access totalOfVisibleOrder = totalOfVisible(Order)
access allowAll(a: Member) = true
resource Order {
  fields { id: Id; title: Text }
  rows read when true
  expose read { select id, total, tight, count, qty; sort id; budget { rows 10; depth 1; deadline 2s; cost 1000 } }
  aggregate total: Decimal(14,2) { source Item; sourceAccess totalOfVisibleOrder; groupKey order; callerFilter none; rowOutput none; release sum(amount) }
  aggregate tight: Decimal(12,2) { source Item; sourceAccess totalOfVisibleOrder; groupKey order; callerFilter none; rowOutput none; release sum(amount) }
  aggregate qty: Int { source Item; sourceAccess totalOfVisibleOrder; groupKey order; callerFilter none; rowOutput none; release sum(units) }
  aggregate allTotal: Decimal(14,2) { source Item; sourceAccess allowAll(actor); callerFilter none; rowOutput none; release sum(amount) }
  expose aggregate allTotal
  aggregate count: Int { source Item; sourceAccess totalOfVisibleOrder; groupKey order; callerFilter none; rowOutput none; release count }
}
resource Item {
  fields { id: Id; order: Order; amount: Decimal(12,2); hidden: Bool; units: Int }
  rows read when hidden = false
}
";

fn load(src: &str) -> Result<Value, String> {
    load_str(src, Form::A).map(|l| l.execution).map_err(|e| format!("{e:?}"))
}

async fn read(db: &mut tokio_postgres::Client, f: &Value, select: Value) -> Result<Vec<Value>, spike_v2_read::plan::Reject> {
    let plan = plan_read(
        f,
        &json!({ "read": "Order", "select": select, "sort": [{ "field": "id" }] }),
        &Caller { actor_id: None, now: "2026-11-21T00:00:00Z".into() },
    )
    .unwrap();
    execute(db, &plan).await
}

#[tokio::test]
async fn decimal_sum_is_exact_scaled_policy_filtered_and_bounded_by_the_declared_precision() {
    sqlgen::set_schema(&format!("aip_decimal_sum_{}", std::process::id()));
    let f = load(DEF).unwrap();
    assert_eq!(f["resources"]["Order"]["aggregates"]["total"]["ty"], "Decimal<14,2>");
    assert_eq!(f["resources"]["Order"]["aggregates"]["total"]["releaseField"], "amount");
    let mut db = connect().await;
    for stmt in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&stmt).await.unwrap_or_else(|e| panic!("DDL {stmt}: {e}"));
    }
    let s = sqlgen::schema();
    // Order 1: the three visible items add to 0.30 + 100.50 - 50.10 (a binary float would give 0.30000000000000004 for the first two).
    // The hidden item (1000.00) must not be counted. Order 2 has no item, order 3 only a hidden one.
    db.batch_execute(&format!(
        "INSERT INTO {s}.order (id, title) VALUES (1, 'a'), (2, 'empty'), (3, 'hiddenOnly');
         INSERT INTO {s}.item (id, order_id, amount, hidden, units) VALUES
           (1, 1, 0.10, false, 1), (2, 1, 0.20, false, 2), (3, 1, 100.50, false, 3), (4, 1, -50.10, false, 4), (5, 1, 1000.00, true, 1000),
           (6, 3, 7.00, true, 7)"
    ))
    .await
    .unwrap();

    let rows = read(&mut db, &f, json!(["id", "total", "count", "qty"])).await.unwrap();
    assert_eq!(rows[0]["qty"], json!(10), "an Int sum next to a Decimal sum stays a JSON number and applies rows read: {rows:?}");
    assert_eq!(rows[0]["total"], json!("50.70"), "0.10+0.20+100.50-50.10, exact: {rows:?}");
    assert_eq!(rows[0]["count"], json!(5), "count does not apply rows read (unchanged): {rows:?}");
    assert_eq!(rows[1]["total"], json!("0.00"), "an empty set sums to zero with the scale kept: {rows:?}");
    assert_eq!(rows[2]["total"], json!("0.00"), "a hidden item is not summed: {rows:?}");
    assert!(rows.iter().all(|r| r["total"].is_string()), "Decimal is a string on the wire: {rows:?}");

    // The standalone aggregate (no groupKey, guard access) returns the same exact string, over every order's visible items.
    let alone = |actor: Option<i64>| plan_read(&f, &json!({"aggregate": "Order.allTotal", "input": {}}), &Caller { actor_id: actor, now: "2026-11-21T00:00:00Z".into() }).unwrap();
    let total = execute(&mut db, &alone(Some(1))).await.unwrap();
    assert_eq!(total, vec![json!("50.70")], "a standalone Decimal sum is a JSON string, not the number 50.7: {total:?}");

    // Scale and precision of the declared type are kept: the same total in a wider type has the same digits.
    let wide = read(&mut db, &f, json!(["id", "tight"])).await.unwrap();
    assert_eq!(wide[0]["tight"], json!("50.70"));

    // Boundary: the largest value of Decimal(12,2) fits exactly, one more cent does not (OUTPUT_INVALID), and Decimal(14,2) still fits.
    db.batch_execute(&format!(
        "INSERT INTO {s}.order (id, title) VALUES (4, 'edge');
         INSERT INTO {s}.item (id, order_id, amount, hidden, units) VALUES (10, 4, 9999999999.99, false, 0)"
    ))
    .await
    .unwrap();
    let edge = read(&mut db, &f, json!(["id", "tight"])).await.unwrap();
    assert_eq!(edge[3]["tight"], json!("9999999999.99"), "{edge:?}");
    db.batch_execute(&format!("INSERT INTO {s}.item (id, order_id, amount, hidden, units) VALUES (11, 4, 0.01, false, 0)")).await.unwrap();
    let over = read(&mut db, &f, json!(["id", "tight"])).await.unwrap_err();
    assert_eq!(over.code, "WIRE.OUTPUT_INVALID", "{}", over.msg);
    assert!(!over.msg.contains("22012") && !over.msg.contains("numeric"), "no database detail leaks: {}", over.msg);
    let wider = read(&mut db, &f, json!(["id", "total"])).await.unwrap();
    assert_eq!(wider[3]["total"], json!("10000000000.00"), "no rounding, the wider type holds it: {wider:?}");

    // A hidden item alone never makes the total overflow: only visible rows are summed.
    db.batch_execute(&format!("DELETE FROM {s}.item WHERE id IN (10, 11); INSERT INTO {s}.item (id, order_id, amount, hidden, units) VALUES (12, 4, 9999999999.99, true, 0)"))
        .await
        .unwrap();
    let ok = read(&mut db, &f, json!(["id", "tight"])).await.unwrap();
    assert_eq!(ok[3]["tight"], json!("0.00"), "{ok:?}");

    db.batch_execute(&format!("DROP SCHEMA {s} CASCADE")).await.unwrap();
}
