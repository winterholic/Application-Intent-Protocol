//! V6 전송 계층 실험. 최소 HTTP/1.1 서버(요청 하나당 연결 하나).
//! POST /read   {query}                 → {ok, rows, deps}
//! POST /apply  {request, key}          → {ok, changed, unchanged, tags}  같은 key 재요청은 저장된 결과
//! POST /status {key}                   → 저장된 결과 또는 조회 시점에 없음. 실행 중인 쓰기의 롤백 증거는 아니다.
//! POST /session {}                    → 서버가 검증한 principal과 남은 수명. 토큰 발급은 하지 않는다.
//! actor는 `authorization: Bearer <서명 토큰>`에서만 온다(V8, auth.rs). 토큰이 없으면 익명, 있는데 틀리면 거부.
//! `x-spike-drop-response: 1`이면 커밋 뒤 응답 없이 연결을 끊는다(시험용).
mod admission;
pub mod auth;
pub mod client_codes;
mod cors;
mod durable;
mod health;
pub mod hosting;
pub mod infrastructure;
mod messaging_transport;
mod metrics;
mod object_transport;
mod operation;
pub mod otlp;
pub mod pool;
pub mod rag_transport;
pub mod realtime;
mod relational;
pub mod schedule;
pub mod server;
pub(crate) mod slots;
pub mod trace;
pub mod triggers;
pub mod wire_codes;
pub mod write_extension;
use serde_json::{json, Value};
use spike_v2_read::backend::{RelationalWorkerWrite, RelationalWorkerWriteStart};
use spike_v2_read::id_wire::{emit_entity_id, encode_rows, IdWire};
use spike_v2_read::plan::{plan_read_with_wire, Caller, Reject};
use spike_v2_read::{execute, sqlgen};
use spike_v3_write::{apply_in_with_wire, checks_in, classify_write_error, Knobs};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_postgres::Client;

fn capability_available(operation: &Value, worker_configured: bool, authenticator_configured: bool, deployment_configured: bool) -> bool {
    if operation["lifetime"] == "durable" {
        worker_configured && authenticator_configured && deployment_configured
    } else {
        worker_configured
    }
}

struct RelationalWriteActionHost {
    session: Box<dyn RelationalWorkerWrite>,
}

impl spike_v4_worker::write::WriteActionHost for RelationalWriteActionHost {
    fn apply<'a>(
        &'a mut self,
        transition: &'a str,
        request: &'a Value,
        caller: &'a Caller,
        wire: IdWire,
    ) -> spike_v4_worker::host::HostFuture<'a, Result<Value, Reject>> {
        self.session.apply(transition, request, caller, wire)
    }
}

async fn run_object_transfer<F>(lifetime: std::time::Duration, transfer: F) -> Result<bool, tokio::time::error::Elapsed>
where
    F: std::future::Future<Output = bool>,
{
    tokio::time::timeout(lifetime, transfer).await
}

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
        for k in ["create", "update", "delete"] {
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

fn managed_file_id<'a>(mount: &hosting::ManagedFileMount, target: &'a str) -> Result<&'a str, ()> {
    let path = target.split_once('?').map_or(target, |(path, _)| path);
    let rest = path.strip_prefix(&mount.prefix).ok_or(())?.strip_prefix('/').ok_or(())?;
    if rest.len() != 36
        || !rest.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            }
        })
    {
        return Err(());
    }
    Ok(rest)
}

fn valid_managed_media_type(value: &str) -> bool {
    let Some((kind, subtype)) = value.split_once('/') else { return false };
    !kind.is_empty()
        && !subtype.is_empty()
        && value.len() <= 127
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'+' | b'.' | b'-'))
}

// 화면에 안 보이거나 표시 방향을 바꾸는 서식 문자. 같아 보이는 다른 key를 만들 수 있다.
fn invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{E0000}'..='\u{E007F}'
    )
}

/// 쓰기 요청 하나를 호출자의 트랜잭션 안에서 실행해 응답 본문을 만든다. 커밋은 호출자가 한다.
/// 형식은 키로 구분한다: `apply`(전이 하나), `atomic`(공개 동작 묶음), `compose`(대상 목록에 단계 조합).
async fn run_write(
    tx: &tokio_postgres::Transaction<'_>,
    facts: &Value,
    req: &Value,
    caller: &Caller,
    knobs: &Knobs,
    wire: IdWire,
) -> Result<Value, Reject> {
    let emit_all = |ids: Vec<spike_v2_read::id_wire::EntityId>| ids.into_iter().map(|id| emit_entity_id(id, wire)).collect::<Result<Vec<_>, _>>();
    if req.get("atomic").is_some() || req.get("compose").is_some() {
        let out = if req.get("atomic").is_some() {
            spike_v3_write::bundle_in(tx, facts, req, caller, knobs, wire).await?
        } else {
            spike_v3_write::compose_in(tx, facts, req, caller, knobs, wire).await?
        };
        let mut applied = vec![];
        for a in out.applied {
            applied.push(json!({ "changed": emit_all(a.changed)?, "unchanged": emit_all(a.unchanged)? }));
        }
        let mut created = vec![];
        for (resource, id) in out.created_rows {
            created.push(json!({ "resource": resource, "id": match id { Some(id) => emit_entity_id(id, wire)?, None => Value::Null } }));
        }
        // wire에 표현할 수 없는 id가 있으면 멱등 결과 기록·commit 전 전체 transaction이 rollback된다.
        return Ok(json!({ "ok": true, "applied": applied, "created": created, "tags": bundle_tags(facts, req) }));
    }
    let applied = apply_in_with_wire(tx, facts, req, caller, knobs, wire).await?;
    Ok(json!({
        "ok": true, "changed": emit_all(applied.changed)?, "unchanged": emit_all(applied.unchanged)?,
        "tags": write_tags(facts, req["apply"].as_str().unwrap_or("")),
    }))
}

/// 묶음·조합이 바꿀 수 있는 resource(캐시 무효화 태그). 연산마다 `write_tags`와 같은 규칙이다.
fn bundle_tags(facts: &Value, req: &Value) -> Vec<String> {
    let mut out = vec![];
    if let Some(res) = req["compose"].as_str() {
        out.push(res.to_string());
        for t in req["steps"].as_array().into_iter().flatten() {
            if let Some(tr) = t["transition"].as_str() {
                out.extend(write_tags(facts, &format!("{res}.{tr}")));
            } else if let Some(target) = t["create"].as_str() {
                out.push(target.to_string());
            }
        }
    }
    for op in req["atomic"].as_array().into_iter().flatten() {
        if let Some(name) = op["apply"].as_str() {
            out.extend(write_tags(facts, name));
        } else if let Some(target) = op["create"].as_str() {
            out.push(target.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

async fn handle_apply(db: &mut Client, facts: &Value, body: &Value, caller: &Caller, wire: IdWire) -> Value {
    let Some(key) = body["key"].as_str().filter(|k| valid_key(k)) else {
        return json!({ "ok": false, "code": "WIRE.BAD_REQUEST", "msg": "key 필요" });
    };
    let req = &body["request"];
    let req_text = req.to_string();
    let principal = principal_key(caller.actor_id, wire);
    // 다른 행을 읽는 정책에 의존하는 전이는 SERIALIZABLE로 실행하고, 직렬화 실패는 같은 요청을 서버가 다시 실행한다(O-26).
    let serializable = spike_v3_write::write_needs_serializable(facts, req);
    let started = std::time::Instant::now();
    let commit_sent = std::sync::atomic::AtomicBool::new(false);
    let operation = async {
        let mut attempt = 0;
        loop {
            let result = apply_attempt(db, facts, key, req, &req_text, &principal, caller, wire, serializable, &commit_sent, started).await;
            let retry = serializable
                && result["code"] == "WIRE.CONFLICT"
                && result["msg"].as_str().is_some_and(|msg| spike_v3_write::is_retryable(&Reject { code: "WIRE.CONFLICT", msg: msg.to_string() }))
                && attempt < spike_v3_write::SERIALIZATION_RETRIES;
            if !retry {
                return result;
            }
            attempt += 1;
            spike_v3_write::retry_backoff(attempt).await;
        }
    };
    match tokio::time::timeout(std::time::Duration::from_millis(spike_v3_write::WRITE_DEADLINE_MS), operation).await {
        Ok(result) => result,
        Err(_) if commit_sent.load(std::sync::atomic::Ordering::SeqCst) => json!({"ok":false,"code":"WIRE.COMMIT_UNKNOWN"}),
        Err(_) => json!({"ok":false,"code":"WIRE.DEADLINE_EXCEEDED"}),
    }
}

/// 트랜잭션 한 번의 시도. 실패하면 트랜잭션이 drop되며 rollback되므로 멱등 결과는 최종 성공 한 번만 저장된다.
#[allow(clippy::too_many_arguments)]
async fn apply_attempt(
    db: &mut Client,
    facts: &Value,
    key: &str,
    req: &Value,
    req_text: &str,
    principal: &str,
    caller: &Caller,
    wire: IdWire,
    serializable: bool,
    commit_sent: &std::sync::atomic::AtomicBool,
    started: std::time::Instant,
) -> Value {
    let s = sqlgen::schema();
    let tx = match db.transaction().await {
        Ok(t) => t,
        Err(e) if spike_v3_write::connection_failed(&e) => return lost_before_commit(),
        Err(_) => return json!({ "ok": false, "code": "WIRE.INTERNAL" }),
    };
    if let Err(error) = tx
        .batch_execute(&format!(
            "{}; SET LOCAL statement_timeout = '2000ms'; SET LOCAL TimeZone = 'UTC'",
            spike_v3_write::isolation_sql(serializable)
        ))
        .await
    {
        return if spike_v3_write::connection_failed(&error) { lost_before_commit() } else { err(classify_write_error("세션 설정", error)) };
    }
    // 같은 키가 이미 커밋됐으면 다시 실행하지 않고 저장된 결과를 준다. 같은 키에 다른 요청이면 거부.
    // 동시에 같은 키가 오면 키 행 잠금으로 하나씩 처리한다.
    if let Err(error) = tx.execute("SELECT pg_advisory_xact_lock(hashtext($1))", &[&format!("{principal}:{key}")]).await {
        return err(classify_write_error("멱등 잠금", error));
    }
    if let Ok(Some(row)) =
        tx.query_opt(format!("SELECT request, result::text FROM {s}.aip_idem WHERE principal = $1 AND key = $2").as_str(), &[&principal, &key]).await
    {
        let (stored_req, result): (String, String) = (row.get(0), row.get(1));
        if stored_req != req_text {
            return json!({ "ok": false, "code": "WIRE.IDEMPOTENCY_MISMATCH", "msg": "같은 키에 다른 요청" });
        }
        let Ok(mut v) = serde_json::from_str::<Value>(&result) else {
            return json!({ "ok": false, "code": "WIRE.INTERNAL", "msg": "저장된 멱등 결과를 읽을 수 없음" });
        };
        v["replayed"] = json!(true);
        return v;
    }
    let knobs = Knobs { audit_via: Some("apply"), audit_key: Some(key.to_string()), fresh_now_after_lock: true, ..Knobs::default() };
    // 전이 하나, 묶음(atomic), 조합(compose)은 같은 트랜잭션·같은 멱등 키·같은 커밋 시점 검사를 쓴다.
    let result = match run_write(&tx, facts, req, caller, &knobs, wire).await {
        Ok(result) => result,
        Err(e) => return err(spike_v3_write::before_commit_rejection(&tx, e)),
    };
    if let Err(e) = checks_in(&tx, facts, caller, &knobs).await {
        return err(spike_v3_write::before_commit_rejection(&tx, e));
    }
    if let Err(error) = tx
        .execute(
            format!("INSERT INTO {s}.aip_idem (principal, key, request, result) VALUES ($1, $2, $3, $4::text::jsonb)").as_str(),
            &[&principal, &key, &req_text, &result.to_string()],
        )
        .await
    {
        // SERIALIZABLE에서는 락을 기다린 뒤에도 옛 스냅샷으로 키를 조회하므로, 같은 키의 먼저 끝난 요청과 여기서 부딪친다.
        // 재시도하면 저장된 결과를 재생한다.
        return match error.code().map(|c| c.code()) {
            Some("23505") if serializable => err(spike_v3_write::serialization_conflict("멱등 기록")),
            Some("40001") | Some("40P01") => err(classify_write_error("멱등 기록", error)),
            _ if spike_v3_write::connection_failed(&error) => lost_before_commit(),
            _ => json!({ "ok": false, "code": "WIRE.INTERNAL" }),
        };
    }
    if started.elapsed().as_millis() + u128::from(spike_v3_write::COMMIT_MARGIN_MS) >= u128::from(spike_v3_write::WRITE_DEADLINE_MS) {
        return json!({"ok":false,"code":"WIRE.DEADLINE_EXCEEDED"});
    }
    commit_attempt(tx, commit_sent, result).await
}

/// COMMIT 전에 연결을 잃은 실패를 wire 응답으로 바꾼다. 분류 규칙은 `spike_v3_write`의 공통 함수가 정한다.
fn lost_before_commit() -> Value {
    json!({"ok":false,"code":spike_v3_write::lost_before_commit().code})
}

/// COMMIT 한 번을 보내고 결과를 분류한다.
#[doc(hidden)]
pub async fn commit_attempt(tx: tokio_postgres::Transaction<'_>, commit_sent: &std::sync::atomic::AtomicBool, result: Value) -> Value {
    use spike_v3_write::CommitFailure;
    match spike_v3_write::send_commit(tx, || commit_sent.store(true, std::sync::atomic::Ordering::SeqCst)).await {
        Ok(()) => result,
        Err(CommitFailure::NotSent) => lost_before_commit(),
        // COMMIT 응답 전에 PostgreSQL 연결이 닫히면 서버가 커밋했는지 이 연결로는 알 수 없다.
        Err(CommitFailure::Unknown) => json!({"ok":false,"code":"WIRE.COMMIT_UNKNOWN"}),
        Err(CommitFailure::Rejected(e)) => {
            // 직렬화 실패로 커밋이 거절되면 커밋되지 않았다. 재시도 구간에서 다시 보내기 전에 표지를 내린다.
            if matches!(e.code().map(|c| c.code()), Some("40001") | Some("40P01")) {
                commit_sent.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            err(classify_write_error("커밋", e))
        }
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
                Err(_) => json!({"ok":false,"code":"WIRE.DEADLINE_EXCEEDED"}),
            },
            Err(e) => err(e),
        },
        "/apply" => handle_apply(db, facts, body, caller, wire).await,
        "/status" => {
            let principal = principal_key(caller.actor_id, wire);
            let Some(key) = body["key"].as_str().filter(|k| valid_key(k)) else {
                return json!({ "ok": false, "code": "WIRE.BAD_REQUEST", "msg": "key 필요" });
            };
            let request_text = body["request"].to_string();
            let q = format!("SELECT request, result::text FROM {}.aip_idem WHERE principal = $1 AND key = $2", sqlgen::schema());
            match tokio::time::timeout(server::DB_PREFLIGHT_TIMEOUT, db.query_opt(q.as_str(), &[&principal, &key])).await {
                // 키만으로 결과를 주지 않는다. 같은 요청인지 확인한다(r7 R7-01).
                Ok(Ok(Some(r))) if r.get::<_, String>(0) != request_text => {
                    json!({ "ok": false, "code": "WIRE.IDEMPOTENCY_MISMATCH", "msg": "같은 키에 다른 요청" })
                }
                Ok(Ok(Some(r))) => {
                    let Ok(mut v) = serde_json::from_str::<Value>(&r.get::<_, String>(1)) else {
                        return json!({ "ok": false, "code": "WIRE.INTERNAL", "msg": "저장된 멱등 결과를 읽을 수 없음" });
                    };
                    v["replayed"] = json!(true);
                    v
                }
                // 조회 시점에 커밋된 결과가 없다는 뜻일 뿐, 진행 중일 수 있다(r7 R7-02). 확정은 같은 키 재요청으로 한다.
                Ok(Ok(None)) => json!({ "ok": false, "code": "WIRE.NOT_FOUND" }),
                Ok(Err(_)) | Err(_) => json!({ "ok": false, "code": "WIRE.DB_UNAVAILABLE" }),
            }
        }
        _ => json!({ "ok": false, "code": "WIRE.NOT_FOUND" }),
    }
}

pub const MAX_BODY: usize = 1 << 20;
pub const MAX_HEADER_LINE: usize = 8 << 10;
pub const MAX_HEADERS: usize = 16 << 10;
pub const MAX_TOKEN: usize = 4 << 10;

async fn header_line(rd: &mut (impl tokio::io::AsyncBufRead + Unpin), line: &mut String, deadline: tokio::time::Instant) -> Result<(), &'static str> {
    let read = tokio::time::timeout_at(deadline, (&mut *rd).take((MAX_HEADER_LINE + 1) as u64).read_line(line)).await;
    match read {
        Err(_) => Err("WIRE.REQUEST_TIMEOUT"),
        Ok(Ok(n)) if n > 0 && n <= MAX_HEADER_LINE && line.ends_with("\r\n") => Ok(()),
        _ => Err("WIRE.BAD_REQUEST"),
    }
}

fn v_length_over_limit(header: &str) -> bool {
    header.split_once(':').and_then(|(_, value)| value.trim().strip_prefix("Bearer ")).is_some_and(|token| token.len() > MAX_TOKEN)
}

struct Response<W> {
    writer: W,
    origin: Option<String>,
    private_object: bool,
    head_only: bool,
    deadline: tokio::time::Instant,
    /// Request id and the line logged when the request ends (dropped with the response).
    trace: trace::Trace,
}

async fn reply(w: &mut Response<impl AsyncWriteExt + Unpin>, out: &str) {
    if w.private_object {
        reply_object(w, out).await;
    } else {
        reply_with_headers(w, out, "vary: Origin\r\n").await;
    }
}

async fn reply_object(w: &mut Response<impl AsyncWriteExt + Unpin>, out: &str) {
    reply_with_headers(w, out, "cache-control: private, no-store\r\nx-content-type-options: nosniff\r\nvary: Origin\r\n").await;
}

async fn reply_with_headers(w: &mut Response<impl AsyncWriteExt + Unpin>, out: &str, extra: &str) {
    reply_with_status(w, "200 OK", out, extra).await;
}

/// Prometheus scrapes read text, not the JSON envelope; the body never carries request data.
async fn reply_metrics(w: &mut Response<impl AsyncWriteExt + Unpin>, body: String) {
    w.trace.record("200 OK", "");
    let headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain; version=0.0.4\r\ncontent-length: {}\r\nconnection: close\r\ncache-control: no-store\r\nx-request-id: {}\r\n\r\n",
        body.len(),
        w.trace.id()
    );
    let head_only = w.head_only;
    let write = async {
        w.writer.write_all(headers.as_bytes()).await?;
        if !head_only {
            w.writer.write_all(body.as_bytes()).await?;
        }
        w.writer.flush().await
    };
    let _ = tokio::time::timeout_at(w.deadline, write).await;
}

async fn reply_with_status(w: &mut Response<impl AsyncWriteExt + Unpin>, status: &str, out: &str, extra: &str) {
    let out = match serde_json::from_str::<serde_json::Value>(out) {
        Ok(serde_json::Value::Object(mut body)) if body.get("ok").and_then(serde_json::Value::as_bool) == Some(false) => {
            let retry = body.get("code").and_then(serde_json::Value::as_str).and_then(wire_codes::lookup).map_or("never", |info| info.retry.name());
            body.insert("retry".into(), retry.into());
            body.insert("requestId".into(), w.trace.id().into());
            std::borrow::Cow::Owned(serde_json::Value::Object(body).to_string())
        }
        _ => std::borrow::Cow::Borrowed(out),
    };
    w.trace.record(status, &out);
    // A browser reads x-request-id only when the reply exposes it.
    let origin = w
        .origin
        .as_ref()
        .map(|origin| format!("access-control-allow-origin: {origin}\r\naccess-control-expose-headers: x-request-id\r\n"))
        .unwrap_or_default();
    let request_id = w.trace.id();
    let headers = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\nx-request-id: {request_id}\r\n{origin}{extra}\r\n",
        out.len()
    );
    let deadline = w.deadline;
    let private_object = w.private_object;
    if private_object && tokio::time::Instant::now() >= deadline {
        return;
    }
    let head_only = w.head_only;
    let write = async {
        w.writer.write_all(headers.as_bytes()).await?;
        if !head_only {
            w.writer.write_all(out.as_bytes()).await?;
        }
        w.writer.flush().await
    };
    // Private data stays within its ingress deadline; other routes retain their send window.
    if private_object {
        let _ = tokio::time::timeout_at(deadline, write).await;
    } else {
        let _ = tokio::time::timeout(server::REQUEST_TIMEOUT, write).await;
    }
}

async fn enforce_admission(
    w: &mut Response<impl AsyncWriteExt + Unpin>,
    runtime: &server::Runtime,
    session: Option<&auth::Session>,
    route: admission::Route,
    deadline: tokio::time::Instant,
) -> bool {
    let Some(admission) = runtime.admission.as_ref() else { return false };
    let Some(pool) = runtime.db_pool.as_ref() else {
        reply_with_status(
            w,
            "503 Service Unavailable",
            &json!({"ok":false,"code":"WIRE.DB_UNAVAILABLE","msg":"rate limit 저장소를 사용할 수 없음"}).to_string(),
            "cache-control: no-store\r\nvary: Origin\r\n",
        )
        .await;
        return true;
    };
    match admission.check(pool, &admission::principal(session), route, deadline).await {
        Ok(admission::Outcome::Allowed) => false,
        Ok(admission::Outcome::Limited { retry_after_ms }) => {
            let retry_after_secs = retry_after_ms.saturating_add(999).saturating_div(1000).max(1);
            let extra = if w.private_object {
                format!("cache-control: private, no-store\r\nx-content-type-options: nosniff\r\nvary: Origin\r\nRetry-After: {retry_after_secs}\r\n")
            } else {
                format!("vary: Origin\r\nRetry-After: {retry_after_secs}\r\n")
            };
            reply_with_status(w, "429 Too Many Requests", &json!({"ok":false,"code":"WIRE.RATE_LIMITED","msg":"요청 한도 초과"}).to_string(), &extra)
                .await;
            true
        }
        Err(_) => {
            let extra = if w.private_object {
                "cache-control: private, no-store\r\nx-content-type-options: nosniff\r\nvary: Origin\r\n"
            } else {
                "cache-control: no-store\r\nvary: Origin\r\n"
            };
            reply_with_status(
                w,
                "503 Service Unavailable",
                &json!({"ok":false,"code":"WIRE.DB_UNAVAILABLE","msg":"rate limit 저장소를 사용할 수 없음"}).to_string(),
                extra,
            )
            .await;
            true
        }
    }
}

async fn reply_managed_file<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    method: &str,
    id: &str,
    media_type: &str,
    bytes: &[u8],
    origin: Option<&str>,
    deadline: tokio::time::Instant,
) -> std::io::Result<()> {
    if !valid_managed_media_type(media_type) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid stored media type"));
    }
    let extra = cors::actual_response_headers(origin);
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {media_type}\r\ncontent-length: {}\r\ncontent-disposition: attachment; filename=\"{id}\"\r\ncache-control: private, no-store\r\nx-content-type-options: nosniff\r\n{extra}connection: close\r\n\r\n",
        bytes.len()
    );
    tokio::time::timeout_at(deadline, async {
        writer.write_all(head.as_bytes()).await?;
        if method != "HEAD" {
            writer.write_all(bytes).await?;
        }
        writer.flush().await
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "managed file response deadline exceeded"))?
}

async fn verify_session(
    token: Option<&str>,
    keys: &auth::Keyring,
    runtime: &server::Runtime,
    deadline: tokio::time::Instant,
) -> Result<Option<auth::Session>, auth::AuthError> {
    let Some(token) = token else { return Ok(None) };
    let verification = match &runtime.authenticator {
        None => keys.verify_session(token),
        Some(provider) => match tokio::time::timeout_at(deadline, provider.verify_session(token)).await {
            Ok(result) => result,
            Err(_) => Err(auth::AuthError::Unavailable),
        },
    };
    verification.map(Some)
}

/// Inbound webhook: authenticate the provider by signature over the exact bytes, then accept a durable job
/// as the configured service principal. Status codes follow provider retry conventions (2xx accepted,
/// 4xx permanent, 503 retry later) instead of the JSON-only envelope used by AIP clients.
#[allow(clippy::too_many_arguments)]
async fn serve_webhook<R: tokio::io::AsyncRead + Unpin>(
    w: &mut Response<impl AsyncWriteExt + Unpin>,
    rd: &mut R,
    webhook: &triggers::Webhook,
    method: &str,
    len: Option<usize>,
    headers: &[(String, String)],
    facts: &Value,
    wire: IdWire,
    runtime: &server::Runtime,
    deadline: tokio::time::Instant,
) {
    let fail = |code: &str| json!({"ok":false,"code":code}).to_string();
    if method != "POST" {
        return reply_with_status(w, "405 Method Not Allowed", &fail("WIRE.BAD_REQUEST"), "allow: POST\r\n").await;
    }
    let Some(len) = len else { return reply_with_status(w, "411 Length Required", &fail("WIRE.BAD_REQUEST"), "").await };
    if len > webhook.max_body_bytes {
        return reply_with_status(w, "413 Payload Too Large", &fail("WIRE.PAYLOAD_TOO_LARGE"), "").await;
    }
    let mut body = vec![0; len];
    match tokio::time::timeout_at(deadline, rd.read_exact(&mut body)).await {
        Err(_) => return reply_with_status(w, "408 Request Timeout", &fail("WIRE.REQUEST_TIMEOUT"), "").await,
        Ok(Err(_)) => return,
        Ok(Ok(_)) => {}
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let delivery = match triggers::verify(webhook, headers, &body, now) {
        Ok(delivery) => delivery,
        Err(triggers::Rejection::Stale) => return reply_with_status(w, "401 Unauthorized", &fail("WIRE.SIGNATURE_EXPIRED"), "").await,
        Err(triggers::Rejection::Signature) => return reply_with_status(w, "401 Unauthorized", &fail("WIRE.SIGNATURE_INVALID"), "").await,
    };
    let Ok(text) = String::from_utf8(body) else { return reply_with_status(w, "400 Bad Request", &fail("WIRE.BAD_VALUE"), "").await };
    let key = triggers::webhook_key(&webhook.name, &delivery);
    match durable::submit_as_service(runtime, facts, wire, &webhook.principal, &webhook.operation, &json!({"body":text}), &key, None).await {
        Ok(run) => reply_with_status(w, "202 Accepted", &json!({"ok":true,"id":run.id,"state":run.state}).to_string(), "").await,
        Err("WIRE.IDEMPOTENCY_MISMATCH") => reply_with_status(w, "409 Conflict", &fail("WIRE.IDEMPOTENCY_MISMATCH"), "").await,
        Err("WIRE.BAD_VALUE") => reply_with_status(w, "400 Bad Request", &fail("WIRE.BAD_VALUE"), "").await,
        // The delivery fits the body limit but not the job input limit once JSON escaping is applied; retrying cannot help.
        Err("WIRE.PAYLOAD_TOO_LARGE") => reply_with_status(w, "413 Payload Too Large", &fail("WIRE.PAYLOAD_TOO_LARGE"), "").await,
        Err("WIRE.QUOTA_EXCEEDED") => reply_with_status(w, "429 Too Many Requests", &fail("WIRE.QUOTA_EXCEEDED"), "retry-after: 60\r\n").await,
        Err(code) => reply_with_status(w, "503 Service Unavailable", &fail(code), "retry-after: 30\r\n").await,
    }
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
    serve_request(&mut sock, facts, keys, contract_fingerprint, wire, runtime, deadline).await;
    // Closing with unread request bytes resets TCP on macOS/Linux and can erase the
    // error reply at a reverse proxy. Send FIN first, then discard a bounded tail.
    // The peer sees the reply immediately, even when it never sends its body.
    let _ = sock.shutdown().await;
    let mut tail = (&mut sock).take((MAX_BODY * 2) as u64);
    let mut discard = tokio::io::sink();
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), tokio::io::copy(&mut tail, &mut discard)).await;
}

async fn serve_request(
    sock: &mut TcpStream,
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
    let mut w = Response { writer: w, origin: None, private_object: false, head_only: false, deadline, trace: trace::Trace::default() };
    w.trace.set_otlp_exporter(runtime.otlp.clone());
    let mut rd = BufReader::new(r);
    let mut line = String::new();
    if let Err(code) = header_line(&mut rd, &mut line, deadline).await {
        return reply(&mut w, &json!({ "ok": false, "code": code, "msg": "요청 줄 상한 또는 형식 오류" }).to_string()).await;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    w.head_only = method == "HEAD";
    w.trace.begin(&method, &path);
    w.private_object =
        path.starts_with("/objects/") || path.starts_with("/rag/") || runtime.infrastructure.file_mounts.iter().any(|mount| mount.matches(&path));
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        return reply(&mut w, &json!({"ok":false,"code":"WIRE.BAD_REQUEST"}).to_string()).await;
    }
    let (mut origin, mut host, mut request_method, mut request_headers) = (None::<String>, None::<String>, None::<String>, None::<String>);
    let (mut accept, mut content_type, mut upgrade, mut connection, mut ws_key, mut ws_version, mut ws_extensions) =
        (None::<String>, None::<String>, None::<String>, None::<String>, None::<String>, None::<String>, None::<String>);
    let (mut len, mut token, mut drop) = (None::<usize>, None::<String>, false);
    let mut header_values = Vec::<(String, String)>::new();
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
            return reply(&mut w, &json!({ "ok": false, "code": "WIRE.BAD_REQUEST", "msg": "헤더 총량 상한" }).to_string()).await;
        }
        if h == "\r\n" {
            // Outbound calls made while this request is handled carry its trace id.
            aip_execution::trace_context::set(w.trace.context().clone());
            break;
        }
        if !h.contains(':') || h.starts_with([' ', '\t']) || h.trim_end_matches("\r\n").bytes().any(|b| b.is_ascii_control() && b != b'\t') {
            bad = Some("잘못된 헤더 형식");
        }
        let (k, v) = h.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).unwrap_or_default();
        header_values.push((k.clone(), v.clone()));
        // RFC 9112 §5.1: whitespace between the field name and the colon makes the message invalid. A proxy that reads
        // `Content-Length : 5` differently from this parser is a request-smuggling primitive.
        if h.split_once(':').is_some_and(|(raw, _)| raw.bytes().any(|b| b == b' ' || b == b'\t')) {
            bad = Some("헤더 이름과 콜론 사이 공백");
        }
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
            "accept" | "content-type" | "upgrade" | "connection" | "sec-websocket-key" | "sec-websocket-version" | "sec-websocket-extensions" => {
                let target = match k.as_str() {
                    "accept" => &mut accept,
                    "content-type" => &mut content_type,
                    "upgrade" => &mut upgrade,
                    "connection" => &mut connection,
                    "sec-websocket-key" => &mut ws_key,
                    "sec-websocket-version" => &mut ws_version,
                    _ => &mut ws_extensions,
                };
                if target.is_some() {
                    bad = Some("중복된 실시간 전송 헤더");
                } else {
                    *target = Some(v);
                }
            }
            "x-aip-contract" => {
                if expected_contract.is_some() || v.len() != 64 || !v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                    bad = Some("잘못되거나 중복된 계약 식별값");
                } else {
                    expected_contract = Some(v);
                }
            }
            "x-request-id" => w.trace.accept_inbound(&v),
            "traceparent" => w.trace.accept_traceparent(&v),
            "x-spike-drop-response" => drop = runtime.fault_injection && v == "1",
            _ => {}
        }
        if k == "authorization" && v_length_over_limit(&h) {
            if let Some(origin) =
                origin.as_deref().filter(|origin| !invalid_browser_headers && runtime.options.allows(origin, host.as_deref(), local))
            {
                w.origin = Some(origin.to_string());
            }
            return reply(&mut w, &json!({ "ok": false, "code": "WIRE.BAD_REQUEST", "msg": bad }).to_string()).await;
        }
    }
    let err = |code: &str, msg: &str| json!({ "ok": false, "code": code, "msg": msg }).to_string();
    if let Some(origin) = origin.as_deref() {
        if invalid_browser_headers || !runtime.options.allows(origin, host.as_deref(), local) {
            return reply(&mut w, &err("WIRE.ORIGIN_NOT_ALLOWED", "허용하지 않은 로컬 출처")).await;
        }
        w.origin = Some(origin.to_string());
    }
    if let Some(m) = bad {
        return reply(&mut w, &err("WIRE.BAD_REQUEST", m)).await;
    }
    if health::is_probe(&path) {
        if !matches!(method.as_str(), "GET" | "HEAD") || len.is_some_and(|length| length != 0) {
            return reply_with_status(&mut w, "405 Method Not Allowed", &err("WIRE.BAD_REQUEST", "GET/HEAD만"), "allow: GET, HEAD\r\n").await;
        }
        let (status, body) = health::probe(&path, &runtime).await;
        return reply_with_status(&mut w, status, &body, "cache-control: no-store\r\n").await;
    }
    if let Some(token) = runtime.infrastructure.metrics_token.as_deref().filter(|_| path == "/metrics") {
        if !matches!(method.as_str(), "GET" | "HEAD") || len.is_some_and(|length| length != 0) {
            return reply_with_status(&mut w, "405 Method Not Allowed", &err("WIRE.BAD_REQUEST", "GET/HEAD만"), "allow: GET, HEAD\r\n").await;
        }
        let authorization = header_values.iter().find(|(key, _)| key == "authorization").map(|(_, value)| value.as_str());
        if !metrics::token_matches(token, authorization) {
            return reply_with_status(&mut w, "401 Unauthorized", &err("WIRE.UNAUTHENTICATED", "metrics token 필요"), "www-authenticate: Bearer\r\n")
                .await;
        }
        return reply_metrics(&mut w, metrics::render()).await;
    }
    if let Some(webhook) = runtime.infrastructure.triggers.webhooks.iter().find(|hook| hook.path == path) {
        return serve_webhook(&mut w, &mut rd, webhook, &method, len, &header_values, &facts, wire, &runtime, deadline).await;
    }
    let mount = runtime.infrastructure.mounts.iter().find(|mount| mount.matches(&path));
    let file_mount = runtime.infrastructure.file_mounts.iter().find(|mount| mount.matches(&path));
    if file_mount.is_some() {
        w.private_object = true;
    }
    let object_path = path.starts_with("/objects/");
    let messages_path = path.starts_with("/messages/");
    if ![
        "/session",
        "/read",
        "/apply",
        "/status",
        "/extension",
        "/operation",
        "/capabilities",
        "/jobs/enqueue",
        "/jobs/status",
        "/jobs/result",
        "/jobs/cancel",
        "/files/put",
        "/files/get",
        "/files/delete",
        "/events/read",
        "/events/subscribe",
        "/events/socket",
        "/rag/search",
        "/rag/answer",
        "/rag/index",
        "/rag/delete",
    ]
    .contains(&path.as_str())
        && mount.is_none()
        && file_mount.is_none()
        && !object_path
        && !messages_path
    {
        return reply(&mut w, &err("WIRE.NOT_FOUND", "알 수 없는 경로")).await;
    }
    if method == "OPTIONS" {
        let Some(requested_method) = request_method.as_deref() else {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "잘못된 preflight")).await;
        };
        let Some(methods) = cors::allowed_methods(&path, mount.is_some() || file_mount.is_some())
            .filter(|methods| cors::method_is_allowed(methods, requested_method))
        else {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "이 경로에서 허용하지 않는 preflight method")).await;
        };
        if w.origin.is_none() || !cors::allowed_request_headers(request_headers.as_deref()) || len.is_some_and(|len| len != 0) {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "잘못된 preflight")).await;
        }
        let extra = format!(
            "vary: Origin, Access-Control-Request-Method, Access-Control-Request-Headers\r\naccess-control-allow-methods: {methods}\r\naccess-control-allow-headers: authorization, content-type, x-aip-contract\r\n"
        );
        return reply_with_headers(&mut w, &json!({"ok":true}).to_string(), &extra).await;
    }
    if let Some(mount) = mount {
        let session = match verify_session(token.as_deref(), &keys, &runtime, deadline).await {
            Ok(session) => session,
            Err(auth::AuthError::Expired) => return reply(&mut w, &err("WIRE.TOKEN_EXPIRED", "세션 만료")).await,
            Err(auth::AuthError::Unavailable) => return reply(&mut w, &err("WIRE.DB_UNAVAILABLE", "인증 제공자 또는 사용자 연결 조회 실패")).await,
            Err(_) => return reply(&mut w, &err("WIRE.UNAUTHENTICATED", "토큰 검증 실패")).await,
        };
        if mount.access == hosting::MountAccess::Authenticated && session.is_none() {
            return reply(&mut w, &err("WIRE.UNAUTHENTICATED", "인증된 정적 mount입니다")).await;
        }
        if enforce_admission(&mut w, &runtime, session.as_ref(), admission::Route::Static, deadline).await {
            return;
        }
        if len.is_some_and(|length| length != 0) {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "정적 파일 요청 본문은 비어 있어야 함")).await;
        }
        let headers: Vec<(&str, &str)> = header_values.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
        let mut tap = trace::HeadTap::new(&mut w.writer, &mut w.trace);
        let _ = hosting::serve_with_origin(&mut tap, mount, &method, &path, &headers, w.origin.as_deref()).await;
        return;
    }
    if object_path && path != "/objects/sign" && !path.starts_with("/objects/multipart/") {
        if expected_contract.as_deref() != contract_fingerprint.as_deref() || expected_contract.is_none() {
            return reply_object(&mut w, &err("WIRE.CONTRACT_MISMATCH", "object API 계약 지문 필요")).await;
        }
        let session = match verify_session(token.as_deref(), &keys, &runtime, deadline).await {
            Ok(session) => session,
            Err(auth::AuthError::Expired) => return reply_object(&mut w, &err("WIRE.TOKEN_EXPIRED", "세션 만료")).await,
            Err(auth::AuthError::Unavailable) => {
                return reply_object(&mut w, &err("WIRE.DB_UNAVAILABLE", "인증 제공자 또는 사용자 연결 조회 실패")).await
            }
            Err(_) => return reply_object(&mut w, &err("WIRE.UNAUTHENTICATED", "토큰 검증 실패")).await,
        };
        if enforce_admission(&mut w, &runtime, session.as_ref(), admission::Route::Objects, deadline).await {
            return;
        }
        match run_object_transfer(
            runtime.infrastructure.object_transfer_lifetime,
            object_transport::serve(
                &mut rd,
                &mut trace::HeadTap::new(&mut w.writer, &mut w.trace),
                &method,
                &path,
                &header_values,
                len,
                session.as_ref(),
                &runtime,
                &facts,
                wire,
                w.origin.as_deref(),
            ),
        )
        .await
        {
            Ok(true) | Err(_) => return,
            Ok(false) => {}
        }
    }
    let websocket = path == "/events/socket";
    let sse = path == "/events/subscribe";
    let mut raw_body = Vec::new();
    let body = if websocket {
        let valid_connection = connection.as_deref().is_some_and(|value| value.split(',').any(|token| token.trim().eq_ignore_ascii_case("upgrade")));
        let key_valid = ws_key.as_deref().is_some_and(|key| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(key)
                .is_ok_and(|decoded| decoded.len() == 16 && base64::engine::general_purpose::STANDARD.encode(decoded) == key)
        });
        if method != "GET"
            || len.is_some_and(|length| length != 0)
            || upgrade.as_deref().is_none_or(|value| !value.eq_ignore_ascii_case("websocket"))
            || !valid_connection
            || !key_valid
            || ws_version.as_deref() != Some("13")
            || ws_extensions.is_some()
            || origin.is_none()
            || host.is_none()
        {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "잘못된 WebSocket handshake")).await;
        }
        json!({})
    } else if file_mount.is_some() {
        if !matches!(method.as_str(), "GET" | "HEAD") || len.is_some_and(|length| length != 0) {
            return reply_object(&mut w, &err("WIRE.BAD_REQUEST", "관리 파일 mount는 본문 없는 GET/HEAD만 지원함").to_string()).await;
        }
        json!({})
    } else {
        if method != "POST" {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "POST만")).await;
        }
        if sse && accept.as_deref() != Some("text/event-stream") {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "Accept: text/event-stream 필요")).await;
        }
        if sse
            && content_type
                .as_deref()
                .is_none_or(|value| !value.split(';').next().is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json")))
        {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "Content-Type: application/json 필요")).await;
        }
        let Some(len) = len else { return reply(&mut w, &err("WIRE.BAD_REQUEST", "content-length 필요")).await };
        // 상한을 넘는 본문은 읽기 전에 거부한다. 잘라서 처리하지 않는다(r7 R7-04).
        if len > MAX_BODY {
            return reply(&mut w, &err("WIRE.PAYLOAD_TOO_LARGE", "본문 1MiB 초과")).await;
        }
        raw_body.resize(len, 0);
        match tokio::time::timeout_at(deadline, rd.read_exact(&mut raw_body)).await {
            Err(_) => return reply(&mut w, &err("WIRE.REQUEST_TIMEOUT", "요청 본문 수신 기한 초과")).await,
            Ok(Err(_)) => return,
            Ok(Ok(_)) => {}
        }
        let Ok(body) = serde_json::from_slice::<Value>(&raw_body) else {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "JSON 형식 오류")).await;
        };
        body
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
        Some(Err(auth::AuthError::Expired)) => return reply(&mut w, &err("WIRE.TOKEN_EXPIRED", "세션 만료")).await,
        Some(Err(auth::AuthError::Unavailable)) => return reply(&mut w, &err("WIRE.DB_UNAVAILABLE", "인증 제공자 또는 사용자 연결 조회 실패")).await,
        Some(Err(_)) => return reply(&mut w, &err("WIRE.UNAUTHENTICATED", "토큰 검증 실패")).await,
    };
    if !(websocket && token.is_none()) {
        let route = if file_mount.is_some() { admission::Route::ManagedFiles } else { admission::Route::from_request(&path, &method) };
        if enforce_admission(&mut w, &runtime, session.as_ref(), route, deadline).await {
            return;
        }
    }
    if path == "/objects/sign" || path.starts_with("/objects/multipart/") {
        if expected_contract.as_deref() != contract_fingerprint.as_deref() || expected_contract.is_none() {
            return reply_object(&mut w, &err("WIRE.CONTRACT_MISMATCH", "object API 계약 지문 필요")).await;
        }
        let result = object_transport::control(&runtime, &facts, wire, session.as_ref(), &body, &path).await;
        return reply_object(&mut w, &result.to_string()).await;
    }
    // 기대 계약은 권한을 부여하지 않는다. 불일치면 DB·멱등 재생·쓰기 실행 전에 거부한다.
    let write_extension = path == "/apply" && body["request"].get("extension").is_some();
    let rag_path = path.starts_with("/rag/");
    if rag_path && (expected_contract.is_none() || expected_contract.as_deref() != contract_fingerprint.as_deref()) {
        return reply(&mut w, &err("WIRE.CONTRACT_MISMATCH", "RAG API 계약 지문 필요")).await;
    }
    if rag_path && method != "POST" {
        return reply(&mut w, &err("WIRE.BAD_REQUEST", "RAG API는 POST만 허용함")).await;
    }
    if ["/read", "/apply", "/status"].contains(&path.as_str()) && !write_extension {
        // O-03: 다른 경로와 같이 헤더 없는 요청도 거부한다. 지문을 모르는 raw 클라이언트는 /capabilities에서 얻는다.
        // 계약이 없는 서버(resources 없는 facts)는 서비스가 만들 수 없는 상태라 기존 동작을 유지한다.
        match expected_contract.as_deref() {
            None if contract_fingerprint.is_some() => {
                return reply(&mut w, &err("WIRE.CONTRACT_REQUIRED", "x-aip-contract 헤더 필요(계약 지문은 /capabilities에서 얻음)")).await;
            }
            Some(expected) if contract_fingerprint.as_deref() != Some(expected) => {
                return reply(&mut w, &err("WIRE.CONTRACT_MISMATCH", "생성 계약과 서버 계약이 다름")).await;
            }
            _ => {}
        }
    }
    if write_extension {
        if !runtime.write_extensions || runtime.extensions.is_none() {
            return reply(&mut w, &err("WIRE.NOT_FOUND", "공식 WRITE 확장은 명시적 서버 설정이 필요함")).await;
        }
        if session.is_none() {
            return reply(&mut w, &err("WIRE.UNAUTHENTICATED", "공식 WRITE 확장은 서명 세션이 필요함")).await;
        }
        if expected_contract.is_none() || expected_contract.as_deref() != contract_fingerprint.as_deref() {
            return reply(&mut w, &err("WIRE.CONTRACT_MISMATCH", "생성 WRITE 계약 지문이 필요함")).await;
        }
        let request = &body["request"];
        let exact = body.as_object().is_some_and(|body| body.len() == 2 && body.contains_key("key") && body.contains_key("request"))
            && request.as_object().is_some_and(|request| request.len() == 2 && request.contains_key("extension") && request.contains_key("input"))
            && body["key"].as_str().is_some_and(valid_key);
        let Some(name) = request["extension"].as_str().filter(|_| exact) else {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "WRITE 요청은 key/request와 extension/input만 허용함")).await;
        };
        if let Err(error) = spike_v4_worker::write::validate_write(&facts, name, &request["input"], wire) {
            return reply(&mut w, &err(error.code, &error.msg)).await;
        }
    }
    if path == "/extension" {
        if runtime.extensions.is_none() {
            return reply(&mut w, &err("WIRE.NOT_FOUND", "공식 확장은 명시적 서버 설정이 필요함")).await;
        }
        if session.is_none() {
            return reply(&mut w, &err("WIRE.UNAUTHENTICATED", "공식 확장은 서명 세션이 필요함")).await;
        }
        if expected_contract.is_none() || expected_contract.as_deref() != contract_fingerprint.as_deref() {
            return reply(&mut w, &err("WIRE.CONTRACT_MISMATCH", "생성 확장 계약 지문이 필요함")).await;
        }
        let valid_body = body.as_object().is_some_and(|o| o.len() == 2 && o.contains_key("extension") && o.contains_key("input"));
        let Some(name) = body["extension"].as_str().filter(|_| valid_body) else {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "확장 본문은 extension/input만 허용함")).await;
        };
        if let Err(e) = spike_v4_worker::validate_read(&facts, name, &body["input"], wire) {
            return reply(&mut w, &err(e.code, &e.msg)).await;
        }
    }
    if path == "/operation" {
        if runtime.extensions.is_none() {
            return reply(&mut w, &err("WIRE.NOT_FOUND", "operation은 서버 worker 설정이 필요함")).await;
        }
        if session.is_none() {
            return reply(&mut w, &err("WIRE.UNAUTHENTICATED", "operation은 검증된 세션이 필요함")).await;
        }
        if expected_contract.is_none() || expected_contract.as_deref() != contract_fingerprint.as_deref() {
            return reply(&mut w, &err("WIRE.CONTRACT_MISMATCH", "생성 operation 계약 지문이 필요함")).await;
        }
        let exact = body.as_object().is_some_and(|body| body.len() == 2 && body.contains_key("operation") && body.contains_key("input"));
        let Some(name) = body["operation"].as_str().filter(|_| exact) else {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "operation 본문은 operation/input만 허용함")).await;
        };
        if let Err(error) = spike_v4_worker::validate_operation(&facts, name, &body["input"], wire) {
            return reply(&mut w, &err(error.code, &error.msg)).await;
        }
    }
    if path == "/capabilities" {
        if session.is_none() {
            return reply(&mut w, &err("WIRE.UNAUTHENTICATED", "capability 탐색은 검증된 세션이 필요함")).await;
        }
        if !body.as_object().is_some_and(|body| body.is_empty()) {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "capabilities 본문은 빈 객체")).await;
        }
    }
    let browser_ws = websocket && token.is_none();
    let managed = path.starts_with("/jobs/")
        || path.starts_with("/files/")
        || file_mount.is_some()
        || rag_path
        || path == "/events/read"
        || messages_path
        || sse
        || websocket;
    if managed {
        if !browser_ws && session.as_ref().and_then(|s| s.durable.as_ref()).is_none() {
            return reply(&mut w, &err("WIRE.UNAUTHENTICATED", "관리 실행은 제품 principal 세션이 필요함")).await;
        }
        if !sse && !websocket && (expected_contract.is_none() || expected_contract.as_deref() != contract_fingerprint.as_deref()) {
            return reply(&mut w, &err("WIRE.CONTRACT_MISMATCH", "관리 실행 계약 지문 필요")).await;
        }
    }
    if rag_path {
        let Ok(_slot) = runtime.extension_slots.try_acquire() else {
            return reply(&mut w, &err("WIRE.SERVER_BUSY", "RAG 동시 실행 상한")).await;
        };
        let Some(engine) = runtime.infrastructure.rag.as_ref() else {
            return reply(&mut w, &err("WIRE.NOT_AVAILABLE", "RAG 서비스가 비활성화됨")).await;
        };
        let Some(session) = session.as_ref() else {
            return reply(&mut w, &err("WIRE.UNAUTHENTICATED", "RAG에는 제품 principal 세션이 필요함")).await;
        };
        let Some(_principal_slot) = runtime.principal_slots.workers.try_acquire(&admission::principal(Some(session))) else {
            return reply(&mut w, &err("WIRE.SERVER_BUSY", "principal별 RAG 동시 실행 상한")).await;
        };
        let owner = match tokio::time::timeout_at(deadline, durable::validate_principal(&runtime, &facts, wire, session)).await {
            Ok(Ok(owner)) => owner,
            Ok(Err("WIRE.UNAUTHENTICATED" | "WIRE.TOKEN_EXPIRED")) => {
                return reply(&mut w, &err("WIRE.UNAUTHENTICATED", "RAG 제품 principal 검증 실패")).await
            }
            Ok(Err(_)) | Err(_) => return reply(&mut w, &err("WIRE.DB_UNAVAILABLE", "RAG 제품 principal 검증 실패")).await,
        };
        let fingerprint = contract_fingerprint.as_deref().unwrap_or_default();
        let result = match tokio::time::timeout_at(
            deadline,
            rag_transport::handle(engine, &owner, &path, &body, runtime.infrastructure.rag_client_index, fingerprint),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => json!({"ok":false,"code":"WIRE.REQUEST_TIMEOUT"}),
        };
        if result["ok"] == true {
            match tokio::time::timeout_at(deadline, durable::validate_principal(&runtime, &facts, wire, session)).await {
                Ok(Ok(current_owner)) if current_owner == owner => {}
                Ok(Err("WIRE.UNAUTHENTICATED" | "WIRE.TOKEN_EXPIRED")) => {
                    return reply(&mut w, &err("WIRE.UNAUTHENTICATED", "RAG 처리 중 신원 폐기 또는 만료")).await
                }
                _ => return reply(&mut w, &err("WIRE.DB_UNAVAILABLE", "RAG 반환 전 신원 검증 실패")).await,
            }
        }
        return reply(&mut w, &result.to_string()).await;
    }
    if sse || websocket {
        if !runtime.infrastructure.realtime.enabled {
            return reply(&mut w, &err("WIRE.NOT_AVAILABLE", "실시간 전송이 비활성화됨")).await;
        }
        let Some(fingerprint) = contract_fingerprint.as_deref() else {
            return reply(&mut w, &err("WIRE.NOT_AVAILABLE", "이벤트 계약 지문 없음")).await;
        };
        let subscription = if sse {
            let subscription = match realtime::Subscription::parse(&raw_body) {
                Ok(subscription) => subscription,
                Err(code) => return reply(&mut w, &err(code, "잘못된 이벤트 구독 본문")).await,
            };
            if subscription.contract_fingerprint != fingerprint {
                return reply(&mut w, &err("WIRE.CONTRACT_MISMATCH", "이벤트 구독 계약 지문이 다름")).await;
            }
            Some(subscription)
        } else {
            None
        };
        // 토큰 없는 브라우저 WebSocket은 첫 프레임 인증 전까지 pre-auth 슬롯만 쓴다. 인증 뒤에 `stream_ws`가 정식 슬롯을 잡는다.
        let browser_slots = if websocket && token.is_none() { realtime::BrowserSlots::acquire_preauth(&runtime) } else { None };
        let _slot = if websocket && token.is_none() { None } else { runtime.realtime_slots.try_acquire().ok() };
        if browser_slots.is_none() && _slot.is_none() {
            return reply(&mut w, &err("WIRE.SERVER_BUSY", "실시간 연결 상한")).await;
        }
        // A token-bearing connection holds its principal's share for as long as the stream runs. Browser sockets take theirs
        // when the auth frame identifies the principal.
        let _principal_slot = if browser_slots.is_none() {
            match runtime.principal_slots.realtime.try_acquire(&admission::principal(session.as_ref())) {
                Some(permit) => Some(permit),
                None => return reply(&mut w, &err("WIRE.SERVER_BUSY", "principal별 실시간 연결 상한")).await,
            }
        } else {
            None
        };
        if sse {
            if let Err(error) = realtime::send_sse_headers(&mut trace::HeadTap::new(&mut w.writer, &mut w.trace), w.origin.as_deref()).await {
                eprintln!("{{\"event\":\"realtime_write_failed\",\"transport\":\"sse\",\"kind\":\"{:?}\"}}", error.kind());
                return;
            }
            let Some(token) = token else { return };
            let _ = realtime::stream_sse(&mut w.writer, subscription.unwrap(), token, fingerprint.to_owned(), facts, runtime.clone(), wire).await;
            return;
        }
        let ws_key = ws_key.as_deref().unwrap();
        let accept_key = tokio_tungstenite::tungstenite::handshake::derive_accept_key(ws_key.as_bytes());
        let response =
            format!("HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: {accept_key}\r\n\r\n");
        let upgrade = async {
            let mut tap = trace::HeadTap::new(&mut w.writer, &mut w.trace);
            tap.write_all(response.as_bytes()).await?;
            tap.flush().await
        };
        match tokio::time::timeout(server::REQUEST_TIMEOUT, upgrade).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) | Err(_) => return,
        }
        let io = tokio::io::join(rd, w.writer);
        let _ = realtime::stream_ws(io, token, browser_slots, fingerprint.to_owned(), facts, runtime.clone(), wire).await;
        return;
    }
    if path == "/session" {
        if !body.as_object().is_some_and(|body| body.is_empty()) {
            return reply(&mut w, &err("WIRE.BAD_REQUEST", "session 본문은 빈 객체")).await;
        }
        let remaining = session.as_ref().map(auth::Session::remaining_ms);
        if remaining == Some(0) {
            return reply(&mut w, &err("WIRE.TOKEN_EXPIRED", "세션 만료")).await;
        }
        let result = json!({
            "ok": true,
            "principal": { "actorId": session.as_ref().map(|s| s.actor_resource_id.clone().unwrap_or_else(||s.actor_id.to_string())) },
            "remainingMs": remaining,
        });
        return reply(&mut w, &result.to_string()).await;
    }
    let _extension_slot = if path == "/extension" || path == "/operation" || write_extension {
        match runtime.extension_slots.try_acquire() {
            Ok(slot) => Some(slot),
            Err(_) => return reply(&mut w, &err("WIRE.WORKER_BUSY", "로컬 확장 동시 호출 상한")).await,
        }
    } else {
        None
    };
    let _extension_principal_slot = if _extension_slot.is_some() {
        match runtime.principal_slots.workers.try_acquire(&admission::principal(session.as_ref())) {
            Some(permit) => Some(permit),
            None => return reply(&mut w, &err("WIRE.WORKER_BUSY", "principal별 로컬 확장 동시 호출 상한")).await,
        }
    } else {
        None
    };
    if let Some(backend) = runtime.infrastructure.relational.clone() {
        if write_extension {
            let Some(config) = runtime.extensions.as_ref() else {
                return reply(&mut w, &err("WIRE.ADAPTER_UNSUPPORTED", "worker write 설정이 없음")).await;
            };
            let request = body["request"].clone();
            let name = request["extension"].as_str().unwrap();
            let now = match tokio::time::timeout(server::DB_PREFLIGHT_TIMEOUT, backend.now()).await {
                Ok(Ok(now)) => now,
                Ok(Err(code)) => return reply(&mut w, &err(code, "worker write 사전 검사 실패")).await,
                Err(_) => return reply(&mut w, &err("WIRE.DB_UNAVAILABLE", "worker write 사전 검사 기한 초과")).await,
            };
            let caller = Caller { actor_id: session.as_ref().map(|session| session.actor_id), now };
            let start =
                match tokio::time::timeout_at(deadline, backend.begin_worker_write(&facts, body["key"].as_str().unwrap(), &request, &caller, wire))
                    .await
                {
                    Ok(Ok(start)) => start,
                    Ok(Err(error)) => return reply(&mut w, &err(error.code, &error.msg)).await,
                    Err(_) => return reply(&mut w, &err("WIRE.DEADLINE_EXCEEDED", "worker write transaction 시작 기한 초과")).await,
                };
            let session_write = match start {
                RelationalWorkerWriteStart::Replay(mut result) => {
                    if let Some(fingerprint) = contract_fingerprint {
                        result["contractFingerprint"] = json!(&*fingerprint);
                    }
                    return reply(&mut w, &result.to_string()).await;
                }
                RelationalWorkerWriteStart::New(session_write) => session_write,
            };
            let mut host = RelationalWriteActionHost { session: session_write };
            let mut worker = match spike_v4_worker::Worker::try_start_configured(
                config.lang,
                config.dir.to_str().unwrap(),
                runtime.infrastructure.worker_launch.clone(),
            )
            .await
            {
                Ok(worker) => worker,
                Err(_) => return reply(&mut w, &err("WIRE.WORKER_FAILED", "공식 worker 실행 실패")).await,
            };
            let output = tokio::time::timeout_at(
                deadline,
                spike_v4_worker::write::invoke_write_action(&mut host, &mut worker, &facts, name, &request["input"], &caller, wire),
            )
            .await;
            worker.stop().await;
            let prepared = match output {
                Ok(Ok(prepared)) => prepared,
                Ok(Err(error)) => return reply(&mut w, &err(error.code, "공식 WRITE 확장 호출 거부")).await,
                Err(_) => return reply(&mut w, &err("WIRE.DEADLINE_EXCEEDED", "공식 WRITE 확장 기한 초과")).await,
            };
            let (resource, extension) = name.split_once('.').unwrap();
            let mut tags = facts["resources"][resource]["extensions"][extension]["access"]
                .as_object()
                .into_iter()
                .flatten()
                .flat_map(|(transition, _)| write_tags(&facts, transition))
                .collect::<Vec<_>>();
            tags.sort();
            tags.dedup();
            let result = json!({"ok":true,"output":prepared.output,"tags":tags});
            // Recheck the same server-verified principal before committing callback effects.
            let token = token.as_deref().unwrap_or("");
            let current = match &runtime.authenticator {
                Some(provider) => match tokio::time::timeout(server::DB_PREFLIGHT_TIMEOUT, provider.verify_session(token)).await {
                    Ok(result) => result,
                    Err(_) => Err(auth::AuthError::Unavailable),
                },
                None => Err(auth::AuthError::Unavailable),
            };
            let rejection = match current {
                Ok(current) if Some(current.actor_id) == caller.actor_id => None,
                Err(auth::AuthError::Expired) => Some(("WIRE.TOKEN_EXPIRED", "세션 만료")),
                Err(auth::AuthError::Unavailable) => Some(("WIRE.DB_UNAVAILABLE", "principal 재검증 실패")),
                _ => Some(("WIRE.UNAUTHENTICATED", "principal 재검증 실패")),
            };
            if let Some((code, message)) = rejection {
                return reply(&mut w, &err(code, message)).await;
            }
            let mut result = match host.session.commit(result.clone()).await {
                Ok(committed) => committed,
                Err(error) => return reply(&mut w, &err(error.code, &error.msg)).await,
            };
            runtime.infrastructure.data_changes.notify();
            result["contractFingerprint"] = json!(contract_fingerprint.as_deref().unwrap_or(""));
            return reply(&mut w, &result.to_string()).await;
        }
        if path == "/extension" {
            let Some(config) = runtime.extensions.as_ref() else {
                return reply(&mut w, &err("WIRE.ADAPTER_UNSUPPORTED", "확장 worker가 설정되지 않음")).await;
            };
            let mut worker = match spike_v4_worker::Worker::try_start_configured(
                config.lang,
                config.dir.to_str().unwrap(),
                runtime.infrastructure.worker_launch.clone(),
            )
            .await
            {
                Ok(worker) => worker,
                Err(_) => return reply(&mut w, &err("WIRE.WORKER_FAILED", "공식 worker 실행 실패")).await,
            };
            let Some(name) = body["extension"].as_str() else {
                worker.stop().await;
                return reply(&mut w, &err("WIRE.BAD_REQUEST", "확장 이름(`extension`) 필요")).await;
            };
            let Some((resource, extension)) = name.split_once('.') else {
                worker.stop().await;
                return reply(&mut w, &err("WIRE.BAD_REQUEST", "확장은 `Resource.name`")).await;
            };
            let deadline = facts["resources"][resource]["extensions"][extension]["deadlineMs"].as_u64().unwrap_or(1000);
            let now = match tokio::time::timeout(server::DB_PREFLIGHT_TIMEOUT, backend.now()).await {
                Ok(Ok(now)) => now,
                Ok(Err(code)) => {
                    worker.stop().await;
                    return reply(&mut w, &err(code, "실행 전 DB 조회 실패")).await;
                }
                Err(_) => {
                    worker.stop().await;
                    return reply(&mut w, &err("WIRE.DB_UNAVAILABLE", "실행 전 DB 조회 기한 초과")).await;
                }
            };
            let caller = Caller { actor_id: session.as_ref().map(|s| s.actor_id), now };
            let mut host = spike_v4_worker::RelationalWorkerHost::with_capabilities(
                &*backend,
                runtime.infrastructure.mongo.clone(),
                runtime.infrastructure.redis.clone(),
            );
            let output = tokio::time::timeout(
                std::time::Duration::from_millis(deadline),
                spike_v4_worker::invoke_with_host_as(
                    &mut host,
                    &mut worker,
                    &facts,
                    name,
                    &body["input"],
                    &caller,
                    wire,
                    session.as_ref().and_then(|s| s.actor_resource_id.as_deref()),
                ),
            )
            .await;
            worker.stop().await;
            let mut result = match output {
                Ok(Ok(output)) => json!({ "ok": true, "output": output }),
                Ok(Err(e)) => json!({ "ok": false, "code": e.code, "msg": "공식 읽기 확장 호출 거부" }),
                Err(_) => json!({ "ok": false, "code": "WIRE.DEADLINE_EXCEEDED", "msg": "공식 읽기 확장 기한 초과" }),
            };
            if result["ok"] == true {
                if let Some(fingerprint) = contract_fingerprint {
                    result["contractFingerprint"] = json!(&*fingerprint);
                }
                if let Some(session) = session {
                    let remaining = session.remaining_ms();
                    if remaining == 0 {
                        return reply(&mut w, &err("WIRE.TOKEN_EXPIRED", "읽는 동안 세션 만료")).await;
                    }
                }
            }
            return reply(&mut w, &result.to_string()).await;
        }

        let mut result = relational::handle(&*backend, &facts, &path, &body, session.as_ref().map(|s| s.actor_id), wire).await;
        if path == "/read" && result["ok"] == true {
            if let Some(fingerprint) = contract_fingerprint {
                result["contractFingerprint"] = json!(&*fingerprint);
            }
            if let Some(session) = session {
                let remaining = session.remaining_ms();
                if remaining == 0 {
                    return reply(&mut w, &err("WIRE.TOKEN_EXPIRED", "읽는 동안 세션 만료")).await;
                }
                result["maxAgeMs"] = json!(result["maxAgeMs"].as_u64().unwrap_or(0).min(remaining));
            }
        }
        return reply(&mut w, &result.to_string()).await;
    }
    let Some(pool) = runtime.db_pool.as_ref() else {
        return reply(&mut w, &err("WIRE.DB_UNAVAILABLE", "DB pool 설정 없음")).await;
    };
    let mut db = match pool.acquire_until(deadline).await {
        Ok(db) => db,
        Err(_) => return reply(&mut w, &err("WIRE.DB_UNAVAILABLE", "DB 연결 또는 pool checkout 기한 초과")).await,
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
            Ok(Ok(Some(actual))) if actual == fence.expected_digest => {
                match tokio::time::timeout_at(deadline, spike_v2_read::external::matches(&*db, &facts)).await {
                    Ok(Ok(true)) => {}
                    _ => {
                        std::mem::drop(db);
                        return reply(&mut w, &err("WIRE.DEPLOYMENT_CHANGED", "외부 업무 schema drift 또는 catalog 검사 실패")).await;
                    }
                }
            }
            Ok(Ok(Some(_))) => {
                std::mem::drop(db);
                return reply(&mut w, &err("WIRE.DEPLOYMENT_CHANGED", "실행 정의와 배포 정의가 다름")).await;
            }
            Ok(Ok(None)) => {
                std::mem::drop(db);
                return reply(&mut w, &err("WIRE.SCHEMA_MISMATCH", "배포 marker 누락")).await;
            }
            Ok(Err(_)) | Err(_) => {
                std::mem::drop(db);
                return reply(&mut w, &err("WIRE.DB_UNAVAILABLE", "배포 잠금 또는 marker 조회 실패")).await;
            }
        }
    }
    let now = match tokio::time::timeout(server::DB_PREFLIGHT_TIMEOUT, db.query_one("SELECT clock_timestamp()::text", &[])).await {
        Ok(Ok(row)) => row.get::<_, String>(0),
        Ok(Err(_)) | Err(_) => {
            std::mem::drop(db);
            return reply(&mut w, &err("WIRE.DB_UNAVAILABLE", "실행 전 DB 조회 실패 또는 기한 초과")).await;
        }
    };
    let caller = Caller { actor_id: session.as_ref().map(|s| s.actor_id), now };
    if let Some(mount) = file_mount {
        let Ok(id) = managed_file_id(mount, &path) else {
            return reply_object(&mut w, &err("WIRE.NOT_FOUND", "관리 파일 경로가 올바르지 않음").to_string()).await;
        };
        let result = match tokio::time::timeout_at(
            deadline,
            durable::handle(
                &mut db,
                &runtime,
                &facts,
                wire,
                "/files/mount-get",
                &json!({"id":id,"bucket":mount.bucket}),
                session.as_ref().and_then(|s| s.durable.as_ref()).unwrap(),
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => return reply_object(&mut w, &err("WIRE.REQUEST_TIMEOUT", "관리 파일 조회 기한 초과").to_string()).await,
        };
        if result["ok"] != true {
            return reply_object(&mut w, &result.to_string()).await;
        }
        let Some(media_type) = result["file"]["mediaType"].as_str() else {
            return reply_object(&mut w, &err("WIRE.INTERNAL", "관리 파일 metadata 오류").to_string()).await;
        };
        use base64::Engine;
        let Some(encoded) = result["base64"].as_str() else {
            return reply_object(&mut w, &err("WIRE.INTERNAL", "관리 파일 payload 오류").to_string()).await;
        };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
            return reply_object(&mut w, &err("WIRE.INTERNAL", "관리 파일 payload 오류").to_string()).await;
        };
        let _ = db.release().await;
        let _ =
            reply_managed_file(&mut trace::HeadTap::new(&mut w.writer, &mut w.trace), &method, id, media_type, &bytes, w.origin.as_deref(), deadline)
                .await;
        return;
    }
    let mut result = if managed {
        durable::handle(&mut db, &runtime, &facts, wire, &path, &body, session.as_ref().unwrap().durable.as_ref().unwrap()).await
    } else if path == "/capabilities" {
        let mut operations = spike_v5_sdk::operation_descriptors(&facts);
        for operation in operations.as_object_mut().unwrap().values_mut() {
            operation["available"] =
                json!(capability_available(operation, runtime.extensions.is_some(), runtime.authenticator.is_some(), runtime.deployment.is_some()));
            operation["authorization"] = json!("checked-on-call");
        }
        json!({"ok":true,"catalogVersion":"aip-capabilities/1","operations":operations,"infrastructure":runtime.infrastructure.catalog()})
    } else if path == "/operation" {
        let operation_deadline =
            std::time::Duration::from_millis(facts["operations"][body["operation"].as_str().unwrap()]["deadlineMs"].as_u64().unwrap());
        let execution = async {
            let output = operation::invoke_with_launch(
                &mut db,
                &facts,
                &body,
                &caller,
                wire,
                runtime.extensions.as_ref().unwrap(),
                &runtime.infrastructure.worker_launch,
            )
            .await;
            if output["ok"] != true {
                return output;
            }
            let token = token.as_deref().unwrap();
            let current = match &runtime.authenticator {
                None => keys.verify_session(token),
                Some(provider) => match tokio::time::timeout(server::DB_PREFLIGHT_TIMEOUT, provider.verify_session(token)).await {
                    Ok(result) => result,
                    Err(_) => Err(auth::AuthError::Unavailable),
                },
            };
            match current {
                Ok(current) if Some(current.actor_id) == caller.actor_id => output,
                Err(auth::AuthError::Expired) => json!({"ok":false,"code":"WIRE.TOKEN_EXPIRED"}),
                Err(auth::AuthError::Unavailable) => json!({"ok":false,"code":"WIRE.DB_UNAVAILABLE"}),
                _ => json!({"ok":false,"code":"WIRE.UNAUTHENTICATED"}),
            }
        };
        match tokio::time::timeout(operation_deadline, execution).await {
            Ok(output) => output,
            Err(_) => json!({"ok":false,"code":"WIRE.DEADLINE_EXCEEDED"}),
        }
    } else if write_extension {
        let mut result = write_extension::apply_with_launch(
            &mut db,
            &facts,
            &body,
            &caller,
            wire,
            runtime.extensions.as_ref().unwrap(),
            &runtime.infrastructure.worker_launch,
        )
        .await;
        if result["ok"] == false {
            result["msg"] = json!("공식 WRITE 확장 호출 거부");
        }
        result
    } else if path == "/extension" {
        let config = runtime.extensions.as_ref().unwrap();
        let mut worker = match spike_v4_worker::Worker::try_start_configured(
            config.lang,
            config.dir.to_str().unwrap(),
            runtime.infrastructure.worker_launch.clone(),
        )
        .await
        {
            Ok(worker) => worker,
            Err(_) => {
                std::mem::drop(db);
                return reply(&mut w, &err("WIRE.WORKER_FAILED", "공식 worker 실행 실패")).await;
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
            Err(_) => json!({"ok":false,"code":"WIRE.DEADLINE_EXCEEDED","msg":"공식 읽기 확장 기한 초과"}),
        }
    } else {
        handle(&mut db, &facts, &path, &body, &caller, wire).await
    };
    let write_status = path == "/status" && body["request"].get("extension").is_some();
    if (["/read", "/extension", "/operation", "/capabilities"].contains(&path.as_str()) || write_extension || write_status || managed)
        && result["ok"] == true
    {
        if let Some(fingerprint) = contract_fingerprint {
            result["contractFingerprint"] = json!(&*fingerprint);
        }
        // 커밋된 결과를 인증 거부로 바꾸면 클라이언트가 실제 쓰기 효과를 놓친다.
        if let Some(session) = session.filter(|_| ["/read", "/extension", "/operation", "/capabilities"].contains(&path.as_str())) {
            let remaining = session.remaining_ms();
            if remaining == 0 {
                std::mem::drop(db);
                return reply(&mut w, &err("WIRE.TOKEN_EXPIRED", "읽는 동안 세션 만료")).await;
            }
            if path == "/read" {
                result["maxAgeMs"] = json!(result["maxAgeMs"].as_u64().unwrap_or(0).min(remaining));
            }
        }
    }
    let out = result.to_string();
    if result["ok"] != true {
        std::mem::drop(db);
        return reply(&mut w, &out).await;
    }
    // A committed write wakes the data subscriptions; a replay of a stored result changed nothing.
    if (path == "/apply" || write_extension) && result["replayed"] != true {
        runtime.infrastructure.data_changes.notify();
    }
    if drop {
        // 응답 유실 흉내: 처리(커밋 포함)는 끝났지만 클라이언트는 응답을 받지 못한다.
        return;
    }
    // Return the connection, and with it the deployment fence's shared lock, before writing: a client that reads slowly
    // must not keep a pool slot or block a migration for as long as the write timeout.
    let _ = db.release().await;
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
            infrastructure: crate::infrastructure::Infrastructure::default(),
            db_pool: None,
            admission: None,
            realtime_slots: tokio::sync::Semaphore::new(32),
            realtime_preauth: tokio::sync::Semaphore::new(32),
            object_slots: tokio::sync::Semaphore::new(4),
            db_url: spike_v2_read::test_database_url(),
            broker: None,
            enable_file_uploads: false,
            fault_injection: true,
            extensions: None,
            write_extensions: false,
            extension_slots: tokio::sync::Semaphore::new(4),
            principal_slots: crate::slots::PrincipalSlotSet::unlimited(),
            options: server::ServerOptions::default(),
            authenticator: None,
            deployment: None,
            otlp: None,
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

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn durable_operation_capabilities_require_worker_authenticator_and_deployment() {
        let sync = json!({"lifetime":"sync"});
        let durable = json!({"lifetime":"durable"});
        assert!(capability_available(&sync, true, false, false));
        assert!(!capability_available(&sync, false, true, true));
        assert!(!capability_available(&durable, false, true, true));
        assert!(!capability_available(&durable, true, false, true));
        assert!(!capability_available(&durable, true, true, false));
        assert!(capability_available(&durable, true, true, true));
    }

    #[tokio::test]
    async fn object_transfer_timeout_ends_a_stalled_transfer() {
        let result = run_object_transfer(std::time::Duration::from_millis(10), std::future::pending::<bool>()).await;
        assert!(result.is_err(), "a stalled transfer must reach its configured lifetime");
    }

    #[tokio::test]
    async fn regular_reply_keeps_send_window_after_ingress_deadline() {
        let (mut client, server) = tokio::io::duplex(1024);
        let deadline = tokio::time::Instant::now() - std::time::Duration::from_millis(1);
        let mut response =
            Response { writer: server, origin: None, private_object: false, head_only: false, deadline, trace: trace::Trace::default() };

        reply_with_headers(&mut response, "{\"ok\":true}", "").await;

        let mut received = vec![0; 128];
        let count = tokio::time::timeout(std::time::Duration::from_millis(10), client.read(&mut received))
            .await
            .expect("regular reply should use its send window")
            .expect("duplex read should succeed");
        assert!(count > 0);
    }

    #[tokio::test]
    async fn private_reply_respects_expired_ingress_deadline() {
        let (mut client, server) = tokio::io::duplex(1024);
        let deadline = tokio::time::Instant::now() - std::time::Duration::from_millis(1);
        let mut response =
            Response { writer: server, origin: None, private_object: true, head_only: false, deadline, trace: trace::Trace::default() };

        reply_with_headers(&mut response, "{\"ok\":true}", "").await;

        let mut received = vec![0; 128];
        assert!(tokio::time::timeout(std::time::Duration::from_millis(1), client.read(&mut received)).await.is_err());
    }
}
