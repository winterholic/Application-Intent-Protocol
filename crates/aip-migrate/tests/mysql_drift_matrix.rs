#![cfg(feature = "mysql")]
//! Which out-of-band DDL changes the catalog fingerprint that the request-time fence compares against.
//!
//! Every statement below changes something the engine's reads or writes depend on (types, nullability, defaults,
//! collation, keys, foreign-key actions, storage engine, generated columns, names, ...). If the fingerprint did not change,
//! a deployment would keep serving after the business schema moved under it. Run with `-- --ignored` against a
//! run-owned container (`scripts/mysql-live-fixture.sh`).
use aip_migrate::mysql_driver::{MysqlDriver, mysql_async::prelude::Queryable};

const SCHEMA: &str = "aip_t_drift_matrix";

/// (label, DDL that must be observable, DDL that restores the baseline)
const CASES: &[(&str, &str, &str)] = &[
    ("varchar length", "ALTER TABLE child MODIFY name VARCHAR(41) NOT NULL", "ALTER TABLE child MODIFY name VARCHAR(40) NOT NULL"),
    ("nullability", "ALTER TABLE child MODIFY name VARCHAR(40) NULL", "ALTER TABLE child MODIFY name VARCHAR(40) NOT NULL"),
    ("column default", "ALTER TABLE child ALTER COLUMN name SET DEFAULT 'x'", "ALTER TABLE child ALTER COLUMN name DROP DEFAULT"),
    ("integer width", "ALTER TABLE child MODIFY score INT NOT NULL", "ALTER TABLE child MODIFY score BIGINT NOT NULL"),
    ("signedness", "ALTER TABLE child MODIFY score BIGINT UNSIGNED NOT NULL", "ALTER TABLE child MODIFY score BIGINT NOT NULL"),
    ("decimal scale", "ALTER TABLE child MODIFY price DECIMAL(10,3) NOT NULL", "ALTER TABLE child MODIFY price DECIMAL(10,2) NOT NULL"),
    ("datetime precision", "ALTER TABLE child MODIFY created DATETIME(3) NOT NULL", "ALTER TABLE child MODIFY created DATETIME(6) NOT NULL"),
    (
        "datetime to timestamp",
        "ALTER TABLE child MODIFY created TIMESTAMP(6) NOT NULL DEFAULT '2026-01-01 00:00:00.000000'",
        "ALTER TABLE child MODIFY created DATETIME(6) NOT NULL",
    ),
    (
        "enum members",
        "ALTER TABLE child MODIFY kind ENUM('MEMO','TODO','DONE') NOT NULL",
        "ALTER TABLE child MODIFY kind ENUM('MEMO','TODO') NOT NULL",
    ),
    (
        "column collation",
        "ALTER TABLE child MODIFY name VARCHAR(40) COLLATE utf8mb4_bin NOT NULL",
        "ALTER TABLE child MODIFY name VARCHAR(40) COLLATE utf8mb4_0900_ai_ci NOT NULL",
    ),
    (
        "column charset",
        "ALTER TABLE child MODIFY name VARCHAR(40) CHARACTER SET latin1 NOT NULL",
        "ALTER TABLE child MODIFY name VARCHAR(40) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL",
    ),
    ("table default collation", "ALTER TABLE child DEFAULT COLLATE utf8mb4_bin", "ALTER TABLE child DEFAULT COLLATE utf8mb4_0900_ai_ci"),
    ("rename column", "ALTER TABLE child RENAME COLUMN note TO memo", "ALTER TABLE child RENAME COLUMN memo TO note"),
    ("column order", "ALTER TABLE child MODIFY note VARCHAR(40) NULL FIRST", "ALTER TABLE child MODIFY note VARCHAR(40) NULL AFTER kind"),
    ("add column", "ALTER TABLE child ADD COLUMN extra INT NULL", "ALTER TABLE child DROP COLUMN extra"),
    ("drop column", "ALTER TABLE child DROP COLUMN note", "ALTER TABLE child ADD COLUMN note VARCHAR(40) NULL AFTER kind, ADD KEY ix_note (note)"),
    ("auto_increment attribute", "ALTER TABLE child MODIFY id BIGINT NOT NULL", "ALTER TABLE child MODIFY id BIGINT NOT NULL AUTO_INCREMENT"),
    (
        "on update clause",
        "ALTER TABLE child MODIFY created DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6)",
        "ALTER TABLE child MODIFY created DATETIME(6) NOT NULL",
    ),
    (
        "generated column expression",
        "ALTER TABLE child MODIFY gen INT GENERATED ALWAYS AS (score + 2) STORED",
        "ALTER TABLE child MODIFY gen INT GENERATED ALWAYS AS (score + 1) STORED",
    ),
    ("drop unique", "ALTER TABLE child DROP INDEX uq_name", "ALTER TABLE child ADD UNIQUE KEY uq_name (name)"),
    (
        "unique to plain index",
        "ALTER TABLE child DROP INDEX uq_name, ADD KEY uq_name (name)",
        "ALTER TABLE child DROP INDEX uq_name, ADD UNIQUE KEY uq_name (name)",
    ),
    (
        "index prefix length",
        "ALTER TABLE child DROP INDEX ix_note, ADD KEY ix_note (note(10))",
        "ALTER TABLE child DROP INDEX ix_note, ADD KEY ix_note (note)",
    ),
    (
        "index column order",
        "ALTER TABLE child DROP INDEX ix_pair, ADD KEY ix_pair (score, name)",
        "ALTER TABLE child DROP INDEX ix_pair, ADD KEY ix_pair (name, score)",
    ),
    (
        "index descending",
        "ALTER TABLE child DROP INDEX ix_score, ADD KEY ix_score (score DESC)",
        "ALTER TABLE child DROP INDEX ix_score, ADD KEY ix_score (score)",
    ),
    ("invisible index", "ALTER TABLE child ALTER INDEX ix_score INVISIBLE", "ALTER TABLE child ALTER INDEX ix_score VISIBLE"),
    (
        "foreign key action",
        "ALTER TABLE child DROP FOREIGN KEY fk_parent; ALTER TABLE child ADD CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES parent (id) ON DELETE CASCADE",
        "ALTER TABLE child DROP FOREIGN KEY fk_parent; ALTER TABLE child ADD CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES parent (id) ON DELETE RESTRICT",
    ),
    (
        "foreign key dropped",
        "ALTER TABLE child DROP FOREIGN KEY fk_parent",
        "ALTER TABLE child ADD CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES parent (id) ON DELETE RESTRICT",
    ),
    ("check constraint added", "ALTER TABLE child ADD CONSTRAINT ck_score CHECK (score >= 0)", "ALTER TABLE child DROP CHECK ck_score"),
    ("storage engine", "ALTER TABLE lonely ENGINE=MyISAM", "ALTER TABLE lonely ENGINE=InnoDB"),
    ("trigger added", "CREATE TRIGGER child_bi BEFORE INSERT ON child FOR EACH ROW SET NEW.score = NEW.score + 1", "DROP TRIGGER child_bi"),
    ("table renamed", "RENAME TABLE child TO child2", "RENAME TABLE child2 TO child"),
    ("table added", "CREATE TABLE unrelated (id INT PRIMARY KEY) ENGINE=InnoDB", "DROP TABLE unrelated"),
    ("table dropped", "DROP TABLE lonely", "CREATE TABLE lonely (id INT PRIMARY KEY) ENGINE=InnoDB"),
    ("view added to the business schema", "CREATE VIEW v_child AS SELECT id FROM child", "DROP VIEW v_child"),
    ("table comment", "ALTER TABLE child COMMENT = 'changed'", "ALTER TABLE child COMMENT = ''"),
];

/// Changes the fingerprint deliberately does not observe.
/// - a table comment changes no read or write semantic;
/// - a view in the business schema is not a business table, and AIP never reads it;
/// - a trigger is invisible to INFORMATION_SCHEMA unless the account holds the TRIGGER privilege, and giving the runtime
///   account that privilege would let it create triggers. This is a known limit of a least-privilege MySQL deployment
///   (PostgreSQL's snapshot sees triggers through pg_catalog); operators must not add triggers to adopted tables out of band.
const IGNORED: &[&str] = &["table comment", "view added to the business schema", "trigger added"];

#[tokio::test]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn every_semantic_ddl_change_changes_the_catalog_fingerprint() {
    let url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(url.starts_with("mysql://"), "this matrix targets MySQL 8.4");
    let driver = MysqlDriver::connect(&url).await.unwrap();
    let mut conn = driver.pool().get_conn().await.unwrap();
    conn.query_drop(format!("DROP DATABASE IF EXISTS `{SCHEMA}`")).await.unwrap();
    conn.query_drop(format!("CREATE DATABASE `{SCHEMA}` DEFAULT CHARACTER SET utf8mb4 DEFAULT COLLATE utf8mb4_0900_ai_ci")).await.unwrap();
    conn.query_drop(format!("USE `{SCHEMA}`")).await.unwrap();
    for ddl in [
        "CREATE TABLE parent (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY) ENGINE=InnoDB",
        "CREATE TABLE lonely (id INT PRIMARY KEY) ENGINE=InnoDB",
        "CREATE TABLE child (
           id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
           parent_id BIGINT NOT NULL,
           name VARCHAR(40) NOT NULL,
           score BIGINT NOT NULL,
           price DECIMAL(10,2) NOT NULL,
           created DATETIME(6) NOT NULL,
           kind ENUM('MEMO','TODO') NOT NULL,
           note VARCHAR(40) NULL,
           gen INT GENERATED ALWAYS AS (score + 1) STORED,
           UNIQUE KEY uq_name (name),
           KEY ix_note (note),
           KEY ix_pair (name, score),
           KEY ix_score (score),
           CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES parent (id) ON DELETE RESTRICT
         ) ENGINE=InnoDB",
    ] {
        conn.query_drop(ddl).await.unwrap_or_else(|e| panic!("{ddl}: {e}"));
    }

    let baseline = driver.catalog_fingerprint(SCHEMA).await.unwrap();
    assert_eq!(driver.catalog_fingerprint(SCHEMA).await.unwrap(), baseline, "the fingerprint is deterministic");

    let mut unobserved = vec![];
    for (label, change, restore) in CASES {
        for statement in change.split("; ") {
            conn.query_drop(statement).await.unwrap_or_else(|e| panic!("{label}: `{statement}` failed: {e}"));
        }
        let after = driver.catalog_fingerprint(SCHEMA).await.unwrap_or_else(|e| panic!("{label}: fingerprint failed: {e}"));
        let changed = after != baseline;
        for statement in restore.split("; ") {
            conn.query_drop(statement).await.unwrap_or_else(|e| panic!("{label}: restore `{statement}` failed: {e}"));
        }
        let restored = driver.catalog_fingerprint(SCHEMA).await.unwrap();
        assert_eq!(restored, baseline, "{label}: restoring the baseline must restore the fingerprint");
        if !changed && !IGNORED.contains(label) {
            unobserved.push(*label);
        }
        if changed && IGNORED.contains(label) {
            panic!("{label} is listed as ignorable but changed the fingerprint; remove it from IGNORED");
        }
    }
    conn.query_drop(format!("DROP DATABASE `{SCHEMA}`")).await.unwrap();
    assert!(unobserved.is_empty(), "DDL changes the fingerprint does not observe: {unobserved:?}");
}
