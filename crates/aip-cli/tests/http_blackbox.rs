//! Executed HTTP black-box attacks against the real `aip service serve` binary (security audit section 6).
//!
//! Only bytes on the socket and a real JWKS-verified token are used. The invariants checked are the security-relevant ones:
//! no forged or malformed request ever reaches a handler as an authenticated one, no attack changes data, error bodies never
//! leak secrets or internals, and the server keeps answering a legitimate caller after every attack. Observations (status and
//! body of each attack) are printed so a run documents what the server actually does, including behavior that is merely odd.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

const DB: &str = "host=localhost dbname=postgres";
const APP: &str = include_str!("../../../prototype/example/app.aip");
const ORIGIN: &str = "https://app.example.com";

async fn service(config: &Path, args: &[&str]) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aip"));
    child.arg("service");
    if args.first() == Some(&"principal") {
        child.arg("principal").arg("--config").arg(config).args(&args[1..]);
    } else {
        child.args(args).arg("--config").arg(config);
    }
    let output = child.env("AIP_TEST_DATABASE", DB).output().await.unwrap();
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Sends raw bytes and returns everything the server answers until it closes, goes quiet or `wait` passes.
async fn raw(address: &str, bytes: &[u8], wait: Duration) -> String {
    let Ok(Ok(mut socket)) = tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(address)).await else {
        return "<connect failed>".into();
    };
    let _ = socket.write_all(bytes).await;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(wait, async {
        let mut buffer = [0u8; 8192];
        loop {
            match socket.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => out.extend_from_slice(&buffer[..n]),
            }
        }
    })
    .await;
    String::from_utf8_lossy(&out).into_owned()
}

fn status_line(response: &str) -> String {
    response.lines().next().unwrap_or("<no response>").trim().to_owned()
}
fn body_of(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

fn http(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Vec<u8> {
    let mut text = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n");
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("content-length")) && !body.is_empty() {
        text.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    text.push_str("\r\n");
    text.push_str(body);
    text.into_bytes()
}

fn b64(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(value.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_forged_and_hostile_requests_never_authenticate_leak_or_change_data() {
    let dir = tempfile::tempdir().unwrap();
    let schema = format!("aip_bb_{}", uuid::Uuid::new_v4().simple());
    let config = dir.path().join("service.json");
    let file = dir.path().join("app.aip");
    std::fs::write(&file, APP).unwrap();
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = private.to_public_key();
    let n = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
    let e = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
    std::fs::write(dir.path().join("jwks.json"), json!({"keys":[{"kty":"RSA","kid":"current","alg":"RS256","use":"sig","n":n,"e":e}]}).to_string())
        .unwrap();
    std::fs::write(
        &config,
        json!({"schema":schema,"database_url_env":"AIP_TEST_DATABASE","listen":"127.0.0.1:0","wire":"decimal","allowed_origins":[ORIGIN],
            "auth":{"issuer":"https://issuer.example","audience":"aip-api","jwks":{"kind":"file","path":"jwks.json"}}})
        .to_string(),
    )
    .unwrap();
    service(&config, &["init", file.to_str().unwrap()]).await;
    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let outcome = {
        use futures_util::FutureExt;
        std::panic::AssertUnwindSafe(async {
            db.batch_execute(&format!(
                "INSERT INTO {schema}.member(id) VALUES(42),(99); INSERT INTO {schema}.event(id,member_id,checked,title,link,phase,date) VALUES(11,42,false,'owned','https://example.com','READY','2026-10-05Z'),(12,99,false,'other','https://example.com','READY','2026-10-05Z')"
            ))
            .await
            .unwrap();
            service(&config, &["principal", "bind", "--subject", "user-42", "--actor", "42"]).await;
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let mut header = Header::new(Algorithm::RS256);
            header.typ = Some("at+jwt".into());
            header.kid = Some("current".into());
            let der = private.to_pkcs1_der().unwrap();
            let key = EncodingKey::from_rsa_der(der.as_bytes());
            let claims = |extra: Value| {
                let mut base = json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-42","iat":now,"nbf":now,"exp":now+300});
                for (name, value) in extra.as_object().unwrap() {
                    base[name] = value.clone();
                }
                base
            };
            let token = encode(&header, &claims(json!({})), &key).unwrap();

            let mut child = Command::new(env!("CARGO_BIN_EXE_aip"))
                .args(["service", "serve"])
                .arg(&file)
                .arg("--config")
                .arg(&config)
                .env("AIP_TEST_DATABASE", DB)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let mut line = String::new();
            let mut reader = BufReader::new(child.stdout.take().unwrap());
            tokio::time::timeout(Duration::from_secs(8), reader.read_line(&mut line)).await.unwrap().unwrap();
            let listening: Value = serde_json::from_str(&line).expect("server started");
            let address = listening["address"].as_str().unwrap().to_owned();
            let fingerprint = listening["fingerprint"].as_str().unwrap().to_owned();
            let read_body = json!({"query":{"read":"Event","select":["id","title"]}}).to_string();
            let apply_body = json!({"key":"bb-apply","request":{"apply":"Event.mark","target":{"ids":["11"]}}}).to_string();
            let valid_read = |token: &str| http("POST", "/read", &[("Authorization", &format!("Bearer {token}")), ("x-aip-contract", &fingerprint), ("Origin", ORIGIN)], &read_body);
            let wait = Duration::from_secs(3);

            // The legitimate caller works before any attack (positive control).
            let baseline = raw(&address, &valid_read(&token), wait).await;
            assert!(body_of(&baseline).contains("\"owned\""), "baseline read failed: {baseline}");
            let alive = |label: String| {
                let (address, request) = (address.clone(), valid_read(&token));
                async move {
                    let response = raw(&address, &request, Duration::from_secs(3)).await;
                    assert!(body_of(&response).contains("\"owned\""), "server stopped answering a legitimate caller after `{label}`: {response}");
                }
            };
            let mut table: Vec<(String, String, String)> = Vec::new();
            let leaks = ["host=localhost", "postgres", "tokio_postgres", "panicked", "RUST_BACKTRACE", "SELECT ", "/Users/", "BEGIN", schema.as_str()];
            let mut record = |name: &str, response: &str| {
                for needle in leaks {
                    assert!(!response.contains(needle), "`{name}` response leaks `{needle}`: {response}");
                }
                assert!(!response.contains(&token), "`{name}` response echoes the token");
                table.push((name.to_owned(), status_line(response), body_of(response).chars().take(110).collect()));
            };
            // The service answers callers without a token as the anonymous actor (the sample policy shows them no rows), so for
            // requests that carry no valid credential the invariant is "sees nothing of member 42's", not "is refused".
            let never_authenticated = |name: &str, response: &str| {
                assert!(!response.contains("\"owned\"") && !response.contains("\"other\""), "`{name}` was served as an authenticated request: {response}");
            };
            // A token that is present but invalid must be refused outright, not quietly downgraded to anonymous.
            let refused = |name: &str, response: &str| {
                assert!(!response.contains("\"ok\":true"), "`{name}` carried an invalid token and was still served: {response}");
            };

            // 1. Forged and malformed tokens. Each must be refused and none may be served.
            let none_header = b64(&json!({"alg":"none","typ":"at+jwt","kid":"current"}));
            let hs_secret = format!("{n}.{e}");
            let hs = |header: Value, claims: Value| {
                let signing = format!("{}.{}", b64(&header), b64(&claims));
                let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(hs_secret.as_bytes()).unwrap();
                mac.update(signing.as_bytes());
                format!("{signing}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
            };
            let resign = |mutate: &dyn Fn(&mut Header), extra: Value| {
                let mut h = header.clone();
                mutate(&mut h);
                encode(&h, &claims(extra), &key).unwrap()
            };
            let mut parts = token.split('.');
            let (p0, p1, p2) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
            let flipped_signature = {
                let mut bytes = URL_SAFE_NO_PAD.decode(p2).unwrap();
                bytes[10] ^= 1;
                URL_SAFE_NO_PAD.encode(bytes)
            };
            let other_key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
            let other_der = other_key.to_pkcs1_der().unwrap();
            let forged_tokens: Vec<(&str, String)> = vec![
                ("alg none, empty signature", format!("{none_header}.{}.", b64(&claims(json!({}))))),
                ("alg none, signature kept", format!("{none_header}.{p1}.{p2}")),
                ("HS256 signed with the public key material", hs(json!({"alg":"HS256","typ":"at+jwt","kid":"current"}), claims(json!({})))),
                ("signature bit flipped", format!("{p0}.{p1}.{flipped_signature}")),
                ("payload swapped to another subject", format!("{p0}.{}.{p2}", b64(&claims(json!({"sub":"user-99"}))))),
                ("signed by a different RSA key", encode(&header, &claims(json!({})), &EncodingKey::from_rsa_der(other_der.as_bytes())).unwrap()),
                ("unknown kid", resign(&|h| h.kid = Some("other".into()), json!({}))),
                ("kid path traversal", resign(&|h| h.kid = Some("../../../etc/passwd".into()), json!({}))),
                ("jku header pointing at the attacker", resign(&|h| h.jku = Some("https://evil.example/jwks.json".into()), json!({}))),
                ("typ JWT instead of at+jwt", resign(&|h| h.typ = Some("JWT".into()), json!({}))),
                ("expired", encode(&header, &claims(json!({"iat":now-3600,"nbf":now-3600,"exp":now-60})), &key).unwrap()),
                ("not yet valid", encode(&header, &claims(json!({"nbf":now+3600,"exp":now+7200})), &key).unwrap()),
                ("lifetime over the 900 s profile", encode(&header, &claims(json!({"exp":now+86_400})), &key).unwrap()),
                ("wrong audience", encode(&header, &claims(json!({"aud":"other-api"})), &key).unwrap()),
                ("wrong issuer", encode(&header, &claims(json!({"iss":"https://evil.example"})), &key).unwrap()),
                ("sub missing", {
                    let mut c = claims(json!({}));
                    c.as_object_mut().unwrap().remove("sub");
                    encode(&header, &c, &key).unwrap()
                }),
                ("unbound subject", encode(&header, &claims(json!({"sub":"nobody"})), &key).unwrap()),
                ("only two segments", format!("{p0}.{p1}")),
                ("four segments", format!("{p0}.{p1}.{p2}.{p2}")),
                ("not base64", "!!!.@@@.###".to_owned()),
                ("empty", String::new()),
                ("100 kB of garbage", "A".repeat(100_000)),
                ("valid token with a NUL", format!("{token}\0")),
            ];
            for (name, forged) in &forged_tokens {
                let response = raw(&address, &valid_read(forged), wait).await;
                never_authenticated(name, &response);
                refused(name, &response);
                record(&format!("token: {name}"), &response);
            }
            // Observation, not an attack: RFC 7519 lets `aud` be an array, and a token that lists this API among others is valid.
            // It is recorded so the behavior is documented; whether to demand an exact single audience is an identity-provider policy.
            let multi_audience = encode(&header, &claims(json!({"aud":["aip-api","other-api"]})), &key).unwrap();
            let response = raw(&address, &valid_read(&multi_audience), wait).await;
            record("token (observation): aud array that includes this API and another", &response);
            // Trailing whitespace in a header value is optional whitespace (RFC 9110), so the same token is still the same token.
            let response = raw(&address, &valid_read(&format!("{token} ")), wait).await;
            record("token (observation): valid token followed by a space", &response);
            alive("forged tokens".into()).await;

            // 2. Authorization header handling.
            let with_headers = |headers: &[(&str, &str)]| http("POST", "/read", headers, &read_body);
            let bearer = format!("Bearer {token}");
            let cases: Vec<(&str, Vec<u8>)> = vec![
                ("no Authorization header", with_headers(&[("x-aip-contract", &fingerprint)])),
                ("Basic scheme carrying the token", with_headers(&[("Authorization", &format!("Basic {token}")), ("x-aip-contract", &fingerprint)])),
                ("token without a scheme", with_headers(&[("Authorization", &token), ("x-aip-contract", &fingerprint)])),
                ("two Authorization headers, bad then good", with_headers(&[("Authorization", "Bearer bad"), ("Authorization", &bearer), ("x-aip-contract", &fingerprint)])),
                ("two Authorization headers, good then bad", with_headers(&[("Authorization", &bearer), ("Authorization", "Bearer bad"), ("x-aip-contract", &fingerprint)])),
                ("token only in the query string", http("POST", &format!("/read?access_token={token}"), &[("x-aip-contract", &fingerprint)], &read_body)),
                ("token only in a cookie", with_headers(&[("Cookie", &format!("token={token}")), ("x-aip-contract", &fingerprint)])),
                ("token in X-Forwarded-Authorization", with_headers(&[("X-Forwarded-Authorization", &bearer), ("x-aip-contract", &fingerprint)])),
            ];
            for (name, request) in &cases {
                let response = raw(&address, request, wait).await;
                // The query-string and duplicate-header cases may legitimately be served only if the bearer header itself is valid.
                if name.starts_with("two Authorization headers, good then bad") {
                    record(&format!("auth: {name}"), &response);
                    continue;
                }
                never_authenticated(name, &response);
                record(&format!("auth: {name}"), &response);
            }

            // 3. Request framing and routing attacks.
            fn authed<'a>(bearer: &'a str, fingerprint: &'a str, extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
                let mut headers = vec![("Authorization", bearer), ("x-aip-contract", fingerprint)];
                headers.extend_from_slice(extra);
                headers
            }
            let hidden_apply = format!(
                "POST /apply HTTP/1.1\r\nHost: localhost\r\nx-aip-contract: {fingerprint}\r\nContent-Length: {}\r\n\r\n{apply_body}",
                apply_body.len()
            );
            let smuggled_len = read_body.len();
            let many_headers = |count: usize| {
                let many: Vec<(String, String)> = (0..count).map(|i| (format!("X-H{i}"), "v".into())).collect();
                let mut refs: Vec<(&str, &str)> = vec![("Authorization", bearer.as_str()), ("x-aip-contract", fingerprint.as_str())];
                refs.extend(many.iter().map(|(a, b)| (a.as_str(), b.as_str())));
                http("POST", "/read", &refs, &read_body)
            };
            let framing: Vec<(&str, Vec<u8>)> = vec![
                (
                    "Content-Length and Transfer-Encoding together (CL.TE)",
                    format!("POST /read HTTP/1.1\r\nHost: localhost\r\nAuthorization: {bearer}\r\nx-aip-contract: {fingerprint}\r\nContent-Length: {smuggled_len}\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n{hidden_apply}").into_bytes(),
                ),
                (
                    "two different Content-Length values",
                    format!("POST /read HTTP/1.1\r\nHost: localhost\r\nAuthorization: {bearer}\r\nx-aip-contract: {fingerprint}\r\nContent-Length: {smuggled_len}\r\nContent-Length: 0\r\n\r\n{read_body}").into_bytes(),
                ),
                (
                    "Content-Length shorter than the body, rest is a second request",
                    format!("POST /read HTTP/1.1\r\nHost: localhost\r\nAuthorization: {bearer}\r\nx-aip-contract: {fingerprint}\r\nContent-Length: {smuggled_len}\r\n\r\n{read_body}{hidden_apply}").into_bytes(),
                ),
                ("Content-Length with a plus sign", http("POST", "/read", &authed(&bearer, &fingerprint, &[("Content-Length", &format!("+{smuggled_len}"))]), &read_body)),
                ("Content-Length in hex", http("POST", "/read", &authed(&bearer, &fingerprint, &[("Content-Length", "0x10")]), &read_body)),
                ("Content-Length negative", http("POST", "/read", &authed(&bearer, &fingerprint, &[("Content-Length", "-1")]), &read_body)),
                ("Content-Length beyond u64", http("POST", "/read", &authed(&bearer, &fingerprint, &[("Content-Length", "99999999999999999999999")]), &read_body)),
                ("Content-Length of 2 GiB with no body", http("POST", "/read", &authed(&bearer, &fingerprint, &[("Content-Length", "2147483648")]), "")),
                ("Transfer-Encoding chunked, valid body", format!("POST /read HTTP/1.1\r\nHost: localhost\r\nAuthorization: {bearer}\r\nx-aip-contract: {fingerprint}\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{read_body}\r\n0\r\n\r\n", read_body.len()).into_bytes()),
                ("Transfer-Encoding obfuscated (\"xchunked\")", format!("POST /read HTTP/1.1\r\nHost: localhost\r\nAuthorization: {bearer}\r\nx-aip-contract: {fingerprint}\r\nTransfer-Encoding: xchunked\r\nContent-Length: {smuggled_len}\r\n\r\n{read_body}").into_bytes()),
                ("obs-fold continuation line", format!("POST /read HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer bad\r\n  {token}\r\nx-aip-contract: {fingerprint}\r\nContent-Length: {smuggled_len}\r\n\r\n{read_body}").into_bytes()),
                ("bare LF line endings", format!("POST /read HTTP/1.1\nHost: localhost\nAuthorization: Bearer bad\nContent-Length: {smuggled_len}\n\n{read_body}").into_bytes()),
                ("header without a colon", format!("POST /read HTTP/1.1\r\nHost: localhost\r\nGarbage\r\nContent-Length: {smuggled_len}\r\n\r\n{read_body}").into_bytes()),
                ("NUL byte in a header value", format!("POST /read HTTP/1.1\r\nHost: localhost\r\nX-Test: a\0b\r\nContent-Length: {smuggled_len}\r\n\r\n{read_body}").into_bytes()),
                ("request line with an absolute URI", http("POST", &format!("http://{address}/read"), &authed(&bearer, &fingerprint, &[]), &read_body)),
                ("path with a double slash", http("POST", "//read", &authed(&bearer, &fingerprint, &[]), &read_body)),
                ("percent-encoded path /%72ead", http("POST", "/%72ead", &authed(&bearer, &fingerprint, &[]), &read_body)),
                ("trailing slash /read/", http("POST", "/read/", &authed(&bearer, &fingerprint, &[]), &read_body)),
                ("dot segments /x/../read", http("POST", "/x/../read", &authed(&bearer, &fingerprint, &[]), &read_body)),
                ("upper-case /READ", http("POST", "/READ", &authed(&bearer, &fingerprint, &[]), &read_body)),
                ("path traversal toward files", http("GET", "/../../../../etc/passwd", &[], "")),
                ("GET on /apply", http("GET", "/apply", &authed(&bearer, &fingerprint, &[]), "")),
                ("PUT on /read", http("PUT", "/read", &authed(&bearer, &fingerprint, &[]), &read_body)),
                ("X-HTTP-Method-Override", http("GET", "/read", &authed(&bearer, &fingerprint, &[("X-HTTP-Method-Override", "POST")]), &read_body)),
                ("TRACE", http("TRACE", "/read", &authed(&bearer, &fingerprint, &[]), "")),
                ("HTTP/1.0 request", format!("POST /read HTTP/1.0\r\nAuthorization: {bearer}\r\nx-aip-contract: {fingerprint}\r\nContent-Length: {smuggled_len}\r\n\r\n{read_body}").into_bytes()),
                ("HTTP/2 prior-knowledge preface", b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec()),
                ("HTTP/0.9 style request", b"GET /read\r\n\r\n".to_vec()),
                ("Expect: 100-continue", http("POST", "/read", &authed(&bearer, &fingerprint, &[("Expect", "100-continue")]), &read_body)),
                ("request line of 70 kB", http("POST", &format!("/{}", "a".repeat(70_000)), &authed(&bearer, &fingerprint, &[]), &read_body)),
                ("one header of 70 kB", http("POST", "/read", &authed(&bearer, &fingerprint, &[("X-Big", &"a".repeat(70_000))]), &read_body)),
                ("500 headers", many_headers(500)),
                ("30000 headers (about 360 kB)", many_headers(30_000)),
                ("300000 headers (about 4 MB)", many_headers(300_000)),
                ("body of 2 MiB", http("POST", "/read", &authed(&bearer, &fingerprint, &[]), &"x".repeat(2 * 1024 * 1024))),
            ];
            for (name, request) in &framing {
                let response = raw(&address, request, wait).await;
                // A request that carries a valid token may legitimately be served when the framing is merely unusual; what must
                // never happen is the unauthenticated hidden request being executed, which the data check below proves.
                record(&format!("framing: {name}"), &response);
            }
            let checked: bool = db.query_one(&format!("SELECT checked FROM {schema}.event WHERE id=11"), &[]).await.unwrap().get(0);
            assert!(!checked, "a smuggled or malformed request changed data (Event 11 was marked)");
            alive("framing attacks".into()).await;

            // 4. Hostile bodies from an authenticated caller.
            let deep = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
            let bodies: Vec<(&str, Vec<u8>)> = vec![
                ("JSON nested 100000 deep", deep.into_bytes()),
                ("duplicate keys, second names another resource", br#"{"query":{"read":"Event","select":["id"]},"query":{"read":"Member","select":["id"]}}"#.to_vec()),
                ("top-level array", b"[]".to_vec()),
                ("empty body", Vec::new()),
                ("invalid UTF-8", vec![b'{', 0xff, 0xfe, b'}']),
                ("UTF-8 BOM before JSON", [b"\xef\xbb\xbf".as_slice(), read_body.as_bytes()].concat()),
                ("trailing garbage after JSON", format!("{read_body} garbage").into_bytes()),
                ("huge integer limit", br#"{"query":{"read":"Event","select":["id"],"limit":99999999999999999999999999}}"#.to_vec()),
                ("negative limit", br#"{"query":{"read":"Event","select":["id"],"limit":-1}}"#.to_vec()),
                ("Cyrillic look-alike resource name", json!({"query":{"read":"\u{415}vent","select":["id"]}}).to_string().into_bytes()),
                ("resource name with SQL", json!({"query":{"read":"Event\"; DROP TABLE event;--","select":["id"]}}).to_string().into_bytes()),
                ("filter value with SQL", json!({"query":{"read":"Event","select":["id"],"filter":[{"field":"title","op":"eq","value":"x' OR '1'='1"}]}}).to_string().into_bytes()),
                ("select 100000 fields", json!({"query":{"read":"Event","select":vec!["id"; 100_000]}}).to_string().into_bytes()),
                ("proto pollution keys", json!({"__proto__":{"admin":true},"constructor":{"prototype":{}},"query":{"read":"Event","select":["id"]}}).to_string().into_bytes()),
            ];
            for (name, body) in &bodies {
                let mut request = format!(
                    "POST /read HTTP/1.1\r\nHost: localhost\r\nAuthorization: {bearer}\r\nx-aip-contract: {fingerprint}\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                request.extend_from_slice(body);
                let response = raw(&address, &request, wait).await;
                assert!(!response.contains("\"title\":\"other\""), "`{name}` exposed another member's row: {response}");
                record(&format!("body: {name}"), &response);
            }
            let events: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.event"), &[]).await.unwrap().get(0);
            assert_eq!(events, 2, "an injection attempt changed the table");
            alive("hostile bodies".into()).await;

            // 5. CORS: an untrusted Origin must never be reflected, and a preflight from one grants nothing.
            for origin in ["https://evil.example", "null", "https://app.example.com.evil.example", "https://evil.example?https://app.example.com", "http://app.example.com", "HTTPS://APP.EXAMPLE.COM"] {
                let response = raw(&address, &http("POST", "/read", &[("Authorization", &bearer), ("x-aip-contract", &fingerprint), ("Origin", origin)], &read_body), wait).await;
                let lower = response.to_ascii_lowercase();
                assert!(
                    !lower.contains(&format!("access-control-allow-origin: {}", origin.to_ascii_lowercase())) || origin.eq_ignore_ascii_case(ORIGIN) && false,
                    "untrusted Origin `{origin}` was reflected: {response}"
                );
                record(&format!("cors: request from {origin}"), &response);
                let preflight = raw(
                    &address,
                    &http("OPTIONS", "/apply", &[("Origin", origin), ("Access-Control-Request-Method", "POST"), ("Access-Control-Request-Headers", "authorization,x-aip-contract")], ""),
                    wait,
                )
                .await;
                assert!(!preflight.to_ascii_lowercase().contains(&format!("access-control-allow-origin: {}", origin.to_ascii_lowercase())), "preflight reflected `{origin}`: {preflight}");
                record(&format!("cors: preflight from {origin}"), &preflight);
            }

            // 6. Idle and stalled connections must not lock everybody out.
            let mut idle = Vec::new();
            for _ in 0..90 {
                if let Ok(Ok(mut socket)) = tokio::time::timeout(Duration::from_secs(1), tokio::net::TcpStream::connect(&address)).await {
                    let _ = socket.write_all(b"POST /read HTTP/1.1\r\nHost: localhost\r\n").await;
                    idle.push(socket);
                }
            }
            let during = raw(&address, &valid_read(&token), Duration::from_secs(2)).await;
            table.push(("slow: legitimate request while 90 connections stall".into(), status_line(&during), body_of(&during).chars().take(110).collect()));
            let mut recovered = false;
            let stalled_at = std::time::Instant::now();
            for _ in 0..16 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if body_of(&raw(&address, &valid_read(&token), Duration::from_secs(2)).await).contains("\"owned\"") {
                    recovered = true;
                    break;
                }
            }
            drop(idle);
            table.push(("slow: recovery after the stalled connections timed out".into(), format!("{:.1}s", stalled_at.elapsed().as_secs_f32()), String::new()));
            assert!(recovered, "the server never recovered from 90 stalled connections");
            alive("stalled connections".into()).await;

            for (name, status, body) in &table {
                eprintln!("{name:<70} | {status:<28} | {body}");
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
        })
        .catch_unwind()
        .await
    };
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
