//! Tests for the durable rekey execution boundary and lease contracts.
use aip_crypto::provider::LocalProvider;
use aip_execution::{
    encryption::{CellLocation, DekStore, Scope},
    rekey::{RekeyContext, RekeyReport, RekeyTarget, RekeyTargetReport, rekey_target_column},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio_postgres::{Client, NoTls};

const DB: &str = aip_test_support::test_database_url!();
static NEXT: AtomicU64 = AtomicU64::new(0);
const OLD_KEY: &str = "rekey-old";
const NEW_KEY: &str = "rekey-new";

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

struct RekeyFixture {
    db: Client,
    budget: Client,
    store: DekStore,
    scope: Scope,
    old_dek: aip_execution::encryption::Dek,
    old_provider: LocalProvider,
    new_provider: LocalProvider,
}

fn rekey_store(name: &str) -> DekStore {
    DekStore::new(&format!("aip_rekey_{name}_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))).unwrap()
}

async fn rekey_fixture(store: DekStore, scope: Scope) -> RekeyFixture {
    let mut db = connect().await;
    db.batch_execute(&store.ddl()).await.unwrap();
    db.batch_execute(&format!("CREATE SCHEMA {}", store.deployment_schema())).await.unwrap();
    let keys = format!("{OLD_KEY}:{},{NEW_KEY}:{}", STANDARD.encode([7; 32]), STANDARD.encode([8; 32]));
    let old_provider = provider(&keys, OLD_KEY);
    let new_provider = provider(&keys, NEW_KEY);
    let tx = db.transaction().await.unwrap();
    let old_dek = store.ensure_active_in(&tx, &scope, &old_provider).await.unwrap();
    tx.commit().await.unwrap();
    RekeyFixture { db, budget: connect().await, store, scope, old_dek, old_provider, new_provider }
}

impl RekeyFixture {
    async fn seal(&mut self, id: &str, plaintext: &[u8]) -> String {
        let scope = self.scope.clone();
        self.seal_in_scope(&scope, id, plaintext).await
    }

    /// Seals under the OLD provider with whatever DEK is ACTIVE for `scope`, creating the scope's first DEK when needed.
    async fn seal_in_scope(&mut self, scope: &Scope, id: &str, plaintext: &[u8]) -> String {
        let tx = self.db.transaction().await.unwrap();
        self.store.ensure_active_in(&tx, scope, &self.old_provider).await.unwrap();
        tx.commit().await.unwrap();
        let location = CellLocation::new("Secret", "value", id).unwrap();
        let mut reservation = self.store.reserve_for_write(&mut self.budget, scope, 1).await.unwrap();
        let tx = self.db.transaction().await.unwrap();
        let frame = self.store.seal_in(&tx, &mut reservation, &location, plaintext, &self.old_provider).await.unwrap();
        tx.commit().await.unwrap();
        frame
    }

    async fn promote(&mut self) -> aip_execution::encryption::WriteDek {
        let tx = self.db.transaction().await.unwrap();
        let epoch = self.store.active_for_write_in(&tx, &self.scope).await.unwrap().epoch();
        let active = self.store.promote_in(&tx, &self.scope, epoch, &self.new_provider).await.unwrap();
        tx.commit().await.unwrap();
        active
    }
}

async fn with_rekey_cleanup<F, Fut>(store: DekStore, extra_schemas: Vec<String>, body: F)
where
    F: FnOnce(DekStore) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let cleanup_schemas =
        [store.deployment_schema().to_owned(), store.infrastructure_schema().to_owned()].into_iter().chain(extra_schemas).collect::<Vec<_>>();
    let result = tokio::spawn(body(store)).await;
    let cleanup = connect().await;
    cleanup.batch_execute("SET lock_timeout='2s'; SET statement_timeout='5s'").await.unwrap();
    for schema in cleanup_schemas {
        cleanup.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.unwrap();
    }
    result.expect("rekey counterexample body panicked after cleanup");
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
    assert_eq!(target.target_key(), "rekey-target-v2:7:service:6:global:5:users:12:email_cipher:2:id:4:User:5:email");

    for changed in [
        RekeyTarget::new("users", "email_cipher", "user_id", "User", "email", Scope::service()),
        RekeyTarget::new("users", "email_cipher", "id", "Account", "email", Scope::service()),
        RekeyTarget::new("users", "email_cipher", "id", "User", "address", Scope::service()),
    ] {
        assert_ne!(target.target_key(), changed.target_key(), "persisted cursor identity must bind the full row/cell mapping");
    }

    let ref_scope = Scope::reference("Organization", "12345").unwrap();
    let ref_target = RekeyTarget::new("secrets", "api_key", "secret_id", "Secret", "apiKey", ref_scope);
    assert_eq!(ref_target.target_key(), "rekey-target-v2:16:ref:Organization:5:12345:7:secrets:7:api_key:9:secret_id:6:Secret:6:apiKey");
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

/// The service connects with the URL as given and sets no `search_path` (`spike_v2_read::connect_owned_with_url`), so a
/// production session resolves bare names through PostgreSQL's default `"$user", public`. A same-named table in `public`
/// must therefore not capture the rekey of the deployment's table.
#[tokio::test]
async fn rekey_ignores_a_public_table_with_the_same_name_under_the_default_search_path() {
    let scope = Scope::service();
    let store = rekey_store("default_path");
    let table = format!("cells_default_path_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    with_rekey_cleanup(store.clone(), vec![], move |store| async move {
        let mut fixture = rekey_fixture(store.clone(), scope.clone()).await;
        let search_path: String = fixture.db.query_one("SHOW search_path", &[]).await.unwrap().get(0);
        assert_eq!(search_path, "\"$user\", public", "the test needs the server default, the same one a service session gets");
        fixture
            .db
            .batch_execute(&format!(
                "CREATE TABLE {}.{table}(id bigint PRIMARY KEY, value text NOT NULL); CREATE TABLE public.{table}(id bigint PRIMARY KEY, value text NOT NULL)",
                store.deployment_schema()
            ))
            .await
            .unwrap();
        let old_frame = fixture.seal("1", b"default-path-secret").await;
        for schema in [store.deployment_schema(), "public"] {
            fixture.db.execute(&format!("INSERT INTO {schema}.{table} VALUES(1,$1)"), &[&old_frame]).await.unwrap();
        }
        let active = fixture.promote().await;
        let target = RekeyTarget::new(table.as_str(), "value", "id", "Secret", "value", scope.clone());
        let context = RekeyContext {
            store: &store,
            old_dek: &fixture.old_dek,
            active_dek: &active,
            holder_id: "default-path-worker",
            provider: &fixture.new_provider,
        };
        let report = rekey_target_column(&mut fixture.db, &mut fixture.budget, &context, &target).await;
        let intended: Option<String> = fixture.db.query_opt(&format!("SELECT value FROM {}.{table} WHERE id=1", store.deployment_schema()), &[]).await.unwrap().map(|row| row.get(0));
        let public: Option<String> = fixture.db.query_opt(&format!("SELECT value FROM public.{table} WHERE id=1"), &[]).await.unwrap().map(|row| row.get(0));
        fixture.db.batch_execute(&format!("DROP TABLE public.{table}")).await.unwrap();
        let report = report.unwrap();
        assert_eq!(report.rekeyed, 1, "{report:?}");
        assert!(
            intended.as_deref().is_some_and(|frame| aip_crypto::cell::parse_header(frame).is_ok_and(|h| h.dek_id == active.dek().id())),
            "the deployment table was not rekeyed"
        );
        assert_eq!(public.as_deref(), Some(old_frame.as_str()), "the public table with the same name was rewritten");
    })
    .await;
}

#[tokio::test]
async fn rekey_schema_shadow_changes_only_the_intended_schema() {
    let scope = Scope::service();
    let store = rekey_store("schema_shadow");
    let decoy = format!("{}_decoy", store.deployment_schema());
    with_rekey_cleanup(store.clone(), vec![decoy.clone()], move |store| async move {
        let mut fixture = rekey_fixture(store.clone(), scope.clone()).await;
        fixture
            .db
            .batch_execute(&format!(
                "CREATE SCHEMA {decoy};
             CREATE TABLE {}.cells(id bigint PRIMARY KEY, value text NOT NULL, note text NOT NULL);
             CREATE TABLE {decoy}.cells(id bigint PRIMARY KEY, value text NOT NULL, note text NOT NULL);
             SET search_path TO {decoy},{},public",
                store.deployment_schema(),
                store.deployment_schema()
            ))
            .await
            .unwrap();
        let old_frame = fixture.seal("1", b"schema-shadow-secret").await;
        for schema in [store.deployment_schema(), decoy.as_str()] {
            fixture.db.execute(&format!("INSERT INTO {schema}.cells VALUES(1,$1,'preserve-this-column')"), &[&old_frame]).await.unwrap();
        }
        let active = fixture.promote().await;
        let target = RekeyTarget::new("cells", "value", "id", "Secret", "value", scope.clone());
        let context = RekeyContext {
            store: &store,
            old_dek: &fixture.old_dek,
            active_dek: &active,
            holder_id: "schema-shadow-worker",
            provider: &fixture.new_provider,
        };
        let report = rekey_target_column(&mut fixture.db, &mut fixture.budget, &context, &target).await.unwrap();
        let intended: String =
            fixture.db.query_one(&format!("SELECT value FROM {}.cells WHERE id=1", store.deployment_schema()), &[]).await.unwrap().get(0);
        let decoy_after: String = fixture.db.query_one(&format!("SELECT value FROM {decoy}.cells WHERE id=1"), &[]).await.unwrap().get(0);
        let intended_note: String =
            fixture.db.query_one(&format!("SELECT note FROM {}.cells WHERE id=1", store.deployment_schema()), &[]).await.unwrap().get(0);
        let decoy_note: String = fixture.db.query_one(&format!("SELECT note FROM {decoy}.cells WHERE id=1"), &[]).await.unwrap().get(0);
        let intended_rows: i64 =
            fixture.db.query_one(&format!("SELECT count(*) FROM {}.cells", store.deployment_schema()), &[]).await.unwrap().get(0);
        let decoy_rows: i64 = fixture.db.query_one(&format!("SELECT count(*) FROM {decoy}.cells"), &[]).await.unwrap().get(0);
        let mut violations = Vec::new();
        if !aip_crypto::cell::parse_header(&intended).is_ok_and(|h| h.dek_id == active.dek().id()) {
            violations.push("intended frame not on active DEK");
        }
        if decoy_after != old_frame {
            violations.push("decoy frame bytes changed");
        }
        if intended_note != "preserve-this-column" || decoy_note != "preserve-this-column" {
            violations.push("unrelated column changed");
        }
        if intended_rows != 1 || decoy_rows != 1 || report.rekeyed != 1 {
            violations.push("row count or selected work count changed");
        }
        assert!(
            violations.is_empty(),
            "schema shadow counterexample: {violations:?}; intended_frame_changed={}; decoy_frame_changed={}",
            intended != old_frame,
            decoy_after != old_frame
        );
    })
    .await;
}

const REF_SCOPE_DDL: &str = "CREATE TABLE {s}.teams(id bigint PRIMARY KEY);
     INSERT INTO {s}.teams VALUES(42),(99);
     CREATE TABLE {s}.secrets(id bigint PRIMARY KEY, team_id bigint NOT NULL REFERENCES {s}.teams(id), value text);
     SET search_path TO {s},public";

async fn ref_scope_fixture(store: DekStore) -> (RekeyFixture, Scope) {
    let scope = Scope::reference("Team", "42").unwrap();
    let fixture = rekey_fixture(store.clone(), scope.clone()).await;
    fixture.db.batch_execute(&REF_SCOPE_DDL.replace("{s}", store.deployment_schema())).await.unwrap();
    (fixture, scope)
}

/// Rows of another ref that are validly sealed under that ref's own DEK are neither rewritten nor counted as failures.
/// The report may still count them as scanned: `RekeyTarget` has no ref column, so the scan cannot be narrowed to Team 42.
#[tokio::test]
async fn rekey_ref_scope_leaves_validly_sealed_rows_of_other_refs_untouched() {
    let store = rekey_store("ref_scope");
    with_rekey_cleanup(store.clone(), vec![], move |store| async move {
        let (mut fixture, scope) = ref_scope_fixture(store.clone()).await;
        let team99 = Scope::reference("Team", "99").unwrap();
        let team42_frame = fixture.seal("1", b"team-42-secret").await;
        let team99_frame = fixture.seal_in_scope(&team99, "2", b"team-99-secret").await;
        fixture
            .db
            .execute(
                &format!("INSERT INTO {}.secrets VALUES(1,42,$1),(2,99,$2),(3,99,NULL)", store.deployment_schema()),
                &[&team42_frame, &team99_frame],
            )
            .await
            .unwrap();
        let active = fixture.promote().await;
        let target = RekeyTarget::new("secrets", "value", "id", "Secret", "value", scope.clone());
        let context = RekeyContext {
            store: &store,
            old_dek: &fixture.old_dek,
            active_dek: &active,
            holder_id: "ref-scope-worker",
            provider: &fixture.new_provider,
        };
        let report = rekey_target_column(&mut fixture.db, &mut fixture.budget, &context, &target).await.unwrap();
        let rows = fixture.db.query(&format!("SELECT team_id,value FROM {}.secrets ORDER BY id", store.deployment_schema()), &[]).await.unwrap();
        let after_42: String = rows[0].get(1);
        let (team99_id, after_99): (i64, String) = (rows[1].get(0), rows[1].get(1));
        let (null_team_id, after_null): (i64, Option<String>) = (rows[2].get(0), rows[2].get(1));
        let location_99 = CellLocation::new("Secret", "value", "2").unwrap();
        let team99_plaintext = store.open_cell(&fixture.db, &team99, &location_99, &after_99, &fixture.old_provider).await;
        let mut violations = Vec::new();
        if report.rekeyed != 1 || report.failed_count != 0 || !report.failed_ids.is_empty() || report.remaining_old != 0 {
            violations.push("Team 42 rekey was not clean");
        }
        if !aip_crypto::cell::parse_header(&after_42).is_ok_and(|h| h.dek_id == active.dek().id()) {
            violations.push("Team 42 frame was not rekeyed");
        }
        if rows.len() != 3 || team99_id != 99 || after_99 != team99_frame || null_team_id != 99 || after_null.is_some() {
            violations.push("Team 99 rows or frame/NULL bytes changed");
        }
        if !team99_plaintext.is_ok_and(|plaintext| plaintext.as_slice() == b"team-99-secret") {
            violations.push("Team 99 cell no longer opens under its own scope");
        }
        assert!(violations.is_empty(), "mixed-ref counterexample: {violations:?}; report={report:?}");
    })
    .await;
}

/// A frame that does not parse cannot be attributed to a ref without the ref column, so rekey fails closed: it reports the
/// row and keeps the old DEK from retiring, but never changes the bytes. Once `RekeyTarget` binds a ref column, rows of other
/// refs fall out of the scan and this test must be replaced by one that expects `failed_count == 0`.
#[tokio::test]
async fn rekey_ref_scope_malformed_frame_of_another_ref_fails_closed_without_changing_bytes() {
    let store = rekey_store("ref_malformed");
    with_rekey_cleanup(store.clone(), vec![], move |store| async move {
        let (mut fixture, scope) = ref_scope_fixture(store.clone()).await;
        let team42_frame = fixture.seal("1", b"team-42-secret").await;
        let malformed_team99 = "malformed-frame-preserve-bytes".to_owned();
        fixture
            .db
            .execute(
                &format!("INSERT INTO {}.secrets VALUES(1,42,$1),(2,99,$2),(3,99,NULL)", store.deployment_schema()),
                &[&team42_frame, &malformed_team99],
            )
            .await
            .unwrap();
        let active = fixture.promote().await;
        let target = RekeyTarget::new("secrets", "value", "id", "Secret", "value", scope.clone());
        let context = RekeyContext {
            store: &store,
            old_dek: &fixture.old_dek,
            active_dek: &active,
            holder_id: "ref-malformed-worker",
            provider: &fixture.new_provider,
        };
        let report = rekey_target_column(&mut fixture.db, &mut fixture.budget, &context, &target).await.unwrap();
        let rows = fixture.db.query(&format!("SELECT team_id,value FROM {}.secrets ORDER BY id", store.deployment_schema()), &[]).await.unwrap();
        let after_42: String = rows[0].get(1);
        let after_99: String = rows[1].get(1);
        let after_null: Option<String> = rows[2].get(1);
        assert_eq!(report.rekeyed, 1, "{report:?}");
        // Each pass re-reports the row, so the id may repeat; only that one id may appear.
        assert!(
            !report.failed_ids.is_empty() && report.failed_ids.iter().all(|id| id == "2"),
            "the unattributable row is reported, not skipped: {report:?}"
        );
        assert!(report.failed_count > 0, "failure must keep the old DEK from retiring: {report:?}");
        assert!(aip_crypto::cell::parse_header(&after_42).is_ok_and(|h| h.dek_id == active.dek().id()), "Team 42 frame was not rekeyed");
        assert_eq!(after_99, malformed_team99, "malformed frame bytes must be preserved");
        assert!(after_null.is_none(), "NULL must stay NULL");
        assert_eq!(rows[1].get::<_, i64>(0), 99);
    })
    .await;
}

/// Known gap, not a regression: the CAS compares only the id and the old frame, so a row whose ref column was changed by
/// direct SQL between the page SELECT and the UPDATE is still rewritten under the old ref's DEK. The runtime never changes a
/// ref field (docs/design/13-field-encryption.md, ref field row), so this needs an out-of-band writer. Fixing it needs a ref
/// column on `RekeyTarget` (plan-docs/hardening-1.0/crypto-rekey.md). Run with `--ignored` to see the counterexample.
///
/// The race is forced with a row lock instead of a hook in the rekey loop: the ref change is made but not committed, so the
/// page SELECT still sees Team 42 and the CAS UPDATE queues behind the lock until the change commits.
#[tokio::test]
#[ignore = "known gap: RekeyTarget has no ref column, see plan-docs/hardening-1.0/crypto-rekey.md"]
async fn rekey_ref_change_after_select_is_rejected_by_cas_scope() {
    let store = rekey_store("ref_cas");
    let deployment_schema = store.deployment_schema().to_owned();
    with_rekey_cleanup(store.clone(), vec![], move |store| async move {
        let (mut fixture, scope) = ref_scope_fixture(store.clone()).await;
        let old_frame = fixture.seal("1", b"scope-race-secret").await;
        fixture.db.execute(&format!("INSERT INTO {}.secrets VALUES(1,42,$1)", store.deployment_schema()), &[&old_frame]).await.unwrap();
        let active = fixture.promote().await;
        let target = RekeyTarget::new("secrets", "value", "id", "Secret", "value", scope.clone());

        let mover = connect().await;
        let mover_pid: i32 = mover.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
        mover.batch_execute(&format!("BEGIN; UPDATE {deployment_schema}.secrets SET team_id=99 WHERE id=1")).await.unwrap();
        let rekey_task = tokio::spawn(async move {
            let context = RekeyContext {
                store: &store,
                old_dek: &fixture.old_dek,
                active_dek: &active,
                holder_id: "ref-cas-worker",
                provider: &fixture.new_provider,
            };
            let result = rekey_target_column(&mut fixture.db, &mut fixture.budget, &context, &target).await;
            (fixture, result)
        });
        let observer = connect().await;
        let mut waited = Duration::ZERO;
        loop {
            let blocked: i64 = observer
                .query_one("SELECT count(*) FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))", &[&mover_pid])
                .await
                .unwrap()
                .get(0);
            if blocked == 1 {
                break;
            }
            assert!(!rekey_task.is_finished(), "rekey returned without queueing behind the ref change");
            assert!(waited < Duration::from_secs(10), "rekey never reached the CAS UPDATE");
            tokio::time::sleep(Duration::from_millis(20)).await;
            waited += Duration::from_millis(20);
        }
        mover.batch_execute("COMMIT").await.unwrap();
        let (fixture, result) =
            tokio::time::timeout(Duration::from_secs(10), rekey_task).await.expect("rekey did not finish after the lock released").unwrap();
        let report = result.unwrap();
        let row = fixture.db.query_one(&format!("SELECT team_id,value FROM {deployment_schema}.secrets WHERE id=1"), &[]).await.unwrap();
        let team_id: i64 = row.get(0);
        let frame: String = row.get(1);
        let violations = [
            (team_id == 99, "the ref change did not commit"),
            (frame == old_frame, "CAS changed the frame after its selected ref changed"),
            (report.raced == 1, "stale selected row was not reported as raced"),
        ]
        .into_iter()
        .filter_map(|(ok, message)| (!ok).then_some(message))
        .collect::<Vec<_>>();
        assert!(
            violations.is_empty(),
            "ref/CAS counterexample: {violations:?}; ref={team_id} frame_changed={} raced={}",
            frame != old_frame,
            report.raced
        );
    })
    .await;
}

#[test]
fn rekey_checkpoint_identity_does_not_bind_deployment_schema() {
    let scope = Scope::reference("Team", "42").unwrap();
    let target_a = RekeyTarget::new("secrets", "value", "id", "Secret", "value", scope.clone());
    let target_b = RekeyTarget::new("secrets", "value", "id", "Secret", "value", scope);
    let store_a = DekStore::new("aip_rekey_identity_a").unwrap();
    let store_b = DekStore::new("aip_rekey_identity_b").unwrap();
    assert_ne!(store_a.deployment_schema(), store_b.deployment_schema());
    assert_eq!(target_a.target_key(), target_b.target_key(), "existing target identity has no deployment schema input");
}

#[test]
fn rekey_existing_api_accepts_ref_scope_without_a_ref_column_selector() {
    let scope = Scope::reference("Team", "42").unwrap();
    let target = RekeyTarget::new("secrets", "value", "id", "Secret", "value", scope);
    assert_eq!(target.id_column, "id");
    assert!(target.target_key().starts_with("rekey-target-v2:"));
}
