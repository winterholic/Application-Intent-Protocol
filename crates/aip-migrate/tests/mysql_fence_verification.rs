#![cfg(feature = "mysql")]

use aip_migrate::mysql_driver::{MysqlDriver, canonical_bound_tables, catalog_fingerprint_on_conn};
use mysql_async::prelude::Queryable;

/// This live test intentionally starts with an incomplete caller-supplied bound set.
/// A successful fence here would permit DDL after catalog validation on an unpinned table.
#[tokio::test]
#[ignore = "requires a disposable MySQL or MariaDB test container in AIP_TEST_MYSQL_URL"]
async fn live_fence_rejects_incomplete_bound_table_set() {
    let url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL");
    let driver = MysqlDriver::connect(&url).await.expect("connect test driver");
    let schema = "aip_stage6_incomplete_bounds_test";
    let mut setup = driver.pool().get_conn().await.expect("get setup connection");
    setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    setup.query_drop(format!("CREATE DATABASE `{schema}`")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{schema}`.`aip_migrate_meta` (singleton TINYINT PRIMARY KEY, facts_digest VARCHAR(64) NOT NULL, schema_sha256 VARCHAR(64) NOT NULL, status VARCHAR(32) NOT NULL) ENGINE=InnoDB")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{schema}`.`aip_principals` (id VARCHAR(36) PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{schema}`.`items` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{schema}`.`empty_rows` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
    let facts = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    setup.exec_drop(format!("INSERT INTO `{schema}`.`aip_migrate_meta` VALUES (1, ?, 'structure', 'READY')"), (facts,)).await.unwrap();
    setup.query_drop(format!("INSERT INTO `{schema}`.`items` VALUES (1)")).await.unwrap();

    let digest = driver.catalog_fingerprint(schema).await.expect("catalog fingerprint");

    // Compare the four same-connection catalog reads with the existing table-at-a-time
    // introspector on both server families.
    let mut catalog_conn = driver.pool().get_conn().await.expect("catalog connection");
    let four_query = catalog_fingerprint_on_conn(&mut catalog_conn, schema, url.starts_with("mariadb://"))
        .await
        .expect("four-query same-connection catalog");
    let legacy_catalog = driver.introspect(schema).await.expect("legacy catalog");
    assert_eq!(four_query.catalog, legacy_catalog);
    assert_eq!(four_query.digest, digest);
    drop(catalog_conn);

    let incomplete = canonical_bound_tables(schema, schema, &[], false);
    let result = driver.begin_fenced_transaction(true, schema, schema, &incomplete, facts, &digest).await;
    let error = match result {
        Ok(tx) => {
            tx.rollback().await.expect("rollback unexpectedly accepted incomplete fence");
            panic!("fence accepted a bound set that omitted source tables");
        }
        Err(error) => error,
    };
    assert_eq!(error.code, "MIGRATE.DRIFT_DETECTED");

    // Both populated and empty tables belong in the same canonical set. A successful
    // fenced transaction must hold MDL against ALTER until rollback releases it.
    let source_tables = vec!["items".to_string(), "empty_rows".to_string()];
    let complete = canonical_bound_tables(schema, schema, &source_tables, false);
    let tx = driver
        .begin_fenced_transaction(true, schema, schema, &complete, facts, &digest)
        .await
        .expect("complete fence");
    let mut ddl = driver.pool().get_conn().await.expect("DDL connection");
    ddl.query_drop("SET SESSION lock_wait_timeout = 1").await.unwrap();
    let blocked = ddl
        .query_drop(format!("ALTER TABLE `{schema}`.`empty_rows` ADD COLUMN marker INT NULL"))
        .await
        .expect_err("ALTER must block on an empty table's MDL");
    assert!(blocked.to_string().contains("1205") || blocked.to_string().contains("Lock wait timeout"), "unexpected DDL error: {blocked}");
    let blocked_nonempty = ddl
        .query_drop(format!("ALTER TABLE `{schema}`.`items` ADD COLUMN marker INT NULL"))
        .await
        .expect_err("ALTER must block on a populated table's MDL");
    assert!(blocked_nonempty.to_string().contains("1205") || blocked_nonempty.to_string().contains("Lock wait timeout"), "unexpected DDL error: {blocked_nonempty}");
    tx.rollback().await.expect("rollback fenced transaction");
    ddl.query_drop(format!("ALTER TABLE `{schema}`.`empty_rows` ADD COLUMN marker INT NULL"))
        .await
        .expect("DDL proceeds after rollback releases MDL");

    let alter_drift = driver
        .begin_fenced_transaction(true, schema, schema, &complete, facts, &digest)
        .await
        .expect_err("next request must reject the preceding ALTER");
    assert_eq!(alter_drift.code, "MIGRATE.DRIFT_DETECTED");
    ddl.query_drop(format!("DROP TABLE `{schema}`.`items`")).await.unwrap();
    let drop_drift = driver
        .begin_fenced_transaction(true, schema, schema, &complete, facts, &digest)
        .await
        .expect_err("missing previously bound table must fail closed");
    assert_eq!(drop_drift.code, "MIGRATE.DRIFT_DETECTED");
    drop(ddl);

    setup.query_drop(format!("DROP DATABASE `{schema}`")).await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MySQL or MariaDB test container in AIP_TEST_MYSQL_URL"]
async fn live_cancelled_fenced_transaction_rolls_back_before_connection_reuse() {
    let raw_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL");
    let url = format!("{raw_url}?pool_max=1");
    let driver = MysqlDriver::connect(&url).await.expect("connect single-connection pool");
    let schema = "aip_stage6_into_conn_test";
    let mut setup = driver.pool().get_conn().await.expect("get setup connection");
    setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    setup.query_drop(format!("CREATE DATABASE `{schema}`")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{schema}`.`aip_migrate_meta` (singleton TINYINT PRIMARY KEY, facts_digest VARCHAR(64) NOT NULL, schema_sha256 VARCHAR(64) NOT NULL, status VARCHAR(32) NOT NULL) ENGINE=InnoDB")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{schema}`.`aip_principals` (id VARCHAR(36) PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{schema}`.`items` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
    let facts = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    setup.exec_drop(format!("INSERT INTO `{schema}`.`aip_migrate_meta` VALUES (1, ?, 'structure', 'READY')"), (facts,)).await.unwrap();
    drop(setup);

    let digest = driver.catalog_fingerprint(schema).await.unwrap();
    let bound = canonical_bound_tables(schema, schema, &["items".to_string()], false);
    let tx = driver.begin_fenced_transaction(true, schema, schema, &bound, facts, &digest).await.unwrap();
    let ddl_pool = mysql_async::Pool::new(driver.opts().clone());
    let mut ddl = ddl_pool.get_conn().await.unwrap();
    ddl.query_drop("SET SESSION lock_wait_timeout = 1").await.unwrap();
    let blocked = ddl.query_drop(format!("ALTER TABLE `{schema}`.`items` ADD COLUMN cancel_probe INT NULL")).await;
    match blocked {
        Err(mysql_async::Error::Server(error)) => assert_eq!(error.code, 1205, "expected lock-wait timeout"),
        Err(error) => panic!("expected lock-wait timeout, got {error}"),
        Ok(()) => panic!("DDL was not blocked by the active fence"),
    }

    let task = tokio::spawn(async move {
        let _fenced = tx;
        std::future::pending::<()>().await;
    });
    task.abort();
    let _ = task.await;
    ddl.query_drop(format!("ALTER TABLE `{schema}`.`items` ADD COLUMN cancel_probe INT NULL"))
        .await
        .expect("cancelled fenced task must roll back and release MDL before pool reuse");
    ddl.query_drop(format!("DROP DATABASE `{schema}`")).await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MySQL or MariaDB test container in AIP_TEST_MYSQL_URL"]
async fn live_readwrite_fence_commits_rolls_back_and_runs_with_dml_only_grants() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL");
    let admin = MysqlDriver::connect(&admin_url).await.expect("connect admin test driver");
    let schema = "aip_stage6_rw_lifecycle_test";
    let mut setup = admin.pool().get_conn().await.unwrap();
    setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    setup.query_drop("DROP USER IF EXISTS 'aip_runtime'@'%' ").await.unwrap();
    setup.query_drop(format!("CREATE DATABASE `{schema}`")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{schema}`.`aip_migrate_meta` (singleton TINYINT PRIMARY KEY, facts_digest VARCHAR(64) NOT NULL, schema_sha256 VARCHAR(64) NOT NULL, status VARCHAR(32) NOT NULL) ENGINE=InnoDB")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{schema}`.`aip_principals` (id VARCHAR(36) PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{schema}`.`items` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
    setup.query_drop(format!("CREATE TABLE `{schema}`.`empty_rows` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
    for table in ["aip_idem", "aip_audit", "aip_events", "aip_tx_locks", "aip_jobs", "aip_outbox"] {
        setup.query_drop(format!("CREATE TABLE `{schema}`.`{table}` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
    }
    let facts = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    setup.exec_drop(format!("INSERT INTO `{schema}`.`aip_migrate_meta` VALUES (1, ?, 'structure', 'READY')"), (facts,)).await.unwrap();
    setup.query_drop("CREATE USER 'aip_runtime'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    setup.query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE ON `{schema}`.* TO 'aip_runtime'@'%' ")).await.unwrap();
    drop(setup);

    let runtime_url = admin_url
        .replace("root:ddltestroot", "aip_runtime:runtime_test_only")
        .replace("/mysql", &format!("/{schema}"));
    let runtime = MysqlDriver::from_url(&runtime_url).expect("parse least-privilege runtime config");
    let _connection_probe = runtime.pool().get_conn().await.expect("connect least-privilege runtime driver");
    let catalog = runtime.introspect(schema).await.unwrap();
    let source_tables = catalog.keys().cloned().collect::<Vec<_>>();
    let bounds = canonical_bound_tables(schema, schema, &source_tables, true);
    let digest = runtime.catalog_fingerprint(schema).await.unwrap();

    let mut tx = runtime.begin_fenced_transaction(false, schema, schema, &bounds, facts, &digest).await.unwrap();
    tx.conn().exec_drop(format!("INSERT INTO `{schema}`.`items` (id) VALUES (1)"), ()).await.unwrap();
    tx.commit().await.unwrap();
    let mut verify = runtime.pool().get_conn().await.unwrap();
    let count: i64 = verify.query_first(format!("SELECT COUNT(*) FROM `{schema}`.`items`")).await.unwrap().unwrap();
    assert_eq!(count, 1, "commit must make the write visible");
    drop(verify);

    let mut tx = runtime.begin_fenced_transaction(false, schema, schema, &bounds, facts, &digest).await.unwrap();
    tx.conn().exec_drop(format!("INSERT INTO `{schema}`.`items` (id) VALUES (2)"), ()).await.unwrap();
    tx.rollback().await.unwrap();
    let mut verify = runtime.pool().get_conn().await.unwrap();
    let count: i64 = verify.query_first(format!("SELECT COUNT(*) FROM `{schema}`.`items`")).await.unwrap().unwrap();
    assert_eq!(count, 1, "rollback must discard the write");
    drop(verify);

    let mut tx = runtime.begin_fenced_transaction(false, schema, schema, &bounds, facts, &digest).await.unwrap();
    tx.conn().exec_drop(format!("INSERT INTO `{schema}`.`items` (id) VALUES (3)"), ()).await.unwrap();
    drop(tx);
    let mut verify = runtime.pool().get_conn().await.unwrap();
    let count: i64 = verify.query_first(format!("SELECT COUNT(*) FROM `{schema}`.`items`")).await.unwrap().unwrap();
    assert_eq!(count, 1, "dropping an active write transaction must roll it back");
    drop(verify);

    // Verify that the runtime grant has no DDL privilege.
    let mut runtime_conn = runtime.pool().get_conn().await.unwrap();
    let ddl = runtime_conn.query_drop(format!("ALTER TABLE `{schema}`.`items` ADD COLUMN forbidden INT NULL")).await;
    assert!(ddl.is_err(), "runtime DML role unexpectedly has ALTER privilege");
    drop(runtime_conn);

    let mut cleanup = admin.pool().get_conn().await.unwrap();
    cleanup.query_drop(format!("DROP DATABASE `{schema}`")).await.unwrap();
    cleanup.query_drop("DROP USER 'aip_runtime'@'%' ").await.unwrap();
}
