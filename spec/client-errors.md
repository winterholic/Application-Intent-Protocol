# SDK 오류 코드

> 이 문서는 `spikes/spike-v6-transport/src/client_codes.rs`의 레지스트리에서 생성된다. 손으로 고치지 않는다. 다시 만들려면 `AIP_UPDATE_GOLDEN=1 cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test client_codes`를 실행한다.

TypeScript 클라이언트와 Python SDK가 서버 응답과 무관하게 스스로 올리는 오류의 `code`다. 서버가 보낸 거부는 `WIRE.*` 코드를 그대로 가지므로(`wire-errors.md`), 오류 하나의 `code`로 어느 쪽이 실패를 만들었는지 알 수 있다.

모든 AIP 오류 코드는 `영역.이름` 형식이다. 이 문서의 영역은 `CLIENT`이고, 정의 검사기의 `DEF` 코드는 [definition-diagnostics.md](definition-diagnostics.md), 제품 HTTP API의 `WIRE` 코드는 [wire-errors.md](wire-errors.md), PoC의 `POC` 코드는 [diagnostics.md](diagnostics.md)에 있다. 2026-10-13 이전에는 접두 없는 이름(`PROTOCOL_ERROR`)이었다. 조회: `aip explain-code <CODE>`.

`retry` 값의 뜻은 `wire-errors.md`와 같다: never(같은 호출은 같은 이유로 다시 실패한다), later(시간이 지나면 성공할 수 있다), check-first(무슨 일이 있었는지 확인한 뒤에만 다시 보낸다).

### `CLIENT.AUTH_TIMEOUT`

`retry` later

- 원인: The server did not acknowledge the authentication message of a WebSocket connection within `authTimeoutMs`; the SDK closed the connection.
- 수정: Open the subscription again; raise `authTimeoutMs` if the network is slow.

### `CLIENT.NETWORK_ERROR`

`retry` later

- 원인: The connection failed before a response arrived (a WebSocket `error` event).
- 수정: Open the connection again after a short delay. A write keeps its idempotency key.

### `CLIENT.NOT_DURABLE`

`retry` never

- 원인: `enqueue` was called for an operation whose lifetime is not `durable`, so the server keeps no job for it.
- 수정: Call the operation directly, or declare it `durable` in the definition and regenerate the client.

### `CLIENT.OFFLINE`

`retry` later

- 원인: A transport adapter reported that the device has no network. The SDK treats it as retryable for event streams.
- 수정: Wait until the device is online; the event stream resumes from its cursor.

### `CLIENT.PROTOCOL_ERROR`

`retry` never

- 원인: A response does not match the contract the client was generated from: it is not JSON, a success marker is wrong, a required key is missing or a value has the wrong type. The SDK refuses to hand it to the caller.
- 수정: Regenerate the client from the deployed definition (`aip service gen`); if it persists, report the `x-request-id` of the response, because the server or a proxy in between answered with something that is not the AIP wire.

### `CLIENT.SCOPE_CONFLICT`

`retry` never

- 원인: A write that is not settled yet belongs to another user session; moving it to this session would let one user's request be sent with another's identity.
- 수정: Settle or discard the pending write of the previous session before signing in as a different user.

### `CLIENT.SERVICE_UNAVAILABLE`

`retry` later

- 원인: A transport adapter reported that the service cannot be reached; the SDK treats it as retryable for event streams.
- 수정: Retry after a short delay.

### `CLIENT.WRITE_UNSETTLED`

`retry` check-first

- 원인: A write may or may not have committed: the connection broke after the request was sent. The SDK keeps the key and the body so the same request can be sent again.
- 수정: Call `retry_pending()` (or send the same request with the same idempotency key); the server answers with the recorded result. Never send it again with a new key.
