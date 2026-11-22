#[path = "src/lib.rs"]
#[allow(dead_code)]
mod guard;
fn main() {
    for key in ["AIP_TEST_DATABASE_URL", "AIP_TEST_ADMIN_URL", "PGHOST", "PGPORT", "PGDATABASE", "PGUSER", "PGPASSWORD", "PGSERVICE", "PGSERVICEFILE"]
    {
        println!("cargo:rerun-if-env-changed={key}");
    }
    println!("cargo:rerun-if-env-changed=AIP_TEST_NO_DB");
    let database_url = std::env::var("AIP_TEST_DATABASE_URL").ok();
    let admin_url = std::env::var("AIP_TEST_ADMIN_URL").ok();
    // AIP_TEST_NO_DB=1 is for a loop on tests that never touch a database (diagnostics, help, generated bindings): the crate then
    // builds without a database configured. It changes nothing once a test DSN is set, and a test that needs the database still
    // cannot be compiled (`test_database_url!()` is `env!`) or run (`database_url()` panics), so nothing connects by default.
    let no_db = std::env::var("AIP_TEST_NO_DB").is_ok_and(|v| v == "1") && database_url.is_none() && admin_url.is_none();
    let strict = std::env::var_os("CARGO_FEATURE_STRICT_DB").is_some() && !no_db;
    if strict && database_url.is_none() {
        panic!("AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults");
    }
    if strict && admin_url.is_none() {
        panic!("AIP_TEST_ADMIN_URL is required; refusing PostgreSQL defaults");
    }
    // libpq 기본값으로 공용 DB에 붙는 일을 막는 안전장치다. 테스트 빌드(strict-db는 dev-dependency만 켠다)에서만 빌드를 막는다.
    // 제품 바이너리 빌드는 이 crate를 일반 의존성으로 끌어오므로, 개발자 셸의 PGHOST 같은 값이 릴리스 빌드를 막으면 안 된다.
    // 일반 빌드에서도 테스트 DSN을 꺼내는 database_url()/admin_database_url()이 같은 검사를 런타임에 한다.
    if strict && let Some((key, _)) = std::env::vars_os().find(|(key, _)| key.to_string_lossy().starts_with("PG")) {
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
