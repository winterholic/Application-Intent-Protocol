#![cfg(feature = "mysql")]
//! Stage 6 (WBS 7.2.1.1) DDL-race safety contract for the MySQL/MariaDB driver.
//!
//! Offline tests always run. Live tests read `AIP_TEST_MYSQL_URL` and/or `AIP_TEST_MARIADB_URL`
//! (disposable containers only, admin account) and print `SKIPPED` when neither is set.

use aip_migrate::mysql_driver::{MysqlCatalogFingerprint, MysqlDeploymentFence, MysqlDriver, canonical_bound_tables};
use mysql_async::prelude::Queryable;

const FACTS: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const DRIFT: &str = "MIGRATE.DRIFT_DETECTED";

fn live_urls() -> Vec<String> {
    let urls: Vec<String> = ["AIP_TEST_MYSQL_URL", "AIP_TEST_MARIADB_URL"].iter().filter_map(|key| std::env::var(key).ok()).collect();
    if urls.is_empty() {
        eprintln!("SKIPPED: set AIP_TEST_MYSQL_URL and/or AIP_TEST_MARIADB_URL to run live fence tests");
    }
    urls
}

fn is_lock_wait_timeout(error: &mysql_async::Error) -> bool {
    matches!(error, mysql_async::Error::Server(server) if server.code == 1205)
}

fn runtime_url(admin_url: &str, database: &str) -> String {
    let (scheme, rest) = admin_url.split_once("://").expect("url scheme");
    let (_, host_part) = rest.split_once('@').expect("url credentials");
    let host = host_part.split('/').next().expect("url host");
    format!("{scheme}://aip_runtime:runtime_test_only@{host}/{database}")
}

/// Create a source schema with two tables and bootstrap the meta schema around it.
async fn deploy(driver: &MysqlDriver, meta: &str, source: &str) -> MysqlCatalogFingerprint {
    let mut setup = driver.pool().get_conn().await.expect("setup connection");
    for schema in [meta, source] {
        setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
    setup.query_drop(format!("CREATE DATABASE `{source}`")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{source}`.`items` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
    setup
        .query_drop(format!(
            "CREATE TABLE `{source}`.`children` (id INT PRIMARY KEY, item_id INT NULL, \
             CONSTRAINT fk_children_item FOREIGN KEY (item_id) REFERENCES `{source}`.`items` (id)) ENGINE=InnoDB"
        ))
        .await
        .unwrap();
    drop(setup);
    driver.ensure_meta_schema_bootstrap(meta, source, FACTS).await.expect("meta bootstrap")
}

async fn cleanup(driver: &MysqlDriver, schemas: &[&str]) {
    let mut conn = driver.pool().get_conn().await.expect("cleanup connection");
    for schema in schemas {
        conn.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
}

#[tokio::test]
async fn deployment_fence_freezes_canonical_read_and_write_sets_at_construction() {
    let tables = vec!["orders".to_string(), "users".to_string(), "orders".to_string()];
    let fence = MysqlDeploymentFence::new("aip_meta", "app", &tables, FACTS, "structure").unwrap();

    assert_eq!(fence.bound_tables_for(true), canonical_bound_tables("aip_meta", "app", &tables, false).as_slice());
    assert_eq!(fence.bound_tables_for(false), canonical_bound_tables("aip_meta", "app", &tables, true).as_slice());
    assert_eq!(fence.bound_tables_for(true).len(), 2 + 2, "read set is 2 meta tables plus every source table");
    assert_eq!(fence.bound_tables_for(false).len(), 8 + 2, "write set is 8 meta tables plus every source table");
    assert_eq!(fence.meta_schema(), "aip_meta");
    assert_eq!(fence.source_schema(), "app");
    assert_eq!(fence.expected_facts_digest(), FACTS);
    assert_eq!(fence.expected_structure_digest(), "structure");
}

#[tokio::test]
async fn deployment_fence_rejects_empty_or_unsafe_inputs_at_construction() {
    let one = vec!["items".to_string()];
    let cases: Vec<Result<MysqlDeploymentFence, _>> = vec![
        MysqlDeploymentFence::new("", "app", &one, FACTS, "structure"),
        MysqlDeploymentFence::new("aip_meta", " ", &one, FACTS, "structure"),
        MysqlDeploymentFence::new("aip_meta", "app", &[], FACTS, "structure"),
        MysqlDeploymentFence::new("aip_meta", "app", &one, "", "structure"),
        MysqlDeploymentFence::new("aip_meta", "app", &one, FACTS, ""),
        MysqlDeploymentFence::new("aip`meta", "app", &one, FACTS, "structure"),
        MysqlDeploymentFence::new("aip_meta", "app", &["bad`table".to_string()], FACTS, "structure"),
    ];
    for (index, case) in cases.into_iter().enumerate() {
        assert_eq!(case.expect_err(&format!("case {index} must be rejected")).code, DRIFT, "case {index}");
    }
}

#[tokio::test]
async fn empty_bound_tables_are_rejected_before_any_connection_is_used() {
    // The pool is lazy and nothing listens on port 1: only Step 0 can produce DRIFT here.
    let driver = MysqlDriver::from_url("mysql://nobody:nothing@127.0.0.1:1/unused").unwrap();
    let error = driver.begin_fenced_transaction(true, "aip_meta", "app", &[], FACTS, "structure").await.unwrap_err();
    assert_eq!(error.code, DRIFT);
    assert!(error.to_string().contains("bound_tables must not be empty"), "{error}");
}

/// Counter-example 1: a bound set that omits source tables must never yield a transaction,
/// and a complete set must block external DDL on every source table.
#[tokio::test]
async fn live_fence_rejects_incomplete_bound_table_set() {
    for url in live_urls() {
        let (meta, source) = ("aip_s6_incomplete_meta", "aip_s6_incomplete_src");
        let driver = MysqlDriver::connect(&url).await.expect("connect test driver");
        let fingerprint = deploy(&driver, meta, source).await;
        let source_tables: Vec<String> = fingerprint.catalog.keys().cloned().collect();
        assert_eq!(source_tables, ["children", "items"]);

        // Slice API: `children` omitted (the exact 8a27b94 defect).
        let missing_children = canonical_bound_tables(meta, source, &["items".to_string()], false);
        let error = driver.begin_fenced_transaction(true, meta, source, &missing_children, FACTS, &fingerprint.digest).await.unwrap_err();
        assert_eq!(error.code, DRIFT, "{url}: {error}");

        // Slice API: meta-only set.
        let meta_only = canonical_bound_tables(meta, source, &[], false);
        let error = driver.begin_fenced_transaction(true, meta, source, &meta_only, FACTS, &fingerprint.digest).await.unwrap_err();
        assert_eq!(error.code, DRIFT, "{url}: {error}");

        // Slice API: read-only set passed to a read-write transaction (missing write meta tables).
        let read_set = canonical_bound_tables(meta, source, &source_tables, false);
        let error = driver.begin_fenced_transaction(false, meta, source, &read_set, FACTS, &fingerprint.digest).await.unwrap_err();
        assert_eq!(error.code, DRIFT, "{url}: {error}");

        // Fence frozen from a stale/partial table list is still caught by the live Step 2c check.
        let partial = MysqlDeploymentFence::new(meta, source, &["items".to_string()], FACTS, &fingerprint.digest).unwrap();
        let error = driver.begin_fenced_transaction_with_fence(true, &partial).await.unwrap_err();
        assert_eq!(error.code, DRIFT, "{url}: {error}");

        // Rejected attempts must have rolled back: the DDL below would time out if any MDL leaked.
        // The complete fence must block external ALTER on every source table until rollback.
        let fence = MysqlDeploymentFence::from_fingerprint(meta, source, FACTS, &fingerprint).unwrap();
        let tx = driver.begin_fenced_transaction_with_fence(true, &fence).await.expect("complete fence");
        let admin = MysqlDriver::connect(&url).await.unwrap();
        let mut ddl = admin.pool().get_conn().await.unwrap();
        ddl.query_drop("SET SESSION lock_wait_timeout = 1").await.unwrap();
        for table in &source_tables {
            let blocked = ddl.query_drop(format!("ALTER TABLE `{source}`.`{table}` ADD COLUMN rogue_col INT NULL")).await;
            assert!(blocked.as_ref().err().is_some_and(is_lock_wait_timeout), "{url}: ALTER on {table} must hit 1205, got {blocked:?}");
        }
        tx.rollback().await.expect("rollback fenced transaction");
        ddl.query_drop(format!("ALTER TABLE `{source}`.`children` ADD COLUMN rogue_col INT NULL")).await.expect("DDL proceeds after rollback");

        let drift = driver.begin_fenced_transaction_with_fence(true, &fence).await.unwrap_err();
        assert_eq!(drift.code, DRIFT, "{url}: next request after external ALTER must be rejected");
        drop(ddl);
        cleanup(&driver, &[meta, source]).await;
    }
}

#[tokio::test]
async fn live_fence_detects_dropped_and_created_source_tables_before_returning() {
    for url in live_urls() {
        let (meta, source) = ("aip_s6_membership_meta", "aip_s6_membership_src");
        let driver = MysqlDriver::connect(&url).await.expect("connect test driver");
        let fingerprint = deploy(&driver, meta, source).await;
        let fence = MysqlDeploymentFence::from_fingerprint(meta, source, FACTS, &fingerprint).unwrap();
        driver.begin_fenced_transaction_with_fence(true, &fence).await.expect("baseline fence").rollback().await.unwrap();

        let mut ddl = driver.pool().get_conn().await.unwrap();
        ddl.query_drop(format!("CREATE TABLE `{source}`.`late_table` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
        let created = driver.begin_fenced_transaction_with_fence(true, &fence).await.unwrap_err();
        assert_eq!(created.code, DRIFT, "{url}: {created}");
        ddl.query_drop(format!("DROP TABLE `{source}`.`late_table`")).await.unwrap();
        driver.begin_fenced_transaction_with_fence(true, &fence).await.expect("fence recovers once the catalog matches again").rollback().await.unwrap();

        ddl.query_drop(format!("DROP TABLE `{source}`.`children`")).await.unwrap();
        let dropped = driver.begin_fenced_transaction_with_fence(true, &fence).await.unwrap_err();
        assert_eq!(dropped.code, DRIFT, "{url}: {dropped}");
        drop(ddl);
        cleanup(&driver, &[meta, source]).await;
    }
}

#[tokio::test]
async fn live_readwrite_fence_commits_rolls_back_and_runs_with_dml_only_grants() {
    for admin_url in live_urls() {
        let (meta, source) = ("aip_s6_rw_meta", "aip_s6_rw_src");
        let admin = MysqlDriver::connect(&admin_url).await.expect("connect admin test driver");
        let fingerprint = deploy(&admin, meta, source).await;
        let mut setup = admin.pool().get_conn().await.unwrap();
        setup.query_drop("DROP USER IF EXISTS 'aip_runtime'@'%'").await.unwrap();
        setup.query_drop("CREATE USER 'aip_runtime'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
        for schema in [meta, source] {
            setup.query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE ON `{schema}`.* TO 'aip_runtime'@'%'")).await.unwrap();
        }
        drop(setup);

        let runtime = MysqlDriver::from_url(&runtime_url(&admin_url, source)).expect("runtime config");
        let fence = MysqlDeploymentFence::from_fingerprint(meta, source, FACTS, &fingerprint).unwrap();
        let count = |driver: MysqlDriver| async move {
            let mut conn = driver.pool().get_conn().await.unwrap();
            conn.query_first::<i64, _>(format!("SELECT COUNT(*) FROM `{source}`.`items`")).await.unwrap().unwrap()
        };

        let mut tx = runtime.begin_fenced_transaction_with_fence(false, &fence).await.expect("read-write fence");
        assert!(!tx.is_read_only());
        tx.as_mut().unwrap().exec_drop(format!("INSERT INTO `{source}`.`items` (id) VALUES (1)"), ()).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(count(runtime.clone()).await, 1, "commit must make the write visible");

        let mut tx = runtime.begin_fenced_transaction_with_fence(false, &fence).await.unwrap();
        tx.as_mut().unwrap().exec_drop(format!("INSERT INTO `{source}`.`items` (id) VALUES (2)"), ()).await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(count(runtime.clone()).await, 1, "rollback must discard the write");

        let mut read_tx = runtime.begin_fenced_transaction_with_fence(true, &fence).await.expect("read-only fence");
        assert!(read_tx.is_read_only());
        let write_in_read_only = read_tx.as_mut().unwrap().exec_drop(format!("INSERT INTO `{source}`.`items` (id) VALUES (9)"), ()).await;
        assert!(write_in_read_only.is_err(), "{admin_url}: read-only fence must refuse writes");
        read_tx.rollback().await.unwrap();

        let mut runtime_conn = runtime.pool().get_conn().await.unwrap();
        let ddl = runtime_conn.query_drop(format!("ALTER TABLE `{source}`.`items` ADD COLUMN forbidden INT NULL")).await;
        assert!(ddl.is_err(), "runtime DML role unexpectedly has ALTER privilege");
        drop(runtime_conn);

        let mut cleanup_conn = admin.pool().get_conn().await.unwrap();
        cleanup_conn.query_drop("DROP USER 'aip_runtime'@'%'").await.unwrap();
        drop(cleanup_conn);
        cleanup(&admin, &[meta, source]).await;
    }
}

/// Counter-example 2: dropping (or cancelling) an active fenced transaction must roll back
/// before the single pooled connection is reused, and must release the metadata locks.
#[tokio::test]
async fn live_cancelled_fenced_transaction_rolls_back_before_connection_reuse() {
    for url in live_urls() {
        let (meta, source) = ("aip_s6_cancel_meta", "aip_s6_cancel_src");
        let admin = MysqlDriver::connect(&url).await.expect("connect admin driver");
        let fingerprint = deploy(&admin, meta, source).await;
        let fence = MysqlDeploymentFence::from_fingerprint(meta, source, FACTS, &fingerprint).unwrap();
        let single = MysqlDriver::connect(&format!("{url}?pool_max=1")).await.expect("connect single-connection pool");
        let mut ddl = admin.pool().get_conn().await.unwrap();
        ddl.query_drop("SET SESSION lock_wait_timeout = 1").await.unwrap();
        let committed_rows = |driver: MysqlDriver| async move {
            let mut conn = driver.pool().get_conn().await.unwrap();
            conn.query_first::<i64, _>(format!("SELECT COUNT(*) FROM `{source}`.`items`")).await.unwrap().unwrap()
        };

        // Early drop after an uncommitted INSERT.
        let mut fenced = single.begin_fenced_transaction_with_fence(false, &fence).await.unwrap();
        fenced.as_mut().unwrap().exec_drop(format!("INSERT INTO `{source}`.`items` (id) VALUES (1)"), ()).await.unwrap();
        let blocked = ddl.query_drop(format!("ALTER TABLE `{source}`.`items` ADD COLUMN cancel_probe INT NULL")).await;
        assert!(blocked.as_ref().err().is_some_and(is_lock_wait_timeout), "{url}: active fence must block DDL, got {blocked:?}");
        drop(fenced);
        // The next transaction reuses the only connection and must not see the dropped write.
        assert_eq!(committed_rows(single.clone()).await, 0, "{url}: dropped fence leaked an uncommitted row");
        ddl.query_drop(format!("ALTER TABLE `{source}`.`items` ADD COLUMN cancel_probe INT NULL")).await.expect("MDL released after drop");

        // The schema changed on purpose, so refresh the fence the way a redeploy would.
        let fingerprint = admin.ensure_meta_schema_bootstrap(meta, source, FACTS).await.unwrap();
        let fence = MysqlDeploymentFence::from_fingerprint(meta, source, FACTS, &fingerprint).unwrap();

        // Cancellation of the owning task (timeout/abort) takes the same path.
        let mut fenced = single.begin_fenced_transaction_with_fence(false, &fence).await.unwrap();
        fenced.as_mut().unwrap().exec_drop(format!("INSERT INTO `{source}`.`items` (id) VALUES (2)"), ()).await.unwrap();
        let task = tokio::spawn(async move {
            let _held = fenced;
            std::future::pending::<()>().await;
        });
        task.abort();
        let _ = task.await;
        assert_eq!(committed_rows(single.clone()).await, 0, "{url}: aborted task leaked an uncommitted row");
        ddl.query_drop(format!("ALTER TABLE `{source}`.`items` ADD COLUMN cancel_probe2 INT NULL")).await.expect("MDL released after abort");
        drop(ddl);
        cleanup(&admin, &[meta, source]).await;
    }
}

/// Counter-example 3: an interrupted meta migration (`status != READY`, or a missing row)
/// must make runtime entry fail closed until the migration is re-run to completion.
#[tokio::test]
async fn live_interrupted_meta_migration_rejects_entry_until_ready_again() {
    for url in live_urls() {
        let (meta, source) = ("aip_s6_migrating_meta", "aip_s6_migrating_src");
        let driver = MysqlDriver::connect(&url).await.expect("connect test driver");
        let fingerprint = deploy(&driver, meta, source).await;
        let fence = MysqlDeploymentFence::from_fingerprint(meta, source, FACTS, &fingerprint).unwrap();
        driver.begin_fenced_transaction_with_fence(false, &fence).await.expect("READY deployment is enterable").rollback().await.unwrap();

        let mut admin = driver.pool().get_conn().await.unwrap();
        async fn status(conn: &mut mysql_async::Conn, meta: &str) -> Option<String> {
            conn.query_first::<String, _>(format!("SELECT status FROM `{meta}`.`aip_migrate_meta` WHERE singleton = 1")).await.unwrap()
        }
        assert_eq!(status(&mut admin, meta).await.as_deref(), Some("READY"));

        // A migration that starts DDL marks MIGRATING first.
        driver.mark_meta_migrating(meta).await.unwrap();
        assert_eq!(status(&mut admin, meta).await.as_deref(), Some("MIGRATING"));
        for read_only in [true, false] {
            let error = driver.begin_fenced_transaction_with_fence(read_only, &fence).await.unwrap_err();
            assert_eq!(error.code, DRIFT, "{url}: {error}");
            assert!(error.to_string().contains("MIGRATING"), "{url}: {error}");
        }

        // A migration that dies before it can finish (here: unreadable source catalog) stays MIGRATING.
        let failed = driver.ensure_meta_schema_bootstrap(meta, "aip_s6_no_such_source_schema", FACTS).await;
        assert!(failed.is_err(), "{url}: bootstrap over a missing source schema must fail");
        assert_eq!(status(&mut admin, meta).await.as_deref(), Some("MIGRATING"), "{url}: failed bootstrap must not leave READY behind");
        assert_eq!(driver.begin_fenced_transaction_with_fence(true, &fence).await.unwrap_err().code, DRIFT);

        // Re-running the bootstrap against the right schema is idempotent and restores READY.
        let rerun = driver.ensure_meta_schema_bootstrap(meta, source, FACTS).await.expect("idempotent re-run");
        assert_eq!(rerun.digest, fingerprint.digest);
        assert_eq!(status(&mut admin, meta).await.as_deref(), Some("READY"));
        driver.begin_fenced_transaction_with_fence(false, &fence).await.expect("READY again").rollback().await.unwrap();

        // Crash before the marker row exists.
        admin.query_drop(format!("DELETE FROM `{meta}`.`aip_migrate_meta`")).await.unwrap();
        let error = driver.begin_fenced_transaction_with_fence(true, &fence).await.unwrap_err();
        assert_eq!(error.code, DRIFT, "{url}: {error}");
        drop(admin);
        cleanup(&driver, &[meta, source]).await;
    }
}
