//! V4-1 공식 읽기 확장. 서버가 actor·허용 집계·입력 바인딩·기한을 ctx 토큰에 묶고, worker는 토큰으로만 데이터를 요청한다.
mod framing;

pub mod outbox;
pub mod write;

#[cfg(test)]
mod grant_tests;
#[cfg(test)]
mod value_tests;

use framing::BoundedLines;
use serde_json::{json, Map, Value};
use spike_v2_read::execute;
use spike_v2_read::id_wire::IdWire;
use spike_v2_read::plan::{Caller, Reject};
use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio_postgres::Client;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lang {
    Node,
    Python,
}

/// worker 격리 방식. NetDeny는 macOS sandbox-exec로 네트워크만 막는다(실험용, 플랫폼 한정).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Isolation {
    None,
    MacNetDeny,
}

#[derive(Clone, Copy, Debug)]
pub struct WorkerLimits {
    pub stdout_frame_bytes: usize,
    pub stderr_frame_bytes: usize,
}

impl Default for WorkerLimits {
    fn default() -> Self {
        Self { stdout_frame_bytes: 1024 * 1024, stderr_frame_bytes: 16 * 1024 }
    }
}

const MAX_RETIRED_TOKENS: usize = 64;
const NODE_WORKER: &str = include_str!("../workers/worker.mjs");
const PYTHON_WORKER: &str = include_str!("../workers/worker.py");

pub struct Worker {
    child: Child,
    /// worker stderr 마지막 줄들. 기동·실행 실패 원인을 서버 쪽에 남긴다(R4-01).
    stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    stdin: ChildStdin,
    lines: BoundedLines<BufReader<ChildStdout>>,
    stderr_task: tokio::task::JoinHandle<()>,
    next_invoke: u64,
    tokens: HashMap<String, Grant>,
    retired_tokens: VecDeque<String>,
}

/// ctx 토큰 하나가 허락하는 것. worker가 보낸 값이 아니라 서버가 정한다.
struct Grant {
    actor_id: Option<i64>,
    now: String,
    /// 확장 계약의 access 목록과 각 집계 입력에 묶인 확장 입력 값.
    access: HashMap<String, Map<String, Value>>,
    id_wire: IdWire,
    expires: Instant,
    active: bool,
}

pub(crate) fn rej<T>(code: &'static str, msg: impl Into<String>) -> Result<T, Reject> {
    Err(Reject { code, msg: msg.into() })
}

impl Worker {
    /// worker 환경 변수는 PATH만 남긴다. DB 연결 정보를 넘기지 않는다.
    /// 한계: 환경 변수를 지워도 네트워크 접근은 막지 않는다(tests의 probeDb 참고).
    pub async fn start(lang: Lang, ext_dir: &str) -> Worker {
        Self::start_with(lang, ext_dir, Isolation::None).await
    }

    pub async fn start_with(lang: Lang, ext_dir: &str, iso: Isolation) -> Worker {
        Self::try_start_with(lang, ext_dir, iso, WorkerLimits::default()).await.expect("worker 실행")
    }

    pub async fn try_start_with(lang: Lang, ext_dir: &str, iso: Isolation, limits: WorkerLimits) -> std::io::Result<Worker> {
        if limits.stdout_frame_bytes == 0 || limits.stderr_frame_bytes == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "worker frame limits must be positive"));
        }
        let prog = match lang {
            Lang::Node => "node",
            Lang::Python => "python3",
        };
        let mut cmd = match iso {
            Isolation::None => Command::new(prog),
            Isolation::MacNetDeny => {
                // ctx는 stdin/stdout으로만 오가므로 네트워크를 막아도 확장 호출은 동작해야 한다.
                let mut c = Command::new("/usr/bin/sandbox-exec");
                c.args(["-p", "(version 1)(allow default)(deny network*)", prog]);
                c
            }
        };
        match lang {
            Lang::Node => {
                cmd.args(["--input-type=module", "-e", NODE_WORKER, "--", ext_dir]);
            }
            Lang::Python => {
                cmd.args(["-P", "-c", PYTHON_WORKER, ext_dir]);
            }
        }
        cmd.env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|error| std::io::Error::new(error.kind(), "worker 실행 실패"))?;
        let missing_pipe = || std::io::Error::other("worker pipe 생성 실패");
        let stdin = child.stdin.take().ok_or_else(missing_pipe)?;
        let lines = BoundedLines::new(BufReader::new(child.stdout.take().ok_or_else(missing_pipe)?), limits.stdout_frame_bytes);
        let stderr_tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let tail = stderr_tail.clone();
        let mut err_lines = BoundedLines::new(BufReader::new(child.stderr.take().ok_or_else(missing_pipe)?), limits.stderr_frame_bytes);
        let stderr_task = tokio::spawn(async move {
            loop {
                let line = match err_lines.next_line().await {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(_) => {
                        {
                            let mut t = tail.lock().unwrap();
                            t.clear();
                            t.push("stderr frame limit exceeded or invalid stream; remaining diagnostics discarded".into());
                        }
                        // Keep draining after a bad diagnostic so a full pipe cannot block stdout.
                        let _ = tokio::io::copy(&mut err_lines.into_inner(), &mut tokio::io::sink()).await;
                        break;
                    }
                };
                let mut t = tail.lock().unwrap();
                t.push(line);
                let n = t.len();
                if n > 20 {
                    t.drain(..n - 20);
                }
            }
        });
        Ok(Worker { child, stderr_tail, stdin, lines, stderr_task, next_invoke: 0, tokens: HashMap::new(), retired_tokens: VecDeque::new() })
    }

    fn remember_expired(&mut self, token: String) {
        self.retired_tokens.push_back(token);
        if self.retired_tokens.len() > MAX_RETIRED_TOKENS {
            self.retired_tokens.pop_front();
        }
    }

    fn retire(&mut self, token: &str) {
        if self.tokens.remove(token).is_some() {
            self.remember_expired(token.to_owned());
        }
    }

    fn retire_previous(&mut self) {
        // A cancelled caller can leave its grant behind. A mutable Worker has one current call.
        for token in std::mem::take(&mut self.tokens).into_keys() {
            self.remember_expired(token);
        }
    }

    pub(crate) async fn read_line(&mut self) -> std::io::Result<Option<String>> {
        let result = self.lines.next_line().await;
        if !matches!(&result, Ok(Some(_))) {
            self.lines.discard();
            self.tokens.clear();
            let _ = self.child.start_kill();
        }
        result
    }

    pub(crate) async fn send(&mut self, m: &Value) -> Result<(), Reject> {
        if self.lines.is_failed() {
            return rej("WORKER_FAILED", "worker protocol이 폐기됨");
        }
        let mut s = m.to_string();
        s.push('\n');
        self.stdin.write_all(s.as_bytes()).await.map_err(|_| Reject { code: "WORKER_FAILED", msg: "worker 입력 실패".into() })
    }

    pub(crate) async fn send_before(&mut self, m: &Value, expires: Instant) -> Result<(), Reject> {
        if Instant::now() < expires {
            if let Ok(result) = tokio::time::timeout(expires.saturating_duration_since(Instant::now()), self.send(m)).await {
                return result;
            }
        }
        // 취소된 write_all은 부분 JSON을 남길 수 있어 같은 protocol을 이어 쓰면 안 된다.
        self.lines.discard();
        self.tokens.clear();
        let _ = self.child.start_kill();
        rej("DEADLINE_EXCEEDED", "worker 입력 전송이 기한 안에 끝나지 않음")
    }

    pub async fn stop(mut self) {
        let _ = self.child.kill().await;
        self.stderr_task.abort();
        let _ = (&mut self.stderr_task).await;
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stderr_task.abort();
    }
}

/// V1이 받는 입출력 타입 전체를 같은 규칙으로 검사한다(F09). 지원하지 않는 타입은 항상 거부.
fn check_typed(facts: &Value, ty: &str, v: &Value) -> bool {
    let nullable = ty.ends_with('?');
    let base = ty.trim_end_matches('?');
    match (base, v) {
        (_, Value::Null) => nullable,
        (t, Value::String(s)) if t.starts_with("Id<") || t.starts_with("Ref<") => !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()),
        // Worker payloads keep their spelling; scalar parsing only validates the shared domain.
        _ => spike_v2_read::scalar::parse(facts, base, v).is_ok(),
    }
}

/// 선언된 입출력과 정확히 같은 키·타입인지. 모르는 키도 거부한다.
pub(crate) fn check_record(facts: &Value, decl: &Value, v: &Value, what: &str) -> Result<(), Reject> {
    let o = v.as_object().ok_or(Reject { code: if what == "출력" { "OUTPUT_INVALID" } else { "BAD_VALUE" }, msg: format!("{what}은 객체") })?;
    let code = if what == "출력" { "OUTPUT_INVALID" } else { "BAD_VALUE" };
    let d = decl.as_array().unwrap();
    for k in o.keys() {
        if !d.iter().any(|p| p[0] == json!(k)) {
            return rej(code, format!("{what}에 선언되지 않은 키 `{k}`"));
        }
    }
    for p in d {
        let (n, ty) = (p[0].as_str().unwrap(), p[1].as_str().unwrap());
        // nullable과 생략은 다르다. 선언된 키는 값이 null이어도 있어야 한다(F08).
        let Some(val) = o.get(n) else {
            return rej(code, format!("{what}에 선언된 키 `{n}`이 없음"));
        };
        if !check_typed(facts, ty, val) {
            return rej(code, format!("{what} `{n}`은 {ty}"));
        }
    }
    Ok(())
}

fn check_read_value(facts: &Value, ty: &str, value: &Value, wire: IdWire, output: bool) -> Result<(), Reject> {
    let base = ty.trim_end_matches('?');
    let code = if output { "OUTPUT_INVALID" } else { "BAD_VALUE" };
    if value.is_null() {
        return if ty.ends_with('?') { Ok(()) } else { rej(code, format!("{ty} null 불가")) };
    }
    if base.starts_with("Id<") || base.starts_with("Ref<") {
        if wire == IdWire::Legacy {
            return if matches!(value, Value::String(s) if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())) {
                Ok(())
            } else {
                rej(code, format!("값이 {ty} 형식과 다름"))
            };
        }
        return spike_v2_read::id_wire::parse_id(value, wire)
            .map(|_| ())
            .map_err(|error| if output { Reject { code, msg: error.msg } } else { error });
    }
    if base == "Int" && !value.as_i64().is_some_and(|n| (-spike_v2_read::id_wire::MAX_SAFE_ID..=spike_v2_read::id_wire::MAX_SAFE_ID).contains(&n)) {
        return rej(code, "Int는 JavaScript 안전 정수 범위여야 함");
    }
    spike_v2_read::scalar::parse(facts, base, value).map(|_| ()).map_err(|error| Reject { code, msg: error.msg })
}

fn check_read_record(facts: &Value, decl: &Value, value: &Value, what: &str, wire: IdWire) -> Result<(), Reject> {
    check_read_record_types(facts, decl, value, what, wire)?;
    let code = if what == "출력" { "OUTPUT_INVALID" } else { "BAD_VALUE" };
    for field in decl.as_array().unwrap() {
        spike_v2_read::scalar::validate_range(field, &value[field[0].as_str().unwrap()], code)?;
    }
    Ok(())
}

fn check_read_record_types(facts: &Value, decl: &Value, value: &Value, what: &str, wire: IdWire) -> Result<(), Reject> {
    if wire == IdWire::Legacy {
        return check_record(facts, decl, value, what);
    }
    let object =
        value.as_object().ok_or(Reject { code: if what == "출력" { "OUTPUT_INVALID" } else { "BAD_VALUE" }, msg: format!("{what}은 객체") })?;
    let output = what == "출력";
    let code = if output { "OUTPUT_INVALID" } else { "BAD_VALUE" };
    let fields = decl.as_array().ok_or(Reject { code, msg: format!("{what} 선언 형식 오류") })?;
    for key in object.keys() {
        if !fields.iter().any(|field| field[0] == json!(key)) {
            return rej(code, format!("{what}에 선언되지 않은 키 `{key}`"));
        }
    }
    for field in fields {
        let (name, ty) = (field[0].as_str().unwrap(), field[1].as_str().unwrap());
        let Some(field_value) = object.get(name) else {
            return rej(code, format!("{what}에 선언된 키 `{name}`이 없음"));
        };
        check_read_value(facts, ty, field_value, wire, output)?;
    }
    Ok(())
}

/// HTTP 계층에서 DB·worker 호출 전에 READ extension 입력을 검증한다.
pub fn validate_read(facts: &Value, name: &str, input: &Value, wire: IdWire) -> Result<(), Reject> {
    let (resource, extension) = name.split_once('.').ok_or(Reject { code: "BAD_REQUEST", msg: "확장은 `Resource.name`".into() })?;
    let x = &facts["resources"][resource]["extensions"][extension];
    if x.is_null() {
        return rej("NOT_EXPOSED", format!("확장 `{name}` 없음"));
    }
    if x["kind"] != "read" || x["effect"] != "none" {
        return rej("UNSUPPORTED", "V4-1은 effect none 읽기 확장만");
    }
    check_read_record(facts, &x["input"], input, "입력", wire)
}

/// 확장 호출. 반환값은 출력 계약 검사를 통과한 값뿐이다.
pub async fn invoke(db: &mut Client, w: &mut Worker, facts: &Value, name: &str, input: &Value, caller: &Caller) -> Result<Value, Reject> {
    invoke_with_wire(db, w, facts, name, input, caller, IdWire::Legacy).await
}

pub async fn invoke_with_wire(
    db: &mut Client,
    w: &mut Worker,
    facts: &Value,
    name: &str,
    input: &Value,
    caller: &Caller,
    wire: IdWire,
) -> Result<Value, Reject> {
    validate_read(facts, name, input, wire)?;
    let (res, ext) = name.split_once('.').ok_or(Reject { code: "BAD_REQUEST", msg: "확장은 `Resource.name`".into() })?;
    let x = &facts["resources"][res]["extensions"][ext];
    invoke_contract(Some(db), w, facts, x, input, caller, wire).await
}

pub fn validate_operation(facts: &Value, name: &str, input: &Value, wire: IdWire) -> Result<(), Reject> {
    let operation = &facts["operations"][name];
    if !operation.is_object() {
        return rej("NOT_EXPOSED", "등록된 operation이 없음");
    }
    if operation["kind"] != "read" || operation["effect"] != "none" || !operation["access"].as_object().is_some_and(|access| access.is_empty()) {
        return rej("UNSUPPORTED", "operation 실행 계약 불일치");
    }
    check_read_record(facts, &operation["input"], input, "입력", wire)
}

pub async fn invoke_operation(worker: &mut Worker, facts: &Value, name: &str, input: &Value, caller: &Caller, wire: IdWire) -> Result<Value, Reject> {
    validate_operation(facts, name, input, wire)?;
    invoke_contract(None, worker, facts, &facts["operations"][name], input, caller, wire).await
}

async fn invoke_contract(
    mut db: Option<&mut Client>,
    w: &mut Worker,
    facts: &Value,
    x: &Value,
    input: &Value,
    caller: &Caller,
    wire: IdWire,
) -> Result<Value, Reject> {
    let deadline = Duration::from_millis(x["deadlineMs"].as_u64().unwrap_or(1000));
    // 집계 입력은 확장 계약의 binding대로 확장 입력 값에 고정한다. worker가 다른 값을 넣으면 거부한다.
    let mut access = HashMap::new();
    for (agg, binding) in x["access"].as_object().unwrap() {
        let mut fixed = Map::new();
        for (param, src) in binding.as_object().unwrap() {
            let key = src.as_str().unwrap().trim_start_matches("input.");
            fixed.insert(param.clone(), input[key].clone());
        }
        access.insert(agg.clone(), fixed);
    }
    w.retire_previous();
    w.next_invoke += 1;
    let invoke_id = w.next_invoke;
    let token = format!("g{invoke_id}-{}", Instant::now().elapsed().as_nanos() ^ (invoke_id as u128).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let expires = Instant::now() + deadline;
    w.tokens.insert(token.clone(), Grant { actor_id: caller.actor_id, now: caller.now.clone(), access, id_wire: wire, expires, active: true });
    if let Err(error) =
        w.send_before(&json!({ "type": "invoke", "invoke": invoke_id, "impl": x["implementation"], "input": input, "token": token }), expires).await
    {
        w.retire(&token);
        return Err(error);
    }

    let result = loop {
        let left = expires.saturating_duration_since(Instant::now());
        let line = match tokio::time::timeout(left, w.read_line()).await {
            Err(_) => break rej("DEADLINE_EXCEEDED", format!("확장이 {}ms 안에 끝나지 않음", deadline.as_millis())),
            Ok(Err(_)) | Ok(Ok(None)) => {
                let tail = w.stderr_tail.lock().unwrap().last().cloned().unwrap_or_default();
                break rej("WORKER_FAILED", format!("worker가 종료됨. stderr: {tail}"));
            }
            Ok(Ok(Some(l))) => l,
        };
        let Ok(m) = serde_json::from_str::<Value>(&line) else {
            break rej("WORKER_FAILED", "worker 메시지 형식 오류");
        };
        match m["type"].as_str() {
            Some("call") => {
                let Some(db) = db.as_deref_mut() else {
                    break rej("ACCESS_NOT_DECLARED", "순수 operation은 ctx 호출을 허용하지 않음");
                };
                // ctx 작업도 호출 전체 기한 안에서만 기다린다. DB 대기 중 기한을 넘기면 호출 전체가 실패한다(F05).
                let left = expires.saturating_duration_since(Instant::now());
                let reply = match tokio::time::timeout(left, serve_call(db, w, facts, &m)).await {
                    Ok(r) => r,
                    Err(_) => break rej("DEADLINE_EXCEEDED", format!("확장 ctx 작업이 {}ms 안에 끝나지 않음", deadline.as_millis())),
                };
                let msg = match reply {
                    Ok(v) => json!({ "type": "reply", "call": m["call"], "value": v }),
                    Err(e) => json!({ "type": "reply", "call": m["call"], "error": { "code": e.code } }),
                };
                if let Err(e) = w.send_before(&msg, expires).await {
                    break Err(e);
                }
            }
            Some("done") if m["invoke"] == json!(invoke_id) => {
                if let Some(e) = m.get("error") {
                    break rej("EXTENSION_ERROR", format!("확장 실패: {}", e["code"].as_str().unwrap_or("?")));
                }
                // 기한 뒤 도착한 완료는 성공으로 전달하지 않는다(F05).
                if Instant::now() > expires {
                    break rej("DEADLINE_EXCEEDED", format!("확장이 {}ms 안에 끝나지 않음", deadline.as_millis()));
                }
                let out = m["output"].clone();
                break check_read_record(facts, &x["output"], &out, "출력", wire).map(|_| out);
            }
            // 이미 끝난 이전 호출의 늦은 done은 버린다.
            _ => continue,
        }
    };
    // 호출이 끝나면 성공·실패와 무관하게 토큰을 폐기한다.
    w.retire(&token);
    result
}

/// worker의 ctx 요청 하나를 처리한다. actor는 토큰에서만 온다.
async fn serve_call(db: &mut Client, w: &mut Worker, facts: &Value, m: &Value) -> Result<Value, Reject> {
    let o = m.as_object().unwrap();
    if let Some(k) = o.keys().find(|k| !["type", "call", "token", "op", "name", "input"].contains(&k.as_str())) {
        return rej("UNKNOWN_KEY", format!("ctx 요청에 알 수 없는 키 `{k}`"));
    }
    let token = m["token"].as_str().unwrap_or("");
    let Some(g) = w.tokens.get(token) else {
        if w.retired_tokens.iter().any(|retired| retired == token) {
            return rej("TOKEN_EXPIRED", "끝난 호출의 ctx");
        }
        return rej("TOKEN_INVALID", "알 수 없는 ctx 토큰");
    };
    if !g.active || Instant::now() > g.expires {
        return rej("TOKEN_EXPIRED", "끝났거나 기한이 지난 호출의 ctx");
    }
    if m["op"] != "aggregate" {
        return rej("UNSUPPORTED", "ctx는 aggregate만 지원");
    }
    let name = m["name"].as_str().unwrap_or("");
    let Some(fixed) = g.access.get(name) else {
        return rej("ACCESS_NOT_DECLARED", format!("확장 계약에 `{name}` 접근이 없음"));
    };
    if m["input"].as_object() != Some(fixed) {
        return rej("INPUT_BINDING", "집계 입력은 확장 입력에 묶인 값과 같아야 함");
    }
    let caller = Caller { actor_id: g.actor_id, now: g.now.clone() };
    let mut plan = spike_v2_read::plan::plan_read_with_wire(facts, &json!({ "aggregate": name, "input": fixed }), &caller, g.id_wire)?;
    // DB 문장 기한도 확장 호출의 남은 시간 이하로 줄인다.
    let left = g.expires.saturating_duration_since(Instant::now()).as_millis() as u64;
    plan.deadline_ms = plan.deadline_ms.min(left.max(1));
    let v = execute(db, &plan).await?;
    Ok(json!({ "value": v[0] }))
}
