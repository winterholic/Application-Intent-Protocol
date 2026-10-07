use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};

const SOURCE: &str = r#"
enum Status { PENDING, APPROVED }
resource Member { fields { id: Id; enabled: Bool; verified: Bool? } }
actor Member
predicate approved(s: Status) = s = APPROVED
resource Audit { fields { id: Id; task: Task; accepted: Bool } }
resource Task {
  fields {
    id: Id; owner: Member; status: Status; wasPending: Bool;
    combined: Bool; membership: Bool; verified: Bool; hasMember: Bool; named: Bool
  }
  rows read when owner = actor
  transition approve {
    allow owner = actor
    from status = PENDING
    to status = APPROVED, wasPending = (status = PENDING),
       combined = (status = PENDING and not status = APPROVED),
       membership = (status in (PENDING, APPROVED)),
       verified = (owner.verified = true),
       hasMember = (exists Member where id = actor.id), named = approved(status)
    create Audit { task = this.id; accepted = approved(status) }
    update Member where id = owner { enabled = (status = APPROVED) }
  }
  expose apply approve { target id; bulk maxRows 1 }
}
"#;

#[tokio::test]
async fn boolean_values_execute_in_transitions_and_post_update_effects() {
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema = format!("aip_boolean_values_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url("host=localhost dbname=postgres").await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.unwrap();
    for sql in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&sql).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id,enabled,verified) VALUES(1,false,NULL), (2,false,true); \
         INSERT INTO {schema}.task(id,owner_id,status,was_pending,combined,membership,verified,has_member,named) \
         VALUES(100,1,'PENDING',false,false,false,true,false,true), (101,2,'PENDING',false,false,false,false,false,false)"
    ))
    .await
    .unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-10-07T00:00:00Z".into() };
    let request = json!({"apply":"Task.approve","target":{"ids":["100"]}});
    let result = apply(&mut db, &facts, &request, &caller, &Knobs::default()).await;
    assert!(result.is_ok(), "typed Bool expressions must execute: {result:?}");
    let row = db
        .query_one(&format!("SELECT status,was_pending,combined,membership,verified,has_member,named FROM {schema}.task WHERE id=100"), &[])
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "APPROVED");
    for column in [1, 2, 3, 5] {
        assert!(row.get::<_, bool>(column), "column {column}");
    }
    assert!(!row.get::<_, bool>(4), "NULL comparison becomes false");
    assert!(!row.get::<_, bool>(6), "transition RHS reads the old PENDING state");
    let effect = db
        .query_one(&format!("SELECT m.enabled,a.accepted FROM {schema}.member m JOIN {schema}.audit a ON a.task_id=100 WHERE m.id=1"), &[])
        .await
        .unwrap();
    assert!(effect.get::<_, bool>(0));
    assert!(effect.get::<_, bool>(1), "effects read the new APPROVED state");
    let denied = apply(&mut db, &facts, &json!({"apply":"Task.approve","target":{"ids":["101"]}}), &caller, &Knobs::default()).await.unwrap_err();
    assert_eq!(denied.code, "MISSING_TARGET");
    let unchanged = db.query_one(&format!("SELECT status,(SELECT count(*) FROM {schema}.audit) FROM {schema}.task WHERE id=101"), &[]).await.unwrap();
    assert_eq!(unchanged.get::<_, String>(0), "PENDING");
    assert_eq!(unchanged.get::<_, i64>(1), 1);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
