use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, plan::Caller, sqlgen};
use spike_v3_write::{apply, bundle, Knobs};

const SOURCE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

fn facts() -> Value {
    let mut source = SOURCE.replace("views: Int", "views: Decimal(5,2)");
    source = source.replace("periodEnd: Time", "periodEnd: Date");
    assert_eq!(SOURCE.matches("r.status = PUBLISHED and r.periodEnd >= now").count(), 1);
    source = source.replace("r.status = PUBLISHED and r.periodEnd >= now", "r.status = PUBLISHED");
    assert_eq!(source.matches("allow managerOf(actor, club)").count(), 1);
    source = source.replace("allow managerOf(actor, club)", "allow true");
    assert_eq!(source.matches("to status = CLOSED").count(), 1);
    source = source.replace("to status = CLOSED", "to status = CLOSED, views = \"12.30\"");
    assert_eq!(source.matches("  docs { summary \"모집 정보\"; visibility internal }\n}\n\nresource Apply {").count(), 1);
    source = source.replace(
        "  docs { summary \"모집 정보\"; visibility internal }\n}\n\nresource Apply {",
        "  expose create { allow true; fields title, periodEnd, status, views, club }\n  expose apply close { target id; bulk maxRows 10 }\n  docs { summary \"모집 정보\"; visibility internal }\n}\n\nresource Apply {",
    );
    load_str(&source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}

#[tokio::test]
async fn decimal_create_filter_and_transition_literals_are_validated_and_written_exactly() {
    sqlgen::set_schema("aip_v3_decimal_scalar");
    let schema = sqlgen::schema();
    let facts = facts();
    let mut db = connect().await;
    for statement in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.school(id,name) VALUES(1,'S'); INSERT INTO {schema}.member(id,school_id) VALUES(1,1); INSERT INTO {schema}.club(id,school_id,name) VALUES(1,1,'C')"
    )).await.unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    let knobs = Knobs::default();
    let create = json!({"atomic":[{"create":"Recruitment","values":{"title":"Created","periodEnd":"2026-12-28","status":"PUBLISHED","views":"99.99","club":"1"}}]});
    assert_eq!(bundle(&mut db, &facts, &create, &caller, &knobs).await.unwrap().created, 1);

    let invalid = json!({"atomic":[{"create":"Recruitment","values":{"title":"Invalid","periodEnd":"2026-12-28","status":"PUBLISHED","views":"1.001","club":"1"}}]});
    assert_eq!(bundle(&mut db, &facts, &invalid, &caller, &knobs).await.unwrap_err().code, "BAD_VALUE");

    let result = apply(&mut db, &facts, &json!({"apply":"Recruitment.close","target":{"ids":["1"]}}), &caller, &knobs).await.unwrap();
    assert_eq!(result.changed, vec![1]);
    let row = db.query_one(&format!("SELECT status, views::text FROM {schema}.recruitment WHERE id=1"), &[]).await.unwrap();
    let (status, views): (String, String) = (row.get(0), row.get(1));
    assert_eq!((status.as_str(), views.as_str()), ("CLOSED", "12.30"));
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
