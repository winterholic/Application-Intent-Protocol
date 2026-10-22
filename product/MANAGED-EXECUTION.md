# 관리형 durable execution

`operation job`을 호출하면 PostgreSQL에 수락된 작업이 요청 연결과 분리되어 실행된다. 기존 `operation read`, read/apply 및 SQL 동기 경로는 그대로 유지된다. 이 문서는 현재 구현된 durable job, 관리 파일, 이벤트 스트림, 이름 기반 outbound HTTP broker의 경계와 호출법을 설명한다.

정적 디렉터리 hosting, 대용량 local/S3 객체, SSE/WebSocket, 메시지 큐, Linux 격리와 의존성 설정은 [서버 인프라](INFRASTRUCTURE.md)를 함께 따른다.

## 선언과 worker context

작업 전용 앱은 기존 `operation read`에 더해 `operation job`을 선언할 수 있다. job에는 입력·출력 계약, `allow`, deadline, `effect none` 또는 `effect managed`, 그리고 필요할 때 `access` capability 목록을 선언한다.

읽기 확장과 job의 서버 ctx 호출이 `WIRE.DEADLINE_EXCEEDED`를 반환하면 서버는 그 코드로 호출을 끝낸다. 확장이 예외를 잡아 성공을 반환해도 서버의 기한 실패를 지울 수 없다. worker가 직접 같은 이름의 오류 코드를 보낸 경우는 신뢰하지 않고 `WIRE.EXTENSION_ERROR`로 처리한다. 검증 경로는 `spikes/spike-v4-worker/tests/ctx_deadline.rs`(실제 Node/Python IPC, 결정적 host 오류)와 `tests/v4_1.rs`(실제 PostgreSQL 잠금 대기)다.

```aip
actor principal
operation job report {
  input { text: Text(1..1000) }
  output { summary: Text(1..1000) }
  allow actor != null
  access jobs.length, files.reports, events.notifications, http.echo
  effect managed
  deadline 10s
  implementation "reports.create"
}
operation read length {
  input { text: Text }
  output { length: Int }
  allow actor != null
  effect none
  deadline 2s
  implementation "reports.length"
}
```

아래 문법과 context API는 구현된 기능이다. Node 예제는 `reports.mjs`, Python 예제는 `reports.py`에 저장한다. HTTP endpoint는 실제 provider 설정으로 교체해야 한다. 서버 확장은 trusted code다. Node와 Python worker 모두 같은 host broker를 통해 다음 context를 받는다.

```js
export function length(input) { return { length: [...input.text].length }; }

export async function create(input, ctx) {
  const measured = await ctx.jobs.run("length", input, "measure-v1");
  const file = await ctx.files.put("reports", {
    name: "report.txt", mediaType: "text/plain", base64: "aGVsbG8="
  }, "summary-file-v1");
  await ctx.events.publish("notifications", "report-created", { fileId: file.id, length: measured.length });
  const remote = await ctx.http.call("echo", { text: input.text }, "send-report-v1");
  return { summary: remote.body.text };
}
```

```python
def length(input):
    return {"length": len(input["text"])}

async def create(input, ctx):
    measured = await ctx.jobs.run("length", input, "measure-v1")
    file = await ctx.files.put("reports", {
        "name": "report.txt", "mediaType": "text/plain", "base64": "aGVsbG8="
    }, "summary-file-v1")
    await ctx.events.publish("notifications", "report-created", {"fileId": file["id"], "length": measured["length"]})
    remote = await ctx.http.call("echo", {"text": input["text"]}, "send-report-v1")
    return {"summary": remote["body"]["text"]}
```

확장 입력은 URL·HTTP method·header·파일 경로·owner를 정하지 못한다. 외부 HTTP와 worker file write에는 안정된 idempotency key를 명시해야 하며, 재시작 후 같은 key에 다른 input을 주면 conflict다. Worker effect key는 ASCII 영숫자와 `_`, `-`, `.`, `:`로 구성한 1바이트 이상 100바이트 이하 문자열이다. 호출마다 현재 durable operation의 선언 capability와 principal, deployment, lease를 다시 검사한다. 결과 반환 직전에도 인증 신원과 operation 권한을 다시 확인한다. 각 managed file/event effect와 해당 effect journal 기록은 자체 PostgreSQL transaction에서 함께 commit되며, Job 최종 output과 한 transaction으로 묶이지는 않는다. HTTP 요청은 먼저 PENDING intent를 commit한 뒤 전송하고 응답 뒤 결과를 journal에 기록하므로 provider 호출과 DB commit은 원자적이지 않다. 자식 `ctx.jobs.run`은 선언된 operation을 같은 worker attempt 안에서 실행하며 별도 queue submission이 아니다. 자식 접근은 부모가 선언한 capability subset으로 제한한다. 중첩 depth는 8, 부모와 자식을 합한 attempt당 broker call은 최대 64회다. 재시도 attempt는 새 예산을 사용하며 최대 시도 횟수로 전체 실행 비용을 제한한다. 알려진 자식 업무 실패는 `WIRE.STEP_FAILED`로 재생할 수 있어 확장에서 분기·보상할 수 있고, capability 선언 위반·비용·lease·불확실한 효과 오류는 잡아서 성공으로 바꿀 수 없다.

## 업무 데이터 읽기와 전이

Job은 선언한 resource에 한해 업무 데이터를 읽고 공개 전이를 실행할 수 있다. 예약 작업·웹훅 처리·후속 정산처럼 요청 연결과 분리된 로직이 Spring의 `@Transactional` service 호출과 같은 일을 하되, 호출자가 직접 할 수 있는 범위를 넘지 않게 하는 경로다.

```aip
resource Order {
 fields { id: Id; member: Member; paid: Bool }
 rows read when member = actor
 transition pay { allow member = actor; from paid = false; to paid = true }
 expose read { select id, paid; sort id; budget { rows 10; depth 1; deadline 1s; cost 100 } }
 expose apply pay { target id; bulk maxRows 5 }
}
operation job settle {
  input { other: Order.Id }
  output { paid: Int }
  allow actor != null
  access read.Order, Order.pay
  effect managed
  deadline 10s
  implementation "data.settle"
}
```

```js
export async function settle(input, ctx) {
  const page = await ctx.data.read({ read: "Order", select: ["id", "paid"] });
  let paid = 0;
  for (const row of page.rows.filter((row) => !row.paid)) {
    const result = await ctx.data.apply("Order", "pay", [row.id], `pay-${row.id}`);
    paid += result.changed.length;
  }
  return { paid };
}
```

Python은 `await ctx.data.read({...})`, `await ctx.data.apply("Order", "pay", [id], key)`다.

| capability | 선언 | ctx 호출 | 실행 계약 |
|---|---|---|---|
| 읽기 | `read.<Resource>` | `ctx.data.read(query)`, `ctx.data.aggregate(resource, name, input)` | `/read`와 같은 planner·행/필드 정책·budget을 job 주체로 실행한다. 효과 journal은 남기지 않는다 |
| 전이 | `<Resource>.<transition>` (공개 `expose apply`만) | `ctx.data.apply(resource, transition, ids, key)` | 전이, deferred 검사, effect journal 기록이 **하나의 PostgreSQL transaction**에서 commit된다 |

- 권한은 job을 수락한 principal의 것이다. 행 정책에서 보이지 않는 행에 대한 전이는 동기 `/apply`와 같은 `WIRE.MISSING_TARGET`으로 끝나 행 존재 여부를 알리지 않는다. 선언하지 않은 전이는 job 전체가 `WIRE.ACCESS_NOT_DECLARED`로 실패한다.
- `apply`에는 다른 managed effect처럼 안정 key가 필요하다. 같은 run에서 같은 key를 다시 부르거나 worker가 commit 뒤에 죽어 재시도되면, 전이를 다시 실행하지 않고 journal의 결과를 그대로 돌려준다. 외부 HTTP와 달리 `WIRE.EFFECT_UNKNOWN`은 없다. 다만 COMMIT을 보낸 뒤 응답을 잃거나 해석하지 못하면 전이가 커밋됐는지 이 연결로는 알 수 없다. 이때 job은 확장 오류가 아니라 `WIRE.COMMIT_UNKNOWN`으로 끝나고 자동으로 다시 실행하지 않는다. 커밋됐다면 journal 행도 함께 남아 있으므로 데이터 상태를 먼저 확인한다. COMMIT을 보내기 전에 연결을 잃은 경우는 아무것도 커밋되지 않았고 `WIRE.DB_UNAVAILABLE`로 job이 다시 시도된다.
- 전이가 `WIRE.MISSING_TARGET`, `WIRE.ACCESS_DENIED`, `WIRE.INVARIANT_VIOLATED`처럼 업무 규칙으로 거부되면 그 결과를 journal에 남겨 재시도에서도 같은 오류를 재생한다. 확장은 이 오류를 잡아 분기·보상할 수 있다. `WIRE.CONFLICT`(교착·직렬화)와 DB 장애는 아무것도 commit하지 않았으므로 기록하지 않고 다시 시도할 수 있다.
- 각 `apply`는 자체 transaction이다. job 전체를 한 transaction으로 묶지 않으므로 여러 전이 사이의 원자성이 필요하면 하나의 전이 effects나 동기 쓰기 확장으로 표현한다. attempt당 managed 호출 64회 상한과 `expose apply`의 bulk 상한이 함께 적용된다.
- 이 capability가 있는 job의 descriptor는 `dependencies.worker`에 `database`를 표시한다.

## SDK 사용

Durable job descriptor가 생성 binding에 있을 때 `connect`가 `jobs`, `files`, `events` client를 노출한다.

```ts
if (!client.jobs || !client.files || !client.events) throw new Error("durable binding이 필요합니다");
const queued = await client.jobs.enqueue("report", { text: "hello" }, { key: "report-42" });
const receipt = await client.jobs.status(queued.id);
if (receipt.status === "SUCCEEDED") {
  const result = await client.jobs.result(queued.id, "report");
}

const file = await client.files.put("reports", "report.txt", bytes, "text/plain");
const downloaded = await client.files.get(file.id);

for await (const event of client.events.watch("notifications", { after: "0", intervalMs: 1000, signal })) {
  console.log(event.cursor, event.payload);
}
```

`jobs.enqueue(operation,input,{key,delayMs?})`는 id와 상태를 반환한다. 영수증에는 선택적 `error`가 포함될 수 있으며 FAILED job의 원인을 확인할 때 사용한다. 같은 owner·operation·contract·deployment·key의 같은 입력은 기존 job을 재사용하며 다른 입력이나 실행 조건은 idempotency conflict다. owner는 issuer·subject·관리형 actor·revocation generation을 묶은 인증 principal scope다. `status`, `cancel`, `result`는 호출자의 소유 job만 조회한다. `result`는 typed output 검증을 수행한다. Job id와 file handle은 접근 권한 자체가 아니다. 이 owner 격리는 일반적인 tenant/organization model을 대신하지 않는다. 결과는 완료 후 기본 24시간 보존된다.

대기 중인 작업 취소는 `CANCELLED`가 되고, 실행 중 취소 요청은 우선 `CANCEL_REQUESTED`가 된다. 실행 중 HTTP effect 결과가 PENDING/UNKNOWN이면 취소 요청은 안전한 취소로 확정하지 않고 `FAILED`와 `WIRE.CANCELLATION_UNCONFIRMED`로 끝난다. 다른 실패나 worker 종료 뒤 unresolved effect가 남아도 `jobs.status()`/`cancel()` 응답의 선택적 `error`는 `WIRE.EFFECT_UNKNOWN`을 우선 표시한다. 이 상태는 외부 provider가 작업을 처리했는지 모른다는 뜻이다.

`events.read(stream,{after?,limit?})`는 decimal string cursor와 최대 100개 이벤트를 반환한다. `events.watch`는 read를 주기적으로 polling하는 async iterator다. 기본 간격은 1초, 허용 범위는 50ms에서 30초이며 AbortSignal로 끝낼 수 있다. 같은 cursor를 사용하는 SSE/WebSocket 전송도 제공하며 연결·인증 경계는 [서버 인프라](INFRASTRUCTURE.md)에 설명한다. 보존 범위보다 오래된 cursor는 `WIRE.CURSOR_EXPIRED`를 반환한다. Event ACL도 enabled principal owner와 현재 revocation generation에 기반하며, publisher operation의 `allow`를 reader에게 재평가하지 않는다. role 변경만으로 저장 event에 대한 접근이 회수되지 않는다. principal revoke/generation 변경은 이전 owner scope의 접근을 막지만 저장된 데이터 자체를 즉시 삭제하지는 않는다. 사용자별 record subscription이나 일반 tenant/organization ACL은 제공하지 않는다.

`files.put(bucket,name,Uint8Array,mediaType)`는 handle metadata를 반환하고, `get(id)`는 바이트를 `Uint8Array`로 돌려준다. 제품 호출 상한은 767KiB다. 다운로드와 삭제는 enabled principal owner 및 현재 revocation generation으로 인증된 호출만 가능하다. ACL은 producer operation의 동적 `allow`를 재평가하지 않는다. role 변경만으로 owner의 파일 접근이 회수되지 않는다. principal revoke/generation 변경은 기존 owner scope의 접근을 막지만 bytes는 삭제 또는 만료 때까지 남는다.

## 서비스 설정

기존 설정은 `workers`를 생략할 수 있으며 그 경우 worker, outbound endpoint, SDK 직접 업로드는 꺼져 있다. `workers`를 둘 때 directory와 language가 필요하다. `integrations`의 URL과 method는 서버 설정에만 두며 worker 입력에서 선택할 수 있는 것은 endpoint name이다. `enable_file_uploads`는 SDK의 직접 업로드 route 설정이며 worker의 `ctx.files`는 선언된 `files.<bucket>` capability로 제한한다.

```json
{
  "workers": {
    "directory": "./extensions",
    "language": "node",
    "enable_writes": false,
    "enable_file_uploads": false,
    "integrations": [{
      "name": "echo",
      "url": "https://api.example.com/echo",
      "method": "POST",
      "timeout_ms": 5000,
      "max_request_bytes": 65536,
      "max_response_bytes": 65536,
      "secret_header_env": { "name": "x-api-key", "env": "REPORTS_API_KEY" }
    }]
  }
}
```

secret은 명시한 process environment variable에서만 읽고 `.env`는 불러오지 않는다. startup은 DNS를 최대 2초 동안 조회하고 결과 주소를 고정한다. HTTPS를 사용하며 기본적으로 private/special-use 주소와 redirect, proxy를 차단한다. 서버 endpoint별 `allow_private_networks:true`는 사설 unicast HTTPS 연동을 허용하지만 link-local 등 특수 주소는 계속 거부한다. 명시적으로 `allow_loopback:true`인 개발 설정만 loopback HTTP를 쓸 수 있다. endpoint, DNS 답, method, 제한값, secret의 digest가 implementation bundle fingerprint에 포함되므로 값이 바뀐 worker 설정으로 기존 job을 이어 실행하지 않는다. 설정 오류 출력은 endpoint URL이나 secret 값을 노출하지 않는다.

MySQL read extensions can also use server-owned MongoDB and Redis read adapters. Configure `workers.mongo` and/or `workers.redis` with environment variable **names**; the service reads URI values in its own process and never adds them to the worker subprocess environment. Mongo uses the Rust driver's bounded connection pool (1–64) and readiness ping. Redis uses a reconnecting connection manager with a bounded in-flight semaphore (1–256) and readiness ping. Timeouts are 1–30,000 ms. If an adapter is absent, the corresponding capability call fails with `WIRE.ADAPTER_UNAVAILABLE`; an empty in-memory result is never substituted.

MySQL 8.4 also supports the explicitly enabled `extension write ... effect db` worker action when `workers.enable_writes` is `true`. Its declared `access` entries are the only transitions that `ctx.data.apply` can call. The server derives the actor from the verified principal, applies each transition's existing policy and contract, and keeps all callback writes, audit rows and the idempotency result in one catalog-fenced MySQL transaction. The validated worker output is committed with those writes; worker failure, invalid output, callback rejection or deadline expiry rolls the transaction back. Read extensions continue to reject `ctx.data.apply`.

**Reading and creating inside a write extension (WBS 7.2.8, MySQL).** A write extension may also declare `access read.<Resource>` and `access create.<Resource>` (the resource needs an `expose create`). `ctx.data.read(query)` runs the public read under the caller's row and field policies inside the extension's own transaction, so the extension sees the row that its next transition is checked against. `ctx.data.create(res, {id: ...})` is a one-operation atomic bundle of the exposed create under the caller's `allow` rule. `ctx.data.apply(res, name, ids, key, values, expect)` passes the caller values and the expected current values of a `patch ... expect` transition (GENERAL-UPDATE), which is how an extension keeps an optimistic lock: when the row changed after the extension read it, the `expect` fails with `WIRE.CONFLICT`, the whole transaction rolls back, and an extension that lets the error out unchanged answers `WIRE.CONFLICT` to the caller, who may send the request again. Any refused ctx call poisons the invocation: an extension that swallows the error and returns normally is refused instead of committing. The Mongo and Redis read commands of the declared grants (`access mongo.<collection>`) are plain reads: they run at once and leave no step or saga log. The transition stays public (`expose apply`), so a caller can also call it directly; use it only for data that nothing else reads.

MySQL durable HTTP jobs now connect enqueue/status/result/cancel to the lease store and an adapter-owned worker supervisor. Managed data reads, aggregates, transitions and nested jobs use declared capabilities. Each data effect commits its business change, audit and receipt together; a retry replays the receipt. Status hides output, and result access rechecks the current principal and operation policy. Existing deployments install the additive effect journal by reapplying `adopt-external --apply` with the migration role; runtime startup checks its InnoDB engine, primary key and columns and performs no DDL. Real Node/Python asynchronous execution passed Linux CI. Configured schedules, signed webhooks and outbox handlers start jobs in the same store as the bound service principal; a user cannot enqueue the operations they feed or use their reserved keys. Managed files, external HTTP capabilities, events, messages and file uploads remain unsupported in this MySQL runtime.

The existing `POST /capabilities` infrastructure catalog and MySQL `service serve` listening event report each adapter's `configured` flag and `status` (`ready` or `unavailable`). An absent optional adapter remains unavailable only for calls that use it; it does not prevent the service or unrelated reads from starting. If an adapter is configured but its connection/readiness check fails, startup still fails before the listening event is emitted. These status fields contain no URI, environment-variable value, credential or data.

```json
{
  "workers": {
    "directory": "./extensions",
    "language": "python",
    "mongo": {"uri_env":"AIP_MONGO_URL", "database":"facereview", "max_pool_size":8, "timeout_ms":2000},
    "redis": {"uri_env":"AIP_REDIS_URL", "prefixes":["facereview:reco:"], "max_in_flight":32, "timeout_ms":2000}
  }
}
```

Each extension must separately declare a least-privilege `access` grant such as `mongo.video_distribution` or `redis.facereview:reco:*`. The server checks the grant on every call. `youtube_watching_data` is private: the server requires an authenticated actor and adds its `user_id` predicate; a caller supplied predicate for another user is denied. Redis `facereview:session:` is not a supported capability because there is no server-owned session-to-principal mapping. A configured Redis prefix alone does not grant access. Mongo supports only allowlisted collections, fields and read commands; filter/projection/sort shapes, positive limits, result bytes, command time and Redis MGET key count/bytes are bounded. Extension reads still reject `ctx.data.apply` with `WIRE.ACCESS_DENIED`.

Read extensions declare Redis grants with `access redis "<key or prefix:*>"` (a key, or a prefix ending in `:` followed by one `*`); `mongo.video_emotion_timeline` is a derived Mongo collection: the server sums every viewer's `emotion_score_timeline` of one video into 100 buckets (`{command:"timeline", video_id, duration}`) and returns only those averages, so a grant to it never opens the private `youtube_watching_data`. `mongo.video_distribution_stats` (WBS 7.2.9) is a derived collection too: `{command:"health"}` runs the administrator dashboard's `$facet` pipeline over the `video_distribution` documents that have frames and returns the rounded averages and the dominant emotion counts, no document; the extension that holds the grant decides who may ask. `mongo.server_status` answers the allowlisted `host` (CPU, memory and disk usage of the server's machine on Linux, `null` elsewhere) and `ping`; `mongo.video_session_stats` (`distribution_sums`, one video) returns only the sums of the sessions of every viewer, so a job can recompute a distribution without the private sessions leaving. A job writes the private MongoDB documents of its owner under the owner's UUID, like an extension (before WBS 7.2.9 it wrote the internal integer handle).

**Writes to MongoDB and Redis (WBS 7.2.5).** A `write` extension may declare the same grants and write through `ctx.mongo` (`insert_one`, `update_one`, `find_one_and_update`, `delete_one`) and `ctx.redis` (`set_ex`, `set_nx_ex`, `delete`, `getdel`, `incr_expire`, `expire`, `push_trim_expire`, `replace_hash_ex`). Mongo writes need `workers.mongo.write_collections` (an allowlist out of `youtube_watching_data`, `video_distribution`, `video_timeline_emotion_count`; empty keeps Mongo read-only) and may set `ensure_indexes` to create the unique and lookup indexes of the original repositories and of the saga log at start. Writes are bounded: per-collection top-level field allowlists, update operators `$set`/`$setOnInsert`/`$inc` only, dotted paths of at most four segments, no `$` keys in values, 1 MB documents, filters limited to allowlisted fields and `$in`, Redis TTL 1 s to 30 days, 1 MB values (hashes: 64 fields, 2 MB in total, replaced as a whole), 20 calls per invocation. Redis reads add `lrange` (a window of at most 1000 elements) next to `get`, `hgetall`, `exists` and `mget`. `youtube_watching_data` stays private: the server forces `user_id` to the verified actor in filters, inserts and updates.

**Writes from durable jobs (WBS 7.2.7, MySQL).** A `job` declares the same grants (`access mongo.video_distribution, redis "facereview:reco:*"`) and writes through the same `ctx.mongo` and `ctx.redis` helpers, adding a stable effect key (Python: `key=` for Mongo, `effect_key=` for Redis; Node: a last argument `{key}`). The server journals each write like an HTTP effect: the intent first, the answer in the same fenced transaction, a finished key replayed and never sent twice, another input under the same key refused (`WIRE.IDEMPOTENCY_MISMATCH`). The store refusing a command drops the intent and returns its code. When the store does not answer, a write that leaves the same data when sent again (`set_ex`, `replace_hash_ex`, `del`, `expire`, `update_one` with only `$set`/`$setOnInsert`, `delete_one`) fails with `WIRE.ADAPTER_UNAVAILABLE` and may be sent again under the same key (catch it and call `ctx.retryLater()`); any other write (a counter, a list push, an insert, `$inc`) is recorded as unknown and never sent again (`WIRE.EFFECT_UNKNOWN`). A job has no saga: `compensate` is refused, because a retry converges on the journaled results. The allowlists above (`write_collections`, Redis `prefixes`) apply unchanged; without `workers.mongo` or `workers.redis` the call fails with `WIRE.NOT_EXPOSED`. One job run makes at most 64 ctx calls; raise it with `limits.job_calls` (1 to 2000), counting one call per page, chunk or effect.

**Creating rows from a job (WBS 7.2.7, MySQL).** `access create.<Resource>` (the resource must have an `expose create`) lets a job call `ctx.data.create(res, values, key)`, which returns the id of the new row. The server runs it as a one-operation atomic bundle in the same fenced transaction as the effect journal row, under the job principal's own `allow` rule, so the create contract (field allowlist, defaults, unique constraints) is the one a client meets. A unique violation is recorded as `WIRE.ALREADY_EXISTS` and replays as that error, which the extension may catch and skip.

**Query-string endpoints (WBS 7.2.7).** An integration with `"method": "GET"` and `"query_params": ["part", "pageToken", ...]` is called with `ctx.http.call(name, {part: "snippet", pageToken: "..."}, key)`: the extension supplies scalar values for the listed names only (percent-encoded into the query, no body); the URL, the method and the secret stay the server's. `"secret_query_env": {"name": "key", "env": "YOUTUBE_API_KEY"}` adds a query parameter whose value comes from the service environment, so an API key never reaches the worker. Names, secret and allowed parameters are part of the endpoint fingerprint. A query endpoint only reads, so a call whose answer was lost is not recorded as unknown: the intent is dropped (`WIRE.DELIVERY_FAILED`) and the same key may ask again.

The relational transaction cannot include these writes, so they follow the original's saga (`common/saga`). Every external effect happens before the relational commit. A call that passes `compensate=True` keeps a way back (insert: delete it; upsert: delete the new row or restore the old document; delete: insert it back; Redis `set_ex`/`set_nx_ex`/`delete`/`getdel`: restore the old value and remaining life, or delete). If the extension fails, the relational checks fail or the commit is refused, the server undoes the compensable steps newest first; one failing undo does not stop the others and the call answers `WIRE.COMPENSATION_FAILED` naming the steps. A commit whose result is unknown (`WIRE.COMMIT_UNKNOWN`) is **not** undone, because the transaction may have committed. Calls without `compensate` (frame upserts, counters, lists) are not undone. The saga log is the original's `saga_transaction_log` document (`transaction_id`, `status` `in_progress`/`completed`/`compensating`/`compensated`/`failed`, `steps[]` with `compensation_data`); it is written before the first compensable effect and rewritten as the saga advances, and its extra status `unknown` marks a lost commit answer. If Mongo cannot take the log entry, the compensable effect does not happen. Without Mongo the log does not exist and compensation still runs from memory. The server is the only writer of the log collection: no extension grant reaches it.

### Circuit breaker

endpoint마다 `circuit_breaker`를 설정하면 Resilience4j·opossum과 같은 방식으로 장애 provider에 요청을 계속 보내지 않는다.

```json
{"name":"crm","url":"https://crm.example/v1/tickets","method":"POST","timeout_ms":5000,
 "max_request_bytes":65536,"max_response_bytes":65536,
 "circuit_breaker":{"failure_threshold":5,"open_ms":30000}}
```

- 연속 `failure_threshold`회(1회에서 1000회) 연결 실패나 5xx 응답이 나면 `open_ms`(최대 1시간) 동안 그 endpoint로 요청을 보내지 않는다. 5xx 응답 자체는 지금처럼 확장에 status로 돌려준다.
- 열린 동안의 `ctx.http.call`은 `WIRE.CIRCUIT_OPEN`을 던진다. 요청을 보내지 않은 것이 확실하므로 effect journal에 기록을 남기지 않고, `WIRE.EFFECT_UNKNOWN`과 달리 확장이 잡아서 분기하거나 `throw ctx.retryLater()`로 같은 key를 나중에 다시 보낼 수 있다. 이미 journal에 기록된 같은 key 호출은 circuit과 무관하게 재생된다.
- `open_ms`가 지나면 요청 하나만 시험으로 보낸다(half-open). 성공하면 닫고, 실패하면 다시 연다. 시험 요청이 끝나지 않으면 `open_ms` 뒤에 다른 시험을 허용한다.
- 상태는 서버 process마다 따로 둔다. 여러 instance가 상태를 공유하지 않는다. 설정값은 요청의 의미를 바꾸지 않으므로 implementation fingerprint에 넣지 않는다.

### OAuth client credentials

endpoint가 OAuth 2.0 access token을 요구하면 `oauth2`를 설정한다. Spring OAuth2 Client의 `client_credentials` 등록, authlib `OAuth2Session`에 해당한다. 서버가 token을 받아 캐시하고 `Authorization: Bearer` header로 붙인다. 확장은 token을 보지 못한다.

```json
{"name":"crm","url":"https://api.crm.example/v1/tickets","method":"POST","timeout_ms":5000,
 "max_request_bytes":65536,"max_response_bytes":65536,
 "oauth2":{"token_url":"https://auth.crm.example/oauth/token","client_id_env":"CRM_CLIENT_ID",
           "client_secret_env":"CRM_CLIENT_SECRET","scope":"tickets.write","auth_method":"client_secret_basic"}}
```

- client id와 secret은 지정한 환경 변수에서만 읽는다. `auth_method`는 `client_secret_basic`(기본, HTTP Basic)이나 `client_secret_post`(form 필드)다.
- token endpoint도 API endpoint와 같은 규칙을 거친다. 시작할 때 DNS를 고정하고, HTTPS여야 하며, 사설 주소는 `allow_private_networks`가 있을 때만 허용한다. redirect와 proxy는 따르지 않는다.
- token은 `expires_in`(없으면 5분, 최대 24시간)보다 30초 먼저 갱신한다. API가 401을 돌려주면 캐시를 버려 다음 호출이 새 token을 받는다. 401 응답 자체는 확장에 그대로 돌려준다.
- token을 받지 못하면 API 요청을 보내지 않고 `WIRE.INTEGRATION_AUTH_FAILED`를 던진다. `WIRE.CIRCUIT_OPEN`처럼 확정 미전송이므로 journal에 기록을 남기지 않고 같은 key로 다시 보낼 수 있다.
- `oauth2`를 쓰는 endpoint는 `secret_header_env`로 `authorization` header를 설정할 수 없다. token endpoint·client id·secret digest·scope는 implementation fingerprint에 들어간다. 어느 계정으로 호출하는지가 바뀌기 때문이다.
- authorization code 흐름(사용자별 token 저장과 refresh)은 지원하지 않는다. 사용자 계정 연동은 앱이 token을 업무 데이터로 저장하고 확장에서 쓰는 방식이 필요하며, 저장 암호화는 아직 제품 기능이 아니다.

## 스케줄과 웹훅 수신

요청한 사용자가 없는 작업(정기 정산, 외부 서비스 webhook 처리)은 서비스 설정의 `schedules`, `webhooks`로 선언한다. 둘 다 별도 controller 없이 durable job을 수락하며, 운영자가 `aip service principal bind`로 등록한 **서비스 주체**로 실행한다. job은 사용자가 수락한 job과 같은 배포 fence, 주체 활성 검사, operation `allow`, capability 선언, 비용 상한을 거친다. 서비스 주체를 `revoke`하거나 비활성화하면 다음 trigger부터 수락하지 않는다.

```json
{
  "schedules": [
    {"name":"nightly-sweep","operation":"sweep","cron":"0 3 * * *","timezone":"Asia/Seoul","input":{},"principal":"svc-scheduler","misfire_grace_ms":60000}
  ],
  "webhooks": [
    {"name":"stripe","path":"/hooks/stripe","operation":"receive","principal":"svc-webhooks","secret_env":"STRIPE_WEBHOOK_SECRET",
     "signature":{"kind":"stripe","tolerance_seconds":300},"max_body_bytes":262144},
    {"name":"github","path":"/hooks/github","operation":"receive","principal":"svc-webhooks","secret_env":"GITHUB_WEBHOOK_SECRET",
     "signature":{"kind":"hmac-sha256","header":"x-hub-signature-256","prefix":"sha256="}}
  ]
}
```

`principal`은 설정한 `auth.issuer` 아래의 subject다. 서비스 주체가 수락한 job은 owner당 active 100개 상한을 그대로 받지만, 보존 job 10,000개 상한은 적용하지 않는다. trigger는 시간이 지나면 끝없이 쌓이므로 보존 상한이 있으면 한 달 뒤 모든 trigger가 멈추기 때문이다. 서버 시작 시 operation 존재, 입력 계약, cron·시간대, 경로 충돌, secret을 검사하고 문제가 있으면 `TRIGGER_CONFIG`로 시작을 거부한다. trigger는 durable job·worker·인증이 설정된 서버에서만 켤 수 있다.

**스케줄**

- cron은 5필드(`분 시 일 월 요일`)와 `@hourly`·`@daily`·`@weekly`·`@monthly`·`@yearly`를 지원한다. 목록·범위·step·영문 월/요일 이름을 쓸 수 있고, 일과 요일을 모두 제한하면 둘 중 하나만 맞아도 실행한다(Vixie cron 규칙). 시간대는 IANA 이름이다. DST로 건너뛴 지역 시각은 실행하지 않는다. 두 번 나타나는 시간대에서 시(hour)를 지정한 스케줄은 첫 번째만, 시가 `*`인 스케줄은 두 번 모두 실행해 주기를 유지한다.
- 각 instance는 5초마다 가장 최근 slot 하나를 확인한다. slot마다 `schedule:<name>:<unix초>` job key를 쓰므로 여러 instance와 재시작이 있어도 한 slot은 job 하나로 수렴한다. 다운타임 동안 놓친 slot 중 `misfire_grace_ms`(기본 60초, 10초 이상 24시간 이하) 안의 가장 최근 것만 실행하고 이전 slot을 몰아서 실행하지 않는다(APScheduler의 coalesce와 같은 선택).
- job 입력은 설정의 고정 `input`이며 job 입력 계약으로 검사한다.

**웹훅 수신**

- 서버 route는 설정한 `path`의 POST만 받는다. job 입력 계약은 정확히 `{ body: Text }`여야 하며 원문 UTF-8 본문이 그대로 전달된다. 파싱과 업무 분기는 확장에서 한다.
- 서명은 받은 바이트 그대로 검증한다. `hmac-sha256`(GitHub·Shopify 계열, hex 또는 `base64:true`), `stripe`(`t=`/`v1=`, 회전 중 여러 v1 허용), `standard-webhooks`(`webhook-id`·`webhook-timestamp`·`webhook-signature`, `whsec_` secret)를 지원한다. 비교는 constant-time이고, 시간 기반 방식은 허용 오차를 넘는 timestamp를 거부한다. 같은 서명 header가 중복되면 거부한다.
- job key는 서명된 값에서만 만든다. Standard Webhooks는 `webhook-id`, `stripe`와 `hmac-sha256`은 본문 SHA-256이다. 서명 범위 밖의 delivery header로 key를 만들면 가로챈 요청의 header만 바꿔 job을 복제할 수 있기 때문이다. provider 재전송은 같은 job을 돌려주고, 같은 `webhook-id`에 다른 본문은 `409`로 거부한다.
- `hmac-sha256`에는 timestamp가 없어 재전송 시간 창이 없다. 같은 본문의 재전송은 job metadata가 남아 있는 동안(결과 만료 후 30일) 같은 job으로 수렴하지만 그 뒤에는 새 job이 된다. 재전송 방어가 중요하면 timestamp를 서명하는 `stripe`·`standard-webhooks` 방식을 쓰거나 handler에서 업무 키로 중복을 막는다.
- 응답은 provider 재시도 관례를 따른다. 수락 `202`(`{"ok":true,"id":...}`), 서명 실패 `401`, 형식·입력 오류 `400`, 본문 상한 초과 `413`, quota 초과 `429`, DB·주체·배포 문제는 `503`과 `Retry-After`다.
- 이 route는 세션 rate limit 대상이 아니다. 서명 검증은 DB 접근 전에 끝나지만 provider IP 제한과 본문·연결 상한은 TLS proxy에서 함께 설정한다. 서명 secret은 환경 변수로만 읽고 출력하지 않는다.

**웹훅 발신 (구독자 URL로 전달)**

`workers.outbound_webhooks`에 `{"name":"subscribers","signing_secret_env":"...","timeout_ms":5000,"max_request_bytes":65536}`을 두고 job에 `access http.subscribers`를 선언하면 `ctx.webhooks.deliver(name, url, payload, key)`로 업무 데이터에 저장된 구독자 URL에 보낸다. 요청은 Standard Webhooks 형식(`webhook-id`·`webhook-timestamp`·`webhook-signature: v1,<base64>`)으로 서명하고, `webhook-id`는 run과 effect key에서 만들어 재시도에도 같다. URL은 매번 DNS를 해석해 공개 주소만 허용하며 사설 주소는 설정으로도 열 수 없다(`allow_loopback`은 개발용). 2xx는 journal에 기록하고 같은 key는 재전송하지 않는다. 거부(`WIRE.DESTINATION_REJECTED`)·비2xx·전송 실패(`WIRE.DELIVERY_FAILED`)는 기록을 지우고 오류를 돌려주므로 확장이 잡거나 `ctx.retryLater()`로 같은 key를 다시 보낼 수 있다. 수신자는 `webhook-id`로 중복을 걸러야 한다. 구독 관리 resource와 자동 비활성화 정책은 앱 선언으로 두고, 아래 정책 예제를 따른다.

### 구독 관리와 연속 실패 자동 비활성화

구독자 URL은 업무 데이터이고 실패를 어떻게 셀지는 업무 판단이라 플랫폼이 비활성화를 대신하지 않는다. 대신 이미 있는 것(전이, `ctx.data`, `ctx.webhooks.deliver`의 오류 코드)만으로 같은 정책을 쓰는 방법을 정하고 시험으로 고정했다. 정의에 구독 행이 연속 실패 수를 세고, 확장이 전달 결과에 따라 전이를 부른다.

```
resource Subscription {
 fields { id: Id; member: Member; url: Text; active: Bool; failures: Int }
 rows read when member = actor
 transition failed    { allow member = actor; from active = true; to failures = failures + 1 }
 transition recovered { allow member = actor; from failures > 0;  to failures = 0 }
 transition disable   { allow member = actor; from active = true; to active = false }
 expose apply failed { target id; bulk maxRows 5 }  // recovered, disable도 같게
}
operation job fanout { input { event: Text } access read.Subscription, Subscription.failed, Subscription.recovered, Subscription.disable, http.subscribers  ... }
```

| 전달 결과 | 정책 | 이유 |
|---|---|---|
| 2xx | 연속 실패 수가 0보다 크면 `recovered`로 0으로 되돌린다 | 한 번의 성공이 연속 실패를 끊는다 |
| `WIRE.DELIVERY_FAILED`(비2xx·전송 실패) | `failed`로 센다. 연속 N번째면 `disable`한다 | 수신자 장애는 잠깐일 수 있어 몇 번은 기다린다 |
| `WIRE.DESTINATION_REJECTED`(공개 주소가 아니거나 URL이 틀림) | 한 번에 `disable`한다 | 기다려도 유효해지지 않는다 |
| 그 밖의 오류(`WIRE.EFFECT_UNKNOWN` 등) | 잡지 않고 던진다 | 결과를 모르는 전달을 실패로 세면 안 된다 |

`active`가 거짓인 구독은 다음 전달부터 건너뛴다. N은 확장의 상수이고 예제는 3이다(운영에서는 5~10처럼 수신자의 복구 시간을 보고 고른다). 다시 켜는 전이(`from active = false; to active = true`와 `failures = 0`)와 소유자에게 알리는 일(`disable` 직후 같은 확장에서 `ctx.messages.publish`나 `ctx.events.publish`)은 앱이 필요에 따라 더한다. 예제에는 없고 시험하지 않았다. 구독 행의 전이는 모두 job 주체의 정책으로 실행되고 감사 행에도 남는다. 근거 테스트는 `spikes/spike-v6-transport/tests/webhook_subscriptions.rs`(Node·Python)이고 예제 정의와 확장은 `spikes/spike-v6-transport/tests/fixtures/webhook-subscriptions*`에 있다.

**전이 commit 뒤 후속 job (outbox 릴레이)**

전이 선언의 `notify <수신자> "<topic>"` 효과는 전이와 같은 transaction에서 `aip_outbox` 행을 남긴다. 전이가 rollback되면 행도 없다. `outbox_handlers`는 이 행을 durable job으로 넘긴다. Spring의 `@TransactionalEventListener(AFTER_COMMIT)`, Django의 `transaction.on_commit`, Rails의 `after_commit` + ActiveJob에 해당하지만, commit 직후 process가 죽어도 행이 남아 있으므로 후속 작업을 잃지 않는다.

```aip
resource Order {
 fields { id: Id; member: Member; paid: Bool; shipped: Bool }
 transition pay { allow member = actor; from paid = false; to paid = true; notify member "order.paid" }
 transition ship { allow member = actor; from shipped = false; to shipped = true }
 expose apply pay { target id }
 expose apply ship { target id }
}
operation job fulfil {
  input { event: Text(1..4096) }
  output { shipped: Int }
  allow actor != null
  access Order.ship
  effect managed
  deadline 10s
  implementation "orders.fulfil"
}
```

```json
{"outbox_handlers":[{"topic":"order.paid","operation":"fulfil","principal":"svc-relay"}]}
```

```js
export async function fulfil(input, ctx) {
  const event = JSON.parse(input.event); // {id, topic, source, sourceId, recipientId, createdAt}
  const result = await ctx.data.apply("Order", "ship", [event.sourceId], `ship-${event.id}`);
  return { shipped: result.changed.length };
}
```

- 각 instance가 1초마다 topic별로 처리하지 않은 행을 id 순으로 최대 50개 읽고 `outbox:<행 id>:<생성 시각>` key로 job을 수락한다. 같은 transaction에서 dispatch 표시를 남기므로 표시만 있고 job이 없는 상태는 생기지 않는다. 여러 instance가 경쟁해도 행당 job은 하나다. 이 표시 테이블은 앱 migration 지문을 바꾸지 않도록 `aip_infra_<digest>` schema에 둔다.
- 전달은 at-least-once 수락, 실행은 job 계약(재시도·journal)을 따른다. 행 사이의 실행 순서는 보장하지 않는다. topic의 job 수락이 실패하면(주체 비활성 등) 그 tick에서 해당 topic의 나머지 행만 미루고 다른 topic은 계속 처리한다. 같은 행이 계속 실패하면 그 topic은 운영자가 원인을 고칠 때까지 진행하지 않는다(dead 표시는 아직 없다). dispatch 기록은 행의 생성 시각까지 묶으므로 앱 schema를 다시 만들어 sequence가 처음부터 시작해도 옛 기록과 섞이지 않는다.
- 설정한 topic은 정의의 `notify` 효과가 실제로 내보내는 것이어야 하고, job 입력은 정확히 `{ event: Text }`여야 한다.
- 처리 대상 조회는 미처리 행 anti-join이다. relay가 넘긴 행과 dispatch 기록은 `retention.outbox_dispatched_days`(기본 30일) 뒤에 지운다. handler가 없는 topic의 행은 기본으로 보존하며 `retention.outbox_unhandled_days`를 정하면 그 기간 뒤에 지운다. handler가 있는 topic의 미전달 행은 지우지 않는다. 설정과 한계는 [서버 인프라](INFRASTRUCTURE.md#기록-보존-정리retention)를 따른다.

### 메시지 큐 소비 job

`message_handlers`는 관리 메시지 큐의 메시지 하나를 durable job 하나로 넘긴다. Spring `@KafkaListener`·`@RabbitListener`, NestJS `@MessagePattern`, Celery consumer에 해당한다. 별도 consumer 프로세스나 ack 코드를 쓰지 않는다.

```json
{"messaging":{"enabled":true},
 "message_handlers":[
   {"queue":"orders","operation":"handleOrder","principal":"svc-orders"},
   {"queue":"partner_events","operation":"handlePartner","principal":"svc-partner","owner":"principal-owner-key"}
 ]}
```

```js
export async function handleOrder(input, ctx) {
  const message = JSON.parse(input.message); // {id, queue, payload, deliveries}
  const result = await ctx.data.apply("Order", "pay", [message.payload.order], `pay-${message.id}`);
  return { paid: result.changed.length };
}
```

- 메시지는 owner 범위로 나뉜다. `owner`를 생략하면 서비스 주체 자신의 범위를 소비한다. 그 주체로 실행되는 job이 `ctx.messages.publish`로 보낸 메시지가 여기에 들어간다. Kafka·RabbitMQ inbound route([BROKERS](BROKERS.md))로 들어온 메시지는 route에 적은 owner key를 같이 적는다.
- 각 instance가 1초마다 handler별로 메시지를 최대 50개 lease하고 `message:<메시지 id>` key로 job을 수락한다. job 수락과 메시지 ack가 한 transaction이다. 수락 뒤 ack 전에 process가 죽으면 lease가 만료되어 다시 전달되지만 같은 key라 job은 하나다. 여러 instance가 경쟁해도 메시지당 job은 하나다. `message:` key는 사용자가 쓸 수 없다.
- 수락이 실패하면(서비스 주체 비활성, quota 초과 등) 메시지를 nack한다. 메시지 큐의 backoff를 거쳐 `max_deliveries`를 넘으면 DLQ(`DEAD`)로 옮기고 `last_error`에 이유를 남긴다. DLQ 조회와 redrive는 기존 메시지 API를 쓴다.
- 메시지 처리 실패(확장 예외)는 메시지가 아니라 job의 재시도 정책을 따른다. 메시지는 job 수락 시점에 ack된다.
- 시작 시 `messaging.enabled`, 정의의 `messages.<queue>` 선언, job 입력이 정확히 `{ message: Text }`인지를 확인한다. 같은 queue·owner·주체 조합은 한 번만 쓸 수 있다.
- 메시지 사이의 실행 순서는 보장하지 않는다(E09).

## 재시도 정책

job은 다음 경우에만 같은 run을 다시 시도한다. 시도 사이는 1초부터 두 배씩 늘려 최대 30초를 기다린다(exponential backoff).

| 실패 | 재시도 | 이유 |
|---|---|---|
| worker 비정상 종료(`WIRE.WORKER_FAILED`), DB 장애(`WIRE.DB_UNAVAILABLE`) | 항상 | 인프라 실패라 다시 하면 결과가 달라질 수 있다 |
| 확장이 `throw ctx.retryLater("이유")`(Python `raise ctx.retry_later("이유")`)로 요청 | 항상 | 외부 서비스 준비 대기, rate limit 같은 일시 실패를 확장이 알고 있다. Celery `self.retry()`, ActiveJob `retry_job`에 해당한다 |
| 잡지 않은 확장 예외(`WIRE.EXTENSION_ERROR`) | 설정한 job만 | Sidekiq처럼 모든 예외를 재시도하면 코드 버그가 비용만 늘린다. 운영자가 `retry_uncaught_errors`로 고른다 |
| 계약·권한·비용·`WIRE.EFFECT_UNKNOWN`·배포 변경 | 안 함 | 다시 시도해도 결과가 같거나, 외부 효과를 반복할 위험이 있다 |

재시도는 effect journal을 재생하므로 이미 기록된 파일·이벤트·메시지·HTTP·데이터 전이를 반복하지 않는다. 확장은 `ctx.job`(`{id, attempt}`, attempt는 1부터)으로 몇 번째 시도인지 알 수 있다. 시도 횟수는 서비스 설정으로 job마다 정한다. 기본은 3회, 최대 10회다.

```json
{"job_policies":[{"operation":"syncOrders","max_attempts":6,"retry_uncaught_errors":true}]}
```

- 시도를 다 쓰면 `FAILED`가 되고 `error`에 마지막 실패 코드(`WIRE.RETRY_LATER` 등)가 남는다.
- 확장이 대기 시간을 정할 수 있다: `throw ctx.retryLater("rate limited", { afterMs: 60000 })`(Python `raise ctx.retry_later("rate limited", after_ms=60000)`). 외부 서비스가 `Retry-After`를 알려 준 경우처럼 확장이 다음 시도 시점을 더 잘 안다. 이 값은 기본 backoff를 대신하고, 서버가 1초 이상 1시간 이하로 맞춘다(그보다 짧거나 긴 값, 음수·정수가 아닌 값은 각각 올리거나 자르거나 무시해 기본 backoff를 쓴다). 늘어난 대기만큼 run의 전체 기한도 늘려 재시도가 시작도 전에 만료되지 않게 한다. 시도 횟수 상한은 그대로 적용된다. 근거 테스트는 `spikes/spike-v6-transport/tests/job_retry.rs`(Node·Python, 3초 요청이 기본 1초가 아닌 3초 뒤에 다시 실행됨)와 `crates/aip-execution/tests/jobs.rs`의 `an_extension_can_choose_the_wait_before_the_next_attempt`다.
- 정책은 job을 수락할 때 run에 기록한다. 운영자가 정책을 바꿔도 이미 수락한 run은 원래 횟수를 쓰고, 같은 key 재요청은 기존 run을 돌려준다(충돌이 아니다).
- 정의에 없는 operation이나 job이 아닌 operation, 1회 미만·10회 초과는 서버 시작 시 거부한다. 같은 operation을 두 번 적어도 거부한다.
- 자식 `ctx.jobs.run`이 `retryLater`를 던지면 부모는 `WIRE.STEP_FAILED`를 받는다. 부모가 다시 `ctx.retryLater()`를 던져야 run 전체가 재시도된다.
- 시도 횟수를 정의 문법이 아니라 서비스 설정에 둔 이유: 재시도 횟수는 업무 의미가 아니라 운영 조정값이고 환경마다 다를 수 있다. 스케줄·webhook과 같은 위치에 두어 정의 문법을 늘리지 않았다. 재시도 여부를 고르는 판단(`retryLater`)은 업무 코드가 하므로 확장 코드에 둔다.

## 저장과 운영 경계

- Job 수락, 상태, lease, 실행 결과와 effect journal은 PostgreSQL에 영속화된다. 입력·출력 JSON은 각각 최대 1MiB, 선언 deadline은 최대 1시간, 요청 지연은 최대 24시간이다. 기본 최대 시도는 3회(설정으로 1회에서 10회, [재시도 정책](#재시도-정책))이고 시도별 lease는 30초 단위 heartbeat를 사용한다. 선언 deadline과 전체 run deadline을 넘긴 실행은 완료로 기록할 수 없다. active job은 owner당 100개, 보존 job metadata는 owner당 10,000개로 제한된다.
- 성공/실패 결과와 effect result는 완료 후 24시간 보존한다. 60초마다 collector가 advisory lock을 얻은 한 인스턴스만 GC를 실행한다. 만료 job payload·effect result·파일을 종류별 최대 1,000개씩, 완료 job metadata는 최대 100개씩 지우며 해당 job의 effect metadata도 함께 정리한다. 결과 만료 30일 뒤 metadata를 제거하며 그 후 같은 Job key는 새 수락에 재사용될 수 있다.
- 기존 `/files/*` 관리 파일 bytes는 PostgreSQL `bytea`에 보관한다. 별도 `/objects/*` 대용량 저장은 [서버 인프라](INFRASTRUCTURE.md)를 따른다. owner당 live object 최대 100개·총 10MiB, object 상한 767KiB다. 같은 effect의 재실행은 안정 key, run/effect scope와 이름을 이용해 같은 content를 복구한다. 만료 시각은 실행 중 생성 파일의 경우 run deadline + 24시간, 직접 SDK 업로드의 경우 생성 시각 + 24시간이다. 이 기능은 대용량 업로드 저장소를 대체하지 않는다.
- Event는 owner+stream 순서의 append-only cursor를 제공하고 payload는 최대 64KiB, page aggregate는 최대 1MiB다. stream당 최근 1,000개 payload만 재생할 수 있으며 key ledger는 재생 구간 밖에서도 중복 발행을 막는다. 이 key ledger에는 자동 만료가 없으므로 운영자는 장기 저장 증가를 고려해야 한다. 만료된 cursor에 대해 빈 성공을 반환하지 않는다. SSE/WebSocket 전송을 지원한다. 외부 broker relay와 일반 webhook delivery는 아직 없다.
- Outbound HTTP는 등록 이름으로만 호출한다. 설정 endpoint는 최대 64개다. 요청과 응답은 JSON, 각각 최대 1MiB 및 endpoint별 더 작은 제한을 적용하며 timeout은 최대 30초다. status와 최대 1시간으로 제한한 `Retry-After`를 반환하고 redirect와 HTTP 자동 retry는 하지 않는다. 비밀 header는 출력하지 않는다.
- 외부 provider 요청과 PostgreSQL commit은 하나의 원자적 transaction이 아니다. provider가 처리한 뒤 응답이 유실될 수 있어 결과를 모르는 상태에서는 `WIRE.EFFECT_UNKNOWN`으로 멈추며 다음 자동 재시도로 같은 외부 mutation을 반복하지 않는다. 설정한 provider가 idempotency header를 올바르게 지원하는 경우에도 end-to-end exactly-once를 보장한다고 주장하지 않는다. 앱은 provider의 idempotency·조회 기능에 맞춘 복구 절차를 정해야 한다.
- Node/Python worker의 기본 `auto`는 macOS에서 `MacSandbox`(네트워크·파일 쓰기·사용자 디렉터리 읽기·프로세스 생성 거부), Linux에서 Bubblewrap을 선택한다. macOS 프로파일은 deny 목록 방식이라 시스템 경로 읽기는 열려 있다. 이전의 네트워크 전용 동작은 `isolation: "mac_net_deny"`다. Linux mount·의존성·검증 경계는 [서버 인프라](INFRASTRUCTURE.md)를 따른다. bundle fingerprint는 확장 폴더 최상위 `.mjs`/`.py` 파일을 포함하지만 nested imports와 설치된 package dependency를 해시하지 않는다. 확장 모듈과 의존성은 배포자가 신뢰하고 immutable하게 고정해야 한다.
- Job/file/event/effect 테이블은 durable capability가 있는 정의를 init/migrate할 때 생성된다. SQL 동기 read/write의 기존 요청·transaction 경로는 바뀌지 않는다.
- Job 실행 관측성과 trace 연결: W3C `traceparent`로 인그레스된 요청의 trace는 job enqueue 시 `aip_jobs.trace_id` 컬럼(text)에 `<32자 trace id>-<16자 span id>` 형식으로 보존된다. 뒤의 span id는 enqueue를 받은 HTTP 요청의 span이며, `traceparent`가 없던 요청은 서버가 새로 시작한 trace와 그 요청의 span을 적는다. 컬럼 구조는 바뀌지 않았고 span 없이 trace id만 적힌 기존 행도 그대로 읽힌다. job 실행의 `JOB <operation>` span은 이 span을 부모(`parentSpanId`)로 내보낸다. 값이 형식에 맞지 않으면 새 trace를 시작한다. worker 실행 시 이 trace 컨텍스트를 복원하여 outbound HTTP 호출 및 후속 자식 job으로 trace가 계속 연결된다. job attempt·retry·완료 시 동일 trace_id의 `JOB <operation>` span을 OTLP exporter로 내보내며, 내부 metrics(`aip_jobs_total`, `aip_job_duration_seconds`)가 상태별·코드별로 기록된다. span 속성에는 `job.operation`, `job.id`, `job.epoch`, `job.state`, `aip.code`만 들어가며 입력/출력 payload나 토큰 등 민감값은 포함하지 않는다.

선언, capability mapping, 오류 응답과 실제 worker 예제는 [EXECUTION](EXECUTION.md), 서비스 시작과 운영 절차는 [OPERATIONS](OPERATIONS.md), SDK 패키지 API는 [SDK](SDK.md), 검증 결과는 [VALIDATION](VALIDATION.md)과 [VERIFICATION](VERIFICATION.md)을 함께 본다.
