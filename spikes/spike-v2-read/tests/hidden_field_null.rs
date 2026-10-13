//! Delegated decision CD-3 (2026-10-13, V2-R1): a field hidden by `field x read when cond` is neither dropped from the
//! row nor does it make the request fail. On a row where `cond` is false the caller gets `null`, and the output type is
//! nullable even when the column is `NOT NULL`. This is the same rule as O-01 (`ref_select_visibility.rs`).
//! The cases cover the root, a to-one traverse and a 1:N traverse, all in one result that mixes visible and hidden rows.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect_with_url, execute,
    plan::{plan_read, Caller},
    sqlgen,
};

const DB: &str = "host=localhost dbname=postgres";

const DEF: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Post {
  fields { id: Id; author: Member; title: Text; secret: Text }
  rows read when true
  field secret read when author = actor
  expose read {
    select id, title, secret
    traverse notes via Note.post { select id, secret; sort id; limit 5 }
    sort id
    budget { rows 10; depth 2; deadline 2s; cost 1000 }
  }
}
resource Note {
  fields { id: Id; post: Post; owner: Member; secret: Text }
  rows read when true
  field secret read when owner = actor
  expose read {
    select id, secret
    traverse post { select id, title, secret }
    sort id
    budget { rows 10; depth 2; deadline 2s; cost 1000 }
  }
}
"#;

async fn setup() -> (Value, tokio_postgres::Client) {
    let facts = load_str(DEF, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_hidden_null_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    // post 1 is member 1's, post 2 is member 2's. note 10 (on post 1) is member 2's, note 11 (on post 2) is member 1's.
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); \
         INSERT INTO {schema}.post(id,author_id,title,secret) VALUES (1,1,'p1','s1'),(2,2,'p2','s2'); \
         INSERT INTO {schema}.note(id,post_id,owner_id,secret) VALUES (10,1,2,'n10'),(11,2,1,'n11')"
    ))
    .await
    .unwrap();
    (facts, db)
}

async fn read(db: &mut tokio_postgres::Client, facts: &Value, request: Value, who: i64) -> (Vec<Value>, Value) {
    let caller = Caller { actor_id: Some(who), now: "2026-10-13T00:00:00Z".into() };
    let plan = plan_read(facts, &request, &caller).unwrap_or_else(|e| panic!("{e:?}"));
    let rows = execute(db, &plan).await.unwrap();
    (rows, serde_json::to_value(&plan.output_type).unwrap())
}

// The schema name is fixed once per process (`sqlgen::set_schema`), so the three scenarios share one test.
#[tokio::test]
async fn a_hidden_field_is_null_per_row_and_the_type_is_nullable() {
    let (facts, mut db) = setup().await;
    // root
    let (rows, ty) = read(&mut db, &facts, json!({"read":"Post","select":["id","secret"]}), 1).await;
    assert_eq!(rows[0]["secret"], json!("s1"), "visible row keeps its value: {rows:?}");
    assert_eq!(rows[1]["secret"], Value::Null, "hidden row is null, not dropped and not an error: {rows:?}");
    assert!(rows[1].as_object().unwrap().contains_key("secret"), "the key stays in the row: {rows:?}");
    assert_eq!(ty["rows"]["secret"]["nullable"], json!(true), "column is NOT NULL but the output may be null: {ty}");
    assert_eq!(ty["rows"]["secret"]["redactable"], json!(true), "{ty}");

    // traverse
    // to-one: note 10's post is post 1 (member 1's, visible to member 1); note 11's post is post 2 (hidden to member 1)
    let (rows, ty) = read(&mut db, &facts, json!({"read":"Note","select":["id","secret",{"post":{"select":["id","secret"]}}]}), 1).await;
    assert_eq!(rows[0]["secret"], Value::Null, "note 10 belongs to member 2: {rows:?}");
    assert_eq!(rows[1]["secret"], json!("n11"), "{rows:?}");
    assert_eq!(rows[0]["post"]["secret"], json!("s1"), "{rows:?}");
    assert_eq!(rows[1]["post"]["secret"], Value::Null, "{rows:?}");
    assert_eq!(ty["rows"]["post"]["object"]["secret"]["nullable"], json!(true), "{ty}");
    // 1:N: post 1's note 10 is hidden to member 1, post 2's note 11 is visible to member 1
    let (rows, ty) = read(&mut db, &facts, json!({"read":"Post","select":["id",{"notes":{"select":["id","secret"]}}]}), 1).await;
    assert_eq!(rows[0]["notes"][0]["secret"], Value::Null, "{rows:?}");
    assert_eq!(rows[1]["notes"][0]["secret"], json!("n11"), "{rows:?}");
    assert_eq!(ty["rows"]["notes"]["array"]["secret"]["nullable"], json!(true), "{ty}");

    // anonymous
    let caller = Caller { actor_id: None, now: "2026-10-13T00:00:00Z".into() };
    let plan = plan_read(&facts, &json!({"read":"Post","select":["id","secret"]}), &caller).unwrap_or_else(|e| panic!("{e:?}"));
    let rows = execute(&mut db, &plan).await.unwrap();
    assert!(rows.iter().all(|r| r["secret"] == Value::Null), "{rows:?}");
}
