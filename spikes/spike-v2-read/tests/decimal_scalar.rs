use serde_json::json;
use spike_v2_read::{
    id_wire::IdWire,
    plan::{plan_read_with_wire, Caller},
    sqlgen,
};

fn parse(ty: &str, input: serde_json::Value) -> Result<(String, &'static str), spike_v2_read::plan::Reject> {
    spike_v2_read::scalar::parse(&json!({"enums":{}}), ty, &input)
}

#[test]
fn decimal_filters_require_lossless_strings_within_precision_and_scale() {
    for input in ["999.99", "0.99", "-0.01", "1", "-0.00"] {
        assert_eq!(parse("Decimal<5,2>", json!(input)).unwrap(), (input.to_string(), "numeric"));
    }
    assert_eq!(parse("Decimal<2,2>", json!("0.99")).unwrap().0, "0.99");
    assert_eq!(parse("Decimal<3,0>", json!("999")).unwrap().0, "999");

    for input in ["1000.00", "1.001", "NaN", "Infinity", "-Infinity", "1e3", "+1", "01.00", "-00.1", "1.", ".5", "١.0"] {
        assert!(parse("Decimal<5,2>", json!(input)).is_err(), "{input} must be rejected");
    }
    assert!(parse("Decimal<2,2>", json!("1.00")).is_err());
    assert!(parse("Decimal<3,0>", json!("1.0")).is_err());
    assert!(parse("Decimal<5,2>", json!(12.50)).is_err());
}

#[tokio::test]
async fn postgres_keeps_decimal_columns_filters_and_nested_json_as_exact_strings() {
    spike_v2_read::sqlgen::set_schema("aip_decimal_scalar_read");
    let schema = spike_v2_read::sqlgen::schema();
    let source = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
    assert_eq!(source.matches("views: Int").count(), 1);
    let source = source.replace("views: Int", "views: Decimal(38,18)");
    assert_eq!(source.matches("r.status = PUBLISHED and r.periodEnd >= now").count(), 1);
    let source = source.replace("r.status = PUBLISHED and r.periodEnd >= now", "r.status = PUBLISHED and r.views >= \"1.00\"");
    let mut facts = spike_v1_fixture::load_str(&source, spike_v1_fixture::Form::A).unwrap().execution;
    facts["resources"]["Recruitment"]["fields"]["title"]["ty"] = json!("Decimal<5,2>");
    facts["resources"]["Recruitment"]["fields"]["title"].as_object_mut().unwrap().remove("range");
    facts["resources"]["Recruitment"]["exposeRead"]["filter"] = json!(["views.gte", "views.lte"]);
    facts["resources"]["Club"]["fields"]["name"]["ty"] = json!("Decimal<5,2>");
    let ddl = sqlgen::ddl(&facts).unwrap().join("\n");
    assert!(ddl.contains("title numeric(5,2)"), "{ddl}");
    assert!(ddl.contains("views numeric(38,18)"), "{ddl}");
    assert!(ddl.contains("name numeric(5,2)"), "{ddl}");

    let mut db = spike_v2_read::connect().await;
    for statement in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.school(id,name) VALUES(1,'S'); INSERT INTO {schema}.member(id,school_id) VALUES(1,1); INSERT INTO {schema}.club(id,school_id,name) VALUES(1,1,'1'); INSERT INTO {schema}.recruitment(id,club_id,title,period_end,status,views) VALUES(1,1,'1','2026-12-28T00:00:00Z','PUBLISHED','12345678901234567890.123456789012345678')"
    )).await.unwrap();

    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    let value = "12345678901234567890.123456789012345678";
    let request = json!({"read":"Recruitment","select":["title","views",{"club":{"select":["name"]}}],"filter":[{"field":"views","op":"gte","value":value}],"sort":[{"field":"views","dir":"asc"}]});
    let plan = plan_read_with_wire(&facts, &request, &caller, IdWire::DecimalString).unwrap();
    assert!(plan.params.contains(&Some(value.into())));
    assert!(!plan.sql.contains(value));
    assert!(plan.sql.contains("::text"));
    let rows = spike_v2_read::execute(&mut db, &plan).await.unwrap();
    assert_eq!(rows, vec![json!({"title":"1.00","views":value,"club":{"name":"1.00"}})]);

    facts["resources"]["Club"]["exposeRead"]["budget"] = facts["resources"]["Recruitment"]["exposeRead"]["budget"].clone();
    facts["resources"]["Club"]["exposeRead"]["budget"]["depth"] = json!(2);
    facts["resources"]["Club"]["exposeRead"]["rootQueryable"] = json!(true);
    facts["resources"]["Club"]["exposeRead"]["traverseMany"]["recruitments"] = json!({
        "target":"Recruitment", "via":"club", "limit":3, "sort":{"field":"id","desc":false}, "select":["views"]
    });
    let many_request = json!({"read":"Club","select":["id",{"recruitments":{"select":["views"]}}]});
    let many_plan = plan_read_with_wire(&facts, &many_request, &caller, IdWire::DecimalString).unwrap();
    let many_rows = spike_v2_read::execute(&mut db, &many_plan).await.unwrap();
    assert_eq!(many_rows, vec![json!({"id":1,"recruitments":[{"views":value}]})]);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
