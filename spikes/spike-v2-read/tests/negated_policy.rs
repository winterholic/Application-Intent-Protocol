//! Security regression: `not <Bool path>` must fail closed when the path is NULL (anonymous caller has no actor row).
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect, execute,
    plan::{plan_read, Caller},
    sqlgen,
};

const DEF: &str = "
resource Member { fields { id: Id; suspended: Bool } }
actor Member
resource Note {
  fields { id: Id; body: Text }
  rows read when not actor.suspended
  expose read { select id, body; sort id; budget { rows 10; depth 1; deadline 2s; cost 1000 } }
}
";

#[tokio::test]
async fn negated_actor_attribute_denies_the_anonymous_caller() {
    sqlgen::set_schema(&format!("aip_security_negated_{}", std::process::id()));
    let facts = load_str(DEF, Form::A).expect("valid negated policy").execution;
    let mut db = connect().await;
    for stmt in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&stmt).await.unwrap();
    }
    let schema = sqlgen::schema();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member (id, suspended) VALUES (1,false),(2,true); INSERT INTO {schema}.note (id, body) VALUES (10,'x')"
    ))
    .await
    .unwrap();
    let request = json!({"read":"Note","select":["id","body"],"sort":[{"field":"id"}]});
    let mut seen = Vec::new();
    for actor_id in [Some(1), Some(2), None] {
        let caller = Caller { actor_id, now: "2026-10-13T00:00:00Z".into() };
        let plan = plan_read(&facts, &request, &caller).unwrap();
        seen.push(execute(&mut db, &plan).await.unwrap().len());
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert_eq!(seen, [1, 0, 0], "an unsuspended member reads; a suspended member and an anonymous caller (no actor row) must not");
}
