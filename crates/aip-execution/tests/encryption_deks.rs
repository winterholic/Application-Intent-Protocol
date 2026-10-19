use aip_crypto::{
    cell::{Context, DataKey},
    provider::{KekProvider, LocalProvider, WrapContext},
};
use aip_execution::encryption::{DekStore, Error, Scope};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio_postgres::{Client, NoTls};

const DB: &str = aip_test_support::test_database_url!();
static NEXT: AtomicU64 = AtomicU64::new(0);

fn provider(id: &str, byte: u8) -> LocalProvider {
    LocalProvider::from_spec(&format!("{id}:{}", STANDARD.encode([byte; 32])), id).unwrap()
}
async fn connect() -> Client {
    let (client, connection) = tokio_postgres::connect(DB, NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}
struct Db {
    client: Client,
    store: DekStore,
}
impl Db {
    async fn new() -> Self {
        let store = DekStore::new(&format!("aip_deks_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))).unwrap();
        let client = connect().await;
        client.batch_execute(&store.ddl()).await.unwrap();
        Self { client, store }
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        let schema = self.store.infrastructure_schema().to_owned();
        std::thread::spawn(move || {
            let status =
                std::process::Command::new("psql").args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA {schema} CASCADE")]).status().unwrap();
            assert!(status.success());
        })
        .join()
        .unwrap();
    }
}
fn wrapping<'a>(store: &'a DekStore, scope: &'a Scope, identity: &'a aip_execution::encryption::Dek) -> WrapContext<'a> {
    WrapContext {
        schema: store.deployment_schema(),
        scope_kind: scope.kind(),
        scope_id: scope.id(),
        dek_id: identity.id(),
        version: identity.version(),
    }
}
#[test]
fn identifiers_and_scope_ids_are_bounded_and_canonical_before_sql() {
    for bad in ["public", "aip_x;drop schema public", "aip_", "aip_X"] {
        assert!(DekStore::new(bad).is_err(), "{bad}");
    }
    for bad in ["01", "-1", "1 OR 1=1", "", "9223372036854775808", "550E8400-E29B-41D4-A716-446655440000"] {
        assert!(Scope::reference("Project", bad).is_err(), "{bad}");
    }
    assert!(Scope::reference("Project", "42").is_ok());
    assert!(Scope::owner("Member", "550e8400-e29b-41d4-a716-446655440000").is_ok());
    assert!(Scope::reference("Project;drop", "42").is_err());
    assert_eq!(Scope::service().kind(), "service");
    assert_eq!(Scope::service().id(), "global");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_use_returns_only_the_committed_winning_dek() {
    let db = Db::new().await;
    let store = Arc::new(db.store.clone());
    let provider = Arc::new(provider("k1", 37));
    let gate = Arc::new(tokio::sync::Barrier::new(12));
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let (store, provider, gate) = (store.clone(), provider.clone(), gate.clone());
        tasks.push(tokio::spawn(async move {
            let mut client = connect().await;
            gate.wait().await;
            let tx = client.transaction().await.unwrap();
            let dek = store.ensure_active_in(&tx, &Scope::service(), provider.as_ref()).await.unwrap();
            tx.commit().await.unwrap();
            dek
        }));
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    assert!(results.iter().all(|dek| dek.id() == results[0].id() && dek.version() == 1 && dek.wrapped().sealed == results[0].wrapped().sealed));
    let scope = Scope::service();
    let raw = provider.unwrap(&wrapping(&store, &scope, &results[0]), results[0].wrapped()).await.unwrap();
    assert_ne!(*raw, [0; 32]);
    let rows: i64 =
        db.client.query_one(&format!("SELECT count(*) FROM {}.aip_encryption_deks", store.infrastructure_schema()), &[]).await.unwrap().get(0);
    assert_eq!(rows, 1);
    assert_ne!(results[0].wrapped().sealed, raw.to_vec());
    assert!(!format!("{:?}", results[0]).contains("sealed"));
}
#[tokio::test]
async fn scope_and_key_creation_roll_back_together_and_missing_kek_never_inserts() {
    let mut db = Db::new().await;
    let provider = provider("k1", 37);
    let scope = Scope::reference("Project", "42").unwrap();
    let tx = db.client.transaction().await.unwrap();
    let rolled_back = db.store.ensure_active_in(&tx, &scope, &provider).await.unwrap();
    tx.rollback().await.unwrap();
    let count: i64 =
        db.client.query_one(&format!("SELECT count(*) FROM {}.aip_encryption_scopes", db.store.infrastructure_schema()), &[]).await.unwrap().get(0);
    assert_eq!(count, 0);
    let tx = db.client.transaction().await.unwrap();
    let retry = db.store.ensure_active_in(&tx, &scope, &provider).await.unwrap();
    assert_ne!(retry.id(), rolled_back.id());
    tx.commit().await.unwrap();
    assert_eq!(db.store.find(&db.client, &Scope::owner("Project", "42").unwrap(), retry.id(), retry.version()).await.unwrap_err(), Error::NotFound);
    assert_eq!(db.store.find(&db.client, &scope, retry.id(), 2).await.unwrap_err(), Error::NotFound);
}
#[tokio::test]
async fn scopes_separate_target_owner_and_deployment_without_plaintext_material() {
    let mut db = Db::new().await;
    let provider = provider("k1", 37);
    let mut ids = std::collections::BTreeSet::new();
    for scope in [
        Scope::service(),
        Scope::reference("Project", "42").unwrap(),
        Scope::reference("Team", "42").unwrap(),
        Scope::owner("Project", "42").unwrap(),
    ] {
        let tx = db.client.transaction().await.unwrap();
        let dek = db.store.ensure_active_in(&tx, &scope, &provider).await.unwrap();
        tx.commit().await.unwrap();
        assert!(ids.insert(dek.id().to_owned()));
        let mut aad = wrapping(&db.store, &scope, &dek);
        aad.schema = "aip_another_deployment";
        assert!(provider.unwrap(&aad, dek.wrapped()).await.is_err());
    }
}
#[tokio::test]
async fn kek_rewrap_keeps_cell_and_dek_identity_and_rejects_stale_metadata() {
    let mut db = Db::new().await;
    let (old, new) = (provider("k1", 37), provider("k2", 89));
    let scope = Scope::service();
    let tx = db.client.transaction().await.unwrap();
    let before = db.store.ensure_active_in(&tx, &scope, &old).await.unwrap();
    tx.commit().await.unwrap();
    let key = old.unwrap(&wrapping(&db.store, &scope, &before), before.wrapped()).await.unwrap();
    let data = DataKey::new(before.id(), before.version(), *key).unwrap();
    let context = Context {
        schema: db.store.deployment_schema(),
        resource: "Secret",
        field: "value",
        row_id: "1",
        scope_kind: scope.kind(),
        scope_id: scope.id(),
    };
    let frame = data.seal(&context, b"explicit-fixture-sensitive-canary").unwrap();
    let tx = db.client.transaction().await.unwrap();
    let after = db.store.rewrap_in(&tx, &scope, before.id(), before.version(), &old, &new).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(after.id(), before.id());
    assert_eq!(after.version(), before.version());
    assert_eq!(after.wrapped().kek_id, "k2");
    let key = new.unwrap(&wrapping(&db.store, &scope, &after), after.wrapped()).await.unwrap();
    assert_eq!(
        DataKey::new(after.id(), after.version(), *key).unwrap().open(&context, &frame).unwrap().as_slice(),
        b"explicit-fixture-sensitive-canary"
    );
    let tx = db.client.transaction().await.unwrap();
    assert_eq!(db.store.rewrap_in(&tx, &scope, before.id(), before.version(), &old, &new).await.unwrap_err(), Error::Crypto);
    tx.rollback().await.unwrap();
    let stored = db.store.find(&db.client, &scope, before.id(), before.version()).await.unwrap();
    assert_eq!(stored.wrapped().sealed, after.wrapped().sealed);
}

#[tokio::test]
async fn missing_provider_and_invalid_wrapping_payload_never_create_dek_rows() {
    use aip_crypto::{
        cell::Error as CryptoError,
        provider::{ProviderFuture, WrappedDek},
    };
    struct Broken(bool);
    impl KekProvider for Broken {
        fn wrap<'a>(&'a self, _: &'a WrapContext<'_>, _: &'a [u8; 32]) -> ProviderFuture<'a, WrappedDek> {
            Box::pin(async move {
                if self.0 { Ok(WrappedDek { kek_id: "bad-provider".into(), sealed: vec![0; 59] }) } else { Err(CryptoError::Unavailable) }
            })
        }
        fn unwrap<'a>(&'a self, _: &'a WrapContext<'_>, _: &'a WrappedDek) -> ProviderFuture<'a, zeroize::Zeroizing<[u8; 32]>> {
            Box::pin(async { Err(CryptoError::Unavailable) })
        }
    }
    let mut db = Db::new().await;
    for malformed in [false, true] {
        let tx = db.client.transaction().await.unwrap();
        assert_eq!(db.store.ensure_active_in(&tx, &Scope::service(), &Broken(malformed)).await.unwrap_err(), Error::Crypto);
        tx.rollback().await.unwrap();
    }
    let rows: i64 =
        db.client.query_one(&format!("SELECT count(*) FROM {}.aip_encryption_deks", db.store.infrastructure_schema()), &[]).await.unwrap().get(0);
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn metadata_constraints_and_retired_scope_fail_closed_instead_of_resetting() {
    let mut db = Db::new().await;
    db.client.batch_execute(&db.store.ddl()).await.unwrap(); // explicit provision is idempotent
    let provider = provider("k1", 37);
    let tx = db.client.transaction().await.unwrap();
    let dek = db.store.ensure_active_in(&tx, &Scope::service(), &provider).await.unwrap();
    tx.commit().await.unwrap();
    let table = format!("{}.aip_encryption_deks", db.store.infrastructure_schema());
    for assignment in ["version=0", "state='UNKNOWN'", "wrapped_dek=decode('00','hex')", "kek_id='invalid:key'", "scope_id='orphan'"] {
        assert!(db.client.batch_execute(&format!("UPDATE {table} SET {assignment}")).await.is_err(), "{assignment}");
    }
    assert!(db.client.batch_execute(&format!("INSERT INTO {table}(dek_id,scope_kind,scope_id,version,state,kek_id,wrapped_dek) SELECT '{}'::uuid,scope_kind,scope_id,2,'ACTIVE',kek_id,wrapped_dek FROM {table}", uuid::Uuid::new_v4())).await.is_err());
    let stored = db.store.find(&db.client, &Scope::service(), dek.id(), 1).await.unwrap();
    assert_eq!(stored.wrapped().sealed, dek.wrapped().sealed);
    db.client.batch_execute(&format!("UPDATE {table} SET state='RETIRED'")).await.unwrap();
    let tx = db.client.transaction().await.unwrap();
    assert_eq!(db.store.ensure_active_in(&tx, &Scope::service(), &provider).await.unwrap_err(), Error::Corrupted);
    tx.rollback().await.unwrap();
    let rows: i64 = db.client.query_one(&format!("SELECT count(*) FROM {table}"), &[]).await.unwrap().get(0);
    assert_eq!(rows, 1);
}
