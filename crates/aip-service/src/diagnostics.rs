//! How `aip service` reports a failure or a check result: the JSON that goes to stdout or stderr.
//!
//! Three shapes leave the process. A rejected definition is `{"ok":false,"code":"DEF.INVALID_DEFINITION","diagnostics":[..]}`
//! (exit 1, stderr). Any other failure is `{"ok":false,"code":"<CODE>","message":".."}` with an optional `reason` (exit 1,
//! stderr). A check that passes is `{"ok":true,"executionDigest":..,"advisories":[..],"workers":{..}}` (exit 0, stdout).
//! The field-by-field contract of the diagnostics is `plan-docs/stabilization-20261122/diagnostics-fields.md`.
//! Everything here only builds values; the exit code and the stream are chosen by `execute`.

use serde_json::{Value, json};
use spike_v1_fixture::diag::Diag;
use spike_v11_dev_checks::{Advisory, Checked};

/// A failure that is not a rejected definition. `code` has no area prefix (`SOURCE_IO`, `UNSUPPORTED_FORM`).
pub(crate) fn error(code: &str, message: &str) -> Value {
    json!({"ok":false,"code":code,"message":message})
}

/// `error` plus a stable `reason` for a code that covers several kinds of failure. A tool branches on `reason`; the `message`
/// is only for a person. `reason` is additive: `code` and `message` keep the values they always had.
pub(crate) fn error_with_reason(code: &str, reason: &str, message: &str) -> Value {
    json!({"ok":false,"code":code,"reason":reason,"message":message})
}

/// The answer to a definition the checker rejected.
pub(crate) fn definition_rejected(diagnostics: &[Diag]) -> Value {
    json!({"ok":false,"code":"DEF.INVALID_DEFINITION","diagnostics":diagnostics.iter().map(Diag::json).collect::<Vec<_>>()})
}

/// One advisory. `severity`, `code`, `anchor` and `message` are the historical fields. `line` and `column` (1-based, the
/// same coordinates as an error) appear when the compiler reported a position; the advisories of `--dev-checks` point at a
/// declaration (`predicate:x`, `resource:X`) and have none. `explain` is the command that prints what the code means.
pub(crate) fn advisory_json(advisory: &Advisory) -> Value {
    let mut m = json!({"severity":"warning","code":advisory.code,"anchor":advisory.anchor,"message":advisory.msg});
    if let Some(span) = advisory.span {
        m["line"] = json!(span.line);
        m["column"] = json!(span.col);
    }
    if spike_v1_fixture::codes::lookup(advisory.code).is_some() {
        m["explain"] = json!(format!("aip explain-code {}", advisory.code));
    }
    m
}

/// The answer to `check` for an accepted definition.
pub(crate) fn check_report(checked: &Checked, workers: Vec<Value>) -> Value {
    json!({
        "ok": true,
        "executionDigest": spike_v1_fixture::digest(&checked.output.execution),
        "advisories": checked.advisories.iter().map(advisory_json).collect::<Vec<_>>(),
        "workers": {"required": !workers.is_empty(), "extensions": workers},
    })
}

/// The error of a listener that did not start. Only the server's own validation sentences (`InvalidInput`, `TimedOut`)
/// are fixed text that names a setting and never a value; every other io error is described by its kind, because it can
/// carry an address, a path or the output of a worker runtime.
pub(crate) fn server_start_error(e: &std::io::Error) -> Value {
    use std::io::ErrorKind;
    let message = match e.kind() {
        ErrorKind::InvalidInput | ErrorKind::TimedOut => format!("서버 시작 설정 검증 실패: {e}"),
        ErrorKind::AddrInUse => "listen 주소가 이미 사용 중임(listen 설정의 포트를 바꾸거나 다른 프로세스를 종료)".to_owned(),
        ErrorKind::AddrNotAvailable => "listen 주소를 이 호스트에서 쓸 수 없음(listen 설정의 주소 확인)".to_owned(),
        ErrorKind::PermissionDenied => {
            "권한 거부로 listener 또는 worker 런타임을 시작하지 못함(포트 권한, 런타임 실행 권한, 격리 설정 확인)".to_owned()
        }
        ErrorKind::NotFound => "worker 런타임 실행 파일을 찾지 못함(workers.language의 node 또는 python3가 PATH에 있어야 함)".to_owned(),
        kind => format!(
            "서버 시작 준비 단계 실패(io {kind}): worker 런타임 점검, rate limit·outbox·audit 저장소 설치, listener 바인딩 중 하나이며 DB 연결과 권한을 먼저 확인"
        ),
    };
    error("SERVER_CONFIG", &message)
}

/// The error of a JSON document that does not parse into its type, with the reason and the position. The
/// reason of serde names keys and types (`unknown field`, `missing field`, `invalid type`), which a person
/// or an AI needs to repair the file; the value that was wrong is dropped, because a configuration file can
/// hold a secret and the output of a failed start is logged.
pub(crate) fn json_error(code: &str, what: &str, e: &serde_json::Error) -> Value {
    let reason = redact_json_reason(&e.to_string());
    error(code, &format!("{what}: {reason} (줄 {}, 열 {})", e.line(), e.column()))
}

pub(crate) fn redact_json_reason(full: &str) -> String {
    // serde_json appends " at line L column C"; the position is reported on its own.
    let msg = full.rsplit_once(" at line ").map_or(full, |(m, _)| m);
    let structural =
        ["unknown field", "missing field", "unknown variant", "duplicate field", "invalid length", "EOF while", "trailing characters", "expected "];
    if let Some(rest) = msg.strip_prefix("invalid type: ").or_else(|| msg.strip_prefix("invalid value: ")) {
        // `invalid type: string "abc", expected u64` -> `invalid type: string, expected u64`
        let kind = rest.split([' ', ',']).next().unwrap_or("");
        let expected = rest.split_once(", expected ").map_or("", |(_, x)| x);
        let label = if msg.starts_with("invalid type") { "invalid type" } else { "invalid value" };
        return format!("{label}: {kind}, expected {expected}");
    }
    // The value of an unknown variant is what was typed; the list of the accepted ones is what repairs it.
    if let Some((_, accepted)) = msg.strip_prefix("unknown variant ").and_then(|rest| rest.split_once(", expected ")) {
        return format!("unknown variant, expected {accepted}");
    }
    if structural.iter().any(|p| msg.starts_with(p)) {
        return msg.to_string();
    }
    // A message written by a `Deserialize` implementation may quote the value: say only that it is wrong.
    "a value is not accepted".to_string()
}
