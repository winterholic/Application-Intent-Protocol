# 서버 인프라와 확장 경계

이 문서는 현재 구현된 설정과 API를 설명한다. 작은 관리 파일은 기존 PostgreSQL `bytea` 저장을 유지한다. 대용량 객체는 선택한 별도 저장소를 사용한다. 디렉터리 URL hosting은 서버 디렉터리를 노출하는 기능이며 DB 파일 저장을 바꾸지 않는다.

## 기본값과 서버 설정

기존 서비스 설정에 필요한 항목만 추가한다. 아래는 **실제 설정 필드**를 보여주는 부분 예제이며 `auth`, `schema` 등 기존 필수 설정은 생략했다. 공개 노출은 `access:"public"`을 명시해야 한다. 상대 경로는 서비스 프로세스 작업 디렉터리 기준이다.

```json
{
  "file_mounts": [{"prefix":"/reports","bucket":"reports"}],
  "static_mounts": [
    {"prefix":"/assets", "directory":"./public", "access":"public", "timeout_ms":5000, "max_lifetime_ms":900000},
    {"prefix":"/downloads", "directory":"./downloads", "access":"authenticated"}
  ],
  "objects": {
    "kind":"local", "directory":"./objects", "max_bytes":1073741824,
    "max_total_bytes":10737418240, "max_objects":1000
  },
  "object_transfer_lifetime_ms":900000,
  "retention":{"outbox_dispatched_days":30, "outbox_unhandled_days":null, "events_days":null},
  "db_pool":{"max_size":16,"acquire_timeout_ms":2000,"idle_timeout_ms":60000},
  "rate_limit":{"limit":120,"window_ms":60000},
  "limits":{"connections":64,"workers":4,"jobs":2,"uploads":4,"per_principal_realtime":4,"per_principal_workers":2,"per_principal_uploads":2},
  "realtime":{"enabled":true,"max_connections":32,"max_lifetime_ms":900000,"heartbeat_ms":15000,"poll_ms":250,"query_poll_ms":5000},
  "messaging":{"enabled":true,"client_publish":false,"client_consume":false},
  "workers":{
    "directory":"./extensions", "language":"python", "isolation":"auto",
    "runtime_executable":"/opt/aip/venv/bin/python",
    "environment":{"PYTHONPATH":"/opt/aip/python-packages"},
    "read_only_paths":["/opt/aip/python-packages"]
  }
}
```

알 수 없는 설정 필드는 거부하며, `limits`는 프로세스별 제한이다. 여러 인스턴스의 합계 제한은 배포 환경에서 별도로 관리한다. `connections` 기본 64, 동기 worker 4, durable job 2, object transfer/control 4다. 실시간 연결도 전체 HTTP 연결 수에 포함된다. 실시간 연결 상한은 일반 요청을 위한 연결 한 개를 남기도록 전체 연결 수보다 작게 제한한다. 설정값 0이나 허용 상한을 넘긴 값은 시작 시 거부한다.

`per_principal_realtime`(기본 4)·`per_principal_workers`(기본 2)·`per_principal_uploads`(기본 2)는 한 principal이 동시에 쥘 수 있는 실시간 연결·동기 worker(RAG 포함)·object 전송 수다. 전역 상한과 함께 적용되므로 인증된 한 사용자가 전역 슬롯을 모두 차지해 다른 사용자를 막을 수 없다. 0은 시작 시 거부한다. 초과하면 `WIRE.SERVER_BUSY`/`WIRE.WORKER_BUSY`/`WIRE.OBJECT_CAPACITY`다. durable job 실행 슬롯(`jobs`)은 principal별로 나누지 않으며 principal당 활성 job 100개 상한만 있다.

`connections:1`은 실시간 연결을 허용하지 않는다. HTTP·durable DB 연결은 `db_pool`의 프로세스별 pool을 공유한다. 정상 반환 때 `ROLLBACK`과 `DISCARD ALL`로 세션 상태를 초기화하며, 취소·오류·reset 실패 연결은 재사용하지 않는다. 대기 시간이 지나면 요청을 거부한다. idle 만료는 다음 checkout 때 정리한다. 인증 제공자·마이그레이션·별도 RAG pool·broker relay의 연결도 PostgreSQL 전체 연결 예산에 포함해야 한다.

선택적 `rate_limit`은 같은 PostgreSQL schema를 쓰는 인스턴스 사이에서 계정과 고정 경로 분류별 quota를 공유한다. DB 시계와 원자적 갱신을 쓰는 fixed window 방식이며, 세션 재발급으로 계정 quota를 초기화하지 않는다. 초과하면 HTTP 429와 `Retry-After`, DB 오류면 실행 거부다. 브라우저 WebSocket은 첫 인증 프레임 뒤 검사하므로 초과 오류는 WebSocket error frame이다. 정적 공개 mount의 익명 quota는 공유 bucket이다. IP별 DDoS 방어나 sliding-window의 균일한 유량 보장은 아니며, TLS proxy의 요청 크기·연결 제한도 필요하다. `rate_limit`를 생략하면 principal·경로 분류별 분당 600회의 기본값이 켜진다(앱 사용 패턴에 맞게 올리거나, 필요하면 `"rate_limit":null`로 명시적으로 끈다). 시작할 때 인프라 schema에 limiter 테이블을 만든다.

rate limiting과 RAG는 앱 schema 이름의 digest로 파생한 별도 `aip_infra_<digest>` schema에 저장한다. 앱 migration의 구조 지문을 유지하기 위한 분리다. 시작 시 schema/table 생성 권한이 필요하며 권한이 없으면 시작을 거부한다. 앱 schema만 선택하는 백업에는 이 데이터가 포함되지 않는다. 인프라 schema도 함께 백업하고, 앱 삭제·owner 폐기 시 데이터 보존과 정리를 별도로 수행한다. 앱 schema 삭제만으로 자동 삭제하지 않는다.

### 기록 보존 정리(`retention`)

서버가 쌓는 기록 중 다른 정리 경로가 없는 것을 정해진 기간 뒤에 지운다. 값은 1~3650일의 정수이고, 범위를 벗어나면 시작 시 거부한다. 정리는 durable job collector가 60초마다 한 인스턴스(advisory lock)에서만 돌린다.

| 설정 | 기본 | 지우는 대상 |
| --- | --- | --- |
| `outbox_dispatched_days` | 30 | relay가 job으로 넘긴 outbox 행과 dispatch 기록을, 넘긴 뒤 이 기간이 지나면 지운다. `outbox_handlers`가 있을 때만 일어난다 |
| `outbox_unhandled_days` | `null`(보존) | handler가 없는 topic의 outbox 행을, 쓴 뒤 이 기간이 지나면 지운다. 운영자가 DB에서 직접 읽는 행일 수 있어 기본은 지우지 않는다. handler가 있는 topic의 미전달 행은 job이 받아야 할 행이므로 이 설정과 무관하게 지우지 않는다 |
| `events_days` | `null`(보존) | 발행 뒤 이 기간이 지난 이벤트. stream마다 최근 1,000개만 남기는 개수 상한은 이 설정과 별개로 항상 적용된다. 지운 범위의 cursor는 빈 구간이 아니라 `WIRE.CURSOR_EXPIRED`를 받는다. 멱등 key 장부와 stream head는 유지한다 |

한계: outbox 행 정리는 id 순서로 가장 오래된 행부터 한 번에 1,000개씩 최대 10번 지우고 첫 번째로 젊은 행에서 멈춘다. `aip_events`의 `created_at`에는 index가 없어(앱 migration 지문을 바꾸지 않으려는 선택) 이벤트 정리는 10분마다 events 테이블을 한 번 훑고 한 번에 최대 1,000개 stream을 처리한다. 이벤트가 수백만 건인 서비스는 이 비용을 감안해 `events_days`를 켠다. 근거 테스트는 `spikes/spike-v6-transport/src/durable.rs`의 `retention_tests`와 `crates/aip-execution/tests/events.rs`다.

## 디렉터리 hosting

`static_mounts`는 별도 controller 없이 URL prefix와 서버 경로를 연결한다. GET/HEAD, MIME, ETag/If-None-Match, 단일 byte Range를 지원한다. 디렉터리 목록, symlink, 경로 순회, 특수 파일은 허용하지 않는다. 파일은 descriptor 기준으로 열고 작은 버퍼로 전송한다. idle write timeout과 전체 전송 수명을 함께 제한한다.

`authenticated`는 유효한 세션을 가진 사용자 모두에게 해당 mount를 허용한다. 사용자별 파일 ACL이나 조직별 ACL이 필요한 파일에는 이를 대신 사용하면 안 된다. 인증된 응답은 `Cache-Control: private, no-store`를 사용한다. principal 폐기는 새 요청에서 확인하며, 이미 전송한 bytes를 회수하지 않는다. 진행 중 정적 스트림의 매 chunk 권한 재검증은 제공하지 않는다.

`file_mounts`는 PostgreSQL 관리 파일을 설정한 URL로 제공한다. 예를 들어 `{"prefix":"/reports","bucket":"reports"}`이면 `GET/HEAD /reports/{fileId}`에서 바이너리를 내려받는다. 저장은 기존 bytea 그대로이고, principal owner·generation·배포 상태와 bucket capability를 검증한다. 공개 파일 링크가 아니므로 Bearer token이 필요하다. 이미 저장한 파일 ID를 경로로 사용하며 임의 SQL이나 저장 경로를 노출하지 않는다.

## 파일과 대용량 객체

| 경로 | 저장 | 목적 |
| --- | --- | --- |
| `/files/*`, `ctx.files`, 설정한 `file_mounts` | PostgreSQL bytea | 작은 관리 파일, job effect journal과 원자적 기록 |
| `/objects/*`, local | 서버 filesystem | 큰 파일을 JSON/base64 없이 streaming |
| `/objects/sign`, multipart | S3 호환 저장소 | 서버를 거치지 않는 signed transfer |
| 설정한 mount prefix | 기존 서버 디렉터리 | 배포 assets 또는 인증된 다운로드 |

Local object ID는 UUID이며 클라이언트가 저장 경로를 지정하지 않는다. 검증된 principal과 generation으로 owner namespace를 정한다. 로컬 API는 다음과 같다. raw 요청에는 Bearer token과 `x-aip-contract`를 보낸다.

- `POST /objects/put`: Content-Length와 Content-Type, binary body. 응답의 `id`로 조회한다.
- `GET /objects/{id}`, `HEAD /objects/{id}`, `DELETE /objects/{id}`.

기본 단일 객체 상한은 5GiB, owner별 합계 10GiB·1,000개다. 크기와 개수는 서버에서 조정한다. namespace 단위 비차단 파일 잠금으로 여러 프로세스의 업로드 quota 경쟁을 막는다. 이 owner quota는 전체 filesystem의 남은 공간이나 서비스 전체 quota를 대체하지 않는다. 서버별 전용 volume quota도 설정해야 한다. GET/HEAD는 다운로드용 attachment, nosniff, private/no-store 헤더를 사용한다. opaque ID와 owner 경계를 검사하며 filesystem 경로 교체에는 descriptor 상대 접근과 `O_NOFOLLOW`를 사용한다.

S3 설정은 `objects`를 다음으로 교체한다. 환경 변수 이름만 설정에 둔다.

```json
{"kind":"s3","endpoint":"https://objects.example.com","bucket":"app-files","region":"us-east-1","access_key_env":"AIP_OBJECT_ACCESS_KEY","secret_key_env":"AIP_OBJECT_SECRET_KEY"}
```

선택 필드 `max_total_bytes`(기본 10GiB)와 `max_objects`(기본 1,000)는 owner별 합계 상한이며 `gcs_hmac`에도 있다. 0이나 허용 범위를 넘는 값은 시작 시 거부한다(`max_objects`는 최대 100,000). S3 객체를 쓰려면 배포 fence가 있는 `aip service serve`여야 한다.

TypeScript 제품 client의 실제 API:

```ts
const signed = await client.objects.sign({method:"PUT", size:file.size, contentType:"application/octet-stream"});
await fetch(signed.url, {method:signed.method, headers:signed.headers, body:file});
const upload = await client.objects.multipartStart({contentType:"application/octet-stream"});
const part = await client.objects.part({...upload, partNumber:1, size:chunk.size});
// 각 PUT 응답의 ETag를 모아서 complete({ ...upload, parts:[{number:1,etag}] }) 호출.
```

SigV4는 Content-Length를 서명해 선언한 크기보다 큰 PUT/part를 막는다. URL 수명은 300초다. presigned URL은 소지자가 사용할 수 있고 발급 이후 principal 폐기가 즉시 URL을 취소하지 않는다. S3 control에는 동시 실행 제한과 owner별 quota가 있다. 객체 bytes는 서버를 지나지 않으므로 서버가 **서명한 크기**를 장부(`aip_infra_<digest>.aip_object_ledger`)에 예약하고 owner 합계(기본 10GiB·1,000개, `objects`의 `max_total_bytes`·`max_objects`로 조정)에 넣어 검사한다. 넘으면 URL을 돌려주지 않고 `WIRE.QUOTA_EXCEEDED`를 낸다. 같은 객체를 다시 서명하면 이전 예약을 대체한다. multipart는 `start`가 객체 한 개, 서명한 part 크기가 같은 part 번호는 대체되어 누적된다. 아직 장부에 없는 upload id의 part 서명과 complete는 `WIRE.NOT_FOUND`다.

예약은 사용되지 않아도 시간이 지나면 정리된다. 서명한 PUT은 발급 65분 뒤(URL 수명 5분과 진행 중 업로드를 위한 1시간), part 서명이 24시간 없던 multipart는 그 시점에, 서버가 `HEAD`로 저장소를 확인한다. 객체가 있으면 실제 크기로 확정하고(계속 quota에 포함), multipart는 저장소에서 abort한 뒤 예약을 푼다. **서명한 PUT의 객체가 아직 없으면 예약을 풀지 않는다.** 서명 URL은 전송을 시작할 때만 유효하면 되므로 한 번의 `HEAD` 부재가 진행 중인 전송이 없다는 증거가 아니다. 그래서 바이트를 계속 센 채 24시간 뒤 한 번 더 확인하고, 그때도 없을 때만 푼다. 늦게 끝난 업로드가 두 번째 확인 전에 도착하면 실제 크기로 확정한다. 이 하루 동안 쓰지 않은 서명 예약도 그 바이트를 계속 차지한다(같은 객체 id를 다시 서명하면 새 예약으로 대체된다). 서명한 DELETE는 URL을 쓰지 않았을 수 있으므로 저장소가 객체 없음을 확인할 때까지 quota를 유지한다. 60초마다 모든 인스턴스가 돌지만 행마다 임대를 잡아 한 곳만 처리한다. 한계: ① 장부가 생기기 전에 올린 객체와 서버를 거치지 않고 provider에 직접 쓴 객체는 quota에 세지 않는다. ② 두 번의 확인(서명 후 약 25시간)이 끝난 뒤에도 끝나지 않은 PUT은 이후 완료되어도 세지 않는다. 따라서 이 quota는 하드 상한이 아니며 아주 느린 전송으로 우회될 수 있다. bucket 수명 주기 규칙과 provider 쪽 용량·비용 제한으로 보완하고 [릴리스 게이트](RELEASE-GATE.md#s3-owner-quota의-10-보장-범위)의 영향 분석을 따른다. ③ 이 장부는 서명한 크기 기준이며 provider 비용이나 bucket 전체 용량을 대신하지 않는다. 배포 시 bucket 정책·수명 주기·비용 제한은 여전히 설정해야 한다. provider에서 Content-Length 서명, path-style URL, ETag, CORS 동작을 검증해야 한다. [TLS MinIO 통합 workflow](../.github/workflows/object-storage.yml)는 실제 S3 호환 HTTP와 multipart를 검사한다. 이는 AWS 계정에서의 IAM·CORS·운영 연동 확인을 대체하지 않는다.

Google Cloud Storage XML API의 HMAC 호환 backend도 같은 객체 API를 사용한다. 실제 설정은 `{"kind":"gcs_hmac","bucket":"app-files","access_key_env":"AIP_GCS_HMAC_ACCESS_ID","secret_key_env":"AIP_GCS_HMAC_SECRET"}`다. endpoint는 Google Storage로 고정하고 서명 scope는 `auto/s3/aws4_request`를 사용한다. ADC·Workload Identity를 사용하는 공식 Node/Python SDK 어댑터는 [providers](providers/README.md)를 참고한다. 이 어댑터는 신뢰한 서버 bridge용이며 `gcs_hmac` 인증과 구분한다. 실제 AWS/GCP 계정 검증은 자격증명 없이 실행할 수 없다.

## SSE와 WebSocket

기존 `events.read/watch` polling을 유지하면서 같은 owner stream과 cursor를 push로 전달한다.

- SSE: `POST /events/subscribe`, Bearer, `Accept: text/event-stream`, JSON `{stream,after,contractFingerprint}`.
- WebSocket: `GET /events/socket`, 허용 Origin으로 upgrade. Bearer header가 있으면 첫 text frame은 구독 JSON이다. 브라우저는 첫 frame에 `{type:"auth",token}`, `{type:"authenticated"}` ACK 뒤 `{type:"subscribe",stream,after,contractFingerprint}`를 보낸다. 인증·구독 제한 시간은 합계 5초이고 frame 크기는 제한된다.

SDK의 `readSse(response,signal)`는 streaming UTF-8·multiline·cursor·취소를 처리하며 line/event 버퍼 상한을 적용한다. 소비자 코드가 연결 종료 뒤 마지막 event ID/cursor로 다시 구독해야 한다. `readSse`는 자동 재연결하지 않는다. 서버가 보존 범위를 벗어난 cursor에 대해 `WIRE.CURSOR_EXPIRED`를 error event로 보내면 소비자가 최신 상태를 다시 조회해야 한다. 서버는 poll마다 인증·배포·generation을 재검증하고 세션 만료나 폐기 시 연결을 끝낸다. 이벤트 조회용 DB 연결이나 배포 lock을 연결 수명 동안 잡고 있지 않는다.

### 읽기 결과 구독

같은 입구(`POST /events/subscribe`, `GET /events/socket`)에 `stream` 대신 `query`를 주면 이벤트가 아니라 읽기 결과를 구독한다. 설계와 한계는 [H04 설계 메모](../plan-docs/backend-capability/h04-data-subscription-2026-10-13.md)에 있다.

```json
{"query":{"read":"Order","select":["id","paid"],"sort":[{"field":"id"}]},"contractFingerprint":"<64 hex>"}
```

- `query`는 `/read`의 요청 본문과 같고 `expose read`로 열린 것만 구독할 수 있다. `stream`·`after`와 함께 쓸 수 없고 `after`도 받지 않는다. 계약 밖 요청은 읽기와 같은 코드로 거부한다.
- 이벤트 payload는 `{"type":"snapshot","rows":[...]}`가 한 번, 이후 `{"type":"changed","rows":[...]}`다. 항상 결과 전체이고 직전에 보낸 것과 같으면 보내지 않는다. 클라이언트는 diff를 적용하지 않고 결과를 통째로 바꾼다. 재연결하면 새 snapshot부터 받는다. SSE `id`와 WebSocket `cursor`는 결과의 digest이며 재개용이 아니다.
- 재조회는 구독자의 세션으로 서버가 한다. 주체 재검증과 배포 fence를 먼저 통과해야 하고, 행·필드 정책은 읽기와 같다. 그래서 다른 구성원의 행은 snapshot에 없고, 접근을 잃은 구독자는 `WIRE.UNAUTHENTICATED` 같은 오류 이벤트를 받고 연결이 끝난다.
- 이 프로세스에서 커밋된 쓰기(`/apply`, 쓰기 확장, durable job의 `ctx.data.apply`)는 100ms 안의 것을 모아 한 번 재조회한다. 다른 인스턴스나 스크립트의 쓰기는 설정 `realtime.query_poll_ms`(기본 5000, 1~3600000)마다의 재조회가 찾는다. 연결 수·수명·heartbeat는 위 이벤트 구독과 같은 상한을 쓴다.
- SDK helper(`connectEventWebSocket`, `readSse`)는 아직 stream cursor 구독만 안다. 읽기 구독은 SSE/WebSocket을 직접 쓴다.

브라우저는 제품 SDK의 `connectEventWebSocket`으로 직접 인증할 수 있다. 토큰을 URL query나 subprotocol에 넣지 않으며 별도 인증 gateway는 필수가 아니다. 외부 연결의 WSS 종료는 기존 TLS reverse proxy가 담당한다. SDK helper는 취소·세션 만료·마지막 cursor를 제공하며 재연결은 호출자가 결정한다. 조직 전체 broadcast나 record별 subscription ACL은 아직 별도 기능이다.

## 메시지 큐와 업무 effect

실제 선언 문법과 Node context 예제:

```aip
actor principal
operation job notify {
  input { text: Text }
  output { length: Int }
  allow actor != null
  access messages.notifications
  effect managed
  deadline 5s
  implementation "jobs.notify"
}
```

```js
export async function notify(input, ctx) {
  await ctx.messages.publish("notifications", {text:input.text}, "notify-v1");
  return {length:[...input.text].length};
}
```

Python도 `await ctx.messages.publish(queue,payload,key)`를 사용한다. 같은 run/effect key를 재실행하면 메시지와 journal을 같은 PostgreSQL transaction에서 한 번 기록한다. job 최종 성공과 모든 effect가 하나의 transaction이라는 뜻은 아니다. 서버가 선언한 queue만 노출하고 각 principal owner로 격리한다. 직접 client publish/consume은 각각 `messaging.client_publish`, `messaging.client_consume`을 서버에서 명시적으로 켜야 한다. 기본은 둘 다 false이며 job capability만으로 직접 접근을 열지 않는다. 직접 queue API는 job의 업무 `allow`를 대신 실행하지 않으므로 이를 켤 때 owner별 payload와 consumer 권한을 별도로 설계한다. `client_consume:true`인 서버의 consumer 예제:

```ts
const lease = await client.messages.claim({queue:"notifications",worker:"mailer",leaseMs:30000});
if (lease) {
  // 외부 메일 전송도 message.id를 이용해 멱등하게 구현한다.
  await client.messages.ack({queue:"notifications",lease});
}
```

ACK/NACK는 owner·queue·worker·epoch·token·lease 만료를 검사한다. at-least-once 전달이므로 consumer 부작용의 멱등성이 필요하다. 재시도·지연·TTL·DLQ·redrive와 페이지 cursor를 제공한다. payload는 최대 512KiB, DLQ 페이지 합계는 1MiB, owner live 메시지는 최대 1,000개·64MiB다. TTL 이후 idempotency 기록도 GC되므로 같은 key의 영구 중복 방지를 주장하지 않는다.

`messages.<queue>`가 있는 정의에만 테이블을 생성한다. 기존 배포에는 migrate plan/apply로 추가하며 마지막 queue capability를 제거할 때 테이블을 자동 삭제하지 않는다. 선택적 `external-brokers` 빌드는 Kafka/RabbitMQ relay와 inbound consumer를 제공한다. 서버 route가 owner·queue·topic/exchange를 결정하며, 확인된 발행 뒤 PostgreSQL lease를 ACK한다. 외부 broker와 DB의 원자적 commit은 보장하지 않으므로 안정 message ID로 소비자 멱등성을 구현한다. 설정과 장애 경계는 [BROKERS](BROKERS.md)를 따른다.

## Linux 격리와 JS/Python 의존성

`auto`는 macOS에서 `MacSandbox`(Seatbelt: 네트워크·파일 쓰기·`/Users` 등 사용자 디렉터리 읽기·프로세스 생성 거부), Linux에서 Bubblewrap을 선택하고 필요한 도구가 없으면 실행을 거부한다. Linux는 빈 mount namespace에 시스템 runtime·확장 폴더·지정한 dependency 디렉터리만 읽기 전용으로 노출한다. 네트워크와 PID namespace를 분리하고 private tmp를 사용한다. HOME 전체나 `/` mount는 거부한다. [Ubuntu CI의 Linux 격리 검사](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38133412454)에서 실제 읽기·쓰기·네트워크 경계와 Node/Python 의존성 import를 확인했다. 전체 transport와 core 회귀 검사는 [Verify workflow](https://github.com/winterholic/Application-Intent-Protocol/actions/workflows/verify.yml)에서 함께 실행한다.

확장을 사용하는 서버는 요청 수신 전에 실제 worker와 같은 runtime·sandbox·mount·환경 설정으로 `--version`을 실행한다. 실행 파일의 존재만 확인하는 대신 namespace 권한이나 runtime 실행 실패도 시작 시 거부한다. 검사는 2초로 제한하고 stdout/stderr는 각각 최대 4KiB만 보관한다. 이는 애플리케이션 확장의 모든 import나 외부 서비스 연결을 보증하는 readiness 검사는 아니다.

Linux 호스트는 Bubblewrap의 user namespace와 namespace 내부 초기화 권한을 허용해야 한다. Ubuntu 24.04 계열 CI에서 `loopback: Failed RTM_NEWADDR: Operation not permitted`로 실행이 종료되는 것을 확인했다. 필요한 AppArmor 프로필은 배포 관리자가 해당 Bubblewrap 실행 파일에 적용하며, AIP는 호스트의 전역 보안 정책을 자동 변경하지 않는다. CI는 배포판 바이너리의 별도 복사본에만 `userns` 프로필을 적용하고 전역 sysctl 제한을 유지한다. [Ubuntu 공식 user namespace 정책과 프로필 설명](https://documentation.ubuntu.com/release-notes/24.04/#unprivileged-user-namespace-restrictions)을 참고한다.

Python venv interpreter, 명시적 PYTHONPATH, Node extension 폴더의 node_modules를 사용한다. 패키지 설치는 배포 과정에서 한다. worker가 임의 패키지를 다운로드하지 않는다. AI/RAG용 파서·토크나이저·계산 라이브러리도 이 경로를 사용하고 외부 API는 이름 기반 `ctx.http` endpoint를 통한다. 모델 서버·vector DB URL이나 secret을 프론트 입력으로 받지 않는다. endpoint별 `allow_private_networks:true`로 MSA HTTPS 사설 주소를 명시적으로 허용할 수 있다. DNS 고정·TLS 검증·redirect/proxy 차단은 유지한다.

Linux의 선택적 `workers.cgroup_root`와 `workers.resource_limits`는 함께 설정한다. 예: `"cgroup_root":"/sys/fs/cgroup/aip.service/workers", "resource_limits":{"memory_bytes":268435456,"pids_max":64,"cpu_quota_micros":50000,"cpu_period_micros":100000}`. 운영자가 cpu/memory/pids controller를 활성화한 cgroup v2 subtree를 미리 위임해야 한다. worker는 exec 전에 leaf cgroup에 들어가며 `memory.max`, `pids.max`, `cpu.max`가 적용된다. 취소·종료는 해당 cgroup의 descendants를 함께 정리한다. 필요한 interface나 권한이 없으면 시작을 거부하고 전역 cgroup·sysctl을 자동 변경하지 않는다. [Linux cgroup workflow](../.github/workflows/cgroup.yml)가 실제 강제 제한을 검사한다.

macOS `MacSandbox`는 deny 목록 방식이라 시스템 경로 읽기는 열려 있고 Linux처럼 보이는 파일 집합을 제한하지는 않는다. `mac_net_deny`는 네트워크만 차단한다. 모든 Unix worker는 process group leader로 시작해 중지 때 후손 프로세스 전체(`setsid` 손자 포함)를 종료한다. cgroup 제한을 설정하면 macOS에서는 시작을 거부한다. worker는 신뢰한 서버 코드다. 의존성 전체 내용은 현재 implementation fingerprint에 들어가지 않으므로 immutable deployment와 lockfile을 사용한다. npm/pip 생태계의 모든 라이브러리 호환성이나 native GPU 사용을 보장하지 않는다.

이름 기반 corpus·embedding/answer provider, owner 격리·문서 분할·색인·검색·출처 반환을 제공하는 RAG 엔진은 [RAG](RAG.md)를 따른다. JS/TS·Python의 특수 파서나 모델은 custom provider/서버 bridge로 연결한다. 프론트에 모델 endpoint·credential·SQL 실행 권한을 주지 않는다.

## 발견과 의존성

인증된 `/capabilities` 응답의 `infrastructure`는 backend 선택, 전송 지원, 공개 URL prefix와 실행 한도를 설명한다. 서버 디렉터리·환경 변수·credential은 노출하지 않는다. 개별 호출의 권한·계약 검사는 계속 수행한다.

### 선택적 패키지 구조

원칙은 두 가지다. 기능의 **동작**은 AIP 선언(`access messages.<queue>`, `operation job` 등)과 서버 설정으로 켜고, 코어가 끌어오는 **외부 라이브러리**는 무거운 것만 옵트인 패키지로 둔다. 소비자는 패키지를 빌드에 포함하는 것으로 끝나며 어댑터 코드를 직접 쓰지 않는다. 권한·owner·route는 계속 서버가 결정한다.

| 기능 | 기본 빌드 | 선택 패키지 | 근거 |
| --- | --- | --- | --- |
| PostgreSQL 저장, 큐, job, 파일(bytea), rate limit, RAG 색인 | 포함 (tokio-postgres) | 없음. 추가 라이브러리가 없어 분리하지 않는다. | `crates/aip-execution/Cargo.toml` |
| local·S3 호환·`gcs_hmac` 객체 저장소 | 포함. `reqwest`·`hmac`·`sha2`만 사용하고 클라우드 SDK는 쓰지 않는다. | 없음. `objects.kind` 설정으로만 선택한다. | `objects.rs`, `gcs.rs` |
| 외부 HTTP 연동, JWKS 인증 | 포함 (`reqwest` rustls, `jsonwebtoken`) | 없음. 서버 계약의 필수 경로다. | `aip-auth`, `integrations` |
| Kafka·RabbitMQ relay와 inbound | **제외** | Cargo feature `external-brokers` (`lapin`, `rdkafka`, C++ librdkafka cmake 빌드) | `aip-execution/Cargo.toml`, `aip-service/Cargo.toml`, `aip-cli/Cargo.toml` |
| MySQL 8.4 읽기 경로(adopt-external·principal·serve·`/read`·`/ready`), MariaDB catalog introspection | 기본 빌드에서 **제외** | Cargo feature `mysql` (`aip-migrate`의 optional `mysql_async`, rustls; MIT OR Apache-2.0). MySQL 8.4.11에서 읽기 경로 live 확인, 쓰기·작업·파일·rate limit은 명시 거절, MariaDB 서빙·채택은 거부([계약](../plan-docs/porting/MYSQL-ADAPTER-CONTRACT.md)) | `crates/aip-migrate/Cargo.toml` → `aip-service/Cargo.toml` → `aip-cli/Cargo.toml` |
| 기존 PoC runtime (`aip run/migrate/rekey/token`, `diff --against deployed`: `axum`, `deadpool-postgres`, `chrono-tz`, `aes-gcm`) | **제외** | Cargo feature `poc-runtime` (`aip-cli`). 제품 경로 `aip service`는 쓰지 않는다. | `aip-cli/Cargo.toml`, [설계](../plan-docs/backend-capability/optional-runtime-package.md) |
| GCS ADC 어댑터 (Node/Python) | 제외 | `product/providers/`. 소비자 서버 프로젝트가 `@google-cloud/storage` 또는 `google-cloud-storage`를 직접 설치한다. | [providers](providers/README.md) |
| JS/TS SDK `@aip/sdk` | 런타임 의존성 없음 | 없음 | `product/tools/build-sdk.mjs`의 package.json |
| W3C `traceparent` 수신·전파 | 포함. 헤더 파싱과 task-local뿐이라 의존성이 늘지 않는다. | 없음 | `crates/aip-execution/src/trace_context.rs` |
| OpenTelemetry exporter (OTLP/HTTP) | 포함. 별도 무거운 SDK 대신 기존 `reqwest` 기반 OTLP/HTTP JSON v1 규격으로 구현. 기본 off. | 없음. `otlp` 설정으로 활성화. | `spikes/spike-v6-transport/src/otlp.rs` |

`aip-cli`의 의존 crate 수는 기본 빌드 293개, `external-brokers` 392개, `poc-runtime` 358개, 둘 다 켜면 450개다(`cargo tree -p aip-cli --locked -e normal --prefix none --features <목록> | sort -u | wc -l`, 2026-10-13 측정). 기본 빌드에는 mysql 의존성이 전혀 포함되지 않는다(`cargo tree -p aip-cli -e normal | grep -i mysql` 0건).

Python 공급 경로도 같은 구조다. 호출자 SDK는 TypeScript `@aip/sdk` 하나이며 Python SDK 패키지는 없다. Python은 서버 extension 언어이고, 그 worker 스크립트(`spikes/spike-v4-worker/workers/worker.py`)는 `asyncio`, `json` 등 표준 라이브러리만 쓰며 Rust 바이너리에 포함된다. extension이 필요로 하는 서드파티 패키지는 배포자가 venv에 설치하고 `workers.runtime_executable`, `workers.environment`의 `PYTHONPATH`, `read_only_paths`로 지정한다. 선택 어댑터(`product/providers/google_storage.py`)는 공식 SDK를 호출 시점에 import하며 없으면 `PROVIDER_PACKAGE_MISSING`이다. `pyproject.toml`이나 pip extras는 만들지 않았다. 배포 가능한 Python 패키지가 생기는 시점에 다시 정한다.

### DB 어댑터 선택 구조와 PostgreSQL 분리 제안

Spring처럼 PostgreSQL과 MySQL 모두 선택한 라이브러리 의존성으로 붙이는 구조로 점진적 이행을 계획한다. 현재는 PostgreSQL이 기본 빌드에 포함되어 있으나, 향후 `postgres` feature(`tokio-postgres` 의존성 분리)로 감싸고 드라이버를 완전히 대칭적인 선택 구조로 전환하는 방안을 제안한다. 기존 프로덕션 테스트 300여 개의 안정성을 고려하여 대규모 PG 분리는 이번에 강행하지 않고 제안 상태로 유지하며, MySQL 어댑터의 실DB 계약 안착 후 진행한다.

### OpenTelemetry (OTLP/HTTP) exporter

무거운 서드파티 SDK 대신 표준 `reqwest` 기반의 OTLP/HTTP JSON v1 프로토콜(`/v1/traces`, `/v1/metrics`)로 구현했다. `otlp` 설정이 없으면 꺼져 있으며(단일 `Option` 검사로 비용 없음), 설정 시 활성화된다.
- 설정: `{"otlp":{"endpoint":"http://collector:4318","headers":{},"timeout_ms":5000}}`.
- 전송: 요청 처리 루프를 차단하지 않도록 bounded mpsc queue(4096)와 백그라운드 tokio worker를 통해 비동기 배치 전송한다. 큐가 가득 차면 span을 버리고 카운트를 집계하여 응답 지연을 방지한다.
- span 원천: `Trace::drop`에서 끝난 HTTP 요청 span(`HTTP <route>`, SERVER)과 durable job 완료/취소 시의 job span(`JOB <operation>`, INTERNAL)을 생성한다.
- durable job trace 연결: HTTP 요청 시 수신된 W3C `traceparent`의 trace_id가 durable job enqueue 시 `aip_jobs.trace_id`에 저장되어 실행 attempt·retry 전반에 보존되며, HTTP 요청과 durable job run이 동일한 trace로 연결된다.
- 민감값 배제: span 속성에는 `http.method`, `http.route`, `http.status_code`, `job.operation`, `job.id`, `job.epoch`, `job.state`, `aip.code`만 들어가며 토큰·쿼리·payload는 제외된다.

패키지가 없는데 설정이 그 기능을 쓰면 **기동 시점**에 거부한다. 기본 빌드에서 `broker` 설정을 주면 DB 접속 전에 `BROKER_FEATURE_DISABLED`로 종료한다(`require_built_features`, `broker_config_is_rejected_when_the_feature_is_not_built` 테스트). 마찬가지로 `mysql` feature 없이 `database_adapter: "mysql"`이나 환경변수 `database_adapter_env` 또는 MySQL/MariaDB URL을 설정하면 DB 접속 시도 전에 `DB_ADAPTER_FEATURE_DISABLED`로 즉시 거부한다(`mysql_config_is_rejected_when_the_feature_is_not_built`, `mysql_scheme_in_url_is_rejected_when_the_feature_is_not_built` 테스트). `mysql` feature build는 `service introspect`의 catalog 읽기만 허용하며, 서빙·정책 read/write·migration/adopt는 `DB_ADAPTER_UNSUPPORTED`로 fail-closed한다. URL 비밀번호는 오류에 넣지 않는다. PoC runtime 명령도 기본 빌드에서는 `RUNTIME_FEATURE_DISABLED`와 재빌드 명령을 출력하고 종료 코드 1이다(`crates/aip-cli/tests/runtime_disabled.rs`). 선택 패키지를 추가하는 기준은 새 외부 C/C++ 빌드 의존성이나 큰 전이 의존성을 끌어오는지다. PostgreSQL과 이미 포함된 crate만으로 구현되는 기능은 feature로 나누지 않고 서버 설정으로 켠다.

새 직접 의존성의 로컬 Cargo package metadata를 확인했다: `tokio-tungstenite 0.30.0`은 MIT, `futures-util 0.3.34`와 `libc 0.2.189`는 MIT OR Apache-2.0이다. 선택 broker 의존성의 locked metadata는 `lapin 4.12.1`, `rdkafka 0.39.0`, `rdkafka-sys 4.10.0+2.12.1`이 MIT이며 `cmake 0.1.58`은 MIT OR Apache-2.0이다. 전체 transitive 의존성의 별도 특허 검토를 끝냈다는 뜻은 아니다.

## 확인된 미해결 운영 경계

다중 프로세스 schema 분리, 운영 tracing/metrics, tenant 전체 broadcast는 후속 검증·구현 대상이다. S3 owner quota와 만료 정리는 [위](#파일과-대용량-객체) 장부로 구현했고 실제 MinIO(TLS)로 검증했으며, AWS·GCS 실계정 검증은 미실행이다. 현재 동시 quota·lease 경쟁·부분 요청 disconnect·느린 WebSocket 소비자·pool 취소를 검사하지만, 실제 운영 트래픽의 장시간 부하나 가용성 SLO를 보증하지 않는다. 기존 HTTP listener는 loopback TLS reverse proxy 뒤에 두는 모델을 유지한다.

참고한 공식 자료: [Bubblewrap README](https://github.com/containers/bubblewrap/blob/main/README.md), [AWS multipart 의미와 수명 주기](https://docs.aws.amazon.com/AmazonS3/latest/userguide/mpuoverview.html). 타 프로젝트 소스를 복사하지 않았으며 Bubblewrap은 배포 환경의 별도 도구로 실행한다.
