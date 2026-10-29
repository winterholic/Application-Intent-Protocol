//! Tests for the durable rekey execution boundary and lease contracts.
use aip_crypto::provider::LocalProvider;
use aip_execution::{
    encryption::{CellLocation, DekStore, Scope},
    rekey::{RekeyContext, RekeyReport, RekeyTarget, RekeyTargetReport, rekey_target_column},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_postgres::{Client, NoTls};

const DB: &str = aip_test_support::test_database_url!();
static NEXT: AtomicU64 = AtomicU64::new(0);

async fn connect() -> Client {
    let (db, driver) = tokio_postgres::connect(DB, NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = driver.await;
    });
    db
}

fn provider(spec: &str, active: &str) -> LocalProvider {
    LocalProvider::from_spec(spec, active).unwrap()
}

#[test]
fn rekey_target_metadata_and_key_formation() {
    let scope = Scope::service();
    let target = RekeyTarget::new("users", "email_cipher", "id", "User", "email", scope);
    assert_eq!(target.table, "users");
    assert_eq!(target.column, "email_cipher");
    assert_eq!(target.id_column, "id");
    assert_eq!(target.resource, "User");
    assert_eq!(target.field, "email");
    assert_eq!(target.target_key(), "service:global:users:email_cipher");

    let ref_scope = Scope::reference("Organization", "12345").unwrap();
    let ref_target = RekeyTarget::new("secrets", "api_key", "secret_id", "Secret", "apiKey", ref_scope);
    assert_eq!(ref_target.target_key(), "ref:Organization:12345:secrets:api_key");
}

#[test]
fn rekey_reports_default_to_empty_and_unretired() {
    let target_rep = RekeyTargetReport {
        table: "users".into(),
        column: "email_cipher".into(),
        scanned: 10,
        rekeyed: 10,
        already_current: 0,
        raced: 0,
        failed_ids: vec![],
        failed_count: 0,
        remaining_old: 0,
    };
    let report = RekeyReport {
        old_dek_id: "00000000-0000-0000-0000-000000000001".into(),
        old_version: 1,
        active_dek_id: "00000000-0000-0000-0000-000000000002".into(),
        active_version: 2,
        target_reports: vec![target_rep],
        fully_retired: true,
    };
    assert!(report.fully_retired);
    assert_eq!(report.target_reports.len(), 1);
    assert_eq!(report.target_reports[0].remaining_old, 0);
}

#[test]
fn dek_store_ddl_contains_rekey_leases_table() {
    let store = DekStore::new("aip_rekey_test_schema").unwrap();
    let ddl = store.ddl();
    assert!(ddl.contains("aip_rekey_leases"), "DDL must declare aip_rekey_leases table for worker serialization");
    assert!(ddl.contains("target_key text PRIMARY KEY"), "aip_rekey_leases must key on target_key");
    assert!(ddl.contains("leased_until timestamptz NOT NULL"), "aip_rekey_leases must contain leased_until timestamp");
    assert!(ddl.contains("cursor_id text NOT NULL"), "rekey progress must persist the keyset cursor for crash recovery");
    assert!(ddl.contains("pass_no integer NOT NULL"), "rekey progress must persist the rescan pass number");
    assert!(ddl.contains("phase text NOT NULL"), "rekey progress must persist whether it is sweeping or verifying");
    assert!(ddl.contains("old_dek_id uuid NOT NULL"), "rekey cursor must be bound to the old DEK generation");
    assert!(ddl.contains("active_dek_id uuid NOT NULL"), "rekey cursor must be bound to the active DEK generation");
    assert!(ddl.contains("failed_count bigint NOT NULL"), "rekey failures must survive a worker restart and block retirement");
}

#[tokio::test]
async fn rekey_resumes_from_the_persisted_cursor_after_worker_restart() {
    let mut db = connect().await;
    let store = DekStore::new(&format!("aip_rekey_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))).unwrap();
    db.batch_execute(&format!(
        "CREATE SCHEMA {};
         CREATE TABLE {}.aip_rekey_leases(
           target_key text PRIMARY KEY, holder text NOT NULL, epoch bigint NOT NULL CHECK(epoch > 0), leased_until timestamptz NOT NULL);
         INSERT INTO {}.aip_rekey_leases(target_key,holder,epoch,leased_until)
           VALUES('legacy-target','legacy-worker',1,clock_timestamp()-interval '1 second');",
        store.infrastructure_schema(),
        store.infrastructure_schema(),
        store.infrastructure_schema()
    ))
    .await
    .unwrap();
    db.batch_execute(&store.ddl()).await.unwrap();
    let legacy_checkpoint = db
        .query_one(
            &format!(
                "SELECT phase,pass_no,cursor_id,failed_count FROM {}.aip_rekey_leases WHERE target_key='legacy-target'",
                store.infrastructure_schema()
            ),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(legacy_checkpoint.get::<_, String>(0), "sweep", "schema upgrade must seed resumable phase defaults");
    assert_eq!(legacy_checkpoint.get::<_, i32>(1), 1, "schema upgrade must start the pass counter at one");
    assert_eq!(legacy_checkpoint.get::<_, String>(2), "", "legacy leases must start without a cursor");
    assert_eq!(legacy_checkpoint.get::<_, i64>(3), 0, "legacy leases must start without recorded failures");
    db.batch_execute(&format!(
        "CREATE SCHEMA {}; CREATE TABLE {}.cells(id bigint PRIMARY KEY, value text NOT NULL); SET search_path TO {},public",
        store.deployment_schema(),
        store.deployment_schema(),
        store.deployment_schema()
    ))
    .await
    .unwrap();

    let scope = Scope::service();
    let keys = format!("rekey-old:{},rekey-new:{}", STANDARD.encode([7; 32]), STANDARD.encode([8; 32]));
    let old_provider = provider(&keys, "rekey-old");
    let new_provider = provider(&keys, "rekey-new");
    let init = db.transaction().await.unwrap();
    let old_dek = store.ensure_active_in(&init, &scope, &old_provider).await.unwrap();
    init.commit().await.unwrap();

    let mut budget = connect().await;
    let mut reservation = store.reserve_for_write(&mut budget, &scope, 2).await.unwrap();
    let tx = db.transaction().await.unwrap();
    for id in ["1", "2"] {
        let location = CellLocation::new("Secret", "value", id).unwrap();
        let frame = store.seal_in(&tx, &mut reservation, &location, format!("old-secret-{id}").as_bytes(), &old_provider).await.unwrap();
        tx.execute("INSERT INTO cells(id,value) VALUES($1,$2)", &[&id.parse::<i64>().unwrap(), &frame]).await.unwrap();
    }
    tx.commit().await.unwrap();

    let tx = db.transaction().await.unwrap();
    let old_epoch = store.active_for_write_in(&tx, &scope).await.unwrap().epoch();
    let active_dek = store.promote_in(&tx, &scope, old_epoch, &new_provider).await.unwrap();
    tx.commit().await.unwrap();

    let mut reservation = store.reserve_for_write(&mut budget, &scope, 1).await.unwrap();
    let tx = db.transaction().await.unwrap();
    let first_location = CellLocation::new("Secret", "value", "1").unwrap();
    let first_frame = store.seal_in(&tx, &mut reservation, &first_location, b"new-secret-1", &new_provider).await.unwrap();
    tx.execute("UPDATE cells SET value=$1 WHERE id=1", &[&first_frame]).await.unwrap();
    tx.commit().await.unwrap();

    let target = RekeyTarget::new("cells", "value", "id", "Secret", "value", scope.clone());
    db.execute(
        &format!(
            "INSERT INTO {}.aip_rekey_leases(target_key,holder,epoch,leased_until,phase,pass_no,cursor_id,old_dek_id,old_version,active_dek_id,active_version,remaining_old,failed_count)
             VALUES($1,'crashed-worker',$2,clock_timestamp()-interval '1 second','sweep',1,'1',$3,1,$4,2,0,0)",
            store.infrastructure_schema()
        ),
        &[&target.target_key(), &(active_dek.epoch() as i64), &old_dek.uuid(), &active_dek.dek().uuid()],
    )
    .await
    .unwrap();

    let context = RekeyContext { store: &store, old_dek: &old_dek, active_dek: &active_dek, holder_id: "restart-worker", provider: &new_provider };
    let report = match rekey_target_column(&mut db, &mut budget, &context, &target).await {
        Ok(report) => report,
        Err(error) => {
            let checkpoint = db
                .query_opt(
                    &format!(
                        "SELECT phase,pass_no,cursor_id,failed_count FROM {}.aip_rekey_leases WHERE target_key=$1",
                        store.infrastructure_schema()
                    ),
                    &[&target.target_key()],
                )
                .await
                .ok()
                .flatten()
                .map(|row| (row.get::<_, String>(0), row.get::<_, i32>(1), row.get::<_, String>(2), row.get::<_, i64>(3)));
            let headers = db.query("SELECT id,value FROM cells ORDER BY id", &[]).await.ok().map(|rows| {
                rows.iter()
                    .map(|row| {
                        let id: i64 = row.get(0);
                        let frame: String = row.get(1);
                        (id, aip_crypto::cell::parse_header(&frame).map(|h| (h.dek_id.to_owned(), h.version)))
                    })
                    .collect::<Vec<_>>()
            });
            panic!(
                "rekey_target_column returned {error:?}; connection_closed={}; checkpoint={checkpoint:?}; cell_headers={headers:?}",
                db.is_closed()
            );
        }
    };
    assert_eq!(report.rekeyed, 1);
    assert_eq!(report.already_current, 0, "restart must resume after row 1 instead of revisiting it");
    assert_eq!(report.remaining_old, 0);
    let rows = db.query("SELECT id,value FROM cells ORDER BY id", &[]).await.unwrap();
    for (row, id) in rows.iter().zip(["1", "2"]) {
        let stored: String = row.get(1);
        let location = CellLocation::new("Secret", "value", id).unwrap();
        assert_eq!(
            store.open_cell(&db, &scope, &location, &stored, &new_provider).await.unwrap().as_slice(),
            format!("{}-secret-{id}", if id == "1" { "new" } else { "old" }).as_bytes()
        );
    }

    db.batch_execute(&format!("DROP SCHEMA {} CASCADE", store.deployment_schema())).await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {} CASCADE", store.infrastructure_schema())).await.unwrap();
}
