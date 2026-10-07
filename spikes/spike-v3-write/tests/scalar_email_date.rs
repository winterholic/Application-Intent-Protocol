use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};

const SOURCE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

fn replace_once(source: &str, old: &str, new: &str) -> String {
    assert_eq!(source.matches(old).count(), 1, "expected one `{old}`");
    source.replacen(old, new, 1)
}

fn scalar_facts() -> Value {
    let source = replace_once(SOURCE, "title: Text(1..100)", "title: Email");
    let source = replace_once(&source, "periodEnd: Time", "periodEnd: Date");
    let source = replace_once(&source, "r.status = PUBLISHED and r.periodEnd >= now", "r.status = PUBLISHED");
    let source = replace_once(&source, "to status = CLOSED", "to status = CLOSED, title = \"next@example.test\", periodEnd = \"2027-02-28\"");
    let source = replace_once(
        &source,
        "  invariant atMostOnePublished per club",
        "  expose apply close { target id; bulk maxRows 10 }\n  invariant atMostOnePublished per club",
    );
    load_str(&source, Form::A).unwrap_or_else(|errors| panic!("scalar source: {errors:?}")).execution
}

#[tokio::test]
async fn typed_email_and_date_transition_literals_bind_as_text_and_date() {
    sqlgen::set_schema("aip_v3_scalar_email_date");
    let schema = sqlgen::schema();
    let facts = scalar_facts();
    let to = &facts["resources"]["Recruitment"]["transitions"]["close"]["to"];
    assert_eq!(to["title"]["ty"], "Email");
    assert_eq!(to["periodEnd"]["ty"], "Date");

    let mut db = connect().await;
    for statement in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("DDL `{statement}`: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.school(id,name) VALUES(1,'S'); INSERT INTO {schema}.member(id,school_id) VALUES(1,1); INSERT INTO {schema}.club(id,name,school_id) VALUES(10,'C',1); INSERT INTO {schema}.club_member(club_id,member_id,role) VALUES(10,1,'MANAGER'); INSERT INTO {schema}.recruitment(id,title,period_end,status,views,club_id) VALUES(100,'before@example.test','2026-12-28','PUBLISHED',0,10)"
    )).await.unwrap();

    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    let request = json!({"apply":"Recruitment.close","target":{"ids":["100"]}});
    let result = apply(&mut db, &facts, &request, &caller, &Knobs::default()).await.unwrap();
    assert_eq!(result.changed, [100]);
    let row = db.query_one(&format!("SELECT title, period_end::text, status FROM {schema}.recruitment WHERE id=100"), &[]).await.unwrap();
    assert_eq!(
        (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2)),
        ("next@example.test".into(), "2027-02-28".into(), "CLOSED".into())
    );
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
