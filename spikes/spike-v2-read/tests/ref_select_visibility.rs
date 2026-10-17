//! Founder decision 2026-10-13 (security audit O-01): selecting a Ref field directly applies the target resource's row
//! policy by default. A caller who cannot read the target row gets null (so the output type is nullable). A definition
//! author who wants the raw foreign-key id public regardless of target visibility declares it with `refIds`.
//! Scenario from the audit: `Task.team: Team?`, `Team rows read when owner = actor`, member 1 reads task 2 whose team 11
//! belongs to member 2.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect_with_url, execute,
    plan::{plan_read, Caller},
    sqlgen,
};

const DB: &str = aip_test_support::test_database_url!();

fn source(declaration: &str) -> String {
    format!(
        r#"
resource Member {{ fields {{ id: Id }} }}
actor Member
resource Team {{
  fields {{ id: Id; owner: Member; name: Text }}
  rows read when owner = actor
  expose read {{
    select id, name
    traverse tasks via Task.team {{ select id, team; limit 10 }}
    budget {{ rows 10; depth 2; deadline 2s; cost 1000 }}
  }}
}}
resource Task {{
  fields {{ id: Id; title: Text; team: Team?; reqTeam: Team }}
  rows read when true
  expose read {{
    select id, team, reqTeam
    {declaration}
    sort id
    budget {{ rows 10; depth 2; deadline 2s; cost 1000 }}
  }}
}}
resource Note {{
  fields {{ id: Id; task: Task }}
  rows read when true
  expose read {{
    select id
    traverse task {{ select id, team, reqTeam }}
    budget {{ rows 10; depth 2; deadline 2s; cost 1000 }}
  }}
}}
"#
    )
}

fn facts_of(src: &str) -> Value {
    load_str(src, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}

/// The schema is fixed once per process, and the declaration only changes facts, not tables, so both scenarios share it.
async fn setup(src: &str) -> (Value, tokio_postgres::Client, String) {
    let facts = facts_of(src);
    let schema =
        format!("aip_ref_select_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    // teams: 10 is member 1's, 11 is member 2's. task 1 is on team 10, task 2 on team 11.
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); \
         INSERT INTO {schema}.team(id,owner_id,name) VALUES (10,1,'mine'),(11,2,'theirs'); \
         INSERT INTO {schema}.task(id,title,team_id,req_team_id) VALUES (1,'a',10,10),(2,'b',11,11),(3,'c',NULL,10); \
         INSERT INTO {schema}.note(id,task_id) VALUES (50,2)"
    ))
    .await
    .unwrap();
    (facts, db, schema)
}

async fn read(db: &mut tokio_postgres::Client, facts: &Value, request: Value, who: i64) -> (Vec<Value>, Value) {
    let caller = Caller { actor_id: Some(who), now: "2026-10-13T00:00:00Z".into() };
    let plan = plan_read(facts, &request, &caller).unwrap_or_else(|e| panic!("{e:?}"));
    let rows = execute(db, &plan).await.unwrap();
    (rows, serde_json::to_value(&plan.output_type).unwrap())
}

#[tokio::test]
async fn a_ref_select_hides_target_rows_the_caller_cannot_read_by_default() {
    let (facts, mut db, schema) = setup(&source("")).await;
    // root select: member 1 sees team 10 on task 1 and null (not 11) on task 2
    let (rows, ty) = read(&mut db, &facts, json!({"read":"Task","select":["id","team","reqTeam"],"sort":[{"field":"id"}]}), 1).await;
    assert_eq!(rows, [json!({"id":1,"team":10,"reqTeam":10}), json!({"id":2,"team":null,"reqTeam":null}), json!({"id":3,"team":null,"reqTeam":10})]);
    // the required reference is nullable in the output type because the target can be invisible
    assert_eq!(ty["rows"]["reqTeam"]["nullable"], true, "{ty}");
    assert_eq!(ty["rows"]["team"]["nullable"], true, "{ty}");
    // member 2 owns team 11
    let (rows, _) = read(&mut db, &facts, json!({"read":"Task","select":["id","team"],"sort":[{"field":"id"}]}), 2).await;
    assert_eq!(rows, [json!({"id":1,"team":null}), json!({"id":2,"team":11}), json!({"id":3,"team":null})]);
    // single traverse: the Ref selected inside the traversed row is guarded the same way
    let (rows, _) = read(&mut db, &facts, json!({"read":"Note","select":["id",{"task":{"select":["id","team","reqTeam"]}}]}), 1).await;
    assert_eq!(rows, [json!({"id":50,"task":{"id":2,"team":null,"reqTeam":null}})]);
    // 1:N traverse: member 2 reads team 11 and its tasks; the Ref inside a child row is guarded too
    let (rows, _) = read(&mut db, &facts, json!({"read":"Team","select":["id",{"tasks":{"select":["id","team"]}}]}), 2).await;
    assert_eq!(rows, [json!({"id":11,"tasks":[{"id":2,"team":11}]})]);
    refids_declares_the_raw_foreign_key_id_public_and_keeps_the_declared_nullability(&mut db).await;
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}

async fn refids_declares_the_raw_foreign_key_id_public_and_keeps_the_declared_nullability(db: &mut tokio_postgres::Client) {
    let facts = facts_of(&source("refIds team, reqTeam"));
    let (rows, ty) = read(db, &facts, json!({"read":"Task","select":["id","team","reqTeam"],"sort":[{"field":"id"}]}), 1).await;
    assert_eq!(rows, [json!({"id":1,"team":10,"reqTeam":10}), json!({"id":2,"team":11,"reqTeam":11}), json!({"id":3,"team":null,"reqTeam":10})]);
    assert_eq!(ty["rows"]["reqTeam"]["nullable"], false, "a declared required reference stays non-null: {ty}");
    assert_eq!(ty["rows"]["team"]["nullable"], true, "a declared optional reference stays nullable: {ty}");
    // the declaration governs traverse selects of this resource too
    let (rows, _) = read(db, &facts, json!({"read":"Note","select":["id",{"task":{"select":["id","team","reqTeam"]}}]}), 1).await;
    assert_eq!(rows, [json!({"id":50,"task":{"id":2,"team":11,"reqTeam":11}})]);
}

#[test]
fn refids_only_names_selected_reference_fields() {
    let load = |declaration: &str| load_str(&source(declaration), Form::A).map(|_| ()).map_err(|e| e.into_iter().map(|d| d.code).collect::<Vec<_>>());
    assert!(load("refIds team").is_ok());
    assert_eq!(load("refIds title"), Err(vec!["DEF.UNKNOWN_FIELD"]), "title is not in select");
    assert_eq!(load("refIds id"), Err(vec!["DEF.TYPE_MISMATCH"]), "id is a select field but not a reference");
    assert_eq!(load("refIds nope"), Err(vec!["DEF.UNKNOWN_FIELD"]));
}
