//! The shared test connection helper must never fall back to a default PostgreSQL endpoint. These checks do not connect.
use spike_v2_read::test_database_url_from;

#[test]
fn missing_dsn_is_refused_instead_of_using_a_default_endpoint() {
    assert!(test_database_url_from(None).is_err());
}

#[test]
fn the_old_default_dsn_and_implicit_endpoints_are_refused() {
    for dsn in [
        "host=localhost dbname=postgres",
        "host=127.0.0.1 port=5432 dbname=aip_test_run user=runner",
        "host=127.0.0.1 port=55432 dbname=postgres user=runner",
        "host=127.0.0.1 dbname=aip_test_run user=runner",
        "dbname=aip_test_run port=55432 user=runner",
        "host=db.example hostaddr=127.0.0.1 port=55432 dbname=aip_test_run user=runner",
    ] {
        assert!(test_database_url_from(Some(dsn)).is_err(), "unsafe DSN accepted");
    }
}

#[test]
fn an_explicit_dedicated_dsn_is_used_unchanged() {
    let dsn = "host=127.0.0.1 port=55432 dbname=aip_test_run user=runner";
    assert_eq!(test_database_url_from(Some(dsn)).as_deref(), Ok(dsn));
}
