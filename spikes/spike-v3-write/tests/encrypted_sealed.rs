//! Sealed native writer: validated plaintext becomes AEAD frames in the resource
//! transaction, after authorization and ID/scope determination. Reads, migration/adopt
//! and durable rekey are not implied; the listener gate stays fenced.
use aip_crypto::provider::LocalProvider;
use aip_execution::encryption::{CellLocation, DekStore, Scope};
use aip_execution::write_sealer::StoreSealer;
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, plan::Caller, sqlgen};
use spike_v3_write::{apply, bundle, Knobs};
use std::sync::Arc;

const VALUE: &str = "project-secret-plaintext-9f2c";
const VALUE_NEXT: &str = "rotated-project-secret-41ab";
const NOTE: &str = "service-note-plaintext-77d0";
const NOTE_NEXT: &str = "rotated-service-note-c3e9";

const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Project { fields { id: Id } rows read when true }
resource Secret {
  fields {
    id: Id
    project: Project
    value: Text(1..100) encrypted(scope: project)
    note: Text(1..50)? encrypted(scope: service)
  }
  rows read when true
  transition redact { from true; allow true; patch value, note }
  expose apply redact { target id; bulk maxRows 5 }
  expose create { allow actor != null; fields project, value, note }
}
resource Label {
  fields { id: Id; tag: Text(1..20)? encrypted(scope: service, lookup: eq) }
  rows read when true
  expose create { allow actor != null; fields tag }
}
resource Profile {
  fields { id: Id; owner: Member; bio: Text(1..50) encrypted(scope: owner) }
  rows read when true
  expose create { allow actor != null; fields owner, bio }
}
"#;

fn provider() -> LocalProvider {
    LocalProvider::from_spec("k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=", "k1").unwrap()
}

fn caller() -> Caller {
    Caller { actor_id: Some(1), now: "2026-10-28T00:00:00Z".into() }
}

#[tokio::test]
async fn sealed_writes_store_frames_after_authorization_and_fail_closed() {
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert_eq!(spike_v1_fixture::sema::require_supported_runtime(&facts).unwrap_err().code, "DEF.ENCRYPTION_RUNTIME_UNAVAILABLE");
    let deployment = format!("aip_encw_{}", std::process::id());
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
    setup.batch_execute(&format!("INSERT INTO {deployment}.member(id) VALUES(1); INSERT INTO {deployment}.project(id) VALUES(1)")).await.unwrap();

    let mut resource = connect().await;
    let budget = connect().await;
    let knobs = Knobs { seal: Some(Arc::new(StoreSealer::new(store.clone(), provider(), budget))), ..Knobs::default() };

    // Sealed create with an explicit id: ref-scoped and service-scoped cells in one row.
    let created = bundle(
        &mut resource,
        &facts,
        &json!({"atomic":[{"create":"Secret","values":{"id":"100","project":"1","value":VALUE,"note":NOTE}}]}),
        &caller(),
        &knobs,
    )
    .await
    .unwrap();
    assert_eq!(created.created, 1);
    // A null service cell stays null; no frame is minted for it.
    bundle(
        &mut resource,
        &facts,
        &json!({"atomic":[{"create":"Secret","values":{"id":"101","project":"1","value":VALUE,"note":null}}]}),
        &caller(),
        &knobs,
    )
    .await
    .unwrap();
    let row = setup.query_one(&format!("SELECT value, note FROM {deployment}.secret WHERE id=100"), &[]).await.unwrap();
    let (stored_value, stored_note): (String, Option<String>) = (row.get(0), row.get(1));
    assert!(stored_value.starts_with("v2:") && stored_note.as_ref().is_some_and(|note| note.starts_with("v2:")));
    assert!(!stored_value.contains(VALUE) && !stored_note.as_ref().unwrap().contains(NOTE));
    assert_eq!(
        store
            .open_cell(&setup, &Scope::reference("Project", "1").unwrap(), &CellLocation::new("Secret", "value", "100").unwrap(), &stored_value, &kek)
            .await
            .unwrap()
            .as_slice(),
        VALUE.as_bytes()
    );
    assert_eq!(
        store
            .open_cell(&setup, &Scope::service(), &CellLocation::new("Secret", "note", "100").unwrap(), stored_note.as_ref().unwrap(), &kek)
            .await
            .unwrap()
            .as_slice(),
        NOTE.as_bytes()
    );
    assert!(setup.query_one(&format!("SELECT note FROM {deployment}.secret WHERE id=101"), &[]).await.unwrap().get::<_, Option<String>>(0).is_none());
    let leaked: i64 = setup
        .query_one(&format!("SELECT count(*)::bigint FROM {deployment}.secret WHERE value LIKE '%{VALUE}%' OR note LIKE '%{NOTE}%'"), &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(leaked, 0, "plaintext must never be stored");

    // Multi-row update seals per-row frames across both scopes in one statement.
    let applied = apply(
        &mut resource,
        &facts,
        &json!({"apply":"Secret.redact","target":{"ids":["100","101"]},"values":{"value":VALUE_NEXT,"note":NOTE_NEXT}}),
        &caller(),
        &knobs,
    )
    .await
    .unwrap();
    assert_eq!(applied.changed.len(), 2);
    for id in ["100", "101"] {
        let row = setup.query_one(&format!("SELECT value, note FROM {deployment}.secret WHERE id={id}"), &[]).await.unwrap();
        let (value, note): (String, String) = (row.get(0), row.get(1));
        assert!(value.starts_with("v2:") && note.starts_with("v2:"));
        assert_eq!(
            store
                .open_cell(&setup, &Scope::reference("Project", "1").unwrap(), &CellLocation::new("Secret", "value", id).unwrap(), &value, &kek)
                .await
                .unwrap()
                .as_slice(),
            VALUE_NEXT.as_bytes()
        );
        assert_eq!(
            store.open_cell(&setup, &Scope::service(), &CellLocation::new("Secret", "note", id).unwrap(), &note, &kek).await.unwrap().as_slice(),
            NOTE_NEXT.as_bytes()
        );
    }
    let first: String = setup.query_one(&format!("SELECT value FROM {deployment}.secret WHERE id=100"), &[]).await.unwrap().get(0);
    let second: String = setup.query_one(&format!("SELECT value FROM {deployment}.secret WHERE id=101"), &[]).await.unwrap().get(0);
    assert_ne!(first, second, "per-row AAD and random nonces must differ");

    // Missing explicit id, lookup:eq, and owner scope fail closed in sealed mode.
    assert_eq!(
        bundle(&mut resource, &facts, &json!({"atomic":[{"create":"Secret","values":{"project":"1","value":VALUE}}]}), &caller(), &knobs)
            .await
            .unwrap_err()
            .code,
        "WIRE.BAD_VALUE"
    );
    assert_eq!(
        bundle(&mut resource, &facts, &json!({"atomic":[{"create":"Label","values":{"id":"1","tag":"lookup-me"}}]}), &caller(), &knobs)
            .await
            .unwrap_err()
            .code,
        "WIRE.ENCRYPTED_FIELD_NOT_QUERYABLE"
    );
    assert_eq!(
        bundle(&mut resource, &facts, &json!({"atomic":[{"create":"Profile","values":{"id":"1","owner":"1","bio":"owner-bio"}}]}), &caller(), &knobs)
            .await
            .unwrap_err()
            .code,
        "WIRE.ENCRYPTED_FIELD_NOT_QUERYABLE"
    );

    // Hand-built facts that smuggle assignments past sema are refused before sealing.
    let mut evil: Value = facts.clone();
    evil["resources"]["Secret"]["transitions"]["redact"]["to"] = json!({"note": "smuggled"});
    assert_eq!(
        apply(&mut resource, &evil, &json!({"apply":"Secret.redact","target":{"ids":["100"]},"values":{"note":"x"}}), &caller(), &knobs)
            .await
            .unwrap_err()
            .code,
        "WIRE.ENCRYPTED_FIELD_NOT_QUERYABLE"
    );
    let mut evil: Value = facts.clone();
    evil["resources"]["Secret"]["transitions"]["redact"]["expect"] = json!(["value"]);
    assert_eq!(
        apply(
            &mut resource,
            &evil,
            &json!({"apply":"Secret.redact","target":{"ids":["100"]},"values":{"note":"y"},"expect":{"value":"z"}}),
            &caller(),
            &knobs
        )
        .await
        .unwrap_err()
        .code,
        "WIRE.ENCRYPTED_FIELD_NOT_QUERYABLE"
    );

    // Without a sealer the prototype passthrough is unchanged: v2: literals store verbatim.
    bundle(
        &mut resource,
        &facts,
        &json!({"atomic":[{"create":"Secret","values":{"id":"102","project":"1","value":"v2:prototype:passthrough","note":null}}]}),
        &caller(),
        &Knobs::default(),
    )
    .await
    .unwrap();
    let stored: String = setup.query_one(&format!("SELECT value FROM {deployment}.secret WHERE id=102"), &[]).await.unwrap().get(0);
    assert_eq!(stored, "v2:prototype:passthrough");

    setup
        .batch_execute(&format!(
            "SET lock_timeout='2s'; SET statement_timeout='5s'; DROP SCHEMA {deployment} CASCADE; DROP SCHEMA {} CASCADE",
            store.infrastructure_schema()
        ))
        .await
        .unwrap();
}
