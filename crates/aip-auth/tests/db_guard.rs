mod common;

use common::parse_and_validate_endpoint;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[test]
fn test_known_default_postgres_is_rejected() {
    let err = parse_and_validate_endpoint("postgresql://postgres@localhost:5432/postgres").unwrap_err();
    assert!(err.contains("default postgres"), "unexpected reason: {err}");
}

#[test]
fn test_explicit_fixture_is_accepted() {
    let endpoint = parse_and_validate_endpoint("postgresql://fixture_user:fixture_pass@127.0.0.1:55439/aip_flaskbb")
        .expect("explicit fixture should be accepted");
    assert_eq!(endpoint.host, "127.0.0.1");
    assert_eq!(endpoint.port, 55439);
    assert_eq!(endpoint.dbname, "aip_flaskbb");
    assert_eq!(endpoint.user, "fixture_user");
    assert_eq!(endpoint.password.as_deref(), Some("fixture_pass"));
}

#[test]
fn test_encoded_db_hostaddr_bypass_is_rejected() {
    // Attack vector from report: encoded /%70ostgres and hostaddr query param
    let err = parse_and_validate_endpoint("postgresql://synthetic@db.example:5432/%70ostgres?hostaddr=127.0.0.1").unwrap_err();
    assert!(err.contains("query override") || err.contains("loopback") || err.contains("default postgres"), "unexpected reason: {err}");
}

#[test]
fn test_partial_dsn_is_rejected() {
    // Attack vector from report: partial DSN falls back to PG* env / defaults
    let err = parse_and_validate_endpoint("host=127.0.0.1").unwrap_err();
    assert!(err.contains("port") || err.contains("dbname") || err.contains("user"), "unexpected reason: {err}");
}

#[test]
fn test_service_dsn_is_rejected() {
    // Attack vector from report: service DSN resolves pg_service.conf
    let err_kv = parse_and_validate_endpoint("service=synthetic_fixture").unwrap_err();
    assert!(err_kv.contains("service"), "unexpected reason: {err_kv}");

    let err_uri = parse_and_validate_endpoint("postgresql://fixture_user:pass@127.0.0.1:55439/aip_flaskbb?service=synthetic_fixture").unwrap_err();
    assert!(err_uri.contains("service"), "unexpected reason: {err_uri}");
}

#[test]
fn test_query_overrides_are_rejected() {
    let err_db = parse_and_validate_endpoint("postgresql://fixture_user:pass@127.0.0.1:55439/aip_flaskbb?dbname=%70ostgres").unwrap_err();
    assert!(err_db.contains("query override"), "unexpected reason: {err_db}");

    let err_user = parse_and_validate_endpoint("postgresql://fixture_user:pass@127.0.0.1:55439/aip_flaskbb?user=postgres").unwrap_err();
    assert!(err_user.contains("query override"), "unexpected reason: {err_user}");

    let err_host = parse_and_validate_endpoint("postgresql://fixture_user:pass@127.0.0.1:55439/aip_flaskbb?host=10.0.0.1").unwrap_err();
    assert!(err_host.contains("query override"), "unexpected reason: {err_host}");
}

#[test]
fn test_missing_required_fields_are_rejected() {
    // Missing port
    assert!(parse_and_validate_endpoint("postgresql://fixture_user:pass@127.0.0.1/aip_flaskbb").is_err());
    assert!(parse_and_validate_endpoint("host=127.0.0.1 user=u dbname=db").is_err());

    // Missing user
    assert!(parse_and_validate_endpoint("postgresql://127.0.0.1:55439/aip_flaskbb").is_err());
    assert!(parse_and_validate_endpoint("host=127.0.0.1 port=55439 dbname=db").is_err());

    // Missing dbname
    assert!(parse_and_validate_endpoint("postgresql://fixture_user:pass@127.0.0.1:55439/").is_err());
    assert!(parse_and_validate_endpoint("host=127.0.0.1 port=55439 user=u").is_err());
}

#[test]
fn test_unix_socket_and_multihost_are_rejected() {
    // Unix socket
    assert!(parse_and_validate_endpoint("postgresql:///aip_flaskbb").is_err());
    assert!(parse_and_validate_endpoint("host=/tmp port=55439 dbname=db user=u").is_err());

    // Multihost
    assert!(parse_and_validate_endpoint("host=127.0.0.1,127.0.0.2 port=55439 dbname=db user=u").is_err());
}

#[test]
fn test_percent_encoded_default_user_and_db_are_rejected() {
    // User is %70ostgres
    let err_u = parse_and_validate_endpoint("postgresql://%70ostgres:pass@127.0.0.1:55439/aip_flaskbb").unwrap_err();
    assert!(err_u.contains("default postgres"), "unexpected reason: {err_u}");

    // Dbname is %70%6f%73%74%67%72%65%73
    let err_db = parse_and_validate_endpoint("postgresql://fixture_user:pass@127.0.0.1:55439/%70%6f%73%74%67%72%65%73").unwrap_err();
    assert!(err_db.contains("default postgres"), "unexpected reason: {err_db}");
}

#[test]
fn test_panic_redacts_password_and_raw_url() {
    let secret = "super_secret_fixture_password_987654";
    let bad_url = format!("postgresql://postgres:{secret}@localhost:5432/postgres");

    let result = catch_unwind(AssertUnwindSafe(|| {
        common::parse_and_validate_endpoint(&bad_url)
            .unwrap_or_else(|err| panic!("Refusing to connect to default or unsafe database endpoint: {err}"));
    }));

    let panic_msg = match result {
        Err(e) => {
            if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            }
        }
        Ok(_) => panic!("expected panic"),
    };

    assert!(!panic_msg.contains(secret), "password leaked in panic message!");
    assert!(!panic_msg.contains("postgresql://"), "raw URL leaked in panic message!");
    assert!(panic_msg.contains("Refusing to connect to default or unsafe database endpoint"));
}

#[test]
fn test_caller_consistency_tokio_postgres_and_psql() {
    let raw = "postgresql://fixture_user:fixture_pass@127.0.0.1:55439/aip_flaskbb";
    let endpoint = parse_and_validate_endpoint(raw).expect("valid endpoint");
    let normalized = endpoint.to_normalized_url();

    // Verify tokio-postgres parses the normalized URL consistently
    let config: tokio_postgres::Config = normalized.parse().expect("tokio-postgres parse");
    assert_eq!(config.get_user(), Some("fixture_user"));
    assert_eq!(config.get_dbname(), Some("aip_flaskbb"));
    assert_eq!(config.get_ports(), &[55439]);
    assert_eq!(config.get_hosts(), &[tokio_postgres::config::Host::Tcp("127.0.0.1".into())]);

    // Verify endpoint fields match what psql flags use
    assert_eq!(endpoint.host, "127.0.0.1");
    assert_eq!(endpoint.port, 55439);
    assert_eq!(endpoint.dbname, "aip_flaskbb");
    assert_eq!(endpoint.user, "fixture_user");
}
