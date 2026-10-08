use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};

const DB: &str = "host=localhost dbname=postgres";

#[tokio::test]
async fn escaped_transition_value_and_notification_reach_postgres_exactly() {
    let source = r#"
resource Member { fields { id: Id } }
actor Member
resource Item {
  fields { id: Id; owner: Member; name: Text }
  rows read when owner = actor and name = "O'Brien\\; SELECT 1;"
  transition rename {
    from name = "O'Brien\\; SELECT 1;"
    to name = "A\"B\\C\n한글"
    allow owner = actor
    notify owner "changed\n\"quoted\""
    create Audit { item = this.id; note = "audit O'Brien\\; SELECT 1;" }
  }
  expose apply rename { target id; bulk maxRows 1 }
}
resource Audit { fields { id: Id; item: Item; note: Text } }
"#;
    let facts = load_str(source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema = format!(
        "aip_string_literal_{}_{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    );
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1),(2)")).await.unwrap();
    let initial = "O'Brien\\; SELECT 1;";
    db.execute(&format!("INSERT INTO {schema}.item(id,owner_id,name) VALUES(10,1,$1)"), &[&initial]).await.unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-10-08T00:00:00Z".into() };
    let outsider = Caller { actor_id: Some(2), now: "2026-10-08T00:00:00Z".into() };
    assert!(apply(&mut db, &facts, &json!({"apply":"Item.rename","target":{"ids":["10"]}}), &outsider, &Knobs::default()).await.is_err());
    apply(&mut db, &facts, &json!({"apply":"Item.rename","target":{"ids":["10"]}}), &caller, &Knobs::default()).await.unwrap();
    let name: String = db.query_one(&format!("SELECT name FROM {schema}.item WHERE id=10"), &[]).await.unwrap().get(0);
    let topic: String = db.query_one(&format!("SELECT topic FROM {schema}.aip_outbox WHERE source_id=10"), &[]).await.unwrap().get(0);
    let note: String = db.query_one(&format!("SELECT note FROM {schema}.audit WHERE item_id=10"), &[]).await.unwrap().get(0);
    assert_eq!(name, "A\"B\\C\n한글");
    assert_eq!(topic, "changed\n\"quoted\"");
    assert_eq!(note, "audit O'Brien\\; SELECT 1;");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
