# 관리형 durable execution

`operation job`을 호출하면 PostgreSQL에 수락된 작업이 요청 연결과 분리되어 실행된다. 기존 `operation read`, read/apply 및 SQL 동기 경로는 그대로 유지된다. 이 문서는 현재 구현된 durable job, 관리 파일, 이벤트 스트림, 이름 기반 outbound HTTP broker의 경계와 호출법을 설명한다.

정적 디렉터리 hosting, 대용량 local/S3 객체, SSE/WebSocket, 메시지 큐, Linux 격리와 의존성 설정은 [서버 인프라](INFRASTRUCTURE.md)를 함께 따른다.

## 선언과 worker context

작업 전용 앱은 기존 `operation read`에 더해 `operation job`을 선언할 수 있다. job에는 입력·출력 계약, `allow`, deadline, `effect none` 또는 `effect managed`, 그리고 필요할 때 `access` capability 목록을 선언한다.

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

확장 입력은 URL·HTTP method·header·파일 경로·owner를 정하지 못한다. 외부 HTTP와 worker file write에는 안정된 idempotency key를 명시해야 하며, 재시작 후 같은 key에 다른 input을 주면 conflict다. Worker effect key는 ASCII 영숫자와 `_`, `-`, `.`, `:`로 구성한 1바이트 이상 100바이트 이하 문자열이다. 호출마다 현재 durable operation의 선언 capability와 principal, deployment, lease를 다시 검사한다. 결과 반환 직전에도 인증 신원과 operation 권한을 다시 확인한다. 각 managed file/event effect와 해당 effect journal 기록은 자체 PostgreSQL transaction에서 함께 commit되며, Job 최종 output과 한 transaction으로 묶이지는 않는다. HTTP 요청은 먼저 PENDING intent를 commit한 뒤 전송하고 응답 뒤 결과를 journal에 기록하므로 provider 호출과 DB commit은 원자적이지 않다. 자식 `ctx.jobs.run`은 선언된 operation을 같은 worker attempt 안에서 실행하며 별도 queue submission이 아니다. 자식 접근은 부모가 선언한 capability subset으로 제한한다. 중첩 depth는 8, 부모와 자식을 합한 attempt당 broker call은 최대 64회다. 재시도 attempt는 새 예산을 사용하며 최대 시도 횟수로 전체 실행 비용을 제한한다. 알려진 자식 업무 실패는 `STEP_FAILED`로 재생할 수 있어 확장에서 분기·보상할 수 있고, capability 선언 위반·비용·lease·불확실한 효과 오류는 잡아서 성공으로 바꿀 수 없다.

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

- 권한은 job을 수락한 principal의 것이다. 행 정책에서 보이지 않는 행에 대한 전이는 동기 `/apply`와 같은 `MISSING_TARGET`으로 끝나 행 존재 여부를 알리지 않는다. 선언하지 않은 전이는 job 전체가 `ACCESS_NOT_DECLARED`로 실패한다.
- `apply`에는 다른 managed effect처럼 안정 key가 필요하다. 같은 run에서 같은 key를 다시 부르거나 worker가 commit 뒤에 죽어 재시도되면, 전이를 다시 실행하지 않고 journal의 결과를 그대로 돌려준다. 실행 결과를 모르는 상태가 생기지 않으므로 외부 HTTP와 달리 `EFFECT_UNKNOWN`이 없다.
- 전이가 `MISSING_TARGET`, `ACCESS_DENIED`, `INVARIANT_VIOLATED`처럼 업무 규칙으로 거부되면 그 결과를 journal에 남겨 재시도에서도 같은 오류를 재생한다. 확장은 이 오류를 잡아 분기·보상할 수 있다. `CONFLICT`(교착·직렬화)와 DB 장애는 아무것도 commit하지 않았으므로 기록하지 않고 다시 시도할 수 있다.
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

대기 중인 작업 취소는 `CANCELLED`가 되고, 실행 중 취소 요청은 우선 `CANCEL_REQUESTED`가 된다. 실행 중 HTTP effect 결과가 PENDING/UNKNOWN이면 취소 요청은 안전한 취소로 확정하지 않고 `FAILED`와 `CANCELLATION_UNCONFIRMED`로 끝난다. 다른 실패나 worker 종료 뒤 unresolved effect가 남아도 `jobs.status()`/`cancel()` 응답의 선택적 `error`는 `EFFECT_UNKNOWN`을 우선 표시한다. 이 상태는 외부 provider가 작업을 처리했는지 모른다는 뜻이다.

`events.read(stream,{after?,limit?})`는 decimal string cursor와 최대 100개 이벤트를 반환한다. `events.watch`는 read를 주기적으로 polling하는 async iterator다. 기본 간격은 1초, 허용 범위는 50ms에서 30초이며 AbortSignal로 끝낼 수 있다. 같은 cursor를 사용하는 SSE/WebSocket 전송도 제공하며 연결·인증 경계는 [서버 인프라](INFRASTRUCTURE.md)에 설명한다. 보존 범위보다 오래된 cursor는 `CURSOR_EXPIRED`를 반환한다. Event ACL도 enabled principal owner와 현재 revocation generation에 기반하며, publisher operation의 `allow`를 reader에게 재평가하지 않는다. role 변경만으로 저장 event에 대한 접근이 회수되지 않는다. principal revoke/generation 변경은 이전 owner scope의 접근을 막지만 저장된 데이터 자체를 즉시 삭제하지는 않는다. 사용자별 record subscription이나 일반 tenant/organization ACL은 제공하지 않는다.

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
- `aip_outbox`는 아직 자동 정리하지 않으며 처리 대상 조회는 미처리 행 anti-join이다. outbox가 매우 커지는 서비스는 보존 정책을 별도로 정해야 한다.

## 재시도 정책

job은 다음 경우에만 같은 run을 다시 시도한다. 시도 사이는 1초부터 두 배씩 늘려 최대 30초를 기다린다(exponential backoff).

| 실패 | 재시도 | 이유 |
|---|---|---|
| worker 비정상 종료(`WORKER_FAILED`), DB 장애(`DB_UNAVAILABLE`) | 항상 | 인프라 실패라 다시 하면 결과가 달라질 수 있다 |
| 확장이 `throw ctx.retryLater("이유")`(Python `raise ctx.retry_later("이유")`)로 요청 | 항상 | 외부 서비스 준비 대기, rate limit 같은 일시 실패를 확장이 알고 있다. Celery `self.retry()`, ActiveJob `retry_job`에 해당한다 |
| 잡지 않은 확장 예외(`EXTENSION_ERROR`) | 설정한 job만 | Sidekiq처럼 모든 예외를 재시도하면 코드 버그가 비용만 늘린다. 운영자가 `retry_uncaught_errors`로 고른다 |
| 계약·권한·비용·`EFFECT_UNKNOWN`·배포 변경 | 안 함 | 다시 시도해도 결과가 같거나, 외부 효과를 반복할 위험이 있다 |

재시도는 effect journal을 재생하므로 이미 기록된 파일·이벤트·메시지·HTTP·데이터 전이를 반복하지 않는다. 확장은 `ctx.job`(`{id, attempt}`, attempt는 1부터)으로 몇 번째 시도인지 알 수 있다. 시도 횟수는 서비스 설정으로 job마다 정한다. 기본은 3회, 최대 10회다.

```json
{"job_policies":[{"operation":"syncOrders","max_attempts":6,"retry_uncaught_errors":true}]}
```

- 시도를 다 쓰면 `FAILED`가 되고 `error`에 마지막 실패 코드(`RETRY_LATER` 등)가 남는다.
- 정책은 job을 수락할 때 run에 기록한다. 운영자가 정책을 바꿔도 이미 수락한 run은 원래 횟수를 쓰고, 같은 key 재요청은 기존 run을 돌려준다(충돌이 아니다).
- 정의에 없는 operation이나 job이 아닌 operation, 1회 미만·10회 초과는 서버 시작 시 거부한다. 같은 operation을 두 번 적어도 거부한다.
- 자식 `ctx.jobs.run`이 `retryLater`를 던지면 부모는 `STEP_FAILED`를 받는다. 부모가 다시 `ctx.retryLater()`를 던져야 run 전체가 재시도된다.
- 시도 횟수를 정의 문법이 아니라 서비스 설정에 둔 이유: 재시도 횟수는 업무 의미가 아니라 운영 조정값이고 환경마다 다를 수 있다. 스케줄·webhook과 같은 위치에 두어 정의 문법을 늘리지 않았다. 재시도 여부를 고르는 판단(`retryLater`)은 업무 코드가 하므로 확장 코드에 둔다.

## 저장과 운영 경계

- Job 수락, 상태, lease, 실행 결과와 effect journal은 PostgreSQL에 영속화된다. 입력·출력 JSON은 각각 최대 1MiB, 선언 deadline은 최대 1시간, 요청 지연은 최대 24시간이다. 기본 최대 시도는 3회(설정으로 1회에서 10회, [재시도 정책](#재시도-정책))이고 시도별 lease는 30초 단위 heartbeat를 사용한다. 선언 deadline과 전체 run deadline을 넘긴 실행은 완료로 기록할 수 없다. active job은 owner당 100개, 보존 job metadata는 owner당 10,000개로 제한된다.
- 성공/실패 결과와 effect result는 완료 후 24시간 보존한다. 60초마다 collector가 advisory lock을 얻은 한 인스턴스만 GC를 실행한다. 만료 job payload·effect result·파일을 종류별 최대 1,000개씩, 완료 job metadata는 최대 100개씩 지우며 해당 job의 effect metadata도 함께 정리한다. 결과 만료 30일 뒤 metadata를 제거하며 그 후 같은 Job key는 새 수락에 재사용될 수 있다.
- 기존 `/files/*` 관리 파일 bytes는 PostgreSQL `bytea`에 보관한다. 별도 `/objects/*` 대용량 저장은 [서버 인프라](INFRASTRUCTURE.md)를 따른다. owner당 live object 최대 100개·총 10MiB, object 상한 767KiB다. 같은 effect의 재실행은 안정 key, run/effect scope와 이름을 이용해 같은 content를 복구한다. 만료 시각은 실행 중 생성 파일의 경우 run deadline + 24시간, 직접 SDK 업로드의 경우 생성 시각 + 24시간이다. 이 기능은 대용량 업로드 저장소를 대체하지 않는다.
- Event는 owner+stream 순서의 append-only cursor를 제공하고 payload는 최대 64KiB, page aggregate는 최대 1MiB다. stream당 최근 1,000개 payload만 재생할 수 있으며 key ledger는 재생 구간 밖에서도 중복 발행을 막는다. 이 key ledger에는 자동 만료가 없으므로 운영자는 장기 저장 증가를 고려해야 한다. 만료된 cursor에 대해 빈 성공을 반환하지 않는다. SSE/WebSocket 전송을 지원한다. 외부 broker relay와 일반 webhook delivery는 아직 없다.
- Outbound HTTP는 등록 이름으로만 호출한다. 설정 endpoint는 최대 64개다. 요청과 응답은 JSON, 각각 최대 1MiB 및 endpoint별 더 작은 제한을 적용하며 timeout은 최대 30초다. status와 최대 1시간으로 제한한 `Retry-After`를 반환하고 redirect와 HTTP 자동 retry는 하지 않는다. 비밀 header는 출력하지 않는다.
- 외부 provider 요청과 PostgreSQL commit은 하나의 원자적 transaction이 아니다. provider가 처리한 뒤 응답이 유실될 수 있어 결과를 모르는 상태에서는 `EFFECT_UNKNOWN`으로 멈추며 다음 자동 재시도로 같은 외부 mutation을 반복하지 않는다. 설정한 provider가 idempotency header를 올바르게 지원하는 경우에도 end-to-end exactly-once를 보장한다고 주장하지 않는다. 앱은 provider의 idempotency·조회 기능에 맞춘 복구 절차를 정해야 한다.
- Node/Python worker의 기본 `auto`는 macOS에서 `MacNetDeny`, Linux에서 Bubblewrap을 선택한다. macOS filesystem은 unrestricted이며 trusted extension의 filesystem 접근을 격리하지 않는다. Linux mount·의존성·검증 경계는 [서버 인프라](INFRASTRUCTURE.md)를 따른다. bundle fingerprint는 확장 폴더 최상위 `.mjs`/`.py` 파일을 포함하지만 nested imports와 설치된 package dependency를 해시하지 않는다. 확장 모듈과 의존성은 배포자가 신뢰하고 immutable하게 고정해야 한다.
- Job/file/event/effect 테이블은 durable capability가 있는 정의를 init/migrate할 때 생성된다. SQL 동기 read/write의 기존 요청·transaction 경로는 바뀌지 않는다.

선언, capability mapping, 오류 응답과 실제 worker 예제는 [EXECUTION](EXECUTION.md), 서비스 시작과 운영 절차는 [OPERATIONS](OPERATIONS.md), SDK 패키지 API는 [SDK](SDK.md), 검증 결과는 [VALIDATION](VALIDATION.md)과 [VERIFICATION](VERIFICATION.md)을 함께 본다.
