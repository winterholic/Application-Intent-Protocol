use std::{collections::HashMap, sync::OnceLock};

/// Embed the one explicitly configured worker-owned test DSN. Compilation fails if it is absent.
#[macro_export]
macro_rules! test_database_url {
    () => {
        env!("AIP_TEST_DATABASE_URL", "AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults")
    };
}

/// Embed the explicit admin DSN used only by test fixture setup.
#[macro_export]
macro_rules! test_admin_database_url {
    () => {
        env!("AIP_TEST_ADMIN_URL", "AIP_TEST_ADMIN_URL is required; refusing PostgreSQL defaults")
    };
}

/// Return the one explicitly configured, isolated database URL for integration tests.
/// The URL is never included in a panic message because it can contain credentials.
pub fn database_url() -> &'static str {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| {
        let raw =
            std::env::var("AIP_TEST_DATABASE_URL").unwrap_or_else(|_| panic!("AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults"));
        validate_database_url(&raw).unwrap_or_else(|reason| panic!("AIP_TEST_DATABASE_URL is unsafe: {reason}"));
        raw
    })
}

/// Return the explicit admin URL used only for isolated fixture setup.
pub fn admin_database_url() -> &'static str {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| {
        let raw = std::env::var("AIP_TEST_ADMIN_URL").unwrap_or_else(|_| panic!("AIP_TEST_ADMIN_URL is required; refusing PostgreSQL defaults"));
        validate_admin_url(&raw).unwrap_or_else(|reason| panic!("AIP_TEST_ADMIN_URL is unsafe: {reason}"));
        raw
    })
}

/// Derive a connection string for another database on the same explicitly configured worker.
pub fn database_url_for(dbname: &str) -> String {
    replace_database_name(database_url(), dbname)
}

/// Derive an admin connection string for another database on the same explicitly configured worker.
pub fn admin_database_url_for(dbname: &str) -> String {
    replace_database_name(admin_database_url(), dbname)
}

/// Derive a connection string with a different role while keeping the same worker endpoint.
pub fn database_url_for_user(raw: &str, user: &str) -> String {
    replace_keyword(raw, "user", user)
}

/// Derive a DSN with one additional PostgreSQL connection option.
pub fn database_url_with_option(raw: &str, key: &str, value: &str) -> String {
    if raw.starts_with("postgres://") || raw.starts_with("postgresql://") {
        let mut url = url::Url::parse(raw).expect("validated test DSN");
        // form 인코딩(`append_pair`)은 공백을 `+`로 쓰는데 libpq와 tokio-postgres는 URI query에서 `+`를 공백으로 읽지 않는다.
        // 리터럴 `+`는 `%2B`로 나오므로 인코딩 뒤 남은 `+`는 모두 공백이다.
        let encode = |text: &str| url::form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>().replace('+', "%20");
        let pair = format!("{}={}", encode(key), encode(value));
        let query = match url.query() {
            Some(existing) if !existing.is_empty() => format!("{existing}&{pair}"),
            _ => pair,
        };
        url.set_query(Some(&query));
        return url.into();
    }
    let mut fields = parse_keywords(raw).expect("validated test DSN");
    fields.insert(key.to_owned(), value.to_owned());
    fields.into_iter().map(|(key, value)| format!("{key}='{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))).collect::<Vec<_>>().join(" ")
}

/// Derive the configured test DSN with another loopback port, for fault-injecting proxies in front of the test database.
/// User and database name stay explicit; they must never fall back to the operating-system user.
pub fn database_url_via_proxy(proxy_port: u16) -> String {
    let raw = database_url();
    if raw.starts_with("postgres://") || raw.starts_with("postgresql://") {
        let mut url = url::Url::parse(raw).expect("validated test DSN");
        let _ = url.set_host(Some("127.0.0.1"));
        let _ = url.set_port(Some(proxy_port));
        return url.into();
    }
    let mut fields = parse_keywords(raw).expect("validated test DSN");
    fields.insert("host".to_owned(), "127.0.0.1".to_owned());
    fields.insert("port".to_owned(), proxy_port.to_string());
    fields.into_iter().map(|(key, value)| format!("{key}='{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))).collect::<Vec<_>>().join(" ")
}

/// Return the normalized endpoint identity without exposing credentials.
pub fn endpoint(raw: &str) -> Result<(String, u16), &'static str> {
    let fields = parse_dsn(raw)?;
    let host = fields.get("host").filter(|host| !host.is_empty()).ok_or("host must be explicit")?;
    let port = fields.get("port").ok_or("port must be explicit")?.parse().map_err(|_| "port must be explicit and valid")?;
    Ok((host.to_ascii_lowercase(), port))
}

fn replace_database_name(raw: &str, dbname: &str) -> String {
    if raw.starts_with("postgres://") || raw.starts_with("postgresql://") {
        let mut url = url::Url::parse(raw).expect("validated test DSN");
        url.set_path(&format!("/{}", url::form_urlencoded::byte_serialize(dbname.as_bytes()).collect::<String>()));
        return url.into();
    }
    replace_keyword(raw, "dbname", dbname)
}

fn replace_keyword(raw: &str, key: &str, value: &str) -> String {
    if raw.starts_with("postgres://") || raw.starts_with("postgresql://") {
        let mut url = url::Url::parse(raw).expect("validated test DSN");
        match key {
            "dbname" => url.set_path(&format!("/{}", url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>())),
            "user" => {
                let _ = url.set_username(value);
            }
            _ => unreachable!("only identity fields are replaceable"),
        }
        return url.into();
    }
    let mut fields = parse_keywords(raw).expect("validated test DSN");
    fields.insert(key.to_owned(), value.to_owned());
    fields.into_iter().map(|(key, value)| format!("{key}='{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))).collect::<Vec<_>>().join(" ")
}

/// Validate that the DSN itself supplies all parameters needed to avoid libpq defaults.
pub fn validate_database_url(raw: &str) -> Result<(), &'static str> {
    let fields = parse_dsn(raw)?;
    validate_fields(&fields, false)
}

/// Extract the explicitly supplied port without consulting libpq environment defaults.
pub fn explicit_port(raw: &str) -> Result<u16, &'static str> {
    let fields = parse_dsn(raw)?;
    fields.get("port").ok_or("port must be explicit")?.parse().map_err(|_| "port must be explicit and valid")
}

/// Extract the explicitly supplied database user without consulting PGUSER.
pub fn explicit_user(raw: &str) -> Result<String, &'static str> {
    let fields = parse_dsn(raw)?;
    fields.get("user").filter(|user| !user.is_empty()).cloned().ok_or("user must be explicit")
}

/// Validate a fixture admin DSN. The maintenance database may be `postgres`, but the
/// endpoint must be an explicit non-default port on loopback and include a user.
pub fn validate_admin_url(raw: &str) -> Result<(), &'static str> {
    let fields = parse_dsn(raw)?;
    validate_fields(&fields, true)
}

fn validate_fields(fields: &HashMap<String, String>, admin: bool) -> Result<(), &'static str> {
    let host = fields.get("host").filter(|s| !s.is_empty()).ok_or("host must be explicit")?;
    if host.contains(',') || fields.contains_key("hostaddr") {
        return Err("multiple or indirect hosts are not allowed");
    }
    let port: u16 = fields.get("port").ok_or("port must be explicit")?.parse().map_err(|_| "port must be explicit and valid")?;
    if port <= 1024 || port == 5432 {
        return Err("port must be a dedicated non-default high port");
    }
    let user = fields.get("user").filter(|s| !s.is_empty()).ok_or("user must be explicit")?;
    let _ = user;
    let dbname = fields.get("dbname").filter(|s| !s.is_empty()).ok_or("database name must be explicit")?;
    if admin {
        if !is_loopback(host) {
            return Err("admin host must be loopback for an isolated worker database");
        }
    } else {
        if !dbname.starts_with("aip_test_") {
            return Err("database name must start with aip_test_");
        }
        if is_loopback(host) && port == 5432 {
            return Err("shared PostgreSQL default endpoint is forbidden");
        }
    }
    Ok(())
}

fn is_loopback(host: &str) -> bool {
    matches!(host.to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

fn parse_dsn(raw: &str) -> Result<HashMap<String, String>, &'static str> {
    if raw.trim().is_empty() {
        return Err("DSN must not be empty");
    }
    if raw.to_ascii_lowercase().starts_with("postgres://") || raw.to_ascii_lowercase().starts_with("postgresql://") {
        return parse_uri(raw);
    }
    parse_keywords(raw)
}

fn parse_uri(raw: &str) -> Result<HashMap<String, String>, &'static str> {
    let url = url::Url::parse(raw).map_err(|_| "URI DSN is malformed")?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        return Err("URI scheme must be postgres or postgresql");
    }
    let host = url.host_str().ok_or("host must be explicit")?;
    let port = url.port().ok_or("port must be explicit")?;
    let user = url.username();
    if user.is_empty() {
        return Err("user must be explicit");
    }
    let dbname = url.path().strip_prefix('/').filter(|s| !s.is_empty()).ok_or("database name must be explicit")?;
    let mut fields = HashMap::from([
        ("host".to_owned(), host.to_owned()),
        ("port".to_owned(), port.to_string()),
        ("user".to_owned(), user.to_owned()),
        ("dbname".to_owned(), dbname.to_owned()),
    ]);
    if let Some(query) = url.query() {
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            let key = key.into_owned().to_ascii_lowercase();
            if matches!(key.as_str(), "host" | "hostaddr" | "port" | "user" | "dbname" | "service" | "servicefile") {
                return Err("connection identity parameters must appear only once in the URI authority/path");
            }
            fields.insert(key, value.into_owned());
        }
    }
    Ok(fields)
}

fn parse_keywords(raw: &str) -> Result<HashMap<String, String>, &'static str> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    let mut fields = HashMap::new();
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i == bytes.len() {
            break;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i == start || i == bytes.len() || bytes[i] != b'=' {
            return Err("keyword DSN is malformed");
        }
        let key = std::str::from_utf8(&bytes[start..i]).map_err(|_| "keyword DSN is malformed")?.to_ascii_lowercase();
        i += 1;
        let mut value = Vec::new();
        if i < bytes.len() && bytes[i] == b'\'' {
            i += 1;
            let mut closed = false;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' if i + 1 < bytes.len() => {
                        i += 1;
                        value.push(bytes[i]);
                        i += 1;
                    }
                    b'\'' => {
                        i += 1;
                        closed = true;
                        break;
                    }
                    byte => {
                        value.push(byte);
                        i += 1;
                    }
                }
            }
            if !closed || (i < bytes.len() && !bytes[i].is_ascii_whitespace()) {
                return Err("keyword DSN quoting is malformed");
            }
        } else {
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                }
                value.push(bytes[i]);
                i += 1;
            }
        }
        let value = String::from_utf8(value).map_err(|_| "keyword DSN value is malformed")?;
        if fields.insert(key, value).is_some() {
            return Err("duplicate DSN keys are not allowed");
        }
    }
    if fields.contains_key("service") || fields.contains_key("servicefile") {
        return Err("libpq service-file fallback is not allowed");
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::{database_url_with_option, parse_dsn, validate_admin_url, validate_database_url};

    /// libpq와 tokio-postgres는 URI query의 `+`를 공백으로 읽지 않고 퍼센트 인코딩만 푼다.
    /// `options=-c+TimeZone=...`은 서버에 `+TimeZone`이라는 이름의 설정으로 전달돼 접속 자체가 거부된다.
    #[test]
    fn uri_option_values_encode_spaces_as_percent_20() {
        let url = database_url_with_option("postgresql://runner@127.0.0.1:55432/aip_test_run", "options", "-c TimeZone=Asia/Seoul");
        assert!(url.ends_with("?options=-c%20TimeZone%3DAsia%2FSeoul"), "{url}");
        let more = database_url_with_option(&url, "application_name", "a+b c");
        assert!(more.ends_with("&application_name=a%2Bb%20c"), "a literal plus stays %2B and an existing query is kept: {more}");
        assert!(validate_database_url(&more).is_ok());
    }

    #[test]
    fn accepts_explicit_run_owned_keyword_and_uri_dsns() {
        assert!(validate_database_url("host=127.0.0.1 port=55432 dbname=aip_test_run user=runner").is_ok());
        assert!(validate_database_url("postgresql://runner@127.0.0.1:55432/aip_test_run").is_ok());
    }

    #[test]
    fn rejects_default_shared_and_implicit_libpq_endpoints() {
        for dsn in [
            "host=localhost dbname=postgres",
            "host=127.0.0.1 port=5432 dbname=aip_test_run user=runner",
            "host=127.0.0.1 port=55432 dbname=postgres user=runner",
            "postgresql://runner@127.0.0.1:5432/postgres",
            "postgresql://runner@localhost/aip_test_run",
            "dbname=aip_test_run port=55432 user=runner",
            "host=localhost port=55432 dbname=aip_test_run",
            "host=localhost port=55432 dbname=aip_test_run user=runner service=aip",
        ] {
            assert!(validate_database_url(dsn).is_err(), "unsafe DSN accepted");
        }
    }

    #[test]
    fn parses_quoted_libpq_values_without_logging_them() {
        let fields = parse_dsn("host='127.0.0.1' port=55432 dbname='aip_test_run' user='runner' ").unwrap();
        assert_eq!(fields.get("dbname").map(String::as_str), Some("aip_test_run"));
    }

    #[test]
    fn admin_endpoint_requires_explicit_loopback_high_port() {
        assert!(validate_admin_url("host=127.0.0.1 port=55432 dbname=postgres user=runner").is_ok());
        assert!(validate_admin_url("host=localhost dbname=postgres").is_err());
        assert!(validate_admin_url("host=remote port=55432 dbname=postgres user=runner").is_err());
    }

    #[test]
    fn rejects_case_variants_of_identity_parameters() {
        for key in ["HOSTADDR", "HOST", "SERVICE", "SERVICEFILE", "Port", "USER", "DbName"] {
            let uri = format!("postgresql://runner@127.0.0.1:55432/aip_test_run?{key}=x");
            assert!(validate_database_url(&uri).is_err(), "URI key {key} accepted");
        }
        for key in ["HOSTADDR", "SERVICE", "SERVICEFILE"] {
            let kw = format!("host=127.0.0.1 port=55432 dbname=aip_test_run user=runner {key}=x");
            assert!(validate_database_url(&kw).is_err(), "keyword {key} accepted");
        }
    }

    #[test]
    fn rejects_flaskbb_bypass_vectors() {
        // 1. encoded_db_hostaddr / indirect host
        assert!(validate_database_url("host=db.example hostaddr=127.0.0.1 port=5432 dbname=postgres user=synthetic").is_err());
        assert!(validate_database_url("host=db.example hostaddr=127.0.0.1 port=55432 dbname=aip_test_run user=synthetic").is_err());
        assert!(validate_database_url("postgresql://synthetic@db.example:55432/aip_test_run?hostaddr=127.0.0.1").is_err());

        // 2. partial_dsn (missing port, dbname, user)
        assert!(validate_database_url("host=127.0.0.1").is_err());
        assert!(validate_database_url("host=127.0.0.1 port=55432").is_err());
        assert!(validate_database_url("host=127.0.0.1 dbname=aip_test_run user=runner").is_err());

        // 3. service_dsn (service file fallback)
        assert!(validate_database_url("service=synthetic_fixture").is_err());
        assert!(validate_database_url("host=127.0.0.1 port=55432 dbname=aip_test_run user=runner service=synthetic_fixture").is_err());
        assert!(validate_database_url("postgresql://runner@127.0.0.1:55432/aip_test_run?service=synthetic_fixture").is_err());
    }
}
