#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The server profile gate (WBS 7.2.1.3, MariaDB gate): what the adapter relies on is found by executing it, MySQL 8.4 passes, and a
//! server that lacks any of it is refused at start with the missing capabilities named. MariaDB is judged by what it actually does,
//! not by its name. Needs `AIP_TEST_MYSQL_URL` (MySQL 8.4) and `AIP_TEST_MARIADB_URL` (MariaDB 11.4); run with `-- --ignored`.
use aip_migrate::mysql_backend::{MysqlBackend, probe_capabilities};
use aip_migrate::mysql_driver::{MysqlDeploymentFence, MysqlDriver};

async fn driver(var: &str, scheme: &str) -> MysqlDriver {
    let url = std::env::var(var).unwrap_or_else(|_| panic!("set {var} to a run-owned container"));
    aip_test_support::validate_mysql_admin_url(&url).unwrap_or_else(|reason| panic!("{var} is unsafe: {reason}"));
    assert!(url.starts_with(scheme), "{var} must be a {scheme} URL");
    MysqlDriver::connect(&url).await.expect("connect")
}

fn backend(driver: MysqlDriver) -> MysqlBackend {
    let fence = MysqlDeploymentFence::new("aip_gate_meta", "aip_gate_src", &["t".to_string()], &"0".repeat(64), &"0".repeat(64)).unwrap();
    MysqlBackend::new(driver, fence)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live: needs dedicated MySQL 8.4 and MariaDB 11.4 containers (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_8_4_has_every_capability_and_passes_the_gate() {
    let caps = probe_capabilities(&driver("AIP_TEST_MYSQL_URL", "mysql://").await).await.unwrap();
    assert_eq!(caps.engine, "MySQL", "{caps:?}");
    assert!(caps.version.starts_with("8.4."), "{caps:?}");
    assert_eq!(caps.missing(), Vec::<&str>::new(), "{caps:?}");
    backend(driver("AIP_TEST_MYSQL_URL", "mysql://").await).check_server_profile().await.expect("MySQL 8.4 passes");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live: needs dedicated MySQL 8.4 and MariaDB 11.4 containers (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mariadb_is_refused_for_the_capabilities_it_really_lacks() {
    let caps = probe_capabilities(&driver("AIP_TEST_MARIADB_URL", "mariadb://").await).await.unwrap();
    assert_eq!(caps.engine, "MariaDB", "{caps:?}");
    eprintln!("MariaDB {} capabilities: {caps:?}", caps.version);
    // What MariaDB 11.4 does and does not have, as measured. SKIP LOCKED, JSON_ARRAYAGG, enforced CHECK and InnoDB exist, so the
    // job store's claim query and the audit/journal tables could run there. LATERAL (traverse joins) and the `FOR SHARE OF`
    // locking clause (policy locks) do not, and the adapter is refused on exactly those.
    assert!(caps.skip_locked && caps.json_arrayagg && caps.check_enforced && caps.innodb, "{caps:?}");
    assert!(!caps.lateral, "MariaDB has no LATERAL: {caps:?}");
    assert!(!caps.for_share_of, "MariaDB has no FOR SHARE OF: {caps:?}");
    let missing = caps.missing();
    assert!(missing.contains(&"LATERAL joins") && missing.contains(&"FOR SHARE OF") && missing.contains(&"MySQL 8.4 or newer"), "{missing:?}");
    let refused = backend(driver("AIP_TEST_MARIADB_URL", "mariadb://").await).check_server_profile().await.unwrap_err();
    assert_eq!(refused.code, "DB_ADAPTER_UNSUPPORTED");
    assert!(refused.message.contains("MariaDB") && refused.message.contains("LATERAL joins") && refused.message.contains("FOR SHARE OF"), "{refused}");
}
