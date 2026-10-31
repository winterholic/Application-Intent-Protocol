//! V4 쓰기 확장. 서버가 소유한 트랜잭션에서만 공개 전이를 실행한다.
use crate::{check_read_record, rej, Worker};
use serde_json::{json, Value};
use spike_v2_read::id_wire::{emit_entity_id, emit_id, parse_id, IdWire, MAX_SAFE_ID};
use spike_v2_read::plan::{Caller, Reject};
use spike_v3_write::{
    apply_in_with_wire, before_commit_rejection, checks_in, classify_write_error, connection_failed, isolation_sql, lost_before_commit, send_commit,
    CommitFailure, Knobs,
};
use std::time::{Duration, Instant};
use tokio_postgres::{Client, Transaction};

pub const MAX_CTX_CALLS: usize = 20;
const COMMIT_MARGIN_MS: u64 = 300;

/// 실행 효과와 caller의 추가 결과 기록을 함께 커밋하기 위한 private spike 경계.
pub struct PreparedWrite {
    pub output: Value,
    pub expires: Instant,
}

struct WriteInvocation<'a> {
    declaration: &'a Value,
    wire: IdWire,
    deadline: Duration,
    expires: Instant,
}

fn write_invocation<'a>(facts: &'a Value, name: &str, input: &Value, wire: IdWire) -> Result<WriteInvocation<'a>, Reject> {
    let (resource, extension) = name.split_once('.').ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "확장은 `Resource.name`".into() })?;
    let declaration = &facts["resources"][resource]["extensions"][extension];
    if declaration.is_null() || declaration["kind"] != "write" {
        return rej("WIRE.NOT_EXPOSED", format!("쓰기 확장 `{name}` 없음"));
    }
    check_write_record(facts, &declaration["input"], input, "입력", wire)?;
    let deadline = Duration::from_millis(declaration["deadlineMs"].as_u64().unwrap_or(1000));
    Ok(WriteInvocation { declaration, wire, deadline, expires: Instant::now() + deadline })
}

/// A write extension may call any declared transition, so its transaction must meet the strictest marker in `access`.
pub fn write_needs_serializable(facts: &Value, name: &str) -> bool {
    let Some((resource, extension)) = name.split_once('.') else { return false };
    facts["resources"][resource]["extensions"][extension]["access"].as_object().is_some_and(|access| {
        access.keys().any(|transition| {
            let Some((resource, transition)) = transition.split_once('.') else { return false };
            facts["resources"][resource]["transitions"][transition]["isolation"] == "serializable"
        })
    })
}

async fn select_transaction_isolation(tx: &Transaction<'_>, serializable: bool) -> Result<(), Reject> {
    tx.batch_execute(isolation_sql(serializable)).await.map_err(|error| {
        if connection_failed(&error) {
            return lost_before_commit();
        }
        if error.code().is_some_and(|code| code.code() == "25001") {
            return Reject {
                code: "WIRE.UNSUPPORTED_ISOLATION",
                msg: "caller-owned transaction은 이미 SQL을 실행해 필요한 격리 수준을 선택할 수 없음".into(),
            };
        }
        classify_write_error("쓰기 격리 설정", error)
    })
}

async fn require_transaction_isolation(tx: &Transaction<'_>, serializable: bool) -> Result<(), Reject> {
    let current: String = tx
        .query_one("SELECT current_setting('transaction_isolation')", &[])
        .await
        .map_err(|error| classify_write_error("쓰기 격리 확인", error))?
        .get(0);
    let required = if serializable { "serializable" } else { "read committed" };
    if current != required {
        return rej("WIRE.UNSUPPORTED_ISOLATION", format!("caller-owned transaction 격리가 {current}; {required} 필요"));
    }
    Ok(())
}

/// The worker side of a write action: the server sends invoke/reply frames and reads the
/// worker's call/done lines. [`Worker`] speaks this over its child-process pipes; tests use a
/// scripted in-process peer so `invoke_write_action` and its validator run without spawning
/// a Node/Python runtime.
pub trait WriteWorker: Send {
    fn next_invoke_id(&mut self) -> u64;
    fn read_line<'a>(&'a mut self) -> crate::host::HostFuture<'a, std::io::Result<Option<String>>>;
    fn send_before<'a>(&'a mut self, message: &'a Value, expires: Instant) -> crate::host::HostFuture<'a, Result<(), Reject>>;
}

impl WriteWorker for Worker {
    fn next_invoke_id(&mut self) -> u64 {
        self.next_invoke += 1;
        self.next_invoke
    }

    fn read_line<'a>(&'a mut self) -> crate::host::HostFuture<'a, std::io::Result<Option<String>>> {
        Box::pin(async move { Worker::read_line(self).await })
    }

    fn send_before<'a>(&'a mut self, message: &'a Value, expires: Instant) -> crate::host::HostFuture<'a, Result<(), Reject>> {
        Box::pin(async move { Worker::send_before(self, message, expires).await })
    }
}

/// A host that applies a declared public transition in the same transaction as the worker result.
pub trait WriteActionHost: Send {
    fn apply<'a>(
        &'a mut self,
        transition: &'a str,
        request: &'a Value,
        caller: &'a Caller,
        wire: IdWire,
    ) -> crate::host::HostFuture<'a, Result<Value, Reject>>;

    /// A Mongo (`kind` = `mongo`, `resource` = collection) or Redis (`redis`, key) write the extension declared with `access`.
    /// The relational transaction cannot include it, so the host keeps its undo record (see [`crate::external::ExternalEffects`]).
    fn external<'a>(&'a mut self, _kind: &'a str, _resource: &'a str, _input: &'a Value) -> crate::host::HostFuture<'a, Result<Value, Reject>> {
        Box::pin(async { rej("WIRE.ADAPTER_UNSUPPORTED", "이 호스트는 Mongo/Redis 쓰기를 지원하지 않음") })
    }
}

/// Run a write extension using an adapter-owned transaction session. The adapter commits only after this returns a
/// contract-valid output; any callback error, worker failure or invalid output leaves commit responsibility with it.
pub async fn invoke_write_action(
    host: &mut dyn WriteActionHost,
    w: &mut impl WriteWorker,
    facts: &Value,
    name: &str,
    input: &Value,
    caller: &Caller,
    wire: IdWire,
) -> Result<PreparedWrite, Reject> {
    let invocation = write_invocation(facts, name, input, wire)?;
    let x = invocation.declaration;
    let expires = invocation.expires;
    let allowed: std::collections::HashSet<&str> = x["access"].as_object().into_iter().flat_map(|access| access.keys().map(String::as_str)).collect();
    let invoke_id = w.next_invoke_id();
    let token = crate::grant_token('w', invoke_id);
    w.send_before(&json!({"type":"invoke","invoke":invoke_id,"impl":x["implementation"],"input":input,"token":token}), expires).await?;
    let mut poisoned: Option<&'static str> = None;
    let mut calls = 0usize;
    let result: Result<Value, Reject> = loop {
        let left = expires.saturating_duration_since(Instant::now());
        let line = match tokio::time::timeout(left, w.read_line()).await {
            Err(_) => break rej("WIRE.DEADLINE_EXCEEDED", "쓰기 action worker 기한 초과"),
            Ok(Err(_)) | Ok(Ok(None)) => break rej("WIRE.WORKER_FAILED", "worker가 종료됨"),
            Ok(Ok(Some(line))) => line,
        };
        let Ok(message) = serde_json::from_str::<Value>(&line) else { break rej("WIRE.WORKER_FAILED", "worker 메시지 형식 오류") };
        match message["type"].as_str() {
            Some("call") => {
                let mine = message["token"] == json!(token);
                if mine {
                    calls += 1;
                }
                let reply = if !mine {
                    rej("WIRE.TOKEN_EXPIRED", "이 worker 호출의 ctx가 아님")
                } else if calls > MAX_CTX_CALLS {
                    rej("WIRE.CALL_LIMIT", format!("ctx 쓰기는 호출당 {MAX_CTX_CALLS}번까지"))
                } else if message
                    .as_object()
                    .is_some_and(|object| object.keys().any(|key| !["type", "call", "token", "op", "name", "input"].contains(&key.as_str())))
                {
                    rej("WIRE.UNKNOWN_KEY", "ctx 요청에 알 수 없는 키")
                } else if message["op"] == "mongo" || message["op"] == "redis" {
                    let kind = message["op"].as_str().unwrap_or("");
                    match message["name"].as_str() {
                        None => rej("WIRE.BAD_VALUE", "external 쓰기의 대상 이름은 문자열이어야 함"),
                        Some(_) if !allowed.iter().any(|grant| grant.strip_prefix(kind).is_some_and(|rest| rest.starts_with('.'))) => {
                            rej("WIRE.ACCESS_NOT_DECLARED", format!("확장 계약에 `{kind}` 쓰기 access가 없음"))
                        }
                        Some(resource) => {
                            match tokio::time::timeout(
                                expires.saturating_duration_since(Instant::now()),
                                host.external(kind, resource, &message["input"]),
                            )
                            .await
                            {
                                Ok(result) => result,
                                Err(_) => break rej("WIRE.DEADLINE_EXCEEDED", "external 쓰기 기한 초과"),
                            }
                        }
                    }
                } else if message["op"] == "create" {
                    // A row of an exposed create, as a one-operation atomic bundle in the same transaction (access `create.<Resource>`).
                    let resource = message["name"].as_str().unwrap_or("");
                    let grant = format!("create.{resource}");
                    if !allowed.contains(grant.as_str()) {
                        rej("WIRE.ACCESS_NOT_DECLARED", "create가 action access에 선언되지 않음")
                    } else {
                        match job_create_request(resource, &message["input"]) {
                            Err(error) => Err(error),
                            Ok(request) => match tokio::time::timeout(
                                expires.saturating_duration_since(Instant::now()),
                                host.apply(&grant, &request, caller, wire),
                            )
                            .await
                            {
                                Ok(result) => result,
                                Err(_) => break rej("WIRE.DEADLINE_EXCEEDED", "ctx 쓰기 기한 초과"),
                            },
                        }
                    }
                } else if message["op"] == "read" {
                    // A public read of the caller's rows inside the write transaction (access `read.<Resource>`), so the extension sees
                    // the row the following patch is checked against (`expect`).
                    let resource = message["name"].as_str().unwrap_or("");
                    let grant = format!("read.{resource}");
                    if !allowed.contains(grant.as_str()) {
                        rej("WIRE.ACCESS_NOT_DECLARED", "read가 action access에 선언되지 않음")
                    } else if message["input"]["read"].as_str() != Some(resource) {
                        rej("WIRE.BAD_VALUE", "ctx.data.read 요청의 read 이름이 다름")
                    } else {
                        match tokio::time::timeout(
                            expires.saturating_duration_since(Instant::now()),
                            host.apply(&grant, &message["input"], caller, wire),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(_) => break rej("WIRE.DEADLINE_EXCEEDED", "ctx 쓰기 기한 초과"),
                        }
                    }
                } else if message["op"] != "apply" {
                    rej("WIRE.UNSUPPORTED", "쓰기 action ctx는 apply·create·read와 mongo·redis 쓰기만")
                } else if !allowed.contains(message["name"].as_str().unwrap_or("")) {
                    rej("WIRE.ACCESS_NOT_DECLARED", "전이가 action access에 선언되지 않음")
                } else {
                    let request = ctx_request(&message, wire);
                    match request {
                        Err(error) => Err(error),
                        Ok(request) => {
                            let transition = message["name"].as_str().unwrap_or("");
                            match tokio::time::timeout(
                                expires.saturating_duration_since(Instant::now()),
                                host.apply(transition, &request, caller, wire),
                            )
                            .await
                            {
                                Ok(result) => result,
                                Err(_) => break rej("WIRE.DEADLINE_EXCEEDED", "ctx 쓰기 기한 초과"),
                            }
                        }
                    }
                };
                let reply = reply.inspect_err(|error| {
                    if mine {
                        poisoned.get_or_insert(error.code);
                    }
                });
                let response = match reply {
                    Ok(value) => json!({"type":"reply","call":message["call"],"value":value}),
                    Err(error) => json!({"type":"reply","call":message["call"],"error":{"code":error.code}}),
                };
                if let Err(error) = w.send_before(&response, expires).await {
                    break Err(error);
                }
            }
            Some("done") if message["invoke"] == json!(invoke_id) => {
                if Instant::now() > expires {
                    break rej("WIRE.DEADLINE_EXCEEDED", "기한 뒤 완료");
                }
                // The extension passed on the refusal of an optimistic `expect` unchanged: the caller may send the request again.
                if poisoned == Some("WIRE.CONFLICT") && message["error"]["code"] == "WIRE.CONFLICT" {
                    break rej("WIRE.CONFLICT", "확장이 읽은 뒤 행이 바뀜(expect 불일치): 다시 보내면 됨");
                }
                // A create the unique constraint refused: the extension ends with the same code (nothing was committed) so the caller can
                // tell "already there" from a failing extension. An extension may also end with CONFLICT after it (a first call that lost
                // the race to create a row), since the caller sends that one again.
                if poisoned == Some("WIRE.ALREADY_EXISTS") {
                    match message["error"]["code"].as_str() {
                        Some("WIRE.ALREADY_EXISTS") => {
                            break rej("WIRE.ALREADY_EXISTS", "이미 있는 행이라 만들 수 없음(유일 제약): 쓴 것은 모두 취소됨");
                        }
                        Some("WIRE.CONFLICT") => break rej("WIRE.CONFLICT", "같은 행을 동시에 만든 호출이 먼저 끝남: 다시 보내면 됨"),
                        _ => {}
                    }
                }
                if let Some(code) = poisoned {
                    break rej("WIRE.EXTENSION_ERROR", format!("ctx 실패 뒤 성공 반환: {code}"));
                }
                if let Some(error) = message.get("error") {
                    break rej("WIRE.EXTENSION_ERROR", format!("확장 실패: {}", error["code"].as_str().unwrap_or("?")));
                }
                let output = message["output"].clone();
                break validate_write_output(facts, name, &output, wire).map(|_| output);
            }
            _ => continue,
        }
    };
    let output = result?;
    if expires.saturating_duration_since(Instant::now()) < Duration::from_millis(COMMIT_MARGIN_MS) {
        return rej("WIRE.DEADLINE_EXCEEDED", "커밋 전 남은 기한 부족");
    }
    Ok(PreparedWrite { output, expires })
}

/// Validate the worker's final record against the declared write output before its host commits.
pub fn validate_write_output(facts: &Value, name: &str, output: &Value, wire: IdWire) -> Result<(), Reject> {
    let (resource, extension) = name.split_once('.').ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "확장은 `Resource.name`".into() })?;
    let declaration = &facts["resources"][resource]["extensions"][extension];
    if declaration.is_null() || declaration["kind"] != "write" {
        return rej("WIRE.NOT_EXPOSED", format!("쓰기 확장 `{name}` 없음"));
    }
    check_write_record(facts, &declaration["output"], output, "출력", wire)
}

pub fn validate_write(facts: &Value, name: &str, input: &Value, wire: IdWire) -> Result<Duration, Reject> {
    write_invocation(facts, name, input, wire).map(|invocation| invocation.deadline)
}

/// 호출자는 반환 기한 안에서 자신의 추가 기록과 commit을 처리하고 commit 불명을 분류해야 한다.
/// 실패하면 호출자는 transaction 전체를 rollback하거나 버려야 한다.
pub async fn prepare_write_in(
    tx: &Transaction<'_>,
    w: &mut Worker,
    facts: &Value,
    name: &str,
    input: &Value,
    caller: &Caller,
) -> Result<PreparedWrite, Reject> {
    let invocation = write_invocation(facts, name, input, IdWire::Legacy)?;
    select_transaction_isolation(tx, write_needs_serializable(facts, name)).await?;
    run_write_in(tx, w, facts, input, caller, invocation).await
}

pub async fn prepare_write_in_with_wire(
    tx: &Transaction<'_>,
    w: &mut Worker,
    facts: &Value,
    name: &str,
    input: &Value,
    caller: &Caller,
    wire: IdWire,
) -> Result<PreparedWrite, Reject> {
    let invocation = write_invocation(facts, name, input, wire)?;
    select_transaction_isolation(tx, write_needs_serializable(facts, name)).await?;
    run_write_in(tx, w, facts, input, caller, invocation).await
}

/// The transaction owner selected isolation before its first query (for example, before an idempotency lookup).
pub async fn prepare_write_in_with_wire_configured_isolation(
    tx: &Transaction<'_>,
    w: &mut Worker,
    facts: &Value,
    name: &str,
    input: &Value,
    caller: &Caller,
    wire: IdWire,
) -> Result<PreparedWrite, Reject> {
    let invocation = write_invocation(facts, name, input, wire)?;
    require_transaction_isolation(tx, write_needs_serializable(facts, name)).await?;
    run_write_in(tx, w, facts, input, caller, invocation).await
}

pub async fn invoke_write(db: &mut Client, w: &mut Worker, facts: &Value, name: &str, input: &Value, caller: &Caller) -> Result<Value, Reject> {
    let invocation = write_invocation(facts, name, input, IdWire::Legacy)?;
    let tx = db.transaction().await.map_err(|error| {
        if connection_failed(&error) {
            lost_before_commit()
        } else {
            Reject { code: "WIRE.INTERNAL", msg: "트랜잭션 시작 실패".into() }
        }
    })?;
    if let Err(error) = select_transaction_isolation(&tx, write_needs_serializable(facts, name)).await {
        let _ = tx.rollback().await;
        return Err(error);
    }
    match run_write_in(&tx, w, facts, input, caller, invocation).await {
        Ok(prepared) => {
            let left = prepared.expires.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, send_commit(tx, || ())).await {
                Ok(result) => result.map_err(CommitFailure::into_reject)?,
                Err(_) => return rej("WIRE.COMMIT_UNKNOWN", "커밋 결과를 기한 안에 확인하지 못함. 상태 조회 필요"),
            }
            Ok(prepared.output)
        }
        Err(error) => {
            let error = before_commit_rejection(&tx, error);
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

async fn run_write_in(
    tx: &Transaction<'_>,
    w: &mut Worker,
    facts: &Value,
    input: &Value,
    caller: &Caller,
    invocation: WriteInvocation<'_>,
) -> Result<PreparedWrite, Reject> {
    let WriteInvocation { declaration: x, wire, deadline, expires } = invocation;
    let allowed: Vec<String> = x["access"].as_object().unwrap().keys().cloned().collect();
    w.next_invoke += 1;
    let invoke_id = w.next_invoke;
    let token = crate::grant_token('w', invoke_id);
    tx.batch_execute(&format!("SET LOCAL statement_timeout = '{}ms'; SET LOCAL TimeZone = 'UTC'", deadline.as_millis()))
        .await
        .map_err(|_| Reject { code: "WIRE.INTERNAL", msg: "세션 설정 실패".into() })?;
    w.send_before(&json!({ "type": "invoke", "invoke": invoke_id, "impl": x["implementation"], "input": input, "token": token }), expires).await?;
    // ctx 쓰기가 한 번이라도 실패하면 확장이 오류를 잡고 성공을 돌려줘도 커밋하지 않는다(트랜잭션 오염 방지).
    let mut poisoned: Option<&'static str> = None;
    let mut calls = 0usize;
    let knobs = Knobs { audit_via: Some("extension"), fresh_now_after_lock: true, internal: true, ..Knobs::default() };
    let result: Result<Value, Reject> = loop {
        let left = expires.saturating_duration_since(Instant::now());
        let line = match tokio::time::timeout(left, w.read_line()).await {
            Err(_) => break rej("WIRE.DEADLINE_EXCEEDED", format!("쓰기 확장이 {}ms 안에 끝나지 않음", deadline.as_millis())),
            Ok(Err(_)) | Ok(Ok(None)) => break rej("WIRE.WORKER_FAILED", "worker가 종료됨"),
            Ok(Ok(Some(l))) => l,
        };
        let Ok(m) = serde_json::from_str::<Value>(&line) else { break rej("WIRE.WORKER_FAILED", "worker 메시지 형식 오류") };
        match m["type"].as_str() {
            Some("call") => {
                let mine = m["token"] == json!(token);
                if mine {
                    calls += 1;
                }
                let reply: Result<Value, Reject> = if !mine {
                    // 이 호출의 토큰이 아니면(끝난 호출의 ctx 등) 아무것도 쓰지 않는다. 현재 호출의 실패로 세지도 않는다(r6 F03).
                    rej("WIRE.TOKEN_EXPIRED", "이 쓰기 트랜잭션의 ctx가 아님")
                } else if calls > MAX_CTX_CALLS {
                    rej("WIRE.CALL_LIMIT", format!("ctx 쓰기는 호출당 {MAX_CTX_CALLS}번까지"))
                } else if let Some(k) =
                    m.as_object().unwrap().keys().find(|k| !["type", "call", "token", "op", "name", "input"].contains(&k.as_str()))
                {
                    rej("WIRE.UNKNOWN_KEY", format!("ctx 요청에 알 수 없는 키 `{k}`"))
                } else if m["op"] != "apply" {
                    rej("WIRE.UNSUPPORTED", "쓰기 확장 ctx는 apply만")
                } else if !allowed.iter().any(|a| m["name"] == json!(a)) {
                    rej("WIRE.ACCESS_NOT_DECLARED", format!("확장 계약에 `{}` 쓰기가 없음", m["name"]))
                } else {
                    match ctx_request(&m, wire) {
                        Err(error) => Err(error),
                        Ok(req) => {
                            let left = expires.saturating_duration_since(Instant::now());
                            match tokio::time::timeout(left, apply_in_with_wire(tx, facts, &req, caller, &knobs, wire)).await {
                                Ok(result) => result.and_then(|applied| {
                                    let changed = applied.changed.into_iter().map(|id| emit_entity_id(id, wire)).collect::<Result<Vec<_>, _>>()?;
                                    let unchanged =
                                        applied.unchanged.into_iter().map(|id| emit_entity_id(id, wire)).collect::<Result<Vec<_>, _>>()?;
                                    Ok(json!({"changed":changed,"unchanged":unchanged}))
                                }),
                                Err(_) => break rej("WIRE.DEADLINE_EXCEEDED", "ctx 쓰기가 기한 안에 끝나지 않음"),
                            }
                        }
                    }
                };
                // ctx 쓰기가 연결을 잃어 실패하면 확장이 어떻게 끝나든 호출자에게는 연결 끊김이다(아직 COMMIT 전이라 커밋된 것이 없다).
                let reply = reply.map_err(|e| before_commit_rejection(tx, e));
                if let (Err(e), true) = (&reply, mine) {
                    poisoned.get_or_insert(e.code);
                }
                let msg = match reply {
                    Ok(v) => json!({ "type": "reply", "call": m["call"], "value": v }),
                    Err(e) => json!({ "type": "reply", "call": m["call"], "error": { "code": e.code } }),
                };
                if let Err(e) = w.send_before(&msg, expires).await {
                    break Err(e);
                }
            }
            Some("done") if m["invoke"] == json!(invoke_id) => {
                if Instant::now() > expires {
                    break rej("WIRE.DEADLINE_EXCEEDED", "기한 뒤 완료");
                }
                if poisoned == Some("WIRE.DB_UNAVAILABLE") {
                    break Err(lost_before_commit());
                }
                if let Some(e) = m.get("error") {
                    break rej("WIRE.EXTENSION_ERROR", format!("확장 실패: {}", e["code"].as_str().unwrap_or("?")));
                }
                if let Some(code) = poisoned {
                    break rej("WIRE.EXTENSION_ERROR", format!("ctx 쓰기 실패 뒤 성공 반환: {code}"));
                }
                let out = m["output"].clone();
                break check_write_record(facts, &x["output"], &out, "출력", wire).map(|_| out);
            }
            _ => continue,
        }
    };
    let output = result?;
    let left = expires.saturating_duration_since(Instant::now());
    if left < Duration::from_millis(COMMIT_MARGIN_MS) {
        return rej("WIRE.DEADLINE_EXCEEDED", "커밋 전 남은 기한 부족");
    }
    match tokio::time::timeout(left, checks_in(tx, facts, caller, &knobs)).await {
        Ok(result) => result?,
        Err(_) => return rej("WIRE.DEADLINE_EXCEEDED", "커밋 검사가 기한 안에 끝나지 않음"),
    }
    Ok(PreparedWrite { output, expires })
}

fn check_write_record(facts: &Value, declaration: &Value, value: &Value, what: &str, wire: IdWire) -> Result<(), Reject> {
    check_read_record(facts, declaration, value, what, wire)?;
    if wire != IdWire::Legacy {
        for field in declaration.as_array().unwrap() {
            if field[1].as_str().unwrap().trim_end_matches('?') == "Int" {
                let scalar = &value[field[0].as_str().unwrap()];
                if !scalar.is_null() && !scalar.as_i64().is_some_and(|integer| (-MAX_SAFE_ID..=MAX_SAFE_ID).contains(&integer)) {
                    return rej(if what == "출력" { "WIRE.OUTPUT_INVALID" } else { "WIRE.BAD_VALUE" }, "Int가 JS 안전 정수 범위를 넘음");
                }
            }
        }
    }
    Ok(())
}

/// Builds the transition request for a durable job's `ctx.data.apply`, using the same Id conversion as write extensions.
pub fn job_apply_request(name: &str, input: &Value, wire: IdWire) -> Result<Value, Reject> {
    let exact = input.as_object().is_some_and(|object| object.keys().all(|key| key == "ids" || key == "key") && object.contains_key("ids"));
    if !exact || !input["ids"].is_array() {
        return rej("WIRE.BAD_VALUE", "ctx.data.apply 입력은 ids와 key만 허용");
    }
    ctx_request(&json!({"name": name, "input": {"ids": input["ids"]}}), wire)
}

/// Builds the create request of a durable job's ctx.data.create: one row of an exposed create, as a one-operation atomic bundle, so the
/// create contract (fields, allow, defaults) is the one a client would meet.
pub fn job_create_request(name: &str, input: &Value) -> Result<Value, Reject> {
    let exact = input.as_object().is_some_and(|object| object.keys().all(|key| key == "values" || key == "key") && object.contains_key("values"));
    if !exact || !input["values"].is_object() {
        return rej("WIRE.BAD_VALUE", "ctx.data.create 입력은 values와 key만 허용");
    }
    Ok(json!({"atomic": [{"create": name, "values": input["values"]}]}))
}

fn ctx_request(message: &Value, wire: IdWire) -> Result<Value, Reject> {
    let mut target = message["input"].clone();
    if wire != IdWire::Legacy {
        if let Some(ids) = target["ids"].as_array() {
            // 공식 worker ctx의 문자열 Id를 선택 wire로 바꿔 JS parse 전에 정밀도를 보존한다.
            let encoded = ids
                .iter()
                .map(|id| {
                    if id.as_str().is_some_and(|text| spike_v2_read::id_wire::parse_uuid(text).is_ok()) {
                        Ok(id.clone())
                    } else {
                        parse_id(id, IdWire::DecimalString).and_then(|id| emit_id(id, wire))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            target["ids"] = json!(encoded);
        }
    }
    // A patch transition takes the caller's values and the expected current values (GENERAL-UPDATE); they are not part of the target.
    let mut request = json!({"apply":message["name"]});
    if let Some(object) = target.as_object_mut() {
        for key in ["values", "expect"] {
            if let Some(value) = object.remove(key) {
                request[key] = value;
            }
        }
    }
    request["target"] = target;
    Ok(request)
}

#[cfg(test)]
mod job_create_tests {
    use super::job_create_request;
    use serde_json::json;

    #[test]
    fn a_job_create_is_a_one_operation_bundle_of_exactly_values_and_key() {
        let request = job_create_request("Video", &json!({"values": {"title": "t"}, "key": "video:a"})).unwrap();
        assert_eq!(request, json!({"atomic": [{"create": "Video", "values": {"title": "t"}}]}));
        for bad in [json!({}), json!({"values": "x"}), json!({"values": {}, "key": "k", "extra": 1}), json!({"key": "k"}), json!("x")] {
            assert_eq!(job_create_request("Video", &bad).unwrap_err().code, "WIRE.BAD_VALUE", "{bad}");
        }
    }
}

#[cfg(test)]
mod ctx_request_tests {
    use super::ctx_request;
    use serde_json::json;
    use spike_v2_read::id_wire::IdWire;

    #[test]
    fn a_patch_transition_carries_values_and_expect_next_to_its_target() {
        let message = json!({"name": "R.t", "input": {"ids": ["u"], "values": {"a": 1}, "expect": {"lock_version": 3}, "key": "k"}});
        let request = ctx_request(&message, IdWire::Legacy).unwrap();
        assert_eq!(request, json!({"apply": "R.t", "values": {"a": 1}, "expect": {"lock_version": 3}, "target": {"ids": ["u"], "key": "k"}}));
        let plain = ctx_request(&json!({"name": "R.t", "input": {"ids": ["u"]}}), IdWire::Legacy).unwrap();
        assert_eq!(plain, json!({"apply": "R.t", "target": {"ids": ["u"]}}));
    }
}

#[cfg(test)]
mod duplicate_create_conflict_tests {
    use super::{invoke_write_action, WriteActionHost, WriteWorker};
    use crate::host::HostFuture;
    use serde_json::{json, Value};
    use spike_v2_read::id_wire::IdWire;
    use spike_v2_read::plan::{Caller, Reject};
    use std::{collections::VecDeque, time::Instant};

    struct Peer {
        lines: VecDeque<String>,
    }

    impl WriteWorker for Peer {
        fn next_invoke_id(&mut self) -> u64 {
            1
        }

        fn read_line<'a>(&'a mut self) -> HostFuture<'a, std::io::Result<Option<String>>> {
            Box::pin(async move { Ok(self.lines.pop_front()) })
        }

        fn send_before<'a>(&'a mut self, message: &'a Value, _: Instant) -> HostFuture<'a, Result<(), Reject>> {
            Box::pin(async move {
                if message["type"] == "invoke" {
                    let token = message["token"].as_str().unwrap();
                    self.lines.push_back(
                        json!({"type":"call", "call":1, "token":token,
                        "op":"create", "name":"UserEmotionSummary", "input":{"values":{"id":"member-1"}}})
                        .to_string(),
                    );
                    self.lines.push_back(json!({"type":"done", "invoke":1, "error":{"code":"WIRE.CONFLICT"}}).to_string());
                }
                Ok(())
            })
        }
    }

    struct DuplicateCreate;

    impl WriteActionHost for DuplicateCreate {
        fn apply<'a>(&'a mut self, grant: &'a str, request: &'a Value, _: &'a Caller, _: IdWire) -> HostFuture<'a, Result<Value, Reject>> {
            Box::pin(async move {
                assert_eq!(grant, "create.UserEmotionSummary");
                assert_eq!(request, &json!({"atomic":[{"create":"UserEmotionSummary", "values":{"id":"member-1"}}]}));
                Err(Reject { code: "WIRE.ALREADY_EXISTS", msg: "duplicate primary key".into() })
            })
        }
    }

    #[tokio::test]
    async fn duplicate_first_row_creation_is_returned_as_a_retryable_conflict() {
        let facts = json!({"resources":{"User":{"extensions":{"myEmotionSummary":{
            "kind":"write", "input":[], "deadlineMs":5000, "implementation":"test",
            "access":{"create.UserEmotionSummary":{}}
        }}}}});
        let caller = Caller { actor_id: Some(1), now: "2026-10-22T00:00:00Z".into() };
        let result = invoke_write_action(
            &mut DuplicateCreate,
            &mut Peer { lines: VecDeque::new() },
            &facts,
            "User.myEmotionSummary",
            &json!({}),
            &caller,
            IdWire::Legacy,
        )
        .await;
        let error = match result {
            Ok(_) => panic!("the row creation race must ask the caller to retry"),
            Err(error) => error,
        };
        assert_eq!(error.code, "WIRE.CONFLICT");
    }
}

#[cfg(test)]
mod write_isolation_selection_tests {
    use super::write_needs_serializable;
    use serde_json::json;

    #[test]
    fn extension_requires_strictest_isolation_of_every_allowed_transition() {
        let facts = json!({"resources":{"Event":{
            "transitions":{
                "plain":{"isolation":null},
                "guarded":{"isolation":"serializable"}
            },
            "extensions":{
                "plain":{"access":{"Event.plain":{}}},
                "mixed":{"access":{"Event.plain":{},"Event.guarded":{}}},
                "guarded":{"access":{"Event.guarded":{}}}
            }
        }}});
        assert!(!write_needs_serializable(&facts, "Event.plain"));
        assert!(write_needs_serializable(&facts, "Event.mixed"));
        assert!(write_needs_serializable(&facts, "Event.guarded"));
        assert!(!write_needs_serializable(&facts, "Event.missing"));
    }
}
