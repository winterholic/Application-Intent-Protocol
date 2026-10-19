use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v2_read::plan::{plan_read, plan_read_in, Caller};
use spike_v2_read::sqlgen::{self, Dialect};
use spike_v2_read::{connect, execute};
use std::sync::Once;

const SOURCE: &str = "actor principal resource Topic { fields { id: Id; important: Bool; archived: Bool?; visible: Bool } \
    rows read when visible expose read { select id, important, archived; sort important, archived, id; \
    budget { rows 10; depth 1; deadline 1s; cost 100 } } }";
static SCHEMA: Once = Once::new();

fn facts(cursor: bool) -> Value {
    SCHEMA.call_once(|| sqlgen::set_schema(&format!("aip_bool_sort_{}", std::process::id())));
    let source = if cursor {
        SOURCE.replace("sort important, archived, id", "sort important, id").replace("cost 100", "cost 100; cursor")
    } else {
        SOURCE.to_string()
    };
    load_str(&source, Form::A).expect("boolean sort definition").execution
}

fn caller() -> Caller {
    Caller { actor_id: Some(1), now: "2026-10-19T00:00:00Z".into() }
}

fn request(field: &str, direction: &str) -> Value {
    json!({"read":"Topic", "select":["id","important","archived"], "sort":[{"field":field,"dir":direction}]})
}

#[test]
fn mysql_boolean_sort_preserves_postgres_null_ordering_and_typed_cursor_values() {
    let f = facts(false);
    for dir in ["asc", "desc"] {
        let plan = plan_read_in(Dialect::MySql, &f, &request("archived", dir), &caller(), IdWire::Legacy).unwrap();
        let direction = dir.to_uppercase();
        assert!(plan.sql.contains(&format!("ORDER BY (t.`archived` IS NULL) {direction}, t.`archived` {direction}, t.`id` ASC")), "{}", plan.sql);
    }
    for value in [false, true] {
        let mut req = request("important", "asc");
        req["after"] = json!({"important":value,"id":2});
        let plan = plan_read_in(Dialect::MySql, &facts(true), &req, &caller(), IdWire::Legacy).unwrap();
        assert!(plan.params.iter().any(|p| p.as_deref() == Some(if value { "1" } else { "0" })), "{:?}", plan.params);
        let postgres = plan_read(&facts(true), &req, &caller()).unwrap();
        assert!(postgres.params.iter().any(|p| p.as_deref() == Some(&value.to_string())), "{:?}", postgres.params);
    }
}

#[test]
fn boolean_cursor_rejects_numeric_string_and_null_boundaries() {
    let f = facts(true);
    for value in [json!(0), json!(1), json!("true"), json!(null)] {
        let mut req = request("important", "asc");
        req["after"] = json!({"important":value,"id":2});
        assert!(matches!(plan_read(&f, &req, &caller()), Err(e) if e.code == "WIRE.BAD_VALUE"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn postgres_boolean_order_nulls_and_cursor_walk_preserve_policy_and_ties() {
    let f = facts(false);
    let mut db = connect().await;
    for statement in sqlgen::ddl(&f).expect("DDL") {
        db.batch_execute(&statement).await.expect("create owned schema");
    }
    db.batch_execute(&format!(
        "INSERT INTO {}.topic (id, important, archived, visible) VALUES \
         (1,false,NULL,true),(2,false,false,true),(3,true,true,true),(4,true,false,true),(9,false,true,false)",
        sqlgen::schema()
    ))
    .await
    .expect("seed");
    let mut observed = Vec::new();
    for (field, dir, expected) in [
        ("important", "asc", vec![1, 2, 3, 4]),
        ("important", "desc", vec![3, 4, 1, 2]),
        ("archived", "asc", vec![2, 4, 3, 1]),
        ("archived", "desc", vec![1, 3, 2, 4]),
    ] {
        let plan = plan_read(&f, &request(field, dir), &caller()).unwrap();
        let rows = execute(&mut db, &plan).await.expect("read");
        let ids: Vec<i64> = rows.iter().map(|row| row["id"].as_i64().unwrap()).collect();
        observed.push((format!("{field} {dir}"), ids, expected));
    }
    let cursor = facts(true);
    for (dir, expected) in [("asc", vec![1, 2, 3, 4]), ("desc", vec![3, 4, 1, 2])] {
        let mut req = request("important", dir);
        req["limit"] = json!(1);
        let mut ids = Vec::new();
        for _ in 0..6 {
            let plan = plan_read(&cursor, &req, &caller()).unwrap();
            let rows = execute(&mut db, &plan).await.expect("cursor read");
            let Some(last) = rows.last() else { break };
            ids.push(last["id"].as_i64().unwrap());
            req["after"] = json!({"important":last["important"],"id":last["id"]});
        }
        observed.push((format!("cursor {dir}"), ids, expected));
    }
    db.batch_execute(&format!("DROP SCHEMA {} CASCADE", sqlgen::schema())).await.expect("cleanup");
    for (case, actual, expected) in observed {
        assert_eq!(actual, expected, "{case}");
    }
}
