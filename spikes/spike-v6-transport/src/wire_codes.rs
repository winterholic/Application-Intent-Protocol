//! Registry of the codes a caller of the product HTTP API can receive: what the code means, who has to act and
//! whether sending the same request again can help.
//!
//! The product wire answers most failures as HTTP 200 with `{"ok":false,"code":...,"msg":...}`, so the status line
//! does not say whether to retry; this table does. `aip explain-code <CODE>` prints an entry and `spec/wire-errors.md`
//! is generated from it. `tests/wire_codes.rs` scans the sources of the read, write, worker and transport layers so
//! that a code cannot be added without an entry here. The explanations come from reading the emitting code; where a
//! code covers several causes the entry says what is common to them.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// The request is wrong; change it.
    Request,
    /// No valid identity: sign in again.
    Authentication,
    /// The identity is known and the definition does not allow this.
    Authorization,
    /// The client and the server disagree on the definition or the deployment.
    Contract,
    /// A bound was reached.
    Limit,
    /// A dependency or capacity is temporarily missing.
    Availability,
    /// The result of a write or job is unknown or in conflict.
    Outcome,
    /// A failure on the server or in an extension; the operator acts.
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// The same request fails the same way.
    Never,
    /// The same request may succeed after a delay. A write keeps its idempotency key.
    Later,
    /// Find out what happened before sending again; sending blind can repeat an effect.
    Check,
    /// Obtain a new token, then send again.
    Reauthenticate,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Request => "request",
            Class::Authentication => "authentication",
            Class::Authorization => "authorization",
            Class::Contract => "contract",
            Class::Limit => "limit",
            Class::Availability => "availability",
            Class::Outcome => "outcome",
            Class::Server => "server",
        }
    }
}

impl Retry {
    pub fn name(self) -> &'static str {
        match self {
            Retry::Never => "never",
            Retry::Later => "later",
            Retry::Check => "check-first",
            Retry::Reauthenticate => "reauthenticate",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Info {
    pub code: &'static str,
    pub class: Class,
    pub retry: Retry,
    pub explain: &'static str,
    pub fix: &'static str,
}

const fn w(code: &'static str, class: Class, retry: Retry, explain: &'static str, fix: &'static str) -> Info {
    Info { code, class, retry, explain, fix }
}

use Class::{Authentication as Au, Authorization as Az, Availability as Av, Contract as Co, Limit as Li, Outcome as Ou, Request as Rq, Server as Sv};
use Retry::{Check, Later, Never, Reauthenticate as Re};

pub fn lookup(code: &str) -> Option<&'static Info> {
    REGISTRY.iter().find(|c| c.code == code)
}

pub fn all() -> &'static [Info] {
    REGISTRY
}

pub fn render_text(c: &Info) -> String {
    format!("{}  wire error  class: {}  retry: {}\n\nwhy:\n  {}\n\nfix:\n  {}\n", c.code, c.class.name(), c.retry.name(), c.explain, c.fix)
}

/// `spec/wire-errors.md`, generated; `tests/wire_codes.rs` keeps it equal to the registry.
pub fn render_markdown() -> String {
    let mut out = String::from(
        "# 제품 wire 오류 코드\n\n\
         > 이 문서는 `spikes/spike-v6-transport/src/wire_codes.rs`의 레지스트리에서 생성된다. 손으로 고치지 않는다. 다시 만들려면 `AIP_UPDATE_GOLDEN=1 cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test wire_codes`를 실행한다.\n\n\
         제품 HTTP API(`/read`, `/apply`, `/operation`, `/jobs/*`, `/files/*`, `/events/*` 등)의 응답에서 호출자가 받는 `code`다. 대부분의 실패가 HTTP 200과 `{\"ok\":false,\"code\":...,\"msg\":...}`로 오므로 상태 줄로는 재시도 여부를 알 수 없고, 이 표가 그것을 말한다. 메시지 문장은 계약이 아니고 코드가 계약이다. 모든 AIP 오류 코드는 `영역.이름` 형식이다. 이 문서의 영역은 `WIRE`이고, 정의 검사기의 `DEF` 코드는 [definition-diagnostics.md](definition-diagnostics.md), PoC의 `POC` 코드는 [diagnostics.md](diagnostics.md)에 있다. 2026-10-13 이전에는 접두 없는 이름(`WIRE.BAD_VALUE`)이었고 정의 진단 코드와 이름이 겹쳤다. 조회: `aip explain-code <CODE>`. 작업 상태 값(`QUEUED`, `RUNNING`, `SUCCEEDED`, `FAILED`, `CANCEL_REQUESTED`, `CANCELLED`)은 오류 코드가 아니므로 접두 없이 그대로다.\n\n\
         ## 분류와 재시도\n\n\
         | `class` | 뜻 |\n|---|---|\n\
         | request | 요청이 틀렸다. 고친다 |\n| authentication | 유효한 신원이 없다. 다시 로그인한다 |\n| authorization | 신원은 알지만 정의가 허용하지 않는다 |\n| contract | 클라이언트와 서버의 정의·배포가 다르다 |\n| limit | 한도에 걸렸다 |\n| availability | 의존 대상이나 용량이 일시적으로 없다 |\n| outcome | 쓰기·작업의 결과가 불명이거나 충돌했다 |\n| server | 서버나 확장의 실패. 운영자가 고친다 |\n\n\
         | `retry` | 뜻 |\n|---|---|\n\
         | never | 같은 요청은 같은 이유로 다시 실패한다 |\n| later | 시간이 지나면 같은 요청이 성공할 수 있다. 쓰기는 같은 멱등 키를 쓴다 |\n| check-first | 무슨 일이 있었는지 확인한 뒤에만 다시 보낸다. 모르고 보내면 효과가 반복될 수 있다 |\n| reauthenticate | 새 토큰을 받은 뒤 다시 보낸다 |\n\n\
         응답 헤더 `x-request-id`를 신고에 함께 준다. 운영자는 같은 id의 로그 줄을 찾는다.\n",
    );
    for c in REGISTRY {
        out.push_str(&format!(
            "\n### `{}`\n\n`class` {}, `retry` {}\n\n- 원인: {}\n- 수정: {}\n",
            c.code,
            c.class.name(),
            c.retry.name(),
            c.explain,
            c.fix
        ));
    }
    out
}

pub static REGISTRY: &[Info] = &[
    w("WIRE.ACCESS_DENIED", Az, Never, "The `allow`/`access` rule of the definition is false for this caller, or a guard aggregate gave them nothing.", "Use an identity the definition allows, or change the rule in the definition if the rule is wrong. The same call by the same caller gives the same answer."),
    w("WIRE.ACCESS_NOT_DECLARED", Sv, Never, "An extension asked its context for an access that the extension does not list.", "Add the access to the `access` list of the extension, or stop calling it."),
    w("WIRE.ALREADY_EXISTS", Rq, Never, "A unique constraint rejected the write: a row with these values exists.", "Read the existing row instead of creating it, or send different values."),
    w("WIRE.BAD_CONFIGURATION", Sv, Never, "A managed job or extension is configured inconsistently (a missing file name or deadline).", "Operator: fix the service configuration or the definition and restart."),
    w("WIRE.BAD_CURSOR", Rq, Never, "The `after` cursor does not have the sort fields and the id of the request.", "Build `after` from the last row of the previous page, with the same sort."),
    w("WIRE.BAD_REQUEST", Rq, Never, "The request is malformed: its headers, its JSON shape or a required part.", "Read `msg` and send a well-formed request. A generated client does not produce this."),
    w("WIRE.BAD_VALUE", Rq, Never, "A value does not have the form of its type or is outside its range.", "Send a value of the declared type (see the contract); `msg` names the field."),
    w("WIRE.BULK_LIMIT", Li, Never, "More targets than the `bulk maxRows` of the exposed transition.", "Send fewer targets per call, or raise `maxRows` in the definition."),
    w("WIRE.CALL_LIMIT", Li, Never, "An extension made more calls to its context than one run allows.", "Make fewer calls from the extension."),
    w("WIRE.CANCELLATION_UNCONFIRMED", Ou, Check, "A job was asked to cancel, but its worker has not confirmed that it stopped.", "Read `/jobs/status` until the job has a final status before assuming it stopped."),
    w("WIRE.CHECK_FAILED", Rq, Never, "After the write, rows did not satisfy a declared `check`; the write was rolled back.", "Send values that satisfy the check named in `msg`."),
    w("WIRE.COMMIT_UNKNOWN", Ou, Check, "The connection failed after COMMIT was sent: the change may or may not have been made.", "Send the same request again with the same idempotency key (the recorded result is returned), or read the state. Never retry with a new key."),
    w("WIRE.CONFLICT", Ou, Later, "A concurrent change made this write lose (a serialization or version conflict).", "Read the current state and send the write again, with the same idempotency key."),
    w("WIRE.CIRCUIT_OPEN", Av, Later, "An outbound integration is paused after repeated failures; the call was not made.", "Wait for the circuit to close; a job step is retried by the server."),
    w("WIRE.DELIVERY_FAILED", Av, Later, "A job's outbound delivery did not succeed.", "A job step is retried by the server; report the `x-request-id` if it persists."),
    w("WIRE.DESTINATION_REJECTED", Sv, Never, "The destination of an outbound delivery was refused by the address policy.", "Operator: fix the destination or the allowed address settings."),
    w("WIRE.INTEGRATION_AUTH_FAILED", Sv, Never, "The token for an outbound integration could not be obtained.", "Operator: fix the integration's credential configuration."),
    w("WIRE.RETRY_LATER", Av, Later, "An extension asked the job to be retried later.", "None for the caller; the server retries the job step."),
    w("WIRE.CONSUMER_FAILED", Sv, Later, "The reason recorded when a message consumer released a message without giving one; the message is delivered again.", "Consumer: pass an `error` text when releasing a message."),
    w("WIRE.CONTRACT_MISMATCH", Co, Never, "The `x-aip-contract` fingerprint of the request is not the server's: the client was generated from another definition.", "Regenerate the client binding (`aip service gen`) from the deployed definition."),
    w("WIRE.COST_EXCEEDED", Li, Never, "The estimated cost exceeds the `cost` of the budget.", "Select fewer fields, traverse less or use a smaller limit; or raise `cost` in the definition."),
    w("WIRE.CURSOR_EXPIRED", Rq, Never, "The event cursor is older than the retained events.", "Start from the current position and read the state again."),
    w("WIRE.CURSOR_FIELD_NOT_ALLOWED", Rq, Never, "A cursor boundary field is not selected, is nullable or has a field policy.", "Select the sort fields and `id`, and sort on non-null fields without a field policy."),
    w("WIRE.CURSOR_WITH_OFFSET", Rq, Never, "`after` and `offset` were sent together.", "Use one paging mode."),
    w("WIRE.DB_UNAVAILABLE", Av, Later, "The database (or a lookup that needs it) could not be reached or did not answer in time.", "Send the same request again after a short delay; a write keeps its idempotency key."),
    w("WIRE.DEADLINE_EXCEEDED", Li, Later, "The request ran past its deadline (the `deadline` of the budget, extension or operation).", "Send a smaller request, or raise the deadline in the definition; a write keeps its idempotency key."),
    w("WIRE.DEPLOYMENT_CHANGED", Co, Later, "The deployed definition changed after this server process started; it still holds the old one.", "Operator: restart the service with the new definition. Clients send again afterwards."),
    w("WIRE.DEPTH_EXCEEDED", Li, Never, "The traversal is deeper than the `depth` of the budget.", "Traverse less, or raise `depth` in the definition."),
    w("WIRE.DUPLICATE", Rq, Never, "A select, filter or sort entry is repeated.", "Remove the repeated entry."),
    w("WIRE.DUPLICATE_FILTER", Rq, Never, "A `where` target condition is repeated.", "Remove the repeated condition."),
    w("WIRE.DUPLICATE_TARGET", Rq, Never, "An id appears twice among the targets.", "Send each id once."),
    w("WIRE.EFFECT_TARGET_MISSING", Li, Never, "A many-row `update` effect of the transition would change more rows than its declared `maxRows`, so its declared scope cannot be processed; nothing was changed.", "Raise `maxRows` in the definition, or reduce the rows that belong to the parent."),
    w("WIRE.EFFECT_UNKNOWN", Ou, Check, "An effect of a job step may have run, but its result was lost; it is not repeated automatically.", "Look at the state the effect changes before running it again."),
    w("WIRE.EVENT_INVALID", Rq, Never, "An event could not be published: its payload or key is invalid.", "Send a payload that matches the declared event."),
    w("WIRE.EXTENSION_ERROR", Sv, Never, "A write extension returned an error; its code is in `msg`.", "Fix the extension or the input that makes it fail."),
    w("WIRE.FIELD_NOT_EXPOSED", Az, Never, "A field is not open to this read in the contract.", "Select only fields the contract opens. Opening one is a change of the definition (`expose read select`)."),
    w("WIRE.FILTER_NOT_ALLOWED", Rq, Never, "The filter (field and operator) is not in the allow list of the exposure.", "Use a filter from the contract, or add it to `filter` in the definition."),
    w("WIRE.FORBIDDEN", Az, Never, "The caller may not apply this transition to this row or use this corpus.", "Use an identity that the `allow` rule of the transition accepts."),
    w("WIRE.IDEMPOTENCY_MISMATCH", Rq, Never, "The idempotency key was used before for a different request.", "Use a new key for a new request; send the identical request again to get the recorded result."),
    w("WIRE.ID_OUT_OF_RANGE", Sv, Never, "An id is above the JavaScript safe integer range of the safe id wire.", "Use the decimal-string id wire."),
    w("WIRE.IMPLEMENTATION_CHANGED", Co, Never, "An extension implementation changed after the job was queued.", "Queue the job again."),
    w("WIRE.INPUT_BINDING", Sv, Never, "An aggregate input of an extension must equal the value bound from the extension input.", "Pass the extension input through to the aggregate unchanged."),
    w("WIRE.INTERNAL", Sv, Later, "An unexpected failure on the server; the details are not returned.", "Report the `x-request-id`; the operator finds the log line with that id. A write keeps its idempotency key."),
    w("WIRE.INVALID_STATE", Rq, Never, "The row is not in the state the transition starts from (its `from` condition is false).", "Read the row and apply a transition that fits its state."),
    w("WIRE.INVARIANT_VIOLATED", Rq, Never, "The write would violate a declared invariant.", "Change the values so the invariant (for example one published row per club) holds."),
    w("WIRE.IN_LIST_EXCEEDED", Li, Never, "An `in` filter has more values than the limit.", "Send at most 50 values."),
    w("WIRE.JOB_FAILED", Ou, Never, "The job ended in failure; asking for its result returns this.", "Read the failure in the result and queue a corrected job."),
    w("WIRE.JOB_NOT_READY", Av, Later, "The job has not finished yet.", "Read `/jobs/status` until it has a final status."),
    w("WIRE.LEASE_LOST", Ou, Check, "A worker lost the lease of a queue entry; another worker may have taken it over.", "Read the state before re-running."),
    w("WIRE.LENGTH_REQUIRED", Rq, Never, "The request has no `content-length`.", "Send the length."),
    w("WIRE.LIMIT_EXCEEDED", Li, Never, "A document search limit was exceeded.", "Narrow the search."),
    w("WIRE.METHOD_NOT_ALLOWED", Rq, Never, "The route does not accept this HTTP method.", "Use the method the message names."),
    w("WIRE.MISSING_TARGET", Rq, Never, "Some target ids do not exist or are not visible to the caller.", "Send ids of rows the caller can read."),
    w("WIRE.NESTED_TRAVERSE_NOT_ALLOWED", Rq, Never, "A one-to-many traversal contains another traversal.", "Flatten the request."),
    w("WIRE.NOT_AVAILABLE", Av, Never, "The route is not enabled on this server.", "Operator: enable the feature in the service configuration, or do not call it."),
    w("WIRE.NOT_EXPOSED", Az, Never, "The resource, aggregate, operation or transition has no exposure in the contract.", "Call only what the contract lists. Exposing it is a change of the definition."),
    w("WIRE.NOT_FOUND", Rq, Never, "The object, job, document or route does not exist or is not visible to the caller (the two are not told apart).", "Check the identifier and the caller."),
    w("WIRE.NOT_ROOT_QUERYABLE", Rq, Never, "The resource has an `expose read` without `budget`, so it can only be read through another resource's traverse.", "Add `budget` to the definition to read it directly, or read it through its parent."),
    w("WIRE.NOT_SAME_SCOPE", Rq, Never, "The targets do not share the `sameScope` value that the exposure requires.", "Send targets of one scope per call."),
    w("WIRE.OBJECT_CAPACITY", Av, Later, "The limit of concurrent object transfers was reached.", "Send again after a short delay."),
    w("WIRE.OBJECT_STORE_ERROR", Av, Later, "The object store failed to complete the operation.", "Send again after a short delay; report the `x-request-id` if it persists."),
    w("WIRE.OFFSET_EXCEEDED", Li, Never, "`offset` is above the `offset` limit of the budget.", "Page with `after` or lower the offset."),
    w("WIRE.ORIGIN_NOT_ALLOWED", Az, Never, "The browser `Origin` is not in the allowed origins of the service.", "Operator: add the origin to `allowed_origins`."),
    w("WIRE.OUTPUT_INVALID", Sv, Never, "An extension returned output that does not match its declaration.", "Fix the extension or its `output` declaration."),
    w("WIRE.OUTPUT_TOO_LARGE", Li, Never, "The response would be larger than the output limit.", "Select fewer fields or use a smaller limit."),
    w("WIRE.PAYLOAD_TOO_LARGE", Li, Never, "The request body is larger than the limit.", "Send less."),
    w("WIRE.POLICY_FIELD_NOT_FILTERABLE", Az, Never, "A field with a read policy cannot be used in a filter, a sort or a cursor, because the result would reveal it.", "Use a field without a read policy."),
    w("WIRE.PRINCIPAL_UNAVAILABLE", Au, Re, "The identity that queued the job no longer exists or is disabled.", "Sign in again and queue the job anew."),
    w("WIRE.PROVIDER_UNAVAILABLE", Av, Later, "The embedding or answer provider of a document search failed.", "Send again after a short delay."),
    w("WIRE.QUOTA_EXCEEDED", Li, Later, "A quota of the caller was used up (HTTP 429 with `retry-after`).", "Wait for `retry-after`."),
    w("WIRE.RATE_LIMITED", Li, Later, "The caller sent too many requests in the window.", "Slow down and send again after the window."),
    w("WIRE.REQUEST_TIMEOUT", Li, Later, "The request did not finish within the server's request time limit.", "Send a smaller request; a write keeps its idempotency key."),
    w("WIRE.RESULT_EXPIRED", Rq, Never, "The job result is past its retention.", "Queue the job again if the result is still needed."),
    w("WIRE.ROWS_EXCEEDED", Li, Never, "The limit asks for more rows than the `rows` of the budget (or of the traverse).", "Ask for fewer rows, or raise `rows` in the definition."),
    w("WIRE.SCHEMA_MISMATCH", Co, Never, "The deployment marker is missing: the database was not initialized for this definition.", "Operator: run `aip service init` or `aip service migrate`."),
    w("WIRE.SERVER_BUSY", Av, Later, "A concurrency cap of the server is reached.", "Send again after a short delay."),
    w("WIRE.SIGNATURE_EXPIRED", Au, Later, "The signed webhook timestamp is outside the accepted window.", "Sender: sign again with the current time and resend."),
    w("WIRE.SIGNATURE_INVALID", Au, Never, "The signature of the webhook does not match.", "Sender: sign the exact body with the shared secret."),
    w("WIRE.SORT_NOT_ALLOWED", Rq, Never, "The sort field is not in the `sort` allow list of the exposure.", "Sort by a field from the contract, or add it to `sort` in the definition."),
    w("WIRE.STEP_FAILED", Sv, Never, "A step of a job failed; its details are not returned.", "Report the `x-request-id`; the operator finds the log line."),
    w("WIRE.STEP_NOT_ALLOWED", Rq, Never, "A step of the composed write is not in the contract (`transitions`, `create`, or the target).", "Send only the steps the exposure lists."),
    w("WIRE.STORAGE_UNAVAILABLE", Av, Later, "The object storage could not be prepared or read.", "Send again after a short delay."),
    w("WIRE.TARGET_NOT_ALLOWED", Rq, Never, "The kind of target (`id` or `where`) is not in the exposure's `target` list.", "Use a target kind the exposure lists."),
    w("WIRE.TOKEN_EXPIRED", Au, Re, "The access token has expired.", "Get a new access token and send again."),
    w("WIRE.TOKEN_INVALID", Sv, Never, "An extension used a context token that the server does not know.", "Extension: use only the token it was given for this run."),
    w("WIRE.TRAVERSE_NOT_ALLOWED", Rq, Never, "The relation traversal is not in the contract.", "Traverse only relations the contract lists."),
    w("WIRE.TYPE_MISMATCH", Rq, Never, "A value of one type cannot be put into a field of another.", "Send a value of the field's type."),
    w("WIRE.UNAUTHENTICATED", Au, Re, "No valid access token was sent.", "Send `authorization: Bearer <token>` with a valid token."),
    w("WIRE.UNKNOWN_FIELD", Rq, Never, "The field is not a field of the resource.", "Use a field of the contract."),
    w("WIRE.UNKNOWN_KEY", Rq, Never, "The request has a key the server does not know. Unknown keys are never ignored.", "Remove the key; check the spelling against the contract."),
    w("WIRE.UNKNOWN_RESOURCE", Rq, Never, "The resource is not in the definition.", "Use a resource of the contract."),
    w("WIRE.UNSUPPORTED", Sv, Never, "The runtime does not support this kind of declaration or the extension does not fit the operation contract.", "Use the supported form named in `msg`."),
    w("WIRE.UNSUPPORTED_ISOLATION", Sv, Never, "A write with a capacity constraint needs a READ COMMITTED transaction.", "Operator: do not change the transaction isolation of the service connection."),
    w("WIRE.VALUE_SOURCE_NOT_ALLOWED", Rq, Never, "A value of a composed write comes from a source the contract does not list, or a reference was given as a constant.", "Take values only from the sources the exposure lists."),
    w("WIRE.VALUE_TOO_LONG", Li, Never, "A `contains` or `prefix` value is longer than the limit.", "Send at most 200 characters."),
    w("WIRE.WORKER_BUSY", Av, Later, "The limit of concurrent extension calls is reached.", "Send again after a short delay."),
    w("WIRE.WORKER_CONFIG", Sv, Never, "The extension worker could not be started with the configured settings.", "Operator: fix the worker configuration."),
    w("WIRE.WORKER_FAILED", Sv, Later, "The official extension worker failed to run or answer. A job step is retried by the server itself.", "Send again after a short delay; report the `x-request-id` if it persists."),
];
