use std::collections::HashMap;
use std::net::IpAddr;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedEndpoint {
    pub host: String,
    pub port: u16,
    pub dbname: String,
    pub user: String,
    pub password: Option<String>,
}

impl VerifiedEndpoint {
    pub fn to_normalized_url(&self) -> String {
        let pass_part = self.password.as_deref().map(|p| format!(":{}", percent_encode(p))).unwrap_or_default();
        format!("postgresql://{}{pass_part}@{}:{}/{}", percent_encode(&self.user), self.host, self.port, percent_encode(&self.dbname),)
    }
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn percent_decode(input: &str) -> Result<String, &'static str> {
    let mut bytes = Vec::with_capacity(input.len());
    let mut chars = input.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let h1 = chars.next().ok_or("truncated percent encoding")?;
            let h2 = chars.next().ok_or("truncated percent encoding")?;
            let hex_val = (hex_digit(h1)? << 4) | hex_digit(h2)?;
            bytes.push(hex_val);
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes).map_err(|_| "invalid UTF-8 in percent encoding")
}

fn hex_digit(b: u8) -> Result<u8, &'static str> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err("invalid hex digit in percent encoding"),
    }
}

fn is_loopback_host(host: &str) -> bool {
    let trimmed = host.trim().trim_start_matches('[').trim_end_matches(']');
    if trimmed.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        return ip.is_loopback();
    }
    false
}

fn is_forbidden_user(user: &str) -> bool {
    let lower = user.trim().to_ascii_lowercase();
    lower.is_empty() || lower == "postgres" || lower == "root"
}

fn is_forbidden_dbname(dbname: &str) -> bool {
    let lower = dbname.trim().to_ascii_lowercase();
    lower.is_empty() || lower == "postgres" || lower == "template0" || lower == "template1"
}

fn parse_kv_dsn(input: &str) -> Result<HashMap<String, String>, &'static str> {
    let mut map = HashMap::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut key = String::new();
        while let Some(&k_char) = chars.peek() {
            if k_char == '=' || k_char.is_whitespace() {
                break;
            }
            key.push(k_char);
            chars.next();
        }
        while let Some(&ws) = chars.peek() {
            if !ws.is_whitespace() {
                break;
            }
            chars.next();
        }
        if chars.peek() != Some(&'=') {
            return Err("malformed key-value DSN: expected '='");
        }
        chars.next(); // skip '='
        while let Some(&ws) = chars.peek() {
            if !ws.is_whitespace() {
                break;
            }
            chars.next();
        }
        let mut val = String::new();
        if chars.peek() == Some(&'\'') {
            chars.next(); // skip opening quote
            while let Some(v_char) = chars.next() {
                if v_char == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                        val.push('\'');
                    } else {
                        break;
                    }
                } else if v_char == '\\' {
                    if let Some(escaped) = chars.next() {
                        val.push(escaped);
                    }
                } else {
                    val.push(v_char);
                }
            }
        } else {
            while let Some(&v_char) = chars.peek() {
                if v_char.is_whitespace() {
                    break;
                }
                val.push(v_char);
                chars.next();
            }
        }
        if !key.is_empty() {
            map.insert(key.to_ascii_lowercase(), val);
        }
    }
    Ok(map)
}

pub fn parse_and_validate_endpoint(raw: &str) -> Result<VerifiedEndpoint, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("database connection string is empty".into());
    }
    if trimmed.bytes().any(|b| b.is_ascii_control()) {
        return Err("database connection string contains control characters".into());
    }

    if trimmed.starts_with("postgresql://") || trimmed.starts_with("postgres://") {
        let url = reqwest::Url::parse(trimmed).map_err(|_| "invalid database URI format".to_string())?;
        if url.scheme() != "postgresql" && url.scheme() != "postgres" {
            return Err("unsupported database scheme".into());
        }
        if url.fragment().is_some() {
            return Err("URL fragment is not permitted in database URI".into());
        }

        for (k, _) in url.query_pairs() {
            let k_lower = k.to_ascii_lowercase();
            if k_lower == "service" {
                return Err("service parameter is not permitted in database URI".into());
            }
            if k_lower == "host" || k_lower == "hostaddr" || k_lower == "port" || k_lower == "dbname" || k_lower == "user" || k_lower == "password" {
                return Err("connection target query override is not permitted".into());
            }
            if k_lower.starts_with("target_session_attrs") || k_lower.contains("multihost") {
                return Err("multihost or target_session_attrs query parameter is not permitted".into());
            }
        }

        let host = url.host_str().ok_or_else(|| "database URI must specify an explicit host".to_string())?;
        if !is_loopback_host(host) {
            return Err("database host must be a dedicated loopback address".into());
        }

        let port = url.port().ok_or_else(|| "database port must be explicitly specified in URI".to_string())?;

        let user_raw = url.username();
        if user_raw.is_empty() {
            return Err("database user must be explicitly specified in URI".into());
        }
        let user = percent_decode(user_raw).map_err(|e| format!("database user decode error: {e}"))?;
        if is_forbidden_user(&user) {
            return Err("refusing connection using default postgres or root user".into());
        }

        let path = url.path().trim_start_matches('/');
        if path.is_empty() {
            return Err("database name must be explicitly specified in URI path".into());
        }
        if path.contains('/') {
            return Err("database URI path cannot contain nested segments".into());
        }
        let dbname = percent_decode(path).map_err(|e| format!("database name decode error: {e}"))?;
        if is_forbidden_dbname(&dbname) {
            return Err("refusing connection to default postgres or template database".into());
        }

        let password = url.password().and_then(|p| percent_decode(p).ok());

        Ok(VerifiedEndpoint { host: host.to_string(), port, dbname, user, password })
    } else {
        let kv = parse_kv_dsn(trimmed).map_err(|e| e.to_string())?;
        if kv.contains_key("service") {
            return Err("service DSN is not permitted".into());
        }

        let host = kv.get("host").ok_or_else(|| "explicit host is required in key-value DSN".to_string())?;
        if host.starts_with('/') || host.starts_with('.') {
            return Err("Unix domain socket connection is not permitted".into());
        }
        if host.contains(',') {
            return Err("multihost connection is not permitted".into());
        }
        if !is_loopback_host(host) {
            return Err("database host must be a dedicated loopback address".into());
        }

        if let Some(hostaddr) = kv.get("hostaddr") {
            if !is_loopback_host(hostaddr) {
                return Err("hostaddr must be a dedicated loopback IP".into());
            }
            if let Ok(ip) = host.parse::<IpAddr>() {
                if let Ok(addr_ip) = hostaddr.parse::<IpAddr>() {
                    if ip != addr_ip {
                        return Err("host and hostaddr mismatch".into());
                    }
                }
            }
        }

        let port_str = kv.get("port").ok_or_else(|| "explicit port is required in key-value DSN".to_string())?;
        let port: u16 = port_str.parse().map_err(|_| "port must be a valid positive integer".to_string())?;
        if port == 0 {
            return Err("port must be greater than zero".into());
        }

        let user = kv.get("user").ok_or_else(|| "explicit user is required in key-value DSN".to_string())?;
        if is_forbidden_user(user) {
            return Err("refusing connection using default postgres or root user".into());
        }

        let dbname = kv.get("dbname").ok_or_else(|| "explicit dbname is required in key-value DSN".to_string())?;
        if is_forbidden_dbname(dbname) {
            return Err("refusing connection to default postgres or template database".into());
        }

        let password = kv.get("password").cloned();

        Ok(VerifiedEndpoint { host: host.to_string(), port, dbname: dbname.to_string(), user: user.to_string(), password })
    }
}

#[allow(dead_code)]
pub fn get_test_endpoint(purpose: &str) -> VerifiedEndpoint {
    let raw = std::env::var("AIP_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("TEST_DATABASE_URL"))
        .unwrap_or_else(|_| panic!("fail-closed: AIP_TEST_DATABASE_URL or TEST_DATABASE_URL environment variable is required for {purpose}"));
    parse_and_validate_endpoint(&raw).unwrap_or_else(|err| panic!("Refusing to connect to default or unsafe database endpoint: {err}"))
}

#[allow(dead_code)]
pub fn get_dummy_db_url() -> String {
    match std::env::var("AIP_TEST_DATABASE_URL").or_else(|_| std::env::var("TEST_DATABASE_URL")) {
        Ok(raw) if !raw.trim().is_empty() => {
            let endpoint =
                parse_and_validate_endpoint(&raw).unwrap_or_else(|err| panic!("Refusing to connect to default or unsafe database endpoint: {err}"));
            endpoint.to_normalized_url()
        }
        _ => "postgresql://unconfigured.test.invalid:5432/unconfigured".into(),
    }
}

#[allow(dead_code)]
pub fn run_psql(endpoint: &VerifiedEndpoint, sql: &str) -> std::process::Output {
    let mut cmd = Command::new("psql");
    cmd.args(["-X", "-q", "-Atc", sql])
        .arg("-h")
        .arg(&endpoint.host)
        .arg("-p")
        .arg(endpoint.port.to_string())
        .arg("-U")
        .arg(&endpoint.user)
        .arg("-d")
        .arg(&endpoint.dbname);

    if let Some(ref pass) = endpoint.password {
        cmd.env("PGPASSWORD", pass);
    } else {
        cmd.arg("--no-password");
    }

    // Isolate child psql from host environment and prevent service fallback
    // An empty PGSERVICE is looked up as a service named "" and fails, so remove it.
    cmd.env_remove("PGSERVICE")
        .env("PGSERVICEFILE", "/dev/null")
        .env("PGSYSCONFDIR", "/dev/null")
        .env("PGHOST", &endpoint.host)
        .env("PGPORT", endpoint.port.to_string())
        .env("PGDATABASE", &endpoint.dbname)
        .env("PGUSER", &endpoint.user);

    cmd.output().expect("test setup: psql execution")
}
