//! Audit rows carry the before and after of every field a transition sets, for counters and for wide transitions.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect, sqlgen};
use spike_v3_write::{apply, audit_ddl, set_audit_table, Knobs};

const WIDE: usize = 60;

fn definition() -> String {
    let fields: Vec<String> = (0..WIDE).map(|i| format!("f{i}: Bool")).collect();
    let sets: Vec<String> = (0..WIDE).map(|i| format!("f{i} = true")).collect();
    format!(
        "resource Member {{ fields {{ id: Id }} }}
actor Member
resource Product {{
  fields {{ id: Id; stock: Int(0..1000); {} }}
  rows read when true
  transition sell {{ allow actor != null; from stock > 0; to stock = stock - 1 }}
  transition flip {{ allow actor != null; from f0 = false; to {} }}
  expose apply sell {{ target id; bulk maxRows 5 }}
  expose apply flip {{ target id; bulk maxRows 1 }}
}}",
        fields.join("; "),
        sets.join(", ")
    )
}

#[tokio::test]
async fn audit_rows_hold_counter_and_wide_transition_values() {
    let facts: Value = load_str(&definition(), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_audit_rows_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let mut db = connect().await;
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&audit_ddl(&schema)).await.unwrap();
    let columns: Vec<String> = (0..WIDE).map(|i| format!("f{i}")).collect();
    let falses = vec!["false"; WIDE].join(",");
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.product(id,stock,{}) VALUES(1,5,{falses}),(2,3,{falses})",
        columns.join(",")
    ))
    .await
    .unwrap();
    set_audit_table(Some(format!("{schema}.aip_audit"))).unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-10-13T00:00:00Z".into() };
    let knobs = Knobs { audit_via: Some("apply"), audit_key: Some("k".into()), ..Knobs::default() };

    let sold = apply(&mut db, &facts, &json!({"apply":"Product.sell","target":{"ids":["1","2"]}}), &caller, &knobs).await;
    let flipped = apply(&mut db, &facts, &json!({"apply":"Product.flip","target":{"ids":["1"]}}), &caller, &knobs).await;
    set_audit_table(None).unwrap();
    let (sold, flipped) = (sold.unwrap(), flipped.unwrap());
    assert_eq!(sold.changed, vec![1, 2]);
    assert_eq!(flipped.changed, vec![1]);

    let rows: Vec<(String, i64, Value, Value)> = db
        .query(&format!("SELECT transition,target_id,before,after FROM {schema}.aip_audit ORDER BY id"), &[])
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
        .collect();
    assert_eq!(rows.len(), 3, "{rows:?}");
    // One row per changed target of a bulk transition, each with its own counter value.
    let mut counters: Vec<(i64, Value, Value)> = rows[..2].iter().map(|(_, id, b, a)| (*id, b.clone(), a.clone())).collect();
    counters.sort_by_key(|row| row.0);
    assert_eq!(counters, vec![(1, json!({"stock":5}), json!({"stock":4})), (2, json!({"stock":3}), json!({"stock":2}))]);
    // A transition that sets more fields than jsonb_build_object takes arguments still records all of them.
    let (_, _, before, after) = &rows[2];
    assert_eq!(before.as_object().unwrap().len(), WIDE);
    assert!(before.as_object().unwrap().values().all(|v| v == false) && after.as_object().unwrap().values().all(|v| v == true), "{after}");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
