use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    id_wire::IdWire,
    plan::{plan_read_with_wire, Caller},
    sqlgen,
};

const SOURCE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

fn facts() -> Value {
    let mut facts = load_str(SOURCE, Form::A).unwrap().execution;
    let resource = &mut facts["resources"]["Recruitment"];
    resource["fields"]["title"]["ty"] = json!("Email");
    resource["fields"]["title"].as_object_mut().unwrap().remove("range");
    resource["fields"]["internalNote"]["ty"] = json!("Email?");
    resource["fields"]["periodEnd"]["ty"] = json!("Date");
    resource["fields"]["periodEnd"].as_object_mut().unwrap().remove("range");
    resource["exposeRead"]["filter"] = json!(["title.eq", "periodEnd.gte", "periodEnd.lte"]);
    resource["exposeRead"]["sort"] = json!(["periodEnd", "id"]);
    facts
}

#[test]
fn email_and_date_filters_are_validated_and_remain_bound_parameters() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    for wire in [IdWire::Legacy, IdWire::SafeNumber, IdWire::DecimalString] {
        let request = json!({"read":"Recruitment","select":["periodEnd"],"filter":[{"field":"title","op":"eq","value":"person@example.test"},{"field":"periodEnd","op":"gte","value":"2026-02-28"}]});
        let plan = plan_read_with_wire(&facts, &request, &caller, wire).unwrap();
        assert!(plan.params.contains(&Some("2026-02-28".into())));
        assert!(plan.params.contains(&Some("person@example.test".into())));
        assert!(!plan.sql.contains("2026-02-28"));
        assert!(!plan.sql.contains("person@example.test"));
        assert!(plan.sql.contains("::date"));
    }
    for invalid in ["0000-01-01", "2026-02-29", "2026-2-09", "2026-13-01", "2026-01-01x", "2026-01-٠١", "10000-01-01"] {
        let request = json!({"read":"Recruitment","select":["periodEnd"],"filter":[{"field":"periodEnd","op":"gte","value":invalid}]});
        assert_eq!(plan_read_with_wire(&facts, &request, &caller, IdWire::DecimalString).err().unwrap().code, "BAD_VALUE");
    }
    for invalid in ["not-an-email", "person@.test", "person@example.test ", "person\u{0000}@example.test"] {
        let request = json!({"read":"Recruitment","select":["periodEnd"],"filter":[{"field":"title","op":"eq","value":invalid}]});
        assert_eq!(plan_read_with_wire(&facts, &request, &caller, IdWire::DecimalString).err().unwrap().code, "BAD_VALUE");
    }
}

#[test]
fn email_filter_and_schema_use_text_while_date_schema_uses_postgres_date() {
    let facts = facts();
    let ddl = sqlgen::ddl(&facts).unwrap().join("\n");
    assert!(ddl.contains("title text"), "{ddl}");
    assert!(ddl.contains("internal_note text"), "{ddl}");
    assert!(ddl.contains("period_end date"), "{ddl}");
}

#[tokio::test]
async fn postgres_accepts_date_filter_and_preserves_email_and_date_json_values() {
    sqlgen::set_schema("aip_scalar_email_date");
    let schema = sqlgen::schema();
    let facts = facts();
    let mut db = spike_v2_read::connect().await;
    for statement in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.school(id,name) VALUES(1,'S'); INSERT INTO {schema}.member(id,school_id) VALUES(1,1); INSERT INTO {schema}.club(id,school_id,name) VALUES(1,1,'C'); INSERT INTO {schema}.recruitment(id,club_id,title,period_end,status,views,internal_note) VALUES(1,1,'person@example.test','2026-12-28','PUBLISHED',0,'person@example.test')"
    )).await.unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    let request = json!({"read":"Recruitment","select":["title","periodEnd"],"filter":[{"field":"title","op":"eq","value":"person@example.test"},{"field":"periodEnd","op":"gte","value":"2026-02-28"}]});
    let plan = plan_read_with_wire(&facts, &request, &caller, IdWire::DecimalString).unwrap();
    let rows = spike_v2_read::execute(&mut db, &plan).await.unwrap();
    assert_eq!(rows, vec![json!({"title":"person@example.test","periodEnd":"2026-12-28"})]);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
