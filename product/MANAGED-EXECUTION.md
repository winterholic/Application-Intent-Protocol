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

## 저장과 운영 경계

- Job 수락, 상태, lease, 실행 결과와 effect journal은 PostgreSQL에 영속화된다. 입력·출력 JSON은 각각 최대 1MiB, 선언 deadline은 최대 1시간, 요청 지연은 최대 24시간이다. 기본 최대 시도는 3회이고 시도별 lease는 30초 단위 heartbeat를 사용한다. 선언 deadline과 전체 run deadline을 넘긴 실행은 완료로 기록할 수 없다. active job은 owner당 100개, 보존 job metadata는 owner당 10,000개로 제한된다.
- 성공/실패 결과와 effect result는 완료 후 24시간 보존한다. 60초마다 collector가 advisory lock을 얻은 한 인스턴스만 GC를 실행한다. 만료 job payload·effect result·파일을 종류별 최대 1,000개씩, 완료 job metadata는 최대 100개씩 지우며 해당 job의 effect metadata도 함께 정리한다. 결과 만료 30일 뒤 metadata를 제거하며 그 후 같은 Job key는 새 수락에 재사용될 수 있다.
- 기존 `/files/*` 관리 파일 bytes는 PostgreSQL `bytea`에 보관한다. 별도 `/objects/*` 대용량 저장은 [서버 인프라](INFRASTRUCTURE.md)를 따른다. owner당 live object 최대 100개·총 10MiB, object 상한 767KiB다. 같은 effect의 재실행은 안정 key, run/effect scope와 이름을 이용해 같은 content를 복구한다. 만료 시각은 실행 중 생성 파일의 경우 run deadline + 24시간, 직접 SDK 업로드의 경우 생성 시각 + 24시간이다. 이 기능은 대용량 업로드 저장소를 대체하지 않는다.
- Event는 owner+stream 순서의 append-only cursor를 제공하고 payload는 최대 64KiB, page aggregate는 최대 1MiB다. stream당 최근 1,000개 payload만 재생할 수 있으며 key ledger는 재생 구간 밖에서도 중복 발행을 막는다. 이 key ledger에는 자동 만료가 없으므로 운영자는 장기 저장 증가를 고려해야 한다. 만료된 cursor에 대해 빈 성공을 반환하지 않는다. SSE/WebSocket 전송을 지원한다. 외부 broker relay와 일반 webhook delivery는 아직 없다.
- Outbound HTTP는 등록 이름으로만 호출한다. 설정 endpoint는 최대 64개다. 요청과 응답은 JSON, 각각 최대 1MiB 및 endpoint별 더 작은 제한을 적용하며 timeout은 최대 30초다. status와 최대 1시간으로 제한한 `Retry-After`를 반환하고 redirect와 HTTP 자동 retry는 하지 않는다. 비밀 header는 출력하지 않는다.
- 외부 provider 요청과 PostgreSQL commit은 하나의 원자적 transaction이 아니다. provider가 처리한 뒤 응답이 유실될 수 있어 결과를 모르는 상태에서는 `EFFECT_UNKNOWN`으로 멈추며 다음 자동 재시도로 같은 외부 mutation을 반복하지 않는다. 설정한 provider가 idempotency header를 올바르게 지원하는 경우에도 end-to-end exactly-once를 보장한다고 주장하지 않는다. 앱은 provider의 idempotency·조회 기능에 맞춘 복구 절차를 정해야 한다.
- Node/Python worker의 기본 `auto`는 macOS에서 `MacNetDeny`, Linux에서 Bubblewrap을 선택한다. macOS filesystem은 unrestricted이며 trusted extension의 filesystem 접근을 격리하지 않는다. Linux mount·의존성·검증 경계는 [서버 인프라](INFRASTRUCTURE.md)를 따른다. bundle fingerprint는 확장 폴더 최상위 `.mjs`/`.py` 파일을 포함하지만 nested imports와 설치된 package dependency를 해시하지 않는다. 확장 모듈과 의존성은 배포자가 신뢰하고 immutable하게 고정해야 한다.
- Job/file/event/effect 테이블은 durable capability가 있는 정의를 init/migrate할 때 생성된다. SQL 동기 read/write의 기존 요청·transaction 경로는 바뀌지 않는다.

선언, capability mapping, 오류 응답과 실제 worker 예제는 [EXECUTION](EXECUTION.md), 서비스 시작과 운영 절차는 [OPERATIONS](OPERATIONS.md), SDK 패키지 API는 [SDK](SDK.md), 검증 결과는 [VALIDATION](VALIDATION.md)과 [VERIFICATION](VERIFICATION.md)을 함께 본다.
