use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect_with_url, execute,
    id_wire::{encode_rows, IdWire},
    plan::{plan_read_with_wire, Caller},
    sqlgen,
};

const DB: &str = "host=localhost dbname=postgres";

#[tokio::test]
async fn typed_id_columns_store_values_without_fk_and_keep_all_three_wire_forms() {
    let source = r#"
resource First {
  fields { id: Id; second: Second.Id? }
  rows read when true
  expose read { select id, second; filter second.eq; sort id; budget { rows 10; depth 1; deadline 2s; cost 100 } }
}
resource Second { fields { id: Id; first: First.Id? } }
resource Child { fields { id: Id; parent: First } }
actor First
"#;
    let facts = load_str(source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema =
        format!("aip_typed_id_pg_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.first(id,second) VALUES (10,20),(11,9223372036854775807),(12,NULL); \
         INSERT INTO {schema}.second(id,first) VALUES (20,10)"
    ))
    .await
    .unwrap();
    let error = db.execute(&format!("INSERT INTO {schema}.child(parent_id) VALUES (999)"), &[]).await.unwrap_err();
    assert_eq!(error.code().unwrap().code(), "23503", "Ref<T> still has an FK");

    let caller = Caller { actor_id: Some(10), now: "2026-10-08T00:00:00Z".into() };
    for (wire, input, expected) in [
        (IdWire::Legacy, json!(20), json!({"id":10,"second":20})),
        (IdWire::SafeNumber, json!(20), json!({"id":10,"second":20})),
        (IdWire::DecimalString, json!("20"), json!({"id":"10","second":"20"})),
    ] {
        let request = json!({"read":"First","select":["id","second"],"filter":[{"field":"second","op":"eq","value":input}]});
        let plan = plan_read_with_wire(&facts, &request, &caller, wire).unwrap();
        let raw = execute(&mut db, &plan).await.unwrap();
        assert_eq!(encode_rows(raw, &plan.output_type, wire).unwrap(), [expected], "{wire:?}");
    }
    let big = json!({"read":"First","select":["id","second"],"filter":[{"field":"second","op":"eq","value":"9223372036854775807"}]});
    let decimal_plan = plan_read_with_wire(&facts, &big, &caller, IdWire::DecimalString).unwrap();
    let decimal = encode_rows(execute(&mut db, &decimal_plan).await.unwrap(), &decimal_plan.output_type, IdWire::DecimalString).unwrap();
    assert_eq!(decimal, [json!({"id":"11","second":"9223372036854775807"})]);
    let all = json!({"read":"First","select":["id","second"],"sort":[{"field":"id"}]});
    let all_plan = plan_read_with_wire(&facts, &all, &caller, IdWire::DecimalString).unwrap();
    let all_rows = encode_rows(execute(&mut db, &all_plan).await.unwrap(), &all_plan.output_type, IdWire::DecimalString).unwrap();
    assert_eq!(all_rows[2], json!({"id":"12","second":null}));
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
