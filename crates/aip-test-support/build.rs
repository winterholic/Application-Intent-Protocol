#[path = "src/lib.rs"]
#[allow(dead_code)]
mod guard;
fn main() {
    for key in ["AIP_TEST_DATABASE_URL", "AIP_TEST_ADMIN_URL", "PGHOST", "PGPORT", "PGDATABASE", "PGUSER", "PGPASSWORD", "PGSERVICE", "PGSERVICEFILE"]
    {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let strict = std::env::var_os("CARGO_FEATURE_STRICT_DB").is_some();
    let database_url = std::env::var("AIP_TEST_DATABASE_URL").ok();
    let admin_url = std::env::var("AIP_TEST_ADMIN_URL").ok();
    if strict && database_url.is_none() {
        panic!("AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults");
    }
    if strict && admin_url.is_none() {
        panic!("AIP_TEST_ADMIN_URL is required; refusing PostgreSQL defaults");
    }
    if let Some((key, _)) = std::env::vars_os().find(|(key, _)| key.to_string_lossy().starts_with("PG")) {
        let key = key.to_string_lossy();
        panic!("{key} must be unset; configure the explicit AIP test DSN instead");
    }
    let expected_port = database_url.as_deref().and_then(|url| guard::explicit_port(url).ok());
    if let Some(raw) = database_url.as_deref() {
        guard::validate_database_url(raw).unwrap_or_else(|reason| panic!("AIP_TEST_DATABASE_URL is unsafe: {reason}"));
    }
    if let Some(raw) = admin_url.as_deref() {
        guard::validate_admin_url(raw).unwrap_or_else(|reason| panic!("AIP_TEST_ADMIN_URL is unsafe: {reason}"));
        if expected_port != guard::explicit_port(raw).ok()
            || database_url.as_deref().and_then(|url| guard::endpoint(url).ok()) != guard::endpoint(raw).ok()
        {
            panic!("AIP_TEST_ADMIN_URL and AIP_TEST_DATABASE_URL must target the same dedicated endpoint");
        }
    }
}
