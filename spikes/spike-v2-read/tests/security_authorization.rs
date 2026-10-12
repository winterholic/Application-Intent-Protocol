//! Security regression: the origin reference policy also applies to relation traversal.
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect, execute,
    plan::{plan_read, Caller},
    sqlgen,
};

const DEF: &str = "
resource Member { fields { id: Id } }
actor Member
resource Team {
  fields { id: Id; name: Text }
  rows read when true
  expose read { select id, name }
}
resource Task {
  fields { id: Id; owner: Member; team: Team }
  rows read when true
  field team read when owner = actor
  expose read {
    select id, team
    traverse team { select id, name }
    sort id
    budget { rows 10; depth 2; deadline 2s; cost 1000 }
  }
}
";

#[tokio::test]
async fn single_reference_traverse_respects_origin_field_policy() {
    sqlgen::set_schema(&format!("aip_security_authz_{}", std::process::id()));
    let facts = load_str(DEF, Form::A).expect("valid relation policy").execution;
    let mut db = connect().await;
    for stmt in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&stmt).await.unwrap();
    }
    let schema = sqlgen::schema();
    db.batch_execute(&format!("INSERT INTO {schema}.member (id) VALUES (1),(2); INSERT INTO {schema}.team (id,name) VALUES (10,'private association'); INSERT INTO {schema}.task (id,owner_id,team_id) VALUES (1,1,10)")).await.unwrap();
    let request = json!({"read":"Task","select":["id",{"team":{"select":["id","name"]}}]});
    let mut actual = Vec::new();
    for actor_id in [Some(1), Some(2), None] {
        let caller = Caller { actor_id, now: "2026-10-13T00:00:00Z".into() };
        let plan = plan_read(&facts, &request, &caller).unwrap();
        actual.push(execute(&mut db, &plan).await.unwrap());
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert_eq!(actual[0][0]["team"], json!({"id":10,"name":"private association"}));
    assert!(actual[1][0]["team"].is_null(), "field policy must hide the association for another member: {:?}", actual[1]);
    assert!(actual[2][0]["team"].is_null(), "field policy must hide the association for anonymous callers: {:?}", actual[2]);
}

#[test]
fn single_traverse_rejects_repeated_select_fields() {
    let facts = load_str(DEF, Form::A).expect("valid relation policy").execution;
    let ids = vec![json!("id"); 60];
    let request = json!({"read":"Task","select":["id",{"team":{"select":ids}}]});
    let caller = Caller { actor_id: Some(1), now: "2026-10-13T00:00:00Z".into() };
    let error = plan_read(&facts, &request, &caller).err().expect("repeated traverse select must be rejected");
    assert_eq!(error.code, "DUPLICATE");
}
