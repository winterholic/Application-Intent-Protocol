//! Registry of the codes the SDKs raise themselves (TypeScript client in `client/`, Python SDK in `product/python`):
//! failures that the server did not send, such as a response that does not match the contract or a lost connection.
//!
//! An SDK error carries one `code`. A rejection the server sent keeps its `WIRE.*` code (`wire_codes.rs`); the codes
//! here have the `CLIENT.` area so that a caller can tell at a glance which side produced the failure and never has to
//! guess whether `PROTOCOL_ERROR` means the server or the library. `aip explain-code` prints an entry and
//! `spec/client-errors.md` is generated from it; `tests/client_codes.rs` scans the SDK sources for leftovers.

use crate::wire_codes::Retry;

#[derive(Debug, Clone, Copy)]
pub struct Info {
    pub code: &'static str,
    pub retry: Retry,
    pub explain: &'static str,
    pub fix: &'static str,
}

const fn c(code: &'static str, retry: Retry, explain: &'static str, fix: &'static str) -> Info {
    Info { code, retry, explain, fix }
}

pub fn lookup(code: &str) -> Option<&'static Info> {
    REGISTRY.iter().find(|c| c.code == code)
}

pub fn all() -> &'static [Info] {
    REGISTRY
}

pub fn render_text(c: &Info) -> String {
    format!("{}  client error  retry: {}\n\nwhy:\n  {}\n\nfix:\n  {}\n", c.code, c.retry.name(), c.explain, c.fix)
}

/// `spec/client-errors.md`, generated; `tests/client_codes.rs` keeps it equal to the registry.
pub fn render_markdown() -> String {
    let mut out = String::from(
        "# SDK 오류 코드\n\n\
         > 이 문서는 `spikes/spike-v6-transport/src/client_codes.rs`의 레지스트리에서 생성된다. 손으로 고치지 않는다. 다시 만들려면 `AIP_UPDATE_GOLDEN=1 cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test client_codes`를 실행한다.\n\n\
         TypeScript 클라이언트와 Python SDK가 서버 응답과 무관하게 스스로 올리는 오류의 `code`다. 서버가 보낸 거부는 `WIRE.*` 코드를 그대로 가지므로(`wire-errors.md`), 오류 하나의 `code`로 어느 쪽이 실패를 만들었는지 알 수 있다.\n\n\
         모든 AIP 오류 코드는 `영역.이름` 형식이다. 이 문서의 영역은 `CLIENT`이고, 정의 검사기의 `DEF` 코드는 [definition-diagnostics.md](definition-diagnostics.md), 제품 HTTP API의 `WIRE` 코드는 [wire-errors.md](wire-errors.md), PoC의 `POC` 코드는 [diagnostics.md](diagnostics.md)에 있다. 2026-10-13 이전에는 접두 없는 이름(`PROTOCOL_ERROR`)이었다. 조회: `aip explain-code <CODE>`.\n\n\
         `retry` 값의 뜻은 `wire-errors.md`와 같다: never(같은 호출은 같은 이유로 다시 실패한다), later(시간이 지나면 성공할 수 있다), check-first(무슨 일이 있었는지 확인한 뒤에만 다시 보낸다).\n",
    );
    for c in REGISTRY {
        out.push_str(&format!("\n### `{}`\n\n`retry` {}\n\n- 원인: {}\n- 수정: {}\n", c.code, c.retry.name(), c.explain, c.fix));
    }
    out
}

use Retry::{Check, Later, Never};

pub static REGISTRY: &[Info] = &[
    c("CLIENT.AUTH_TIMEOUT", Later, "The server did not acknowledge the authentication message of a WebSocket connection within `authTimeoutMs`; the SDK closed the connection.", "Open the subscription again; raise `authTimeoutMs` if the network is slow."),
    c("CLIENT.NETWORK_ERROR", Later, "The connection failed before a response arrived (a WebSocket `error` event).", "Open the connection again after a short delay. A write keeps its idempotency key."),
    c("CLIENT.NOT_DURABLE", Never, "`enqueue` was called for an operation whose lifetime is not `durable`, so the server keeps no job for it.", "Call the operation directly, or declare it `durable` in the definition and regenerate the client."),
    c("CLIENT.OFFLINE", Later, "A transport adapter reported that the device has no network. The SDK treats it as retryable for event streams.", "Wait until the device is online; the event stream resumes from its cursor."),
    c("CLIENT.PROTOCOL_ERROR", Never, "A response does not match the contract the client was generated from: it is not JSON, a success marker is wrong, a required key is missing or a value has the wrong type. The SDK refuses to hand it to the caller.", "Regenerate the client from the deployed definition (`aip service gen`); if it persists, report the `x-request-id` of the response, because the server or a proxy in between answered with something that is not the AIP wire."),
    c("CLIENT.SCOPE_CONFLICT", Never, "A write that is not settled yet belongs to another user session; moving it to this session would let one user's request be sent with another's identity.", "Settle or discard the pending write of the previous session before signing in as a different user."),
    c("CLIENT.SERVICE_UNAVAILABLE", Later, "A transport adapter reported that the service cannot be reached; the SDK treats it as retryable for event streams.", "Retry after a short delay."),
    c("CLIENT.WRITE_UNSETTLED", Check, "A write may or may not have committed: the connection broke after the request was sent. The SDK keeps the key and the body so the same request can be sent again.", "Call `retry_pending()` (or send the same request with the same idempotency key); the server answers with the recorded result. Never send it again with a new key."),
];
