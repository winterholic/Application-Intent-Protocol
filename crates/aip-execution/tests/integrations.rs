use aip_execution::integrations::{Broker, EndpointConfig};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::time::Duration;

fn config(url: String, resolved: Vec<SocketAddr>, allow_loopback: bool) -> EndpointConfig {
    EndpointConfig {
        name: "crm.createTicket".into(),
        url,
        method: "POST".into(),
        timeout: Duration::from_secs(2),
        max_request_bytes: 1024,
        max_response_bytes: 1024,
        secret_header: Some(("x-api-key".into(), "server-secret-value".into())),
        resolved,
        allow_loopback,
        allow_private_networks: false,
        circuit_breaker: None,
        oauth: None,
    }
}

#[test]
fn endpoint_configuration_rejects_user_controlled_or_private_destinations() {
    let private = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 443);
    assert!(Broker::new(vec![config("https://api.example/v1".into(), vec![private], false)]).is_err());
    for special in [
        "[2002:0808:0808::1]:443",
        "[::ffff:127.0.0.1]:443",
        "[2001:20::1]:443",
        "[2001:2f::1]:443",
        "[2001:2::1]:443",
        "[3fff:100::1]:443",
        "[5f00::1]:443",
        "0.1.2.3:443",
    ] {
        let address = special.parse().unwrap();
        assert!(Broker::new(vec![config("https://api.example/v1".into(), vec![address], false)]).is_err());
    }
    assert!(Broker::new(vec![config("http://api.example/v1".into(), vec!["1.1.1.1:443".parse().unwrap()], false)]).is_err());
    assert!(Broker::new(vec![config("https://user:pass@api.example/v1".into(), vec!["1.1.1.1:443".parse().unwrap()], false)]).is_err());
    assert!(Broker::new(vec![config("https://api.example/v1".into(), vec![], false)]).is_err());
    let loopback_http = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 80);
    assert!(Broker::new(vec![config("http://127.0.0.1/v1".into(), vec![loopback_http], true)]).is_ok());
    let mut reserved = config("https://api.example/v1".into(), vec!["1.1.1.1:443".parse().unwrap()], false);
    reserved.secret_header = Some(("Idempotency-Key".into(), "must-not-override".into()));
    assert!(Broker::new(vec![reserved]).is_err());
}

#[test]
fn private_networks_require_endpoint_opt_in_and_allow_loopback_https_when_enabled() {
    let loopback = "127.0.0.1:443".parse().unwrap();
    let private = "10.24.0.8:443".parse().unwrap();
    for address in [loopback, private] {
        let endpoint = config("https://internal.example/health".into(), vec![address], false);
        assert!(Broker::new(vec![endpoint]).is_err(), "private destination must default to denied");

        let mut endpoint = config("https://internal.example/health".into(), vec![address], false);
        endpoint.allow_private_networks = true;
        assert!(Broker::new(vec![endpoint]).is_ok(), "explicit private-network opt-in should permit internal HTTPS");
    }

    let mut localhost = config("https://127.0.0.1/health".into(), vec![loopback], false);
    localhost.allow_private_networks = true;
    assert!(Broker::new(vec![localhost]).is_ok());

    for special in ["169.254.1.2:443", "224.0.0.1:443", "0.0.0.0:443", "[fe80::1]:443", "[ff02::1]:443"] {
        let mut endpoint = config("https://internal.example/health".into(), vec![special.parse().unwrap()], false);
        endpoint.allow_private_networks = true;
        assert!(Broker::new(vec![endpoint]).is_err(), "private-network opt-in must not permit special-use destinations");
    }
}

#[test]
fn ipv6_literal_cannot_bypass_the_vetted_destination_set() {
    for url in ["https://[::1]/", "https://[::ffff:127.0.0.1]/", "https://[fe80::1]/"] {
        let endpoint = config(url.into(), vec!["1.1.1.1:443".parse().unwrap()], false);
        assert!(Broker::new(vec![endpoint]).is_err(), "unvetted literal accepted: {url}");
    }
}

#[test]
fn ipv6_loopback_requires_opt_in_and_matching_addresses() {
    let address = "[::1]:80".parse().unwrap();
    assert!(Broker::new(vec![config("http://[::1]/".into(), vec![address], false)]).is_err());
    assert!(Broker::new(vec![config("http://[::1]/".into(), vec![address], true)]).is_ok());
    let address = "[::1]:443".parse().unwrap();
    let mut endpoint = config("https://[::1]/".into(), vec![address], false);
    endpoint.allow_private_networks = true;
    assert!(Broker::new(vec![endpoint]).is_ok());
}

#[test]
fn fingerprint_is_order_independent_and_changes_with_effective_configuration() {
    let a = config("https://api.example/v1".into(), vec!["1.1.1.1:443".parse().unwrap()], false);
    let mut b = config("https://api.example/other".into(), vec!["8.8.8.8:443".parse().unwrap()], false);
    b.name = "crm.lookup".into();
    let first = Broker::new(vec![a, b]).unwrap();
    let a = config("https://api.example/v1".into(), vec!["1.1.1.1:443".parse().unwrap()], false);
    let mut b = config("https://api.example/other".into(), vec!["8.8.8.8:443".parse().unwrap()], false);
    b.name = "crm.lookup".into();
    let same = Broker::new(vec![b, a]).unwrap();
    assert_eq!(first.fingerprint(), same.fingerprint());

    let mut rotated = config("https://api.example/v1".into(), vec!["1.1.1.1:443".parse().unwrap()], false);
    rotated.secret_header = Some(("x-api-key".into(), "rotated-secret".into()));
    let unchanged = config("https://api.example/v1".into(), vec!["1.1.1.1:443".parse().unwrap()], false);
    assert_ne!(Broker::new(vec![unchanged]).unwrap().fingerprint(), Broker::new(vec![rotated]).unwrap().fingerprint());

    let mut private_networks = config("https://api.example/v1".into(), vec!["1.1.1.1:443".parse().unwrap()], false);
    private_networks.allow_private_networks = true;
    let default_policy = config("https://api.example/v1".into(), vec!["1.1.1.1:443".parse().unwrap()], false);
    assert_ne!(Broker::new(vec![default_policy]).unwrap().fingerprint(), Broker::new(vec![private_networks]).unwrap().fingerprint());
}

#[tokio::test]
async fn named_call_uses_server_endpoint_secret_and_returns_only_bounded_json() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut keys = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            assert!(request.to_ascii_lowercase().contains("x-api-key: server-secret-value"));
            assert!(request.contains("\"ticket\":\"T-1\""));
            let key = request
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase().strip_prefix("idempotency-key:").map(|_| line.split_once(':').unwrap().1.trim().to_owned())
                })
                .unwrap();
            keys.push(key);
            let body = br#"{"ok":true,"ticket":"T-1"}"#;
            write!(
                stream,
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nRetry-After: 7200\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        }
        keys
    });

    let broker = Broker::new(vec![config(format!("http://{address}/tickets"), vec![address], true)]).unwrap();
    assert!(broker.contains("crm.createTicket"));
    assert!(!broker.contains("crm.deleteTicket"));
    let response = broker.call_with_key("crm.createTicket", &json!({"ticket":"T-1"}), Some("run-1/effect-1")).await.unwrap();
    let retry = broker.call_with_key("crm.createTicket", &json!({"ticket":"T-1"}), Some("run-1/effect-1")).await.unwrap();
    let keys = server.join().unwrap();

    assert_eq!(response.status, 201);
    assert_eq!(response.body, json!({"ok":true,"ticket":"T-1"}));
    assert_eq!(response.retry_after_seconds, Some(3600));
    assert_eq!(retry, response);
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], keys[1]);
    assert!(!keys[0].contains("run-1"));
    assert!(broker.call("https://attacker.example", &json!({})).await.is_err());
    assert!(broker.call("crm.createTicket", &json!("x".repeat(1025))).await.is_err());
}

#[tokio::test]
async fn endpoint_response_over_limit_is_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_request(&mut stream);
        let body = br#"{"this":"is too long"}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
        stream.write_all(body).unwrap();
    });
    let mut endpoint = config(format!("http://{address}/large"), vec![address], true);
    endpoint.max_response_bytes = 8;
    let broker = Broker::new(vec![endpoint]).unwrap();

    assert!(broker.call("crm.createTicket", &json!({})).await.is_err());
    server.join().unwrap();
}

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut one = [0; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut one).unwrap();
        bytes.push(one[0]);
    }
    let headers = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    let content_length =
        headers.lines().find_map(|line| line.strip_prefix("content-length:")).and_then(|value| value.trim().parse::<usize>().ok()).unwrap_or(0);
    let mut body = vec![0; content_length];
    stream.read_exact(&mut body).unwrap();
    bytes.extend_from_slice(&body);
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn circuit_breaker_stops_sending_after_consecutive_failures_and_closes_after_a_good_trial() {
    use aip_execution::integrations::{CircuitBreaker, Error};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let healthy = Arc::new(AtomicBool::new(false));
    let (seen, ok) = (requests.clone(), healthy.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            seen.fetch_add(1, Ordering::SeqCst);
            let status = if ok.load(Ordering::SeqCst) { "200 OK" } else { "503 Service Unavailable" };
            let _ = stream.write_all(
                format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: 2\r\n\r\n{{}}").as_bytes(),
            );
        }
    });
    let mut endpoint = config(format!("http://127.0.0.1:{}/hook", address.port()), vec![address], true);
    endpoint.secret_header = None;
    endpoint.circuit_breaker = Some(CircuitBreaker { failure_threshold: 2, open_for: Duration::from_millis(300) });
    let broker = Broker::new(vec![endpoint]).unwrap();

    // A 5xx is still returned to the caller; it only counts toward opening the circuit.
    assert_eq!(broker.call("crm.createTicket", &json!({})).await.unwrap().status, 503);
    assert_eq!(broker.call("crm.createTicket", &json!({})).await.unwrap().status, 503);
    assert_eq!(broker.call("crm.createTicket", &json!({})).await, Err(Error::CircuitOpen));
    assert_eq!(requests.load(Ordering::SeqCst), 2, "an open circuit must not contact the provider");

    // After the open period one trial goes through; a failing trial reopens the circuit.
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(broker.call("crm.createTicket", &json!({})).await.unwrap().status, 503);
    assert_eq!(broker.call("crm.createTicket", &json!({})).await, Err(Error::CircuitOpen));
    assert_eq!(requests.load(Ordering::SeqCst), 3);

    // A successful trial closes it again.
    healthy.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(broker.call("crm.createTicket", &json!({})).await.unwrap().status, 200);
    assert_eq!(broker.call("crm.createTicket", &json!({})).await.unwrap().status, 200);
    assert_eq!(requests.load(Ordering::SeqCst), 5);

    let mut invalid = config("https://api.example/v1".into(), vec!["1.1.1.1:443".parse().unwrap()], false);
    invalid.circuit_breaker = Some(CircuitBreaker { failure_threshold: 0, open_for: Duration::from_secs(1) });
    assert!(Broker::new(vec![invalid]).is_err());
}

#[tokio::test]
async fn oauth_client_credentials_token_is_fetched_cached_refreshed_on_401_and_never_sent_without_one() {
    use aip_execution::integrations::{Error, OAuthConfig};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    // Which token the provider currently issues and accepts; None makes the token endpoint fail.
    let current = Arc::new(Mutex::new(Some("t1".to_string())));
    let (fetches, api_calls) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let token_requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let (issued, fetched, called, seen) = (current.clone(), fetches.clone(), api_calls.clone(), token_requests.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut buffer = vec![0_u8; 8192];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let token = issued.lock().unwrap().clone();
            let (status, body) = if request.starts_with("POST /token") {
                fetched.fetch_add(1, Ordering::SeqCst);
                seen.lock().unwrap().push(request.clone());
                match token {
                    Some(token) => ("200 OK", format!("{{\"access_token\":\"{token}\",\"token_type\":\"Bearer\",\"expires_in\":3600}}")),
                    None => ("500 Internal Server Error", "{}".to_string()),
                }
            } else {
                called.fetch_add(1, Ordering::SeqCst);
                let authorized = token.is_some_and(|token| request.to_ascii_lowercase().contains(&format!("authorization: bearer {token}")));
                if authorized { ("200 OK", "{\"ok\":true}".to_string()) } else { ("401 Unauthorized", "{}".to_string()) }
            };
            let _ = stream.write_all(
                format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}", body.len())
                    .as_bytes(),
            );
        }
    });
    let endpoint = |basic: bool| {
        let mut endpoint = config(format!("http://127.0.0.1:{}/api", address.port()), vec![address], true);
        endpoint.secret_header = None;
        endpoint.oauth = Some(OAuthConfig {
            token_url: format!("http://127.0.0.1:{}/token", address.port()),
            resolved: vec![address],
            client_id: "client-a".into(),
            client_secret: "secret-a".into(),
            scope: Some("tickets.write".into()),
            basic_auth: basic,
        });
        endpoint
    };
    let broker = Broker::new(vec![endpoint(true)]).unwrap();
    assert_eq!(broker.call("crm.createTicket", &json!({})).await.unwrap().status, 200);
    assert_eq!(broker.call("crm.createTicket", &json!({})).await.unwrap().status, 200);
    assert_eq!(fetches.load(Ordering::SeqCst), 1, "a valid token is cached");
    let first = token_requests.lock().unwrap()[0].clone();
    assert!(first.contains("grant_type=client_credentials") && first.contains("scope=tickets.write"), "{first}");
    // client_secret_basic: base64("client-a:secret-a"), and the secret is not in the form body.
    assert!(first.contains("authorization: Basic Y2xpZW50LWE6c2VjcmV0LWE="), "{first}");
    assert!(!first.contains("client_secret="), "{first}");

    // The provider rotates its token: the stale one is rejected once, then a fresh one is fetched.
    *current.lock().unwrap() = Some("t2".into());
    assert_eq!(broker.call("crm.createTicket", &json!({})).await.unwrap().status, 401);
    assert_eq!(broker.call("crm.createTicket", &json!({})).await.unwrap().status, 200);
    assert_eq!(fetches.load(Ordering::SeqCst), 2);

    // Without a token the API is never contacted, which is a certain not-sent outcome.
    *current.lock().unwrap() = None;
    let before = api_calls.load(Ordering::SeqCst);
    let post = Broker::new(vec![endpoint(false)]).unwrap();
    assert_eq!(post.call("crm.createTicket", &json!({})).await, Err(Error::TokenUnavailable));
    assert_eq!(api_calls.load(Ordering::SeqCst), before);
    let last = token_requests.lock().unwrap().last().unwrap().clone();
    assert!(last.contains("client_id=client-a") && last.contains("client_secret=secret-a"), "client_secret_post sends form fields: {last}");

    let mut clash = endpoint(true);
    clash.secret_header = Some(("Authorization".into(), "Bearer static".into()));
    assert!(Broker::new(vec![clash]).is_err(), "a static authorization header cannot shadow the OAuth token");
    let mut leaked = endpoint(true);
    leaked.oauth.as_mut().unwrap().token_url = "https://10.0.0.5/token".into();
    leaked.oauth.as_mut().unwrap().resolved = vec!["10.0.0.5:443".parse().unwrap()];
    assert!(Broker::new(vec![leaked]).is_err(), "the token endpoint follows the same private-network rule");
}

#[test]
fn oauth_config_debug_output_never_contains_the_client_secret() {
    let oauth = aip_execution::integrations::OAuthConfig {
        token_url: "https://auth.example/token".into(),
        resolved: vec!["1.1.1.1:443".parse().unwrap()],
        client_id: "client".into(),
        client_secret: "super-secret-value".into(),
        scope: None,
        basic_auth: true,
    };
    let text = format!("{oauth:?}");
    assert!(!text.contains("super-secret-value"), "{text}");
}
