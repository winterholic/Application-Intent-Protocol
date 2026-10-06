//! V6 전송 계층 실험. 최소 HTTP/1.1 서버(요청 하나당 연결 하나).
//! POST /read   {query}                 → {ok, rows, deps}
//! POST /apply  {request, key}          → {ok, changed, unchanged, tags}  같은 key 재요청은 저장된 결과
//! POST /status {key}                   → 저장된 결과 또는 조회 시점에 없음. 실행 중인 쓰기의 롤백 증거는 아니다.
//! POST /session {}                    → 서버가 검증한 principal과 남은 수명. 토큰 발급은 하지 않는다.
//! actor는 `authorization: Bearer <서명 토큰>`에서만 온다(V8, auth.rs). 토큰이 없으면 익명, 있는데 틀리면 거부.
//! `x-spike-drop-response: 1`이면 커밋 뒤 응답 없이 연결을 끊는다(시험용).
pub mod auth;
mod cors;
pub mod server;
pub mod write_extension;
use serde_json::{json, Value};
use spike_v2_read::id_wire::{emit_id, encode_rows, IdWire};
use spike_v2_read::plan::{plan_read_with_wire, Caller, Reject};
use spike_v2_read::{execute, sqlgen};
use spike_v3_write::{apply_in_with_wire, checks_in, classify_write_error, Knobs};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_postgres::Client;

/// 신선도 비교 실험의 상한. 제품 기본값이나 즉시 권한 회수 보장이 아니다.
pub const READ_CACHE_MS: u64 = 1000;

/// 멱등 키 저장소. 쓰기와 같은 트랜잭션에 결과를 남겨, 커밋됐으면 키가 있고 아니면 없다.
pub async fn prepare(db: &Client) {
    db.batch_execute(&idempotency_ddl()).await.unwrap();
}

pub fn idempotency_ddl() -> String {
    format!(
        "CREATE TABLE {}.aip_idem (principal text NOT NULL, key text, request text NOT NULL, result jsonb NOT NULL, PRIMARY KEY (principal, key))",
        sqlgen::schema()
    )
}

/// 쓰기가 바꿀 수 있는 resource(캐시 무효화 태그). 전이 대상 + create·update 효과 대상.
fn write_tags(facts: &Value, name: &str) -> Vec<String> {
    let Some((res, tr)) = name.split_once('.') else { return vec![] };
    let mut out = vec![res.to_string()];
    for e in facts["resources"][res]["transitions"][tr]["effects"].as_array().into_iter().flatten() {
        for k in ["create", "update"] {
            if let Some(t) = e[k].as_str() {
                out.push(t.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn err(e: Reject) -> Value {
    json!({ "ok": false, "code": e.code, "msg": e.msg })
}

fn principal_key(actor: Option<i64>, wire: IdWire) -> String {
    let principal = actor.map_or_else(|| "anonymous".to_string(), |id| format!("actor:{id}"));
    if wire == IdWire::Legacy {
        principal
    } else {
        format!("{principal}:ids:{}", wire.label())
    }
}

// 제어 문자는 DB text에 들어가지 못하거나(NUL) 로그·화면을 오염시키므로 key에서 받지 않는다.
fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 100 && !key.chars().any(|c| c.is_control() || invisible_format(c))
}

// 화면에 안 보이거나 표시 방향을 바꾸는 서식 문자. 같아 보이는 다른 key를 만들 수 있다.
fn invisible_format(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

async fn handle_apply(db: &mut Client, facts: &Value, body: &Value, caller: &Caller, wire: IdWire) -> Value {
    let Some(key) = body["key"].as_str().filter(|k| valid_key(k)) else {
        return json!({ "ok": false, "code": "BAD_REQUEST", "msg": "key 필요" });
    };
    let req = &body["request"];
    let req_text = req.to_string();
    let principal = principal_key(caller.actor_id, wire);
    let s = sqlgen::schema();
    let started = std::time::Instant::now();
    let mut commit_sent = false;
    let operation = async {
        let tx = match db.transaction().await {
            Ok(t) => t,
            Err(_) => return json!({ "ok": false, "code": "INTERNAL" }),
        };
        if let Err(error) = tx.batch_execute("SET LOCAL statement_timeout = '2000ms'; SET LOCAL TimeZone = 'UTC'").await {
            return err(classify_write_error("세션 설정", error));
        }
        // 같은 키가 이미 커밋됐으면 다시 실행하지 않고 저장된 결과를 준다. 같은 키에 다른 요청이면 거부.
        // 동시에 같은 키가 오면 키 행 잠금으로 하나씩 처리한다.
        if let Err(error) = tx.execute("SELECT pg_advisory_xact_lock(hashtext($1))", &[&format!("{principal}:{key}")]).await {
            return err(classify_write_error("멱등 잠금", error));
        }
        if let Ok(Some(row)) = tx
            .query_opt(format!("SELECT request, result::text FROM {s}.aip_idem WHERE principal = $1 AND key = $2").as_str(), &[&principal, &key])
            .await
        {
            let (stored_req, result): (String, String) = (row.get(0), row.get(1));
            if stored_req != req_text {
                return json!({ "ok": false, "code": "IDEMPOTENCY_MISMATCH", "msg": "같은 키에 다른 요청" });
            }
            let Ok(mut v) = serde_json::from_str::<Value>(&result) else {
                return json!({ "ok": false, "code": "INTERNAL", "msg": "저장된 멱등 결과를 읽을 수 없음" });
            };
            v["replayed"] = json!(true);
            return v;
        }
        let knobs = Knobs::default();
        let applied = match apply_in_with_wire(&tx, facts, req, caller, &knobs, wire).await {
            Ok(a) => a,
            Err(e) => return err(e),
        };
        if let Err(e) = checks_in(&tx, facts, caller, &knobs).await {
            return err(e);
        }
        // wire에 표현할 수 없는 결과면 멱등 결과 기록·commit 전 전체 transaction이 rollback된다.
        let changed = match applied.changed.into_iter().map(|id| emit_id(id, wire)).collect::<Result<Vec<_>, _>>() {
            Ok(ids) => ids,
            Err(e) => return err(e),
        };
        let unchanged = match applied.unchanged.into_iter().map(|id| emit_id(id, wire)).collect::<Result<Vec<_>, _>>() {
            Ok(ids) => ids,
            Err(e) => return err(e),
        };
        let result = json!({
            "ok": true, "changed": changed, "unchanged": unchanged,
            "tags": write_tags(facts, req["apply"].as_str().unwrap_or("")),
        });
        if tx
            .execute(
                format!("INSERT INTO {s}.aip_idem (principal, key, request, result) VALUES ($1, $2, $3, $4::text::jsonb)").as_str(),
                &[&principal, &key, &req_text, &result.to_string()],
            )
            .await
            .is_err()
        {
            return json!({ "ok": false, "code": "INTERNAL" });
        }
        if started.elapsed().as_millis() + u128::from(spike_v3_write::COMMIT_MARGIN_MS) >= u128::from(spike_v3_write::WRITE_DEADLINE_MS) {
            return json!({"ok":false,"code":"DEADLINE_EXCEEDED"});
        }
        commit_sent = true;
        match tx.commit().await {
            Ok(()) => result,
            Err(e) => err(classify_write_error("커밋", e)),
        }
    };
    match tokio::time::timeout(std::time::Duration::from_millis(spike_v3_write::WRITE_DEADLINE_MS), operation).await {
        Ok(result) => result,
        Err(_) if commit_sent => json!({"ok":false,"code":"COMMIT_UNKNOWN"}),
        Err(_) => json!({"ok":false,"code":"DEADLINE_EXCEEDED"}),
    }
}

async fn handle(db: &mut Client, facts: &Value, path: &str, body: &Value, caller: &Caller, wire: IdWire) -> Value {
    match path {
        "/read" => match plan_read_with_wire(facts, &body["query"], caller, wire) {
            Ok(p) => match tokio::time::timeout(std::time::Duration::from_millis(p.deadline_ms), execute(db, &p)).await {
                Ok(Ok(rows)) => {
                    let rows = match encode_rows(rows, &p.output_type, wire) {
                        Ok(rows) => rows,
                        Err(e) => return err(e),
                    };
                    // 현재 Ctx는 now를 반드시 이 값으로 바인딩한다. 같은 값의 일반 필터는 보수적으로 미캐시.
                    let time_dependent = p.params.iter().any(|v| v.as_deref() == Some(caller.now.as_str()));
                    json!({ "ok": true, "rows": rows, "deps": p.deps, "maxAgeMs": if time_dependent { 0 } else { READ_CACHE_MS } })
                }
                Ok(Err(e)) => err(e),
                Err(_) => json!({"ok":false,"code":"DEADLINE_EXCEEDED"}),
            },
            Err(e) => err(e),
        },
        "/apply" => handle_apply(db, facts, body, caller, wire).await,
        "/status" => {
            let principal = principal_key(caller.actor_id, wire);
            let Some(key) = body["key"].as_str().filter(|k| valid_key(k)) else {
                return json!({ "ok": false, "code": "BAD_REQUEST", "msg": "key 필요" });
            };
            let request_text = body["request"].to_string();
            let q = format!("SELECT request, result::text FROM {}.aip_idem WHERE principal = $1 AND key = $2", sqlgen::schema());
            match tokio::time::timeout(server::DB_PREFLIGHT_TIMEOUT, db.query_opt(q.as_str(), &[&principal, &key])).await {
                // 키만으로 결과를 주지 않는다. 같은 요청인지 확인한다(r7 R7-01).
                Ok(Ok(Some(r))) if r.get::<_, String>(0) != request_text => {
                    json!({ "ok": false, "code": "IDEMPOTENCY_MISMATCH", "msg": "같은 키에 다른 요청" })
                }
                Ok(Ok(Some(r))) => {
                    let Ok(mut v) = serde_json::from_str::<Value>(&r.get::<_, String>(1)) else {
                        return json!({ "ok": false, "code": "INTERNAL", "msg": "저장된 멱등 결과를 읽을 수 없음" });
                    };
                    v["replayed"] = json!(true);
                    v
                }
                // 조회 시점에 커밋된 결과가 없다는 뜻일 뿐, 진행 중일 수 있다(r7 R7-02). 확정은 같은 키 재요청으로 한다.
                Ok(Ok(None)) => json!({ "ok": false, "code": "NOT_FOUND" }),
                Ok(Err(_)) | Err(_) => json!({ "ok": false, "code": "DB_UNAVAILABLE" }),
            }
        }
        _ => json!({ "ok": false, "code": "NOT_FOUND" }),
    }
}

pub const MAX_BODY: usize = 1 << 20;
pub const MAX_HEADER_LINE: usize = 8 << 10;
pub const MAX_HEADERS: usize = 16 << 10;
pub const MAX_TOKEN: usize = 4 << 10;

async fn header_line(rd: &mut (impl tokio::io::AsyncBufRead + Unpin), line: &mut String, deadline: tokio::time::Instant) -> Result<(), &'static str> {
    let read = tokio::time::timeout_at(deadline, (&mut *rd).take((MAX_HEADER_LINE + 1) as u64).read_line(line)).await;
    match read {
        Err(_) => Err("REQUEST_TIMEOUT"),
        Ok(Ok(n)) if n > 0 && n <= MAX_HEADER_LINE && line.ends_with("\r\n") => Ok(()),
        _ => Err("BAD_REQUEST"),
    }
}

fn v_length_over_limit(header: &str) -> bool {
    header.split_once(':').and_then(|(_, value)| value.trim().strip_prefix("Bearer ")).is_some_and(|token| token.len() > MAX_TOKEN)
}

struct Response<W> {
    writer: W,
    origin: Option<String>,
}

async fn reply(w: &mut Response<impl AsyncWriteExt + Unpin>, out: &str) {
    reply_with_headers(w, out, "vary: Origin\r\n").await;
}

async fn reply_with_headers(w: &mut Response<impl AsyncWriteExt + Unpin>, out: &str, extra: &str) {
    let origin = w.origin.as_ref().map(|origin| format!("access-control-allow-origin: {origin}\r\n")).unwrap_or_default();
    let resp = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n{origin}{extra}\r\n{out}",
        out.len()
    );
    // A stalled response may be lost after commit; it never cancels DB execution.
    let _ = tokio::time::timeout(server::REQUEST_TIMEOUT, w.writer.write_all(resp.as_bytes())).await;
}

async fn serve_conn(
    mut sock: TcpStream,
    facts: Arc<Value>,
    keys: auth::Keyring,
    contract_fingerprint: Option<Arc<str>>,
    wire: IdWire,
    runtime: Arc<server::Runtime>,
    deadline: tokio::time::Instant,
) {
    let Ok(local) = sock.local_addr() else {
        return;
    };
    let (r, w) = sock.split();
    let mut w = Response { writer: w, origin: None };
    let mut rd = BufReader::new(r);
    let mut line = String::new();
    if let Err(code) = header_line(&mut rd, &mut line, deadline).await {
        return reply(&mut w, &json!({ "ok": false, "code": code, "msg": "요청 줄 상한 또는 형식 오류" }).to_string()).await;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        return reply(&mut w, &json!({"ok":false,"code":"BAD_REQUEST"}).to_string()).await;
    }
    let (mut origin, mut host, mut request_method, mut request_headers) = (None::<String>, None::<String>, None::<String>, None::<String>);
    let (mut len, mut token, mut drop) = (None::<usize>, None::<String>, false);
    let mut expected_contract: Option<String> = None;
    let mut bad: Option<&str> = None;
    let mut invalid_browser_headers = false;
    let mut header_bytes = line.len();
    loop {
        let mut h = String::new();
        if let Err(code) = header_line(&mut rd, &mut h, deadline).await {
            return reply(&mut w, &json!({ "ok": false, "code": code, "msg": "헤더 줄 상한 또는 형식 오류" }).to_string()).await;
        }
        header_bytes += h.len();
        if header_bytes > MAX_HEADERS {
            return reply(&mut w, &json!({ "ok": false, "code": "BAD_REQUEST", "msg": "헤더 총량 상한" }).to_string()).await;
        }
        if h == "\r\n" {
            break;
        }
        if !h.contains(':') || h.starts_with([' ', '\t']) || h.trim_end_matches("\r\n").bytes().any(|b| b.is_ascii_control() && b != b'\t') {
            bad = Some("잘못된 헤더 형식");
        }
        let (k, v) = h.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).unwrap_or_default();
        if k.is_empty() || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)) {
            bad = Some("잘못된 헤더 이름");
        }
        match k.as_str() {
            "origin" | "host" | "access-control-request-method" | "access-control-request-headers" => {
                let target = match k.as_str() {
                    "origin" => &mut origin,
                    "host" => &mut host,
                    "access-control-request-method" => &mut request_method,
                    _ => &mut request_headers,
                };
                if target.is_some() {
                    bad = Some("중복된 브라우저 헤더");
                    invalid_browser_headers = true;
                } else {
                    *target = Some(v);
                }
            }
            // 길이는 한 번, 숫자로만. 잘못되거나 중복이면 처리하지 않는다(r7 R7-05).
            "content-length" => match (len, v.parse::<usize>()) {
                (None, Ok(n)) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => len = Some(n),
                _ => bad = Some("잘못되거나 중복된 content-length"),
            },
            "transfer-encoding" => bad = Some("transfer-encoding 미지원"),
            "authorization" => match (token.is_some(), v.strip_prefix("Bearer ")) {
                (false, Some(t)) if t.len() <= MAX_TOKEN => token = Some(t.to_string()),
                _ => bad = Some("잘못되거나 중복된 authorization"),
            },
            "x-aip-contract" => {
                if expected_contract.is_some() || v.len() != 64 || !v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                    bad = Some("잘못되거나 중복된 계약 식별값");
                } else {
                    expected_contract = Some(v);
                }
            }
            "x-spike-drop-response" => drop = runtime.fault_injection && v == "1",
            _ => {}
        }
        if k == "authorization" && v_length_over_limit(&h) {
            if let Some(origin) =
                origin.as_deref().filter(|origin| !invalid_browser_headers && runtime.options.allows(origin, host.as_deref(), local))
            {
                w.origin = Some(origin.to_string());
            }
            return reply(&mut w, &json!({ "ok": false, "code": "BAD_REQUEST", "msg": bad }).to_string()).await;
        }
    }
    let err = |code: &str, msg: &str| json!({ "ok": false, "code": code, "msg": msg }).to_string();
    if let Some(origin) = origin.as_deref() {
        if invalid_browser_headers || !runtime.options.allows(origin, host.as_deref(), local) {
            return reply(&mut w, &err("ORIGIN_NOT_ALLOWED", "허용하지 않은 로컬 출처")).await;
        }
        w.origin = Some(origin.to_string());
    }
    if let Some(m) = bad {
        return reply(&mut w, &err("BAD_REQUEST", m)).await;
    }
    if !["/session", "/read", "/apply", "/status", "/extension"].contains(&path.as_str()) {
        return reply(&mut w, &err("NOT_FOUND", "알 수 없는 경로")).await;
    }
    if method == "OPTIONS" {
        if w.origin.is_none()
            || request_method.as_deref() != Some("POST")
            || !cors::allowed_request_headers(request_headers.as_deref())
            || len.is_some_and(|len| len != 0)
        {
            return reply(&mut w, &err("BAD_REQUEST", "잘못된 preflight")).await;
        }
        return reply_with_headers(&mut w, &json!({"ok":true}).to_string(), "vary: Origin, Access-Control-Request-Method, Access-Control-Request-Headers\r\naccess-control-allow-methods: POST\r\naccess-control-allow-headers: authorization, content-type, x-aip-contract\r\n").await;
    }
    if method != "POST" {
        return reply(&mut w, &err("BAD_REQUEST", "POST만")).await;
    }
    let Some(len) = len else { return reply(&mut w, &err("BAD_REQUEST", "content-length 필요")).await };
    // 상한을 넘는 본문은 읽기 전에 거부한다. 잘라서 처리하지 않는다(r7 R7-04).
    if len > MAX_BODY {
        return reply(&mut w, &err("PAYLOAD_TOO_LARGE", "본문 1MiB 초과")).await;
    }
    let mut buf = vec![0u8; len];
    match tokio::time::timeout_at(deadline, rd.read_exact(&mut buf)).await {
        Err(_) => return reply(&mut w, &err("REQUEST_TIMEOUT", "요청 본문 수신 기한 초과")).await,
        Ok(Err(_)) => return,
        Ok(Ok(_)) => {}
    }
    let Ok(body) = serde_json::from_slice::<Value>(&buf) else {
        return reply(&mut w, &err("BAD_REQUEST", "JSON 형식 오류")).await;
    };
    let verification = match token.as_deref() {
        None => None,
        Some(token) => Some(match &runtime.authenticator {
            None => keys.verify_session(token),
            Some(provider) => match tokio::time::timeout_at(deadline, provider.verify_session(token)).await {
                Ok(result) => result,
                Err(_) => Err(auth::AuthError::Unavailable),
            },
        }),
    };
    let session = match verification {
        None => None,
        Some(Ok(s)) => Some(s),
        Some(Err(auth::AuthError::Expired)) => return reply(&mut w, &err("TOKEN_EXPIRED", "세션 만료")).await,
        Some(Err(auth::AuthError::Unavailable)) => return reply(&mut w, &err("DB_UNAVAILABLE", "인증 제공자 또는 사용자 연결 조회 실패")).await,
        Some(Err(_)) => return reply(&mut w, &err("UNAUTHENTICATED", "토큰 검증 실패")).await,
    };
    // 기대 계약은 권한을 부여하지 않는다. 불일치면 DB·멱등 재생·쓰기 실행 전에 거부한다.
    let write_extension = path == "/apply" && body["request"].get("extension").is_some();
    if ["/read", "/apply", "/status"].contains(&path.as_str()) && !write_extension {
        if let Some(expected) = expected_contract.as_deref() {
            if contract_fingerprint.as_deref() != Some(expected) {
                return reply(&mut w, &err("CONTRACT_MISMATCH", "생성 계약과 서버 계약이 다름")).await;
            }
        }
    }
    if write_extension {
        if !runtime.write_extensions || runtime.extensions.is_none() {
            return reply(&mut w, &err("NOT_FOUND", "공식 WRITE 확장은 명시적 서버 설정이 필요함")).await;
        }
        if session.is_none() {
            return reply(&mut w, &err("UNAUTHENTICATED", "공식 WRITE 확장은 서명 세션이 필요함")).await;
        }
        if expected_contract.is_none() || expected_contract.as_deref() != contract_fingerprint.as_deref() {
            return reply(&mut w, &err("CONTRACT_MISMATCH", "생성 WRITE 계약 지문이 필요함")).await;
        }
        let request = &body["request"];
        let exact = body.as_object().is_some_and(|body| body.len() == 2 && body.contains_key("key") && body.contains_key("request"))
            && request.as_object().is_some_and(|request| request.len() == 2 && request.contains_key("extension") && request.contains_key("input"))
            && body["key"].as_str().is_some_and(valid_key);
        let Some(name) = request["extension"].as_str().filter(|_| exact) else {
            return reply(&mut w, &err("BAD_REQUEST", "WRITE 요청은 key/request와 extension/input만 허용함")).await;
        };
        if let Err(error) = spike_v4_worker::write::validate_write(&facts, name, &request["input"], wire) {
            return reply(&mut w, &err(error.code, &error.msg)).await;
        }
    }
    if path == "/extension" {
        if runtime.extensions.is_none() {
            return reply(&mut w, &err("NOT_FOUND", "공식 확장은 명시적 서버 설정이 필요함")).await;
        }
        if session.is_none() {
            return reply(&mut w, &err("UNAUTHENTICATED", "공식 확장은 서명 세션이 필요함")).await;
        }
        if expected_contract.is_none() || expected_contract.as_deref() != contract_fingerprint.as_deref() {
            return reply(&mut w, &err("CONTRACT_MISMATCH", "생성 확장 계약 지문이 필요함")).await;
        }
        let valid_body = body.as_object().is_some_and(|o| o.len() == 2 && o.contains_key("extension") && o.contains_key("input"));
        let Some(name) = body["extension"].as_str().filter(|_| valid_body) else {
            return reply(&mut w, &err("BAD_REQUEST", "확장 본문은 extension/input만 허용함")).await;
        };
        if let Err(e) = spike_v4_worker::validate_read(&facts, name, &body["input"], wire) {
            return reply(&mut w, &err(e.code, &e.msg)).await;
        }
    }
    if path == "/session" {
        if !body.as_object().is_some_and(|body| body.is_empty()) {
            return reply(&mut w, &err("BAD_REQUEST", "session 본문은 빈 객체")).await;
        }
        let remaining = session.as_ref().map(auth::Session::remaining_ms);
        if remaining == Some(0) {
            return reply(&mut w, &err("TOKEN_EXPIRED", "세션 만료")).await;
        }
        let result = json!({
            "ok": true,
            "principal": { "actorId": session.as_ref().map(|s| s.actor_id.to_string()) },
            "remainingMs": remaining,
        });
        return reply(&mut w, &result.to_string()).await;
    }
    let _extension_slot = if path == "/extension" || write_extension {
        match runtime.extension_slots.try_acquire() {
            Ok(slot) => Some(slot),
            Err(_) => return reply(&mut w, &err("WORKER_BUSY", "로컬 확장 동시 호출 상한")).await,
        }
    } else {
        None
    };
    let mut db = match tokio::time::timeout(server::DB_CONNECT_TIMEOUT, spike_v2_read::connect_owned_with_url(&runtime.db_url)).await {
        Ok(Ok(db)) => db,
        Ok(Err(_)) | Err(_) => return reply(&mut w, &err("DB_UNAVAILABLE", "DB 연결 실패 또는 기한 초과")).await,
    };
    if let Some(fence) = &runtime.deployment {
        let checked = tokio::time::timeout_at(deadline, async {
            db.batch_execute("SET lock_timeout='2s'; SET statement_timeout='5s'").await?;
            db.query_one("SELECT pg_advisory_lock_shared(1095323725,hashtext($1))", &[&fence.schema]).await?;
            let marker = db.query_opt(&format!("SELECT facts_digest FROM {}.aip_migrate_meta WHERE singleton=true", fence.schema), &[]).await?;
            db.batch_execute("RESET lock_timeout; RESET statement_timeout").await?;
            Ok::<_, tokio_postgres::Error>(marker.map(|row| row.get::<_, String>(0)))
        })
        .await;
        match checked {
            Ok(Ok(Some(actual))) if actual == fence.expected_digest => {}
            Ok(Ok(Some(_))) => {
                std::mem::drop(db);
                return reply(&mut w, &err("DEPLOYMENT_CHANGED", "실행 정의와 배포 정의가 다름")).await;
            }
            Ok(Ok(None)) => {
                std::mem::drop(db);
                return reply(&mut w, &err("SCHEMA_MISMATCH", "배포 marker 누락")).await;
            }
            Ok(Err(_)) | Err(_) => {
                std::mem::drop(db);
                return reply(&mut w, &err("DB_UNAVAILABLE", "배포 잠금 또는 marker 조회 실패")).await;
            }
        }
    }
    let now = match tokio::time::timeout(server::DB_PREFLIGHT_TIMEOUT, db.query_one("SELECT clock_timestamp()::text", &[])).await {
        Ok(Ok(row)) => row.get::<_, String>(0),
        Ok(Err(_)) | Err(_) => {
            std::mem::drop(db);
            return reply(&mut w, &err("DB_UNAVAILABLE", "실행 전 DB 조회 실패 또는 기한 초과")).await;
        }
    };
    let caller = Caller { actor_id: session.as_ref().map(|s| s.actor_id), now };
    let mut result = if write_extension {
        let mut result = write_extension::apply(&mut db, &facts, &body, &caller, wire, runtime.extensions.as_ref().unwrap()).await;
        if result["ok"] == false {
            result["msg"] = json!("공식 WRITE 확장 호출 거부");
        }
        result
    } else if path == "/extension" {
        let config = runtime.extensions.as_ref().unwrap();
        let mut worker = match spike_v4_worker::Worker::try_start_with(
            config.lang,
            config.dir.to_str().unwrap(),
            spike_v4_worker::Isolation::MacNetDeny,
            spike_v4_worker::WorkerLimits::default(),
        )
        .await
        {
            Ok(worker) => worker,
            Err(_) => {
                std::mem::drop(db);
                return reply(&mut w, &err("WORKER_FAILED", "공식 worker 실행 실패")).await;
            }
        };
        let name = body["extension"].as_str().unwrap();
        let (resource, extension) = name.split_once('.').unwrap();
        let deadline = facts["resources"][resource]["extensions"][extension]["deadlineMs"].as_u64().unwrap_or(1000);
        // The outer deadline also covers a worker that stops reading its stdin.
        let output = tokio::time::timeout(
            std::time::Duration::from_millis(deadline),
            spike_v4_worker::invoke_with_wire(&mut db, &mut worker, &facts, name, &body["input"], &caller, wire),
        )
        .await;
        worker.stop().await;
        match output {
            Ok(Ok(output)) => json!({"ok":true,"output":output}),
            Ok(Err(e)) => json!({"ok":false,"code":e.code,"msg":"공식 읽기 확장 호출 거부"}),
            Err(_) => json!({"ok":false,"code":"DEADLINE_EXCEEDED","msg":"공식 읽기 확장 기한 초과"}),
        }
    } else {
        handle(&mut db, &facts, &path, &body, &caller, wire).await
    };
    let write_status = path == "/status" && body["request"].get("extension").is_some();
    if (["/read", "/extension"].contains(&path.as_str()) || write_extension || write_status) && result["ok"] == true {
        if let Some(fingerprint) = contract_fingerprint {
            result["contractFingerprint"] = json!(&*fingerprint);
        }
        // 커밋된 결과를 인증 거부로 바꾸면 클라이언트가 실제 쓰기 효과를 놓친다.
        if let Some(session) = session.filter(|_| ["/read", "/extension"].contains(&path.as_str())) {
            let remaining = session.remaining_ms();
            if remaining == 0 {
                std::mem::drop(db);
                return reply(&mut w, &err("TOKEN_EXPIRED", "읽는 동안 세션 만료")).await;
            }
            if path == "/read" {
                result["maxAgeMs"] = json!(result["maxAgeMs"].as_u64().unwrap_or(0).min(remaining));
            }
        }
    }
    let out = result.to_string();
    std::mem::drop(db);
    if drop {
        // 응답 유실 흉내: 처리(커밋 포함)는 끝났지만 클라이언트는 응답을 받지 못한다.
        return;
    }
    reply(&mut w, &out).await;
}

/// 서버를 띄우고 (포트, 토큰 발급기)를 돌려준다. 발급기는 시험에서 로그인 제공자 역할을 한다.
pub async fn start(facts: Value) -> (u16, auth::Keyring) {
    start_with_id_wire(facts, IdWire::Legacy).await
}

pub async fn start_with_apply_contract(facts: Value, wire: IdWire) -> (u16, auth::Keyring) {
    let fingerprint = facts["resources"].is_object().then(|| spike_v5_sdk::contract_fingerprint_with_apply(&facts, wire).into());
    start_with_fingerprint(facts, wire, fingerprint).await
}

/// V13 비교 후보. wire는 서버 설정이며 호출자의 JSON 옵션으로 받지 않는다.
pub async fn start_with_id_wire(facts: Value, wire: IdWire) -> (u16, auth::Keyring) {
    let fingerprint = facts["resources"].is_object().then(|| spike_v5_sdk::contract_fingerprint_with_wire(&facts, wire).into());
    start_with_fingerprint(facts, wire, fingerprint).await
}

async fn start_with_fingerprint(facts: Value, wire: IdWire, contract_fingerprint: Option<Arc<str>>) -> (u16, auth::Keyring) {
    let server = server::listen_with_fingerprint(
        facts,
        wire,
        "127.0.0.1:0".parse().unwrap(),
        server::Runtime {
            db_url: spike_v2_read::DB_URL.into(),
            fault_injection: true,
            extensions: None,
            write_extensions: false,
            extension_slots: tokio::sync::Semaphore::new(4),
            options: server::ServerOptions::default(),
            authenticator: None,
            deployment: None,
        },
        contract_fingerprint,
    )
    .await
    .unwrap();
    let port = server.address.port();
    let keys = server.keys.clone();
    server.detach();
    (port, keys)
}
