use aip_crypto::provider::{KekProvider, LocalProvider, WrapContext};
use aip_execution::encryption::{DekStore, Error, Scope};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::ops::{Deref, DerefMut};
use tokio_postgres::{Client, NoTls};

const DB: &str = aip_test_support::test_database_url!();

struct Connection {
    client: Client,
    driver: tokio::task::JoinHandle<()>,
}
impl Deref for Connection {
    type Target = Client;
    fn deref(&self) -> &Client {
        &self.client
    }
}
impl DerefMut for Connection {
    fn deref_mut(&mut self) -> &mut Client {
        &mut self.client
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.driver.abort();
    }
}
async fn connect() -> Connection {
    let (client, driver) = tokio_postgres::connect(DB, NoTls).await.unwrap();
    Connection {
        client,
        driver: tokio::spawn(async move {
            let _ = driver.await;
        }),
    }
}
fn provider() -> LocalProvider {
    LocalProvider::from_spec(&format!("fixture:{}", STANDARD.encode([7; 32])), "fixture").unwrap()
}
async fn fixture(name: &str) -> (Connection, DekStore) {
    let client = connect().await;
    let store = DekStore::new(&format!("aip_fence_{name}_{}", std::process::id())).unwrap();
    client.batch_execute(&store.ddl()).await.unwrap();
    (client, store)
}
async fn cleanup(client: &Client, store: &DekStore) {
    client
        .batch_execute(&format!("SET lock_timeout='2s'; SET statement_timeout='5s'; DROP SCHEMA IF EXISTS {} CASCADE", store.infrastructure_schema()))
        .await
        .unwrap();
}

#[tokio::test]
async fn active_writer_fences_share_and_block_scope_promotion_until_every_writer_finishes() {
    let (setup, store) = fixture("writers").await;
    let owned = store.clone();
    // Panic in the body drops owned connections before async cleanup, so a failed
    // lock assertion cannot leave a DROP blocked behind its own open transaction.
    let result = tokio::spawn(async move {
        let store = owned;
        let scope = Scope::reference("Project", "42").unwrap();
        let other = Scope::reference("Project", "43").unwrap();
        let provider = provider();
        let mut init = connect().await;
        let tx = init.transaction().await.unwrap();
        let original = store.ensure_active_in(&tx, &scope, &provider).await.unwrap();
        store.ensure_active_in(&tx, &other, &provider).await.unwrap();
        tx.commit().await.unwrap();

        let mut first = connect().await;
        let a = first.transaction().await.unwrap();
        let current = store.active_for_write_in(&a, &scope).await.unwrap();
        assert_eq!(current.epoch(), 1);
        assert_eq!(current.dek().id(), original.id());
        let mut second = connect().await;
        let b = second.transaction().await.unwrap();
        b.batch_execute("SET LOCAL lock_timeout='200ms'; SET LOCAL statement_timeout='2s'").await.unwrap();
        assert_eq!(store.active_for_write_in(&b, &scope).await.unwrap().dek().id(), original.id(), "writers share the scope lock");

        let scopes = format!("{}.aip_encryption_scopes", store.infrastructure_schema());
        let mut operator = connect().await;
        let tx = operator.transaction().await.unwrap();
        tx.batch_execute("SET LOCAL lock_timeout='100ms'; SET LOCAL statement_timeout='2s'").await.unwrap();
        tx.execute(&format!("UPDATE {scopes} SET write_epoch=write_epoch+1 WHERE scope_kind=$1 AND scope_id=$2"), &[&other.kind(), &other.id()])
            .await
            .unwrap();
        tx.commit().await.unwrap(); // another scope is independent
        let tx = operator.transaction().await.unwrap();
        tx.batch_execute("SET LOCAL lock_timeout='100ms'; SET LOCAL statement_timeout='2s'").await.unwrap();
        let refused = tx
            .execute(&format!("UPDATE {scopes} SET write_epoch=write_epoch+1 WHERE scope_kind=$1 AND scope_id=$2"), &[&scope.kind(), &scope.id()])
            .await
            .unwrap_err();
        assert_eq!(refused.code().map(|code| code.code()), Some("55P03"), "two live writers must fence promotion");
        tx.rollback().await.unwrap();
        a.commit().await.unwrap();
        // The second writer must still prevent promotion after the first commits.
        let tx = operator.transaction().await.unwrap();
        tx.batch_execute("SET LOCAL lock_timeout='100ms'; SET LOCAL statement_timeout='2s'").await.unwrap();
        let refused = tx
            .query_one(&format!("SELECT write_epoch FROM {scopes} WHERE scope_kind=$1 AND scope_id=$2 FOR UPDATE"), &[&scope.kind(), &scope.id()])
            .await
            .unwrap_err();
        assert_eq!(refused.code().map(|code| code.code()), Some("55P03"));
        tx.rollback().await.unwrap();
        b.rollback().await.unwrap();

        let id = uuid::Uuid::new_v4();
        let id_text = id.to_string();
        let wrapping =
            WrapContext { schema: store.deployment_schema(), scope_kind: scope.kind(), scope_id: scope.id(), dek_id: &id_text, version: 2 };
        let wrapped = provider.wrap(&wrapping, &[9; 32]).await.unwrap();
        let tx = operator.transaction().await.unwrap();
        tx.query_one(&format!("SELECT write_epoch FROM {scopes} WHERE scope_kind=$1 AND scope_id=$2 FOR UPDATE"), &[&scope.kind(), &scope.id()])
            .await
            .unwrap();
        let deks = format!("{}.aip_encryption_deks", store.infrastructure_schema());
        tx.execute(&format!("UPDATE {deks} SET state='READABLE' WHERE scope_kind=$1 AND scope_id=$2"), &[&scope.kind(), &scope.id()]).await.unwrap();
        tx.execute(
            &format!("INSERT INTO {deks}(dek_id,scope_kind,scope_id,version,state,kek_id,wrapped_dek) VALUES($1,$2,$3,2,'ACTIVE',$4,$5)"),
            &[&id, &scope.kind(), &scope.id(), &wrapped.kek_id, &wrapped.sealed],
        )
        .await
        .unwrap();
        tx.execute(&format!("UPDATE {scopes} SET write_epoch=write_epoch+1 WHERE scope_kind=$1 AND scope_id=$2"), &[&scope.kind(), &scope.id()])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let tx = init.transaction().await.unwrap();
        let after = store.active_for_write_in(&tx, &scope).await.unwrap();
        assert_eq!(after.epoch(), 2);
        assert_eq!(after.dek().id(), id_text);
        assert_eq!(after.dek().version(), 2);
        assert_ne!(after.dek().id(), original.id(), "a cached prior key cannot replace the active DB identity");
        tx.rollback().await.unwrap();
    })
    .await;
    cleanup(&setup, &store).await;
    result.unwrap();
}

#[tokio::test]
async fn writer_lookup_never_creates_a_scope_or_reuses_readable_or_retired_keys() {
    let (setup, store) = fixture("missing").await;
    let owned = store.clone();
    let result = tokio::spawn(async move {
        let mut client = connect().await;
        let scope = Scope::service();
        let tx = client.transaction().await.unwrap();
        assert_eq!(owned.active_for_write_in(&tx, &scope).await.unwrap_err(), Error::NotFound);
        tx.rollback().await.unwrap();
        let count: i64 =
            client.query_one(&format!("SELECT count(*) FROM {}.aip_encryption_scopes", owned.infrastructure_schema()), &[]).await.unwrap().get(0);
        assert_eq!(count, 0);
        let tx = client.transaction().await.unwrap();
        let prior = owned.ensure_active_in(&tx, &scope, &provider()).await.unwrap();
        tx.commit().await.unwrap();
        for state in ["READABLE", "RETIRED"] {
            client
                .execute(
                    &format!("UPDATE {}.aip_encryption_deks SET state=$1 WHERE dek_id=$2::text::uuid", owned.infrastructure_schema()),
                    &[&state, &prior.id()],
                )
                .await
                .unwrap();
            let tx = client.transaction().await.unwrap();
            assert_eq!(owned.active_for_write_in(&tx, &scope).await.unwrap_err(), Error::NotFound);
            tx.rollback().await.unwrap();
        }
    })
    .await;
    cleanup(&setup, &store).await;
    result.unwrap();
}
