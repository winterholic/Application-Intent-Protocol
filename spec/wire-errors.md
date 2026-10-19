# 제품 wire 오류 코드

> 이 문서는 `spikes/spike-v6-transport/src/wire_codes.rs`의 레지스트리에서 생성된다. 손으로 고치지 않는다. 다시 만들려면 `AIP_UPDATE_GOLDEN=1 cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test wire_codes`를 실행한다.

제품 HTTP API(`/read`, `/apply`, `/operation`, `/jobs/*`, `/files/*`, `/events/*` 등)의 응답에서 호출자가 받는 `code`다. 대부분의 실패가 HTTP 200과 `{"ok":false,"code":...,"msg":...}`로 오므로 상태 줄로는 재시도 여부를 알 수 없고, 이 표가 그것을 말한다. 메시지 문장은 계약이 아니고 코드가 계약이다. 모든 AIP 오류 코드는 `영역.이름` 형식이다. 이 문서의 영역은 `WIRE`이고, 정의 검사기의 `DEF` 코드는 [definition-diagnostics.md](definition-diagnostics.md), PoC의 `POC` 코드는 [diagnostics.md](diagnostics.md)에 있다. 2026-10-13 이전에는 접두 없는 이름(`BAD_VALUE`)이었고 정의 진단 코드와 이름이 겹쳤다. 조회: `aip explain-code <CODE>`. 작업 상태 값(`QUEUED`, `RUNNING`, `SUCCEEDED`, `FAILED`, `CANCEL_REQUESTED`, `CANCELLED`)은 오류 코드가 아니므로 접두 없이 그대로다.

## 분류와 재시도

| `class` | 뜻 |
|---|---|
| request | 요청이 틀렸다. 고친다 |
| authentication | 유효한 신원이 없다. 다시 로그인한다 |
| authorization | 신원은 알지만 정의가 허용하지 않는다 |
| contract | 클라이언트와 서버의 정의·배포가 다르다 |
| limit | 한도에 걸렸다 |
| availability | 의존 대상이나 용량이 일시적으로 없다 |
| outcome | 쓰기·작업의 결과가 불명이거나 충돌했다 |
| server | 서버나 확장의 실패. 운영자가 고친다 |

| `retry` | 뜻 |
|---|---|
| never | 같은 요청은 같은 이유로 다시 실패한다 |
| later | 시간이 지나면 같은 요청이 성공할 수 있다. 쓰기는 같은 멱등 키를 쓴다 |
| check-first | 무슨 일이 있었는지 확인한 뒤에만 다시 보낸다. 모르고 보내면 효과가 반복될 수 있다 |
| reauthenticate | 새 토큰을 받은 뒤 다시 보낸다 |

JSON 실패 본문은 `requestId`와 이 레지스트리의 `retry`를 싣는다. `requestId`는 응답 헤더 `x-request-id`와 같으며, 신고에 함께 주면 운영자가 같은 id의 로그 줄을 찾는다. 성공 본문에는 두 필드를 추가하지 않는다. 재시도 지침은 쓰기의 확정 여부를 대신하지 않으므로 미확정 쓰기는 같은 멱등 키를 유지한다.

### `WIRE.ACCESS_DENIED`

`class` authorization, `retry` never

- 원인: The `allow`/`access` rule of the definition is false for this caller, or a guard aggregate gave them nothing.
- 수정: Use an identity the definition allows, or change the rule in the definition if the rule is wrong. The same call by the same caller gives the same answer.

### `WIRE.ACCESS_NOT_DECLARED`

`class` server, `retry` never

- 원인: An extension asked its context for an access that the extension does not list.
- 수정: Add the access to the `access` list of the extension, or stop calling it.

### `WIRE.ADAPTER_UNAVAILABLE`

`class` availability, `retry` later

- 원인: The managed Mongo or Redis capability that an extension called could not serve the call. The causes are: the adapter is not configured on this server, its settings could not be read, its connection or readiness check failed, the store timed out or closed the connection, or the store answered with an unexpected shape. An empty result is never substituted. The code does not name the cause; `msg` does.
- 수정: Operator: check `workers.mongo` or `workers.redis`, that the store is reachable, and `msg`. Send the same request again only when the cause was an outage or a timeout, and not in a tight loop; a missing or unreadable setting fails the same way until the operator fixes it.

### `WIRE.ADAPTER_UNSUPPORTED`

`class` availability, `retry` never

- 원인: The configured database adapter does not implement this route or feature yet; it is refused instead of being emulated.
- 수정: Operator: use the PostgreSQL adapter for this feature, or wait for the adapter to implement it. The same request fails the same way.

### `WIRE.ALREADY_EXISTS`

`class` request, `retry` never

- 원인: A unique constraint rejected the write: a row with these values exists. The response never names that row or its values, because it may be one the caller cannot read.
- 수정: Read the existing row instead of creating it, or send different values.

### `WIRE.BAD_CONFIG`

`class` server, `retry` never

- 원인: A Mongo or Redis capability setting is out of range when the server creates the adapter: a database name that is empty or longer than 63 bytes, a pool size outside 1 to 64, a timeout of zero or over 30 seconds, an empty Redis prefix list or prefix, or a Redis in-flight limit outside 1 to 256. It is not `WIRE.BAD_CONFIGURATION`, which covers a managed job or extension that is configured inconsistently. `aip service serve` reports any failure to create an adapter as `MONGO_CAPABILITY_UNAVAILABLE` or `REDIS_CAPABILITY_UNAVAILABLE`, so an HTTP caller normally does not receive this code.
- 수정: Operator: put the value inside its range and restart. Sending the same request again does not help.

### `WIRE.BAD_CONFIGURATION`

`class` server, `retry` never

- 원인: A managed job or extension is configured inconsistently (a missing file name or deadline).
- 수정: Operator: fix the service configuration or the definition and restart.

### `WIRE.BAD_CURSOR`

`class` request, `retry` never

- 원인: The `after` cursor does not have the sort fields and the id of the request.
- 수정: Build `after` from the last row of the previous page, with the same sort.

### `WIRE.BAD_REQUEST`

`class` request, `retry` never

- 원인: The request is malformed: its headers, its JSON shape or a required part.
- 수정: Read `msg` and send a well-formed request. A generated client does not produce this.

### `WIRE.BAD_VALUE`

`class` request, `retry` never

- 원인: A value does not have the form of its type or is outside its range.
- 수정: Send a value of the declared type (see the contract); `msg` names the field.

### `WIRE.BULK_LIMIT`

`class` limit, `retry` never

- 원인: More targets than the `bulk maxRows` of the exposed transition, or an atomic bundle or compose over its size cap (operations, steps or target rows).
- 수정: Send fewer targets per call, or raise `maxRows` in the definition.

### `WIRE.CALL_LIMIT`

`class` limit, `retry` never

- 원인: An extension made more calls to its context than one run allows.
- 수정: Make fewer calls from the extension.

### `WIRE.CANCELLATION_UNCONFIRMED`

`class` outcome, `retry` check-first

- 원인: A job was asked to cancel, but its worker has not confirmed that it stopped.
- 수정: Read `/jobs/status` until the job has a final status before assuming it stopped.

### `WIRE.CAPABILITY_DENIED`

`class` server, `retry` never

- 원인: The server-owned Mongo or Redis capability refused a call that the extension did list in its `access`: the collection or key prefix is not on the server allowlist, the command is not an allowed read command or is not implemented, an MGET names a key outside the allowed scope, or the database host of this server provides no Mongo or Redis capability at all. An access that the extension does not list is `WIRE.ACCESS_NOT_DECLARED` instead. The code reaches the extension as the error of its ctx call; an extension that does not catch it ends with `WIRE.EXTENSION_ERROR` naming it.
- 수정: Use only the collections, key prefixes and read commands that the server capability allows (see product/MANAGED-EXECUTION.md), or ask the operator to extend the allowlist. The same call fails the same way.

### `WIRE.CHECK_FAILED`

`class` request, `retry` never

- 원인: After the write, rows did not satisfy a declared `check`; the write was rolled back.
- 수정: Send values that satisfy the check named in `msg`.

### `WIRE.COMMIT_UNKNOWN`

`class` outcome, `retry` check-first

- 원인: The connection failed after COMMIT was sent: the change may or may not have been made.
- 수정: Send the same request again with the same idempotency key (the recorded result is returned), or read the state. Never retry with a new key.

### `WIRE.CONFLICT`

`class` outcome, `retry` later

- 원인: A concurrent change made this write lose (a serialization or declared expect conflict). The whole transaction was rolled back. A write that depends on another row being absent runs in SERIALIZABLE and the server already retried it up to 3 times.
- 수정: Read the current state. Retry an identical request with the same key; if values or expect change, use a new key. COMMIT_UNKNOWN always keeps the original key and body until settled.

### `WIRE.CIRCUIT_OPEN`

`class` availability, `retry` later

- 원인: An outbound integration is paused after repeated failures; the call was not made.
- 수정: Wait for the circuit to close; a job step is retried by the server.

### `WIRE.DELIVERY_FAILED`

`class` availability, `retry` later

- 원인: A job's outbound delivery did not succeed.
- 수정: A job step is retried by the server; report the `x-request-id` if it persists.

### `WIRE.DESTINATION_REJECTED`

`class` server, `retry` never

- 원인: The destination of an outbound delivery was refused by the address policy.
- 수정: Operator: fix the destination or the allowed address settings.

### `WIRE.INTEGRATION_AUTH_FAILED`

`class` server, `retry` never

- 원인: The token for an outbound integration could not be obtained.
- 수정: Operator: fix the integration's credential configuration.

### `WIRE.RETRY_LATER`

`class` availability, `retry` later

- 원인: An extension asked the job to be retried later.
- 수정: None for the caller; the server retries the job step.

### `WIRE.CONSUMER_FAILED`

`class` server, `retry` later

- 원인: The reason recorded when a message consumer released a message without giving one; the message is delivered again.
- 수정: Consumer: pass an `error` text when releasing a message.

### `WIRE.CONTRACT_MISMATCH`

`class` contract, `retry` never

- 원인: The request's `x-aip-contract` fingerprint differs from the server's deployed definition.
- 수정: Regenerate the client binding (`aip service gen`) from the deployed definition.

### `WIRE.CONTRACT_REQUIRED`

`class` contract, `retry` never

- 원인: The request has no `x-aip-contract` header on `/read`, `/apply` or `/status`.
- 수정: Send the deployed fingerprint from `/capabilities` in `x-aip-contract`; generated clients send it automatically.

### `WIRE.COST_EXCEEDED`

`class` limit, `retry` never

- 원인: The estimated cost exceeds the `cost` of the budget.
- 수정: Select fewer fields, traverse less or use a smaller limit; or raise `cost` in the definition.

### `WIRE.CURSOR_EXPIRED`

`class` request, `retry` never

- 원인: The event cursor is older than the retained events.
- 수정: Start from the current position and read the state again.

### `WIRE.CURSOR_FIELD_NOT_ALLOWED`

`class` request, `retry` never

- 원인: A cursor boundary field is not selected, is nullable or has a field policy.
- 수정: Select the sort fields and `id`, and sort on non-null fields without a field policy.

### `WIRE.CURSOR_WITH_OFFSET`

`class` request, `retry` never

- 원인: `after` and `offset` were sent together.
- 수정: Use one paging mode.

### `WIRE.DB_UNAVAILABLE`

`class` availability, `retry` later

- 원인: The database (or a lookup that needs it) could not be reached or did not answer in time.
- 수정: Send the same request again after a short delay; a write keeps its idempotency key.

### `WIRE.DEADLINE_EXCEEDED`

`class` limit, `retry` later

- 원인: The request ran past its deadline (the `deadline` of the budget, extension or operation).
- 수정: Send a smaller request, or raise the deadline in the definition; a write keeps its idempotency key.

### `WIRE.DEPLOYMENT_CHANGED`

`class` contract, `retry` later

- 원인: The deployed definition changed after this server process started; it still holds the old one.
- 수정: Operator: restart the service with the new definition. Clients send again afterwards.

### `WIRE.DEPTH_EXCEEDED`

`class` limit, `retry` never

- 원인: The traversal is deeper than the `depth` of the budget.
- 수정: Traverse less, or raise `depth` in the definition.

### `WIRE.DUPLICATE`

`class` request, `retry` never

- 원인: A select, filter or sort entry is repeated.
- 수정: Remove the repeated entry.

### `WIRE.DUPLICATE_FILTER`

`class` request, `retry` never

- 원인: A `where` target condition is repeated.
- 수정: Remove the repeated condition.

### `WIRE.DUPLICATE_TARGET`

`class` request, `retry` never

- 원인: An id appears twice among the targets.
- 수정: Send each id once.

### `WIRE.EFFECT_TARGET_MISSING`

`class` limit, `retry` never

- 원인: A many-row `update` effect of the transition would change more rows than its declared `maxRows`, so its declared scope cannot be processed; nothing was changed.
- 수정: Raise `maxRows` in the definition, or reduce the rows that belong to the parent.

### `WIRE.EFFECT_UNKNOWN`

`class` outcome, `retry` check-first

- 원인: An effect of a job step may have run, but its result was lost; it is not repeated automatically.
- 수정: Look at the state the effect changes before running it again.

### `WIRE.ENCRYPTED_FIELD_NOT_QUERYABLE`

`class` request, `retry` never

- 원인: An encrypted field cannot be used in unsupported filters, sorting, cursor pagination, or aggregates.
- 수정: Remove the encrypted field from sort, filter, or cursor expressions; only exact equality with lookup: eq and bound single-scope is queryable.

### `WIRE.ENCRYPTION_CORRUPTED`

`class` server, `retry` never

- 원인: The encrypted cell payload or authentication tag is corrupted or tampered.
- 수정: Operator: verify database integrity and encryption key consistency. The data cannot be decrypted.

### `WIRE.ENCRYPTION_UNAVAILABLE`

`class` availability, `retry` later

- 원인: The required encryption key or key provider is currently unavailable.
- 수정: Operator: ensure the encryption provider and key material are active. A write keeps its idempotency key.

### `WIRE.EVENT_INVALID`

`class` request, `retry` never

- 원인: An event could not be published: its payload or key is invalid.
- 수정: Send a payload that matches the declared event.

### `WIRE.EXTENSION_ERROR`

`class` server, `retry` never

- 원인: A write extension returned an error; its code is in `msg`.
- 수정: Fix the extension or the input that makes it fail.

### `WIRE.FIELD_NOT_EXPOSED`

`class` authorization, `retry` never

- 원인: A field is not open to this read in the contract.
- 수정: Select only fields the contract opens. Opening one is a change of the definition (`expose read select`).

### `WIRE.FILTER_NOT_ALLOWED`

`class` request, `retry` never

- 원인: The filter (field and operator) is not in the allow list of the exposure.
- 수정: Use a filter from the contract, or add it to `filter` in the definition.

### `WIRE.FORBIDDEN`

`class` authorization, `retry` never

- 원인: The caller may not apply this transition to this row or use this corpus.
- 수정: Use an identity that the `allow` rule of the transition accepts.

### `WIRE.IDEMPOTENCY_MISMATCH`

`class` request, `retry` never

- 원인: The idempotency key was used before for a different request.
- 수정: Use a new key for a new request; send the identical request again to get the recorded result.

### `WIRE.ID_OUT_OF_RANGE`

`class` server, `retry` never

- 원인: An id is above the JavaScript safe integer range of the safe id wire.
- 수정: Use the decimal-string id wire.

### `WIRE.IMPLEMENTATION_CHANGED`

`class` contract, `retry` never

- 원인: An extension implementation changed after the job was queued.
- 수정: Queue the job again.

### `WIRE.INPUT_BINDING`

`class` server, `retry` never

- 원인: An aggregate input of an extension must equal the value bound from the extension input.
- 수정: Pass the extension input through to the aggregate unchanged.

### `WIRE.INTERNAL`

`class` server, `retry` later

- 원인: An unexpected failure on the server; the details are not returned.
- 수정: Report the `x-request-id`; the operator finds the log line with that id. A write keeps its idempotency key.

### `WIRE.INVALID_STATE`

`class` request, `retry` never

- 원인: The row is not in the state the transition starts from (its `from` condition is false).
- 수정: Read the row and apply a transition that fits its state.

### `WIRE.INVARIANT_VIOLATED`

`class` request, `retry` never

- 원인: The write would violate a declared invariant.
- 수정: Change the values so the invariant (for example one published row per club) holds.

### `WIRE.IN_LIST_EXCEEDED`

`class` limit, `retry` never

- 원인: An `in` filter has more values than the limit.
- 수정: Send at most 50 values.

### `WIRE.JOB_FAILED`

`class` outcome, `retry` never

- 원인: The job ended in failure; asking for its result returns this.
- 수정: Read the failure in the result and queue a corrected job.

### `WIRE.JOB_NOT_READY`

`class` availability, `retry` later

- 원인: The job has not finished yet.
- 수정: Read `/jobs/status` until it has a final status.

### `WIRE.LEASE_LOST`

`class` outcome, `retry` check-first

- 원인: A worker lost the lease of a queue entry; another worker may have taken it over.
- 수정: Read the state before re-running.

### `WIRE.LENGTH_REQUIRED`

`class` request, `retry` never

- 원인: The request has no `content-length`.
- 수정: Send the length.

### `WIRE.LIMIT_EXCEEDED`

`class` limit, `retry` never

- 원인: A document search limit was exceeded.
- 수정: Narrow the search.

### `WIRE.METHOD_NOT_ALLOWED`

`class` request, `retry` never

- 원인: The route does not accept this HTTP method.
- 수정: Use the method the message names.

### `WIRE.MISSING_TARGET`

`class` request, `retry` never

- 원인: Some target ids do not exist or are not visible to the caller.
- 수정: Send ids of rows the caller can read.

### `WIRE.NESTED_TRAVERSE_NOT_ALLOWED`

`class` request, `retry` never

- 원인: A one-to-many traversal contains another traversal.
- 수정: Flatten the request.

### `WIRE.NOT_AVAILABLE`

`class` availability, `retry` never

- 원인: The route is not enabled on this server.
- 수정: Operator: enable the feature in the service configuration, or do not call it.

### `WIRE.NOT_EXPOSED`

`class` authorization, `retry` never

- 원인: The resource, aggregate, operation or transition has no exposure in the contract.
- 수정: Call only what the contract lists. Exposing it is a change of the definition.

### `WIRE.NOT_FOUND`

`class` request, `retry` never

- 원인: The object, job, document or route does not exist or is not visible to the caller (the two are not told apart).
- 수정: Check the identifier and the caller.

### `WIRE.NOT_ROOT_QUERYABLE`

`class` request, `retry` never

- 원인: The resource has an `expose read` without `budget`, so it can only be read through another resource's traverse.
- 수정: Add `budget` to the definition to read it directly, or read it through its parent.

### `WIRE.NOT_SAME_SCOPE`

`class` request, `retry` never

- 원인: The targets do not share the `sameScope` value that the exposure requires.
- 수정: Send targets of one scope per call.

### `WIRE.OR_LIST_EXCEEDED`

`class` limit, `retry` never

- 원인: A read filter exceeds the OR group or condition limit.
- 수정: Send at most one OR group with at most 10 items, and no more than 32 filter conditions in total.

### `WIRE.OBJECT_CAPACITY`

`class` availability, `retry` later

- 원인: The limit of concurrent object transfers was reached.
- 수정: Send again after a short delay.

### `WIRE.OBJECT_STORE_ERROR`

`class` availability, `retry` later

- 원인: The object store failed to complete the operation.
- 수정: Send again after a short delay; report the `x-request-id` if it persists.

### `WIRE.OFFSET_EXCEEDED`

`class` limit, `retry` never

- 원인: `offset` is above the `offset` limit of the budget.
- 수정: Page with `after` or lower the offset.

### `WIRE.ORIGIN_NOT_ALLOWED`

`class` authorization, `retry` never

- 원인: The browser `Origin` is not in the allowed origins of the service.
- 수정: Operator: add the origin to `allowed_origins`.

### `WIRE.OUTPUT_INVALID`

`class` server, `retry` never

- 원인: An extension returned output that does not match its declaration.
- 수정: Fix the extension or its `output` declaration.

### `WIRE.OUTPUT_TOO_LARGE`

`class` limit, `retry` never

- 원인: The response would be larger than the output limit.
- 수정: Select fewer fields or use a smaller limit.

### `WIRE.PAYLOAD_TOO_LARGE`

`class` limit, `retry` never

- 원인: The request body is larger than the limit.
- 수정: Send less.

### `WIRE.POLICY_FIELD_NOT_FILTERABLE`

`class` authorization, `retry` never

- 원인: A field with a read policy cannot be used in a filter, a sort or a cursor, because the result would reveal it.
- 수정: Use a field without a read policy.

### `WIRE.PRINCIPAL_UNAVAILABLE`

`class` authentication, `retry` reauthenticate

- 원인: The identity that queued the job no longer exists or is disabled.
- 수정: Sign in again and queue the job anew.

### `WIRE.PROVIDER_UNAVAILABLE`

`class` availability, `retry` later

- 원인: The embedding or answer provider of a document search failed.
- 수정: Send again after a short delay.

### `WIRE.QUOTA_EXCEEDED`

`class` limit, `retry` later

- 원인: A quota of the caller was used up (HTTP 429 with `retry-after`).
- 수정: Wait for `retry-after`.

### `WIRE.RATE_LIMITED`

`class` limit, `retry` later

- 원인: The caller sent too many requests in the window.
- 수정: Slow down and send again after the window.

### `WIRE.REQUEST_TIMEOUT`

`class` limit, `retry` later

- 원인: The request did not finish within the server's request time limit.
- 수정: Send a smaller request; a write keeps its idempotency key.

### `WIRE.RESULT_EXPIRED`

`class` request, `retry` never

- 원인: The job result is past its retention.
- 수정: Queue the job again if the result is still needed.

### `WIRE.ROWS_EXCEEDED`

`class` limit, `retry` never

- 원인: The limit asks for more rows than the `rows` of the budget (or of the traverse).
- 수정: Ask for fewer rows, or raise `rows` in the definition.

### `WIRE.SCHEMA_MISMATCH`

`class` contract, `retry` never

- 원인: The deployment marker is missing: the database was not initialized for this definition.
- 수정: Operator: run `aip service init` or `aip service migrate`.

### `WIRE.SERVER_BUSY`

`class` availability, `retry` later

- 원인: A concurrency cap of the server is reached.
- 수정: Send again after a short delay.

### `WIRE.SIGNATURE_EXPIRED`

`class` authentication, `retry` later

- 원인: The signed webhook timestamp is outside the accepted window.
- 수정: Sender: sign again with the current time and resend.

### `WIRE.SIGNATURE_INVALID`

`class` authentication, `retry` never

- 원인: The signature of the webhook does not match.
- 수정: Sender: sign the exact body with the shared secret.

### `WIRE.SORT_NOT_ALLOWED`

`class` request, `retry` never

- 원인: The sort field is not in the `sort` allow list of the exposure.
- 수정: Sort by a field from the contract, or add it to `sort` in the definition.

### `WIRE.STEP_FAILED`

`class` server, `retry` never

- 원인: A step of a job failed; its details are not returned.
- 수정: Report the `x-request-id`; the operator finds the log line.

### `WIRE.STEP_NOT_ALLOWED`

`class` request, `retry` never

- 원인: A step of the composed write is not in the contract (`transitions`, `create`, or the target).
- 수정: Send only the steps the exposure lists.

### `WIRE.STORAGE_UNAVAILABLE`

`class` availability, `retry` later

- 원인: The object storage could not be prepared or read.
- 수정: Send again after a short delay.

### `WIRE.TARGET_NOT_ALLOWED`

`class` request, `retry` never

- 원인: The kind of target (`id` or `where`) is not in the exposure's `target` list.
- 수정: Use a target kind the exposure lists.

### `WIRE.TOKEN_EXPIRED`

`class` authentication, `retry` reauthenticate

- 원인: The access token has expired.
- 수정: Get a new access token and send again.

### `WIRE.TOKEN_INVALID`

`class` server, `retry` never

- 원인: An extension used a context token that the server does not know.
- 수정: Extension: use only the token it was given for this run.

### `WIRE.TRAVERSE_NOT_ALLOWED`

`class` request, `retry` never

- 원인: The relation traversal is not in the contract.
- 수정: Traverse only relations the contract lists.

### `WIRE.TYPE_MISMATCH`

`class` request, `retry` never

- 원인: A value of one type cannot be put into a field of another.
- 수정: Send a value of the field's type.

### `WIRE.UNAUTHENTICATED`

`class` authentication, `retry` reauthenticate

- 원인: No valid access token was sent.
- 수정: Send `authorization: Bearer <token>` with a valid token.

### `WIRE.UNKNOWN_FIELD`

`class` request, `retry` never

- 원인: The field is not a field of the resource.
- 수정: Use a field of the contract.

### `WIRE.UNKNOWN_KEY`

`class` request, `retry` never

- 원인: The request has a key the server does not know. Unknown keys are never ignored.
- 수정: Remove the key; check the spelling against the contract.

### `WIRE.UNKNOWN_RESOURCE`

`class` request, `retry` never

- 원인: The resource is not in the definition.
- 수정: Use a resource of the contract.

### `WIRE.UNSUPPORTED`

`class` server, `retry` never

- 원인: The runtime does not support this kind of declaration or the extension does not fit the operation contract.
- 수정: Use the supported form named in `msg`.

### `WIRE.UNSUPPORTED_ISOLATION`

`class` server, `retry` never

- 원인: A write with a capacity constraint needs a READ COMMITTED or SERIALIZABLE transaction.
- 수정: Operator: do not change the transaction isolation of the service connection.

### `WIRE.VALUE_SOURCE_NOT_ALLOWED`

`class` request, `retry` never

- 원인: A value of a composed write comes from a source the contract does not list, or a reference was given as a constant.
- 수정: Take values only from the sources the exposure lists.

### `WIRE.VALUE_TOO_LONG`

`class` limit, `retry` never

- 원인: A `contains`, `like` or `prefix` value is longer than the limit.
- 수정: Send at most 200 characters.

### `WIRE.WORKER_BUSY`

`class` availability, `retry` later

- 원인: The limit of concurrent extension calls is reached.
- 수정: Send again after a short delay.

### `WIRE.WORKER_CONFIG`

`class` server, `retry` never

- 원인: The extension worker could not be started with the configured settings.
- 수정: Operator: fix the worker configuration.

### `WIRE.WORKER_FAILED`

`class` server, `retry` later

- 원인: The official extension worker failed to run or answer. A job step is retried by the server itself.
- 수정: Send again after a short delay; report the `x-request-id` if it persists.
