use aip_migrate::{apply, creation, init_with_wire, plan, preflight};
use serde_json::json;
use spike_v1_fixture::{Form, load_str};
use std::sync::atomic::{AtomicU64, Ordering};

const DB: &str = aip_test_support::test_database_url!();
static NEXT: AtomicU64 = AtomicU64::new(0);

struct OwnedSchema(String);

impl OwnedSchema {
    fn new() -> Self {
        Self(format!("aip_messages_up_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
    }
}

impl Drop for OwnedSchema {
    fn drop(&mut self) {
        let schema = self.0.clone();
        std::thread::spawn(move || {
            let _ =
                std::process::Command::new("psql").args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")]).output();
        })
        .join()
        .ok();
    }
}

fn facts(messages: bool) -> serde_json::Value {
    let base = r#"actor principal
operation job process {
  input { text: Text }
  output { accepted: Bool }
  allow actor != null
  effect none
  deadline 120s
  implementation "worker.process"
}"#;
    let source = if messages { base.replace("effect none", "access messages.invoice_events\n  effect managed") } else { base.into() };
    load_str(&source, Form::A).unwrap().execution
}

#[test]
fn message_queue_capability_installs_a_new_execution_table() {
    let old = facts(false);
    let with_messages = facts(true);
    let (old_ddl, _) = creation("aip_messages_plan", &old).unwrap();
    let (new_ddl, _) = creation("aip_messages_plan", &with_messages).unwrap();
    assert!(!old_ddl.iter().any(|sql| sql.starts_with("CREATE TABLE aip_messages_plan.aip_messages ")));
    assert!(new_ddl.iter().any(|sql| sql.starts_with("CREATE TABLE aip_messages_plan.aip_messages ")));
    assert!(new_ddl.iter().any(|sql| sql.starts_with("CREATE INDEX aip_messages_claim_idx ")));
    let mut invalid = with_messages;
    invalid["operations"]["process"]["access"].as_object_mut().unwrap().insert("messages.인보이스".into(), json!(true));
    assert_eq!(creation("aip_messages_plan", &invalid).unwrap_err()["code"], "BAD_FACTS");
}

#[tokio::test]
async fn adding_message_capability_preserves_old_preflight_and_migrates_additively() {
    let schema = OwnedSchema::new();
    let old = facts(false);
    let with_messages = facts(true);
    init_with_wire(DB, &schema.0, &old, "safe").await.unwrap();
    preflight(DB, &schema.0, &old).await.unwrap();

    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let before: bool = db.query_one("SELECT to_regclass($1) IS NOT NULL", &[&format!("{}.aip_messages", schema.0)]).await.unwrap().get(0);
    assert!(!before, "old job facts must not gain new DDL implicitly");

    let report = plan(DB, &schema.0, &with_messages, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    assert_eq!(report["requiresReview"], true, "{report}");
    assert!(report["steps"].as_array().unwrap().iter().any(|step| step["sql"].as_str().unwrap_or("").contains("aip_messages")), "{report}");
    apply(DB, &schema.0, &with_messages, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema.0, &with_messages).await.unwrap();

    let exists: bool = db.query_one("SELECT to_regclass($1) IS NOT NULL", &[&format!("{}.aip_messages", schema.0)]).await.unwrap().get(0);
    assert!(exists);
    assert_eq!(json!(with_messages["operations"]["process"]["access"]["messages.invoice_events"]), true);

    let removal = plan(DB, &schema.0, &old, &None).await.unwrap();
    assert_eq!(removal["blocked"], true, "removing the last queue capability must retain its table: {removal}");
    assert_eq!(apply(DB, &schema.0, &old, &None, removal["digest"].as_str().unwrap()).await.unwrap_err()["code"], "MIGRATION_BLOCKED");
    let retained: bool = db.query_one("SELECT to_regclass($1) IS NOT NULL", &[&format!("{}.aip_messages", schema.0)]).await.unwrap().get(0);
    assert!(retained, "queue storage survives the rejected capability removal");
}
