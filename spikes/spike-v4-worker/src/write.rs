//! V4 쓰기 확장. 서버가 소유한 트랜잭션에서만 공개 전이를 실행한다.
use crate::{check_read_record, rej, Worker};
use serde_json::{json, Value};
use spike_v2_read::id_wire::{emit_id, parse_id, IdWire, MAX_SAFE_ID};
use spike_v2_read::plan::{Caller, Reject};
use spike_v3_write::{apply_in_with_wire, checks_in, classify_write_error, Knobs};
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
    let (resource, extension) = name.split_once('.').ok_or(Reject { code: "BAD_REQUEST", msg: "확장은 `Resource.name`".into() })?;
    let declaration = &facts["resources"][resource]["extensions"][extension];
    if declaration.is_null() || declaration["kind"] != "write" {
        return rej("NOT_EXPOSED", format!("쓰기 확장 `{name}` 없음"));
    }
    check_write_record(facts, &declaration["input"], input, "입력", wire)?;
    let deadline = Duration::from_millis(declaration["deadlineMs"].as_u64().unwrap_or(1000));
    Ok(WriteInvocation { declaration, wire, deadline, expires: Instant::now() + deadline })
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
    run_write_in(tx, w, facts, input, caller, invocation).await
}

pub async fn invoke_write(db: &mut Client, w: &mut Worker, facts: &Value, name: &str, input: &Value, caller: &Caller) -> Result<Value, Reject> {
    let invocation = write_invocation(facts, name, input, IdWire::Legacy)?;
    let tx = db.transaction().await.map_err(|_| Reject { code: "INTERNAL", msg: "트랜잭션 시작 실패".into() })?;
    match run_write_in(&tx, w, facts, input, caller, invocation).await {
        Ok(prepared) => {
            let left = prepared.expires.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, tx.commit()).await {
                Ok(result) => result.map_err(|error| classify_write_error("커밋", error))?,
                Err(_) => return rej("COMMIT_UNKNOWN", "커밋 결과를 기한 안에 확인하지 못함. 상태 조회 필요"),
            }
            Ok(prepared.output)
        }
        Err(error) => {
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
    let token = format!("w{invoke_id}-{}", Instant::now().elapsed().as_nanos());
    tx.batch_execute(&format!(
        "SET TRANSACTION ISOLATION LEVEL READ COMMITTED; SET LOCAL statement_timeout = '{}ms'; SET LOCAL TimeZone = 'UTC'",
        deadline.as_millis()
    ))
    .await
    .map_err(|_| Reject { code: "INTERNAL", msg: "세션 설정 실패".into() })?;
    w.send_before(&json!({ "type": "invoke", "invoke": invoke_id, "impl": x["implementation"], "input": input, "token": token }), expires).await?;
    // ctx 쓰기가 한 번이라도 실패하면 확장이 오류를 잡고 성공을 돌려줘도 커밋하지 않는다(트랜잭션 오염 방지).
    let mut poisoned: Option<&'static str> = None;
    let mut calls = 0usize;
    let knobs = Knobs::default();
    let result: Result<Value, Reject> = loop {
        let left = expires.saturating_duration_since(Instant::now());
        let line = match tokio::time::timeout(left, w.read_line()).await {
            Err(_) => break rej("DEADLINE_EXCEEDED", format!("쓰기 확장이 {}ms 안에 끝나지 않음", deadline.as_millis())),
            Ok(Err(_)) | Ok(Ok(None)) => break rej("WORKER_FAILED", "worker가 종료됨"),
            Ok(Ok(Some(l))) => l,
        };
        let Ok(m) = serde_json::from_str::<Value>(&line) else { break rej("WORKER_FAILED", "worker 메시지 형식 오류") };
        match m["type"].as_str() {
            Some("call") => {
                let mine = m["token"] == json!(token);
                if mine {
                    calls += 1;
                }
                let reply: Result<Value, Reject> = if !mine {
                    // 이 호출의 토큰이 아니면(끝난 호출의 ctx 등) 아무것도 쓰지 않는다. 현재 호출의 실패로 세지도 않는다(r6 F03).
                    rej("TOKEN_EXPIRED", "이 쓰기 트랜잭션의 ctx가 아님")
                } else if calls > MAX_CTX_CALLS {
                    rej("CALL_LIMIT", format!("ctx 쓰기는 호출당 {MAX_CTX_CALLS}번까지"))
                } else if let Some(k) =
                    m.as_object().unwrap().keys().find(|k| !["type", "call", "token", "op", "name", "input"].contains(&k.as_str()))
                {
                    rej("UNKNOWN_KEY", format!("ctx 요청에 알 수 없는 키 `{k}`"))
                } else if m["op"] != "apply" {
                    rej("UNSUPPORTED", "쓰기 확장 ctx는 apply만")
                } else if !allowed.iter().any(|a| m["name"] == json!(a)) {
                    rej("ACCESS_NOT_DECLARED", format!("확장 계약에 `{}` 쓰기가 없음", m["name"]))
                } else {
                    match ctx_request(&m, wire) {
                        Err(error) => Err(error),
                        Ok(req) => {
                            let left = expires.saturating_duration_since(Instant::now());
                            match tokio::time::timeout(left, apply_in_with_wire(tx, facts, &req, caller, &knobs, wire)).await {
                                Ok(result) => result.and_then(|applied| {
                                    let changed = applied.changed.into_iter().map(|id| emit_id(id, wire)).collect::<Result<Vec<_>, _>>()?;
                                    let unchanged = applied.unchanged.into_iter().map(|id| emit_id(id, wire)).collect::<Result<Vec<_>, _>>()?;
                                    Ok(json!({"changed":changed,"unchanged":unchanged}))
                                }),
                                Err(_) => break rej("DEADLINE_EXCEEDED", "ctx 쓰기가 기한 안에 끝나지 않음"),
                            }
                        }
                    }
                };
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
                    break rej("DEADLINE_EXCEEDED", "기한 뒤 완료");
                }
                if let Some(e) = m.get("error") {
                    break rej("EXTENSION_ERROR", format!("확장 실패: {}", e["code"].as_str().unwrap_or("?")));
                }
                if let Some(code) = poisoned {
                    break rej("EXTENSION_ERROR", format!("ctx 쓰기 실패 뒤 성공 반환: {code}"));
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
        return rej("DEADLINE_EXCEEDED", "커밋 전 남은 기한 부족");
    }
    match tokio::time::timeout(left, checks_in(tx, facts, caller, &knobs)).await {
        Ok(result) => result?,
        Err(_) => return rej("DEADLINE_EXCEEDED", "커밋 검사가 기한 안에 끝나지 않음"),
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
                    return rej(if what == "출력" { "OUTPUT_INVALID" } else { "BAD_VALUE" }, "Int가 JS 안전 정수 범위를 넘음");
                }
            }
        }
    }
    Ok(())
}

fn ctx_request(message: &Value, wire: IdWire) -> Result<Value, Reject> {
    let mut target = message["input"].clone();
    if wire != IdWire::Legacy {
        if let Some(ids) = target["ids"].as_array() {
            // 공식 worker ctx의 문자열 Id를 선택 wire로 바꿔 JS parse 전에 정밀도를 보존한다.
            let encoded = ids.iter().map(|id| parse_id(id, IdWire::DecimalString).and_then(|id| emit_id(id, wire))).collect::<Result<Vec<_>, _>>()?;
            target["ids"] = json!(encoded);
        }
    }
    Ok(json!({"apply":message["name"],"target":target}))
}
