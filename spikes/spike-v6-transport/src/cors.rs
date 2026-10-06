use std::{
    io,
    net::{IpAddr, SocketAddr},
};

#[derive(Default)]
pub struct ServerOptions {
    pub allowed_origins: Vec<String>,
}

struct Origin<'a> {
    host: &'a str,
    port: u16,
}

fn secure_origin(value: &str) -> bool {
    if !value.is_ascii() || value.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace()) {
        return false;
    }
    let Ok(url) = url::Url::parse(value) else { return false };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| !host.contains('*'))
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.origin().ascii_serialization() == value
}

fn parse(value: &str) -> Option<Origin<'_>> {
    let authority = value.strip_prefix("http://")?;
    if !authority.is_ascii() || authority.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace()) {
        return None;
    }
    if authority.ends_with(':') {
        return None;
    }
    let (host, port) = if let Some(ipv6) = authority.strip_prefix('[') {
        let (ip, rest) = ipv6.split_once(']')?;
        if ip != "::1" {
            return None;
        }
        ("[::1]", rest.strip_prefix(':').or_else(|| rest.is_empty().then_some(""))?)
    } else {
        authority.split_once(':').unwrap_or((authority, ""))
    };
    if host != "localhost" && host != "[::1]" {
        let ip = host.parse::<IpAddr>().ok()?;
        if !ip.is_loopback() || ip.to_string() != host {
            return None;
        }
    }
    let port = if port.is_empty() {
        80
    } else {
        if !port.bytes().all(|b| b.is_ascii_digit()) || port.starts_with('0') {
            return None;
        }
        let parsed = port.parse::<u16>().ok()?;
        if parsed == 0 || parsed == 80 {
            return None;
        }
        parsed
    };
    Some(Origin { host, port })
}

impl ServerOptions {
    pub fn validate_product(&self) -> io::Result<()> {
        for origin in &self.allowed_origins {
            if parse(origin).is_none() && !secure_origin(origin) {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "allowed origin must be canonical HTTPS or local HTTP"));
            }
        }
        Ok(())
    }
    pub fn validate(&self) -> io::Result<()> {
        for origin in &self.allowed_origins {
            if parse(origin).is_none() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "allowed origin must be a canonical HTTP loopback origin"));
            }
        }
        Ok(())
    }

    pub(crate) fn allows(&self, origin: &str, host: Option<&str>, local: SocketAddr) -> bool {
        if secure_origin(origin) {
            return self.allowed_origins.iter().any(|allowed| allowed == origin);
        }
        let Some(parsed) = parse(origin) else {
            return false;
        };
        let valid_host = host
            .and_then(|host| {
                let value = format!("http://{host}");
                let request = parse(&value)?;
                let bound_host = request.host == "localhost" || request.host.trim_matches(['[', ']']).parse::<IpAddr>().ok() == Some(local.ip());
                (bound_host && request.port == local.port()).then_some(())
            })
            .is_some();
        if host.is_some() && !valid_host {
            return false;
        }
        self.allowed_origins.iter().any(|allowed| allowed == origin)
            || (valid_host && parsed.port == local.port() && host == origin.strip_prefix("http://"))
    }
}

pub(crate) fn allowed_request_headers(value: Option<&str>) -> bool {
    value.is_none_or(|headers| {
        !headers.is_empty()
            && headers
                .split(',')
                .all(|header| matches!(header.trim().to_ascii_lowercase().as_str(), "authorization" | "content-type" | "x-aip-contract"))
    })
}
