use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    id_wire::IdWire,
    plan::{plan_read_with_wire, Caller},
};

const SOURCE: &str = include_str!("../../../prototype/example/app.aip");

fn query(field: &str, value: Value) -> Value {
    json!({"read":"Event","select":["id"],"filter":[{"field":field,"op":"eq","value":value}]})
}

#[test]
fn declared_url_and_enum_filters_accept_strings_and_members_only() {
    let facts = load_str(SOURCE, Form::A).unwrap().execution;
    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    for wire in [IdWire::Legacy, IdWire::SafeNumber, IdWire::DecimalString] {
        for (field, value) in [("link", "https://example.test/a?x=';--"), ("link", "local:thing"), ("phase", "READY"), ("phase", "DONE")] {
            let plan = plan_read_with_wire(&facts, &query(field, json!(value)), &caller, wire).unwrap();
            assert!(plan.params.contains(&Some(value.into())));
            assert!(!plan.sql.contains(value), "caller values must stay parameters");
        }
        for (field, value) in [("phase", json!("UNKNOWN")), ("phase", json!(true)), ("link", json!(1)), ("link", json!(null))] {
            assert_eq!(plan_read_with_wire(&facts, &query(field, value), &caller, wire).err().unwrap().code, "BAD_VALUE");
        }
    }
}

#[test]
fn time_filters_reject_invalid_calendar_and_offset_before_sql() {
    let facts = load_str(SOURCE, Form::A).unwrap().execution;
    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    let invalid = [
        "2026-02-30T00:00:00Z",
        "2025-02-29T00:00:00Z",
        "2026-13-01T00:00:00Z",
        "2026-10-04T24:00:00Z",
        "2026-10-04T00:60:00Z",
        "2026-10-04T00:00:00+24:00",
        "2026-10-04T00:00:00+00:60",
        "2026-10-04T00:00:00",
        "2026-10-04 00:00:00Z",
        "2026-10-04T00:00:00Z'",
        "0000-01-01T00:00:00Z",
    ];
    let mut accepted = Vec::new();
    for value in invalid {
        match plan_read_with_wire(&facts, &query("date", json!(value)), &caller, IdWire::DecimalString) {
            Ok(_) => accepted.push(value),
            Err(error) => assert_eq!(error.code, "BAD_VALUE", "{value}"),
        }
    }
    assert!(accepted.is_empty(), "invalid Time reached SQL: {accepted:?}");
}

#[test]
fn time_filters_accept_real_calendar_utc_offsets_and_language_iso_outputs() {
    let facts = load_str(SOURCE, Form::A).unwrap().execution;
    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    for value in [
        "2024-02-29T12:34:56Z",
        "2026-10-04T00:00:00.123Z",
        "2026-10-04T09:00:00.123456+09:00",
        "2026-10-03T19:00:00.123456-05:00",
        "2026-10-04T00:00:00.123456789Z",
    ] {
        let plan = plan_read_with_wire(&facts, &query("date", json!(value)), &caller, IdWire::DecimalString).unwrap();
        assert!(plan.sql.contains("timestamptz"));
    }
}

#[tokio::test]
async fn time_binding_preserves_postgres_fraction_rounding_and_normalizes_offsets() {
    use spike_v2_read::{connect, execute, sqlgen};
    sqlgen::set_schema("aip_v15_scalar");
    let schema = sqlgen::schema();
    let facts = load_str(SOURCE, Form::A).unwrap().execution;
    let mut db = connect().await;
    for ddl in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&ddl).await.unwrap();
    }
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1)")).await.unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    let cases = [
        ("2026-10-04T00:00:00.1234565001Z", "2026-10-04T00:00:00.123457Z"),
        ("2026-10-04T00:00:00.123456789Z", "2026-10-04T00:00:00.123457Z"),
        ("2026-10-04T00:00:00+23:59", "2026-10-03T00:01:00Z"),
        ("0001-01-01T00:00:00+14:00", "0001-12-31T10:00:00+00:00 BC"),
        ("9999-12-31T23:59:59-14:00", "10000-01-01T13:59:59Z"),
        ("2024-02-29t00:00:00z", "2024-02-29T00:00:00Z"),
        ("2016-12-31T23:59:60Z", "2017-01-01T00:00:00Z"),
        ("2026-01-01T13:59:59.9999999+14:00", "2026-01-01T00:00:00Z"),
        ("2026-01-01T12:34:60Z", "2026-01-01T12:35:00Z"),
    ];
    let mut failures = Vec::new();
    for (index, (input, expected)) in cases.iter().enumerate() {
        db.execute(&format!("INSERT INTO {schema}.event(id,member_id,checked,title,link,phase,date) VALUES($1,1,false,'date','url','READY',$2::text::timestamptz)"), &[&((index+1) as i64),expected]).await.unwrap();
        let mut request = query("date", json!(input));
        request["filter"].as_array_mut().unwrap().push(json!({"field":"id","op":"eq","value":(index+1).to_string()}));
        match plan_read_with_wire(&facts, &request, &caller, IdWire::DecimalString) {
            Ok(plan) => match execute(&mut db, &plan).await {
                Ok(rows) if rows.len() == 1 && rows[0]["id"] == json!((index + 1) as i64) => {}
                result => failures.push(format!("{input} expected row {}: {result:?}", index + 1)),
            },
            Err(error) => failures.push(format!("{input}: {error:?}")),
        }
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn postgres_text_nul_is_rejected_by_the_planner_before_sql() {
    let facts = load_str(SOURCE, Form::A).unwrap().execution;
    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    for field in ["title", "link"] {
        assert_eq!(
            plan_read_with_wire(&facts, &query(field, json!("bad\u{0000}text")), &caller, IdWire::DecimalString).err().unwrap().code,
            "BAD_VALUE"
        );
    }
}

#[test]
fn time_retains_the_existing_scalar_length_budget() {
    let facts = load_str(SOURCE, Form::A).unwrap().execution;
    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    for input in ["2026-10-04T00:00:00.12345678901234Z", "2026-10-04T00:00:00.123456789+09:00"] {
        assert_eq!(input.len(), 35);
        assert!(plan_read_with_wire(&facts, &query("date", json!(input)), &caller, IdWire::DecimalString).is_ok());
    }
    let input = "2026-10-04T00:00:00.123456789012345Z";
    assert_eq!(input.len(), 36);
    assert_eq!(plan_read_with_wire(&facts, &query("date", json!(input)), &caller, IdWire::DecimalString).err().unwrap().code, "BAD_VALUE");
}

#[test]
fn declared_prefix_is_a_parameterized_literal_text_operation() {
    let facts = load_str(SOURCE, Form::A).unwrap().execution;
    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    for input in ["서울", "100%_\\'", ""] {
        let request = json!({"read":"Event","select":["id"],"filter":[{"field":"title","op":"prefix","value":input}]});
        let plan = plan_read_with_wire(&facts, &request, &caller, IdWire::DecimalString).unwrap();
        assert!(plan.params.contains(&Some(input.into())));
        assert!(plan.sql.contains("char_length("));
        assert!(!plan.sql.contains(" LIKE "));
    }
    let request = json!({"read":"Event","select":["id"],"filter":[{"field":"title","op":"prefix","value":1}]});
    assert_eq!(plan_read_with_wire(&facts, &request, &caller, IdWire::DecimalString).err().unwrap().code, "BAD_VALUE");
}
