//! V4-1 공식 읽기 확장. 서버가 actor·허용 집계·입력 바인딩·기한을 ctx 토큰에 묶고, worker는 토큰으로만 데이터를 요청한다.
mod cgroup;
mod framing;
mod isolation;
mod reap;
pub use cgroup::WorkerResourceLimits;
pub use isolation::{Isolation, WorkerLaunchConfig};

pub mod capability;
pub mod host;
pub mod outbox;
pub mod write;

pub use capability::{
    CapabilityPrincipal, MemoryMongoCapability, MemoryRedisCapability, MongoCapability, MongoDriverCapability, RedisCapability, RedisDriverCapability,
};
pub use host::{PgWorkerHost, RelationalWorkerHost, WorkerHost};

#[cfg(all(test, unix))]
mod cleanup_tests;
#[cfg(test)]
mod grant_tests;
#[cfg(test)]
mod value_tests;
#[cfg(test)]
mod stable_key_tests {
    use super::{rej, requires_stable_key, retry_after_ms, Reject, RETRY_AFTER_KEY, RETRY_LATER_MESSAGE};
    use serde_json::json;

    #[test]
    fn managed_effect_capabilities_require_stable_keys() {
        for op in ["http", "integrations", "operations", "jobs", "events", "messages"] {
            assert!(requires_stable_key(op, &json!({})), "{op} requires a stable key");
        }
        assert!(requires_stable_key("files", &json!({"action":"put"})));
        assert!(!requires_stable_key("files", &json!({"action":"get"})));
        assert!(!requires_stable_key("data", &json!({})));
    }

    #[test]
    fn the_wait_an_extension_asked_for_is_read_back_only_from_its_own_retry_request() {
        let ask = |code: &'static str, msg: &str| Reject { code, msg: msg.to_owned() };
        let asked: Result<(), Reject> = rej("WIRE.RETRY_LATER", format!("{RETRY_LATER_MESSAGE}; {RETRY_AFTER_KEY}2500"));
        assert_eq!(retry_after_ms(&asked.unwrap_err()), Some(2500));
        let plain: Result<(), Reject> = rej("WIRE.RETRY_LATER", RETRY_LATER_MESSAGE);
        assert_eq!(retry_after_ms(&plain.unwrap_err()), None);
        // A message that merely mentions the key, from any other rejection, is not a request.
        assert_eq!(retry_after_ms(&ask("WIRE.EXTENSION_ERROR", "afterMs=9")), None);
        assert_eq!(retry_after_ms(&ask("WIRE.RETRY_LATER", "afterMs=-1")), None);
    }
}

use framing::BoundedLines;
use serde_json::{json, Map, Value};
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

/// 읽기·operation 확장 하나가 한 호출 안에서 보낼 수 있는 ctx DB 호출 수.
const MAX_READ_CTX_CALLS: usize = 64;
const MAX_RETIRED_TOKENS: usize = 64;
const NODE_WORKER: &str = include_str!("../workers/worker.mjs");
const PYTHON_WORKER: &str = include_str!("../workers/worker.py");

pub fn worker_abi_digest() -> String {
    spike_v1_fixture::digest(&json!([NODE_WORKER, PYTHON_WORKER]))
}

pub struct Worker {
    child: Child,
    cgroup: Option<cgroup::CgroupGuard>,
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
    /// 세션 인증 출처가 검증한 actor 공개 id. Mongo capability 호출에서만 principal로 해석한다.
    actor_public_id: Option<String>,
    now: String,
    /// 확장 계약의 access 목록과 각 집계 입력에 묶인 확장 입력 값.
    access: HashMap<String, Map<String, Value>>,
    capability_grants: std::collections::HashSet<String>,
    id_wire: IdWire,
    expires: Instant,
    active: bool,
}

const RETRY_LATER_MESSAGE: &str = "확장이 job 재시도를 요청함";
const RETRY_AFTER_KEY: &str = "afterMs=";

/// The wait an extension asked for with `ctx.retryLater(reason, {afterMs})`, in milliseconds, when this rejection is its request.
pub fn retry_after_ms(reject: &Reject) -> Option<i64> {
    if reject.code != "WIRE.RETRY_LATER" {
        return None;
    }
    reject.msg.split_once(RETRY_AFTER_KEY).and_then(|(_, value)| value.parse::<u64>().ok()).and_then(|value| i64::try_from(value).ok())
}

pub(crate) fn rej<T>(code: &'static str, msg: impl Into<String>) -> Result<T, Reject> {
    Err(Reject { code, msg: msg.into() })
}

fn requires_stable_key(op: &str, input: &Value) -> bool {
    ["http", "integrations", "operations", "jobs", "events", "messages", "apply"].contains(&op) || op == "files" && input["action"] == "put"
}

/// Maps a worker ctx request to the declared job capability key.
/// Data reads and aggregates share `read.<Resource>`; transitions use `apply.<Resource>.<transition>`.
pub fn job_capability(op: &str, name: &str) -> String {
    match op {
        "read" => format!("read.{name}"),
        "aggregate" => format!("read.{}", name.split_once('.').map_or(name, |(resource, _)| resource)),
        _ => format!("{op}.{name}"),
    }
}

impl Worker {
    /// 기본 실행은 PATH만 전달하고 OS 네트워크 격리를 켠다.
    pub async fn start(lang: Lang, ext_dir: &str) -> Worker {
        Self::start_with(lang, ext_dir, Isolation::Auto).await
    }

    pub async fn start_with(lang: Lang, ext_dir: &str, iso: Isolation) -> Worker {
        Self::try_start_with(lang, ext_dir, iso, WorkerLimits::default()).await.expect("worker 실행")
    }

    pub async fn try_start_with(lang: Lang, ext_dir: &str, iso: Isolation, limits: WorkerLimits) -> std::io::Result<Worker> {
        let config = WorkerLaunchConfig { isolation: iso, ..WorkerLaunchConfig::default() };
        Self::try_start_configured_with_limits(lang, ext_dir, config, limits).await
    }

    /// Launch with a trusted explicit runtime and explicitly selected environment entries.
    pub async fn try_start_configured(lang: Lang, ext_dir: &str, config: WorkerLaunchConfig) -> std::io::Result<Worker> {
        Self::try_start_configured_with_limits(lang, ext_dir, config, WorkerLimits::default()).await
    }

    pub async fn try_start_configured_with_limits(
        lang: Lang,
        ext_dir: &str,
        config: WorkerLaunchConfig,
        limits: WorkerLimits,
    ) -> std::io::Result<Worker> {
        if limits.stdout_frame_bytes == 0 || limits.stderr_frame_bytes == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "worker frame limits must be positive"));
        }
        cgroup::validate_config(config.cgroup_root.as_deref(), config.resource_limits)?;
        config.isolation.validate_available()?;
        let prog = isolation::runtime_path(lang, &config)?;
        let ext_dir = isolation::extension_path(config.isolation, std::path::Path::new(ext_dir))?;
        let ext_dir = ext_dir.to_string_lossy();
        let mut runtime_args = Vec::new();
        match lang {
            Lang::Node => {
                runtime_args.extend(["--input-type=module".into(), "-e".into(), NODE_WORKER.into(), "--".into(), ext_dir.to_string().into()]);
            }
            Lang::Python => {
                runtime_args.extend(["-P".into(), "-c".into(), PYTHON_WORKER.into(), ext_dir.to_string().into()]);
            }
        }
        let spec = isolation::command_spec(
            config.isolation,
            prog.as_os_str(),
            &runtime_args,
            std::path::Path::new(ext_dir.as_ref()),
            &config.read_only_paths,
        )?;
        let mut cmd = Command::new(spec.program);
        cmd.args(spec.args);
        isolation::configure_environment(&mut cmd, &config)?;
        isolation::set_working_directory(&mut cmd, config.isolation, std::path::Path::new(ext_dir.as_ref()));
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        // Own process group so teardown can signal the worker's whole tree, not just the direct child.
        #[cfg(unix)]
        cmd.process_group(0);
        let cgroup = cgroup::create(config.cgroup_root.as_deref(), config.resource_limits)?;
        if let Some(cgroup) = &cgroup {
            cgroup.attach(&mut cmd)?;
        }
        let mut child = cmd.spawn().map_err(|error| std::io::Error::new(error.kind(), format!("worker 실행 실패: {error}")))?;
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
        Ok(Worker { child, cgroup, stderr_tail, stdin, lines, stderr_task, next_invoke: 0, tokens: HashMap::new(), retired_tokens: VecDeque::new() })
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
            self.kill_all();
        }
        result
    }

    pub(crate) async fn send(&mut self, m: &Value) -> Result<(), Reject> {
        if self.lines.is_failed() {
            return rej("WIRE.WORKER_FAILED", "worker protocol이 폐기됨");
        }
        let mut s = m.to_string();
        s.push('\n');
        self.stdin.write_all(s.as_bytes()).await.map_err(|_| Reject { code: "WIRE.WORKER_FAILED", msg: "worker 입력 실패".into() })
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
        self.kill_all();
        rej("WIRE.DEADLINE_EXCEEDED", "worker 입력 전송이 기한 안에 끝나지 않음")
    }

    /// Kills the worker, its descendants and its cgroup. Safe to call repeatedly.
    fn kill_all(&mut self) {
        if let Some(pid) = self.child.id() {
            reap::kill_tree(pid);
        }
        let _ = self.child.start_kill();
        if let Some(cgroup) = &self.cgroup {
            let _ = cgroup.kill();
        }
    }

    pub async fn stop(mut self) {
        if let Some(pid) = self.child.id() {
            reap::kill_tree(pid);
        }
        if let Some(cgroup) = &self.cgroup {
            if let Err(error) = cgroup.kill() {
                eprintln!("worker cgroup kill failed during stop: {error}");
            }
        }
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        if let Some(cgroup) = self.cgroup.take() {
            if let Err(error) = cgroup.terminate().await {
                eprintln!("worker cgroup cleanup failed during stop: {error}");
            }
        }
        self.stderr_task.abort();
        let _ = (&mut self.stderr_task).await;
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // `kill_on_drop` only signals the direct child; grandchildren need the tree kill.
        if let Some(pid) = self.child.id() {
            reap::kill_tree(pid);
        }
        self.stderr_task.abort();
    }
}

/// V1이 받는 입출력 타입 전체를 같은 규칙으로 검사한다(F09). 지원하지 않는 타입은 항상 거부.
fn check_typed(facts: &Value, ty: &str, v: &Value) -> bool {
    let nullable = ty.ends_with('?');
    let base = ty.trim_end_matches('?');
    match (base, v) {
        (_, Value::Null) => nullable,
        (t, Value::String(s)) if t.starts_with("Id<") || t.starts_with("Ref<") => {
            if spike_v2_read::id_wire::id_cast(facts, t) == "uuid" {
                spike_v2_read::id_wire::parse_uuid(s).is_ok()
            } else {
                !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
            }
        }
        // Worker payloads keep their spelling; scalar parsing only validates the shared domain.
        _ => spike_v2_read::scalar::parse(facts, base, v).is_ok(),
    }
}

/// 선언된 입출력과 정확히 같은 키·타입인지. 모르는 키도 거부한다.
pub(crate) fn check_record(facts: &Value, decl: &Value, v: &Value, what: &str) -> Result<(), Reject> {
    let o = v
        .as_object()
        .ok_or(Reject { code: if what == "출력" { "WIRE.OUTPUT_INVALID" } else { "WIRE.BAD_VALUE" }, msg: format!("{what}은 객체") })?;
    let code = if what == "출력" { "WIRE.OUTPUT_INVALID" } else { "WIRE.BAD_VALUE" };
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
    let code = if output { "WIRE.OUTPUT_INVALID" } else { "WIRE.BAD_VALUE" };
    if value.is_null() {
        return if ty.ends_with('?') { Ok(()) } else { rej(code, format!("{ty} null 불가")) };
    }
    if base.starts_with("Id<") || base.starts_with("Ref<") {
        if spike_v2_read::id_wire::id_cast(facts, base) == "uuid" {
            return spike_v2_read::id_wire::parse_typed_id(facts, base, value, wire).map(|_| ()).map_err(|e| Reject { code, msg: e.msg });
        }
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
    let code = if what == "출력" { "WIRE.OUTPUT_INVALID" } else { "WIRE.BAD_VALUE" };
    for field in decl.as_array().unwrap() {
        spike_v2_read::scalar::validate_range(field, &value[field[0].as_str().unwrap()], code)?;
    }
    Ok(())
}

fn check_read_record_types(facts: &Value, decl: &Value, value: &Value, what: &str, wire: IdWire) -> Result<(), Reject> {
    if wire == IdWire::Legacy {
        return check_record(facts, decl, value, what);
    }
    let object = value
        .as_object()
        .ok_or(Reject { code: if what == "출력" { "WIRE.OUTPUT_INVALID" } else { "WIRE.BAD_VALUE" }, msg: format!("{what}은 객체") })?;
    let output = what == "출력";
    let code = if output { "WIRE.OUTPUT_INVALID" } else { "WIRE.BAD_VALUE" };
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
    let (resource, extension) = name.split_once('.').ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "확장은 `Resource.name`".into() })?;
    let x = &facts["resources"][resource]["extensions"][extension];
    if x.is_null() {
        return rej("WIRE.NOT_EXPOSED", format!("확장 `{name}` 없음"));
    }
    if x["kind"] != "read" || x["effect"] != "none" {
        return rej("WIRE.UNSUPPORTED", "V4-1은 effect none 읽기 확장만");
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
    let mut host = host::PgWorkerHost { client: db };
    invoke_with_host(&mut host, w, facts, name, input, caller, wire).await
}

pub async fn invoke_with_host(
    host: &mut dyn WorkerHost,
    w: &mut Worker,
    facts: &Value,
    name: &str,
    input: &Value,
    caller: &Caller,
    wire: IdWire,
) -> Result<Value, Reject> {
    invoke_with_host_as(host, w, facts, name, input, caller, wire, None).await
}

/// `invoke_with_host` with the session's server-verified public actor id, so a Uuid actor owns its private Mongo rows.
/// The id comes from the session authentication source only; `Caller` and extension input stay unchanged.
#[allow(clippy::too_many_arguments)]
pub async fn invoke_with_host_as(
    host: &mut dyn WorkerHost,
    w: &mut Worker,
    facts: &Value,
    name: &str,
    input: &Value,
    caller: &Caller,
    wire: IdWire,
    actor_public_id: Option<&str>,
) -> Result<Value, Reject> {
    validate_read(facts, name, input, wire)?;
    let (res, ext) = name.split_once('.').ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "확장은 `Resource.name`".into() })?;
    let x = &facts["resources"][res]["extensions"][ext];
    invoke_contract(Some(host), w, facts, x, input, caller, wire, actor_public_id, None).await
}

pub fn validate_operation(facts: &Value, name: &str, input: &Value, wire: IdWire) -> Result<(), Reject> {
    let operation = &facts["operations"][name];
    if !operation.is_object() {
        return rej("WIRE.NOT_EXPOSED", "등록된 operation이 없음");
    }
    if operation["kind"] != "read" || operation["effect"] != "none" || !operation["access"].as_object().is_some_and(|access| access.is_empty()) {
        return rej("WIRE.UNSUPPORTED", "operation 실행 계약 불일치");
    }
    check_read_record(facts, &operation["input"], input, "입력", wire)
}

pub async fn invoke_operation(worker: &mut Worker, facts: &Value, name: &str, input: &Value, caller: &Caller, wire: IdWire) -> Result<Value, Reject> {
    validate_operation(facts, name, input, wire)?;
    invoke_contract(None, worker, facts, &facts["operations"][name], input, caller, wire, None, None).await
}

pub trait JobHost: Send {
    fn call<'a>(
        &'a mut self,
        operation: &'a str,
        name: &'a str,
        input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, Reject>> + Send + 'a>>;
    /// Run identity the extension sees as `ctx.job` (e.g. `{id, attempt}`), so a retry can tell itself apart.
    fn job(&self) -> Value {
        Value::Null
    }
}

pub fn validate_job(facts: &Value, name: &str, input: &Value, wire: IdWire) -> Result<(), Reject> {
    let job = &facts["operations"][name];
    if job["kind"] != "job" {
        return rej("WIRE.NOT_EXPOSED", "등록된 job이 없음");
    }
    if job["effect"] != "none" && job["effect"] != "managed" {
        return rej("WIRE.UNSUPPORTED", "job 효과 계약 불일치");
    }
    check_read_record(facts, &job["input"], input, "입력", wire)
}

pub async fn invoke_job(
    worker: &mut Worker,
    facts: &Value,
    name: &str,
    input: &Value,
    caller: &Caller,
    wire: IdWire,
    host: &mut dyn JobHost,
) -> Result<Value, Reject> {
    validate_job(facts, name, input, wire)?;
    invoke_contract(None, worker, facts, &facts["operations"][name], input, caller, wire, None, Some(host)).await
}

#[allow(clippy::too_many_arguments)]
async fn invoke_contract(
    mut host: Option<&mut dyn WorkerHost>,
    w: &mut Worker,
    facts: &Value,
    x: &Value,
    input: &Value,
    caller: &Caller,
    wire: IdWire,
    actor_public_id: Option<&str>,
    mut job_host: Option<&mut dyn JobHost>,
) -> Result<Value, Reject> {
    let deadline = Duration::from_millis(x["deadlineMs"].as_u64().unwrap_or(1000));
    // 집계 입력은 확장 계약의 binding대로 확장 입력 값에 고정한다. worker가 다른 값을 넣으면 거부한다.
    let mut access = HashMap::new();
    let mut capability_grants = std::collections::HashSet::new();
    for (agg, binding) in x["access"].as_object().unwrap() {
        if agg.starts_with("mongo.") || agg.starts_with("redis.") {
            if binding.as_object().is_none_or(|map| !map.is_empty()) {
                return rej("WIRE.BAD_VALUE", "mongo/redis capability grant must be an empty object");
            }
            capability_grants.insert(agg.clone());
        }
        // 읽기 확장의 `ctx.data.read(Resource)` 권한. managed job과 같은 `"read.<Resource>": true` 표기만 받고 다른 값은 거부한다.
        // 현재 언어는 읽기 확장에서 이 항목을 선언할 수 없어 수동 facts에만 존재할 수 있다.
        if job_host.is_none() && agg.starts_with("read.") {
            if binding != &Value::Bool(true) {
                return rej("WIRE.BAD_VALUE", "read capability grant must be `true`");
            }
            capability_grants.insert(agg.clone());
            continue;
        }
        if job_host.is_some() {
            continue;
        }
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
    let token = grant_token('g', invoke_id);
    let expires = Instant::now() + deadline;
    w.tokens.insert(
        token.clone(),
        Grant {
            actor_id: caller.actor_id,
            actor_public_id: actor_public_id.map(str::to_string),
            now: caller.now.clone(),
            access,
            capability_grants,
            id_wire: wire,
            expires,
            active: true,
        },
    );
    let mut invoke = json!({ "type": "invoke", "invoke": invoke_id, "impl": x["implementation"], "input": input, "token": token });
    if let Some(jh) = job_host.as_deref() {
        invoke["job"] = jh.job();
    }
    if let Err(error) = w.send_before(&invoke, expires).await {
        w.retire(&token);
        return Err(error);
    }

    let mut managed_calls = 0;
    let mut db_calls = 0usize;
    let result = loop {
        let left = expires.saturating_duration_since(Instant::now());
        let line = match tokio::time::timeout(left, w.read_line()).await {
            Err(_) => break rej("WIRE.DEADLINE_EXCEEDED", format!("확장이 {}ms 안에 끝나지 않음", deadline.as_millis())),
            Ok(Err(_)) | Ok(Ok(None)) => {
                let tail = w.stderr_tail.lock().unwrap().last().cloned().unwrap_or_default();
                break rej("WIRE.WORKER_FAILED", format!("worker가 종료됨. stderr: {tail}"));
            }
            Ok(Ok(Some(l))) => l,
        };
        let Ok(m) = serde_json::from_str::<Value>(&line) else {
            break rej("WIRE.WORKER_FAILED", "worker 메시지 형식 오류");
        };
        match m["type"].as_str() {
            Some("call") => {
                if job_host.is_none() && host.is_none() {
                    break rej("WIRE.ACCESS_NOT_DECLARED", "순수 operation은 ctx 호출을 허용하지 않음");
                }
                let left = expires.saturating_duration_since(Instant::now());
                let call = async {
                    if let Some(jh) = job_host.as_deref_mut() {
                        managed_calls += 1;
                        if managed_calls > 64 {
                            return rej("WIRE.COST_EXCEEDED", "job capability 호출 상한");
                        }
                        let op = m["op"].as_str().unwrap_or("");
                        let name = m["name"].as_str().unwrap_or("");
                        if m["token"] != token || x["access"][job_capability(op, name)] != true {
                            return rej("WIRE.ACCESS_NOT_DECLARED", "선언되지 않은 job capability");
                        }
                        if requires_stable_key(op, &m["input"])
                            && !m["input"]["key"].as_str().is_some_and(|key| {
                                !key.is_empty()
                                    && key.len() <= 100
                                    && key.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b':'))
                            })
                        {
                            return rej("WIRE.BAD_VALUE", "managed effect에는 안정적인 key가 필요함");
                        }
                        jh.call(op, name, &m["input"]).await
                    } else if let Some(h) = host.as_mut() {
                        // 읽기·operation 확장도 deadline 안에서 DB 호출을 무한히 보낼 수 없게 한다(쓰기 20, job 64와 같은 취지).
                        db_calls += 1;
                        if db_calls > MAX_READ_CTX_CALLS {
                            return rej("WIRE.CALL_LIMIT", "ctx 호출 상한");
                        }
                        serve_call(*h, w, facts, &m).await
                    } else {
                        rej("WIRE.ACCESS_NOT_DECLARED", "순수 operation은 ctx 호출을 허용하지 않음")
                    }
                };
                let reply = match tokio::time::timeout(left, call).await {
                    Ok(r) => r,
                    Err(_) => break rej("WIRE.DEADLINE_EXCEEDED", format!("확장 ctx 작업이 {}ms 안에 끝나지 않음", deadline.as_millis())),
                };
                if reply.as_ref().is_err_and(|e| e.code == "WIRE.CALL_LIMIT") {
                    break reply;
                }
                if job_host.is_some()
                    && reply.as_ref().is_err_and(|e| {
                        [
                            "WIRE.ACCESS_NOT_DECLARED",
                            "WIRE.COST_EXCEEDED",
                            "WIRE.LEASE_LOST",
                            "WIRE.EFFECT_UNKNOWN",
                            "WIRE.COMMIT_UNKNOWN",
                            "WIRE.DB_UNAVAILABLE",
                            "WIRE.BAD_VALUE",
                        ]
                        .contains(&e.code)
                    })
                {
                    break reply;
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
                if let Some(e) = m.get("error") {
                    // Only a durable job can be retried, so only there does the explicit request keep its code.
                    if job_host.is_some() && e["code"] == "WIRE.RETRY_LATER" {
                        // The extension may choose the wait (`afterMs`); it rides in the message because `Reject` is shared by every path.
                        break match e["afterMs"].as_u64() {
                            Some(after_ms) => rej("WIRE.RETRY_LATER", format!("{RETRY_LATER_MESSAGE}; {RETRY_AFTER_KEY}{after_ms}")),
                            None => rej("WIRE.RETRY_LATER", RETRY_LATER_MESSAGE),
                        };
                    }
                    break rej("WIRE.EXTENSION_ERROR", format!("확장 실패: {}", e["code"].as_str().unwrap_or("?")));
                }
                // 기한 뒤 도착한 완료는 성공으로 전달하지 않는다(F05).
                if Instant::now() > expires {
                    break rej("WIRE.DEADLINE_EXCEEDED", format!("확장이 {}ms 안에 끝나지 않음", deadline.as_millis()));
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

/// ctx grant 토큰. 확장 코드가 다른 호출의 grant를 추측하지 못하도록 OS 난수 128비트를 쓴다.
pub(crate) fn grant_token(kind: char, invoke_id: u64) -> String {
    let mut bytes = [0u8; 16];
    // SAFETY: getentropy는 256바이트 이하 버퍼를 채운다. 실패하면 난수 없는 토큰을 쓰지 않고 중단한다.
    let rc = unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) };
    assert!(rc == 0, "OS 난수를 얻지 못함");
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{kind}{invoke_id}-{hex}")
}

/// worker의 ctx 요청 하나를 처리한다. actor는 토큰에서만 온다.
async fn serve_call(host: &mut dyn WorkerHost, w: &mut Worker, facts: &Value, m: &Value) -> Result<Value, Reject> {
    let o = m.as_object().unwrap();
    if let Some(k) = o.keys().find(|k| !["type", "call", "token", "op", "name", "input"].contains(&k.as_str())) {
        return rej("WIRE.UNKNOWN_KEY", format!("ctx 요청에 알 수 없는 키 `{k}`"));
    }
    let token = m["token"].as_str().unwrap_or("");
    let Some(g) = w.tokens.get(token) else {
        if w.retired_tokens.iter().any(|retired| retired == token) {
            return rej("WIRE.TOKEN_EXPIRED", "끝난 호출의 ctx");
        }
        return rej("WIRE.TOKEN_INVALID", "알 수 없는 ctx 토큰");
    };
    if !g.active || Instant::now() > g.expires {
        return rej("WIRE.TOKEN_EXPIRED", "끝났거나 기한이 지난 호출의 ctx");
    }

    let op = m["op"].as_str().unwrap_or("");
    let name = m["name"].as_str().unwrap_or("");
    let left = g.expires.saturating_duration_since(Instant::now()).as_millis() as u64;

    match op {
        "aggregate" => {
            let Some(fixed) = g.access.get(name) else {
                return rej("WIRE.ACCESS_NOT_DECLARED", format!("확장 계약에 `{name}` 접근이 없음"));
            };
            if m["input"].as_object() != Some(fixed) {
                return rej("WIRE.INPUT_BINDING", "집계 입력은 확장 입력에 묶인 값과 같아야 함");
            }
            let caller = Caller { actor_id: g.actor_id, now: g.now.clone() };
            let mut plan =
                spike_v2_read::plan::plan_read_in(host.dialect(), facts, &json!({ "aggregate": name, "input": fixed }), &caller, g.id_wire)?;
            // DB 문장 기한도 확장 호출의 남은 시간 이하로 줄인다.
            plan.deadline_ms = plan.deadline_ms.min(left.max(1));
            let v = host.execute(&plan, g.id_wire).await?;
            Ok(json!({ "value": v[0] }))
        }
        "read" => {
            if !g.capability_grants.contains(&job_capability("read", name)) {
                return rej("WIRE.ACCESS_NOT_DECLARED", format!("확장 계약에 `read.{name}` 접근이 없음"));
            }
            let query = &m["input"];
            if !query.is_object() {
                return rej("WIRE.BAD_VALUE", "read query는 객체여야 함");
            }
            if query.get("read").and_then(|v| v.as_str()) != Some(name) {
                return rej("WIRE.BAD_VALUE", "query의 `read` 대상이 일치하지 않음");
            }
            if !facts["resources"].get(name).is_some_and(Value::is_object) {
                return rej("WIRE.NOT_EXPOSED", format!("등록되지 않은 resource `{name}`"));
            }
            let caller = Caller { actor_id: g.actor_id, now: g.now.clone() };
            let mut plan = spike_v2_read::plan::plan_read_in(host.dialect(), facts, query, &caller, g.id_wire)?;
            plan.deadline_ms = plan.deadline_ms.min(left.max(1));
            let rows = host.execute(&plan, g.id_wire).await?;
            let encoded = spike_v2_read::id_wire::encode_rows(rows, &plan.output_type, g.id_wire)?;
            Ok(json!({ "rows": encoded }))
        }
        "mongo" => {
            let actor_uuid = capability::resolve_actor_uuid(g.actor_id, g.actor_public_id.as_deref())?;
            let context = CapabilityPrincipal { actor_id: g.actor_id, actor_uuid, grants: g.capability_grants.clone() };
            host.mongo(&context, name, &m["input"]).await
        }
        "redis" => {
            let context = CapabilityPrincipal { actor_id: g.actor_id, actor_uuid: None, grants: g.capability_grants.clone() };
            host.redis(&context, name, &m["input"]).await
        }
        "apply" => rej("WIRE.ACCESS_DENIED", "읽기 확장에서 쓰기(apply) 호출 거부"),
        _ => rej("WIRE.UNSUPPORTED", format!("지원하지 않는 ctx op `{op}`")),
    }
}
