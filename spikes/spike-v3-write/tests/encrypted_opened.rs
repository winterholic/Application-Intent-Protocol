//! Opened reads: authorized SQL runs first, then stored frames open through the key
//! boundary at root, 1:1 traverse, and 1:N traverse levels. Redacted cells stay null and
//! are never opened. Lives in the write crate tests because the write side seals the rows
//! and the read crate cannot depend back on it.
use aip_crypto::provider::LocalProvider;
use aip_execution::encryption::{CellLocation, DekStore, Scope};
use aip_execution::read_opener::StoreOpener;
use aip_execution::write_sealer::StoreSealer;
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect,
    opened::execute_opened,
    plan::{plan_read, Caller},
    sqlgen,
};
use spike_v3_write::{bundle, Knobs};
use std::sync::Arc;

const VALUE: &str = "project-secret-plaintext-9f2c";
const NOTE: &str = "service-note-plaintext-77d0";
const MEMO: &str = "redacted-memo-plaintext-1b4e";

const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Project {
  fields { id: Id; name: Text(1..50) }
  rows read when true
  expose read {
    select id, name
    sort id
    traverse secrets via Secret.project { select id, value, note; sort id; limit 5 }
    budget { rows 10; depth 2; deadline 2s; cost 1000 }
  }
}
resource Secret {
  fields {
    id: Id
    project: Project
    value: Text(1..100) encrypted(scope: project)
    note: Text(1..50)? encrypted(scope: service)
    memo: Text(1..30)? encrypted(scope: service)
  }
  rows read when true
  field memo read when false
  expose read {
    select id, value, note, memo
    sort id
    budget { rows 10; depth 1; deadline 2s; cost 1000 }
  }
  expose create { allow actor != null; fields project, value, note, memo }
}
resource Vault {
  fields { id: Id; secret: Secret }
  rows read when true
  expose read {
    select id
    sort id
    traverse secret { select id, value }
    budget { rows 10; depth 2; deadline 2s; cost 1000 }
  }
  expose create { allow actor != null; fields id, secret }
}
"#;

fn provider() -> LocalProvider {
    LocalProvider::from_spec("k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=", "k1").unwrap()
}

fn caller() -> Caller {
    Caller { actor_id: Some(1), now: "2026-10-28T00:00:00Z".into() }
}

#[tokio::test]
async fn opened_reads_decrypt_after_authorization_at_every_level() {
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert_eq!(spike_v1_fixture::sema::require_supported_runtime(&facts).unwrap_err().code, "DEF.ENCRYPTION_RUNTIME_UNAVAILABLE");
    let deployment = format!("aip_enco_{}", std::process::id());
    sqlgen::set_schema(&deployment);
    let store = DekStore::new(&deployment).unwrap();
    let mut setup = connect().await;
    for statement in sqlgen::ddl(&facts).unwrap() {
        setup.batch_execute(&statement).await.unwrap();
    }
    setup.batch_execute(&store.ddl()).await.unwrap();
    let kek = provider();
    let provision = setup.transaction().await.unwrap();
    store.ensure_active_in(&provision, &Scope::service(), &kek).await.unwrap();
    store.ensure_active_in(&provision, &Scope::reference("Project", "1").unwrap(), &kek).await.unwrap();
    provision.commit().await.unwrap();
    setup
        .batch_execute(&format!("INSERT INTO {deployment}.member(id) VALUES(1); INSERT INTO {deployment}.project(id, name) VALUES(1, 'p1')"))
        .await
        .unwrap();

    let mut resource = connect().await;
    let budget = connect().await;
    let knobs = Knobs { seal: Some(Arc::new(StoreSealer::new(store.clone(), provider(), budget))), ..Knobs::default() };
    for req in [
        json!({"atomic":[{"create":"Secret","values":{"id":"100","project":"1","value":VALUE,"note":NOTE,"memo":MEMO}}]}),
        json!({"atomic":[{"create":"Secret","values":{"id":"101","project":"1","value":VALUE,"note":null,"memo":null}}]}),
    ] {
        bundle(&mut resource, &facts, &req, &caller(), &knobs).await.unwrap();
    }
    bundle(&mut resource, &facts, &json!({"atomic":[{"create":"Vault","values":{"id":"1","secret":"100"}}]}), &caller(), &knobs).await.unwrap();
    bundle(
        &mut resource,
        &facts,
        &json!({"atomic":[{"create":"Secret","values":{"id":"102","project":"1","value":VALUE,"note":null,"memo":null}}]}),
        &caller(),
        &knobs,
    )
    .await
    .unwrap();

    let opener = StoreOpener::new(store.clone(), provider());
    let mut reader = connect().await;

    // Root level: frames open, hidden keys are stripped, ids stay intact.
    let plan =
        plan_read(&facts, &json!({"read":"Secret","select":["id","value","note","memo"],"sort":[{"field":"id"}],"limit":10}), &caller()).unwrap();
    let rows = execute_opened(&mut reader, &plan, &opener).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["value"], json!(VALUE));
    assert_eq!(rows[0]["note"], json!(NOTE));
    assert_eq!(rows[0]["id"], json!(100));
    assert!(rows[0].get("__aip_id").is_none() && rows[0].get("__aip_scope").is_none());
    assert!(rows[1]["note"].is_null());
    // The policy-redacted cell stays null even though a real frame is stored.
    assert!(rows[0]["memo"].is_null());
    let memo_frame: String = setup.query_one(&format!("SELECT memo FROM {deployment}.secret WHERE id=100"), &[]).await.unwrap().get(0);
    assert!(memo_frame.starts_with("v2:"));

    // 1:1 traverse target opens through the relation.
    let plan =
        plan_read(&facts, &json!({"read":"Vault","select":["id",{"secret":{"select":["id","value"]}}],"sort":[{"field":"id"}]}), &caller()).unwrap();
    let rows = execute_opened(&mut reader, &plan, &opener).await.unwrap();
    assert_eq!(rows[0]["secret"]["value"], json!(VALUE));
    assert!(rows[0]["secret"].get("__aip_id").is_none());

    // 1:N traverse children open; null cells stay null.
    let plan = plan_read(
        &facts,
        &json!({"read":"Project","select":["id","name",{"secrets":{"select":["id","value","note"]}}],"sort":[{"field":"id"}]}),
        &caller(),
    )
    .unwrap();
    let rows = execute_opened(&mut reader, &plan, &opener).await.unwrap();
    assert_eq!(rows[0]["secrets"][0]["value"], json!(VALUE));
    assert_eq!(rows[0]["secrets"][0]["note"], json!(NOTE));
    assert!(rows[0]["secrets"][1]["note"].is_null());
    assert!(rows[0]["secrets"][0].get("__aip_scope").is_none());

    // Without an opener the prototype output is unchanged: frames plus hidden keys.
    let plan = plan_read(&facts, &json!({"read":"Secret","select":["id","value"],"sort":[{"field":"id"}],"limit":1}), &caller()).unwrap();
    let rows = spike_v2_read::execute(&mut reader, &plan).await.unwrap();
    assert!(rows[0]["value"].as_str().is_some_and(|v| v.starts_with("v2:")));
    assert!(rows[0].get("__aip_id").is_some());

    // A tampered cell fails the whole read instead of leaking or guessing.
    setup.batch_execute(&format!("UPDATE {deployment}.secret SET value='not-a-frame' WHERE id=102")).await.unwrap();
    let plan = plan_read(&facts, &json!({"read":"Secret","select":["id","value"],"sort":[{"field":"id"}],"limit":10}), &caller()).unwrap();
    assert_eq!(execute_opened(&mut reader, &plan, &opener).await.unwrap_err().code, "WIRE.ENCRYPTION_CORRUPTED");
    let scope = Scope::reference("Project", "1").unwrap();
    assert!(store.open_cell(&setup, &scope, &CellLocation::new("Secret", "value", "102").unwrap(), "not-a-frame", &kek).await.is_err());

    setup
        .batch_execute(&format!(
            "SET lock_timeout='2s'; SET statement_timeout='5s'; DROP SCHEMA {deployment} CASCADE; DROP SCHEMA {} CASCADE",
            store.infrastructure_schema()
        ))
        .await
        .unwrap();
}
