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
  "db_pool":{"max_size":16,"acquire_timeout_ms":2000,"idle_timeout_ms":60000},
  "rate_limit":{"limit":120,"window_ms":60000},
  "limits":{"connections":64,"workers":4,"jobs":2,"uploads":4},
  "realtime":{"enabled":true,"max_connections":32,"max_lifetime_ms":900000,"heartbeat_ms":15000,"poll_ms":250},
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

`connections:1`은 실시간 연결을 허용하지 않는다. HTTP·durable DB 연결은 `db_pool`의 프로세스별 pool을 공유한다. 정상 반환 때 `ROLLBACK`과 `DISCARD ALL`로 세션 상태를 초기화하며, 취소·오류·reset 실패 연결은 재사용하지 않는다. 대기 시간이 지나면 요청을 거부한다. idle 만료는 다음 checkout 때 정리한다. 인증 제공자·마이그레이션·별도 RAG pool·broker relay의 연결도 PostgreSQL 전체 연결 예산에 포함해야 한다.

선택적 `rate_limit`은 같은 PostgreSQL schema를 쓰는 인스턴스 사이에서 계정과 고정 경로 분류별 quota를 공유한다. DB 시계와 원자적 갱신을 쓰는 fixed window 방식이며, 세션 재발급으로 계정 quota를 초기화하지 않는다. 초과하면 HTTP 429와 `Retry-After`, DB 오류면 실행 거부다. 브라우저 WebSocket은 첫 인증 프레임 뒤 검사하므로 초과 오류는 WebSocket error frame이다. 정적 공개 mount의 익명 quota는 공유 bucket이다. IP별 DDoS 방어나 sliding-window의 균일한 유량 보장은 아니며, TLS proxy의 요청 크기·연결 제한도 필요하다. 생략하면 기존 동작을 유지한다.

rate limiting과 RAG는 앱 schema 이름의 digest로 파생한 별도 `aip_infra_<digest>` schema에 저장한다. 앱 migration의 구조 지문을 유지하기 위한 분리다. 시작 시 schema/table 생성 권한이 필요하며 권한이 없으면 시작을 거부한다. 앱 schema만 선택하는 백업에는 이 데이터가 포함되지 않는다. 인프라 schema도 함께 백업하고, 앱 삭제·owner 폐기 시 데이터 보존과 정리를 별도로 수행한다. 앱 schema 삭제만으로 자동 삭제하지 않는다.

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

TypeScript 제품 client의 실제 API:

```ts
const signed = await client.objects.sign({method:"PUT", size:file.size, contentType:"application/octet-stream"});
await fetch(signed.url, {method:signed.method, headers:signed.headers, body:file});
const upload = await client.objects.multipartStart({contentType:"application/octet-stream"});
const part = await client.objects.part({...upload, partNumber:1, size:chunk.size});
// 각 PUT 응답의 ETag를 모아서 complete({ ...upload, parts:[{number:1,etag}] }) 호출.
```

SigV4는 Content-Length를 서명해 선언한 크기보다 큰 PUT/part를 막는다. URL 수명은 300초다. presigned URL은 소지자가 사용할 수 있고 발급 이후 principal 폐기가 즉시 URL을 취소하지 않는다. S3 control에는 동시 실행 제한이 있지만 **owner별 S3 합계 quota와 미완료 multipart 자동 정리는 아직 없다**. 배포 시 bucket 정책·수명 주기·비용 제한을 설정해야 한다. provider에서 Content-Length 서명, path-style URL, ETag, CORS 동작을 검증해야 한다. [TLS MinIO 통합 workflow](../.github/workflows/object-storage.yml)는 실제 S3 호환 HTTP와 multipart를 검사한다. 이는 AWS 계정에서의 IAM·CORS·운영 연동 확인을 대체하지 않는다.

Google Cloud Storage XML API의 HMAC 호환 backend도 같은 객체 API를 사용한다. 실제 설정은 `{"kind":"gcs_hmac","bucket":"app-files","access_key_env":"AIP_GCS_HMAC_ACCESS_ID","secret_key_env":"AIP_GCS_HMAC_SECRET"}`다. endpoint는 Google Storage로 고정하고 서명 scope는 `auto/s3/aws4_request`를 사용한다. ADC·Workload Identity를 사용하는 공식 Node/Python SDK 어댑터는 [providers](providers/README.md)를 참고한다. 이 어댑터는 신뢰한 서버 bridge용이며 `gcs_hmac` 인증과 구분한다. 실제 AWS/GCP 계정 검증은 자격증명 없이 실행할 수 없다.

## SSE와 WebSocket

기존 `events.read/watch` polling을 유지하면서 같은 owner stream과 cursor를 push로 전달한다.

- SSE: `POST /events/subscribe`, Bearer, `Accept: text/event-stream`, JSON `{stream,after,contractFingerprint}`.
- WebSocket: `GET /events/socket`, 허용 Origin으로 upgrade. Bearer header가 있으면 첫 text frame은 구독 JSON이다. 브라우저는 첫 frame에 `{type:"auth",token}`, `{type:"authenticated"}` ACK 뒤 `{type:"subscribe",stream,after,contractFingerprint}`를 보낸다. 인증·구독 제한 시간은 합계 5초이고 frame 크기는 제한된다.

SDK의 `readSse(response,signal)`는 streaming UTF-8·multiline·cursor·취소를 처리하며 line/event 버퍼 상한을 적용한다. 소비자 코드가 연결 종료 뒤 마지막 event ID/cursor로 다시 구독해야 한다. `readSse`는 자동 재연결하지 않는다. 서버가 보존 범위를 벗어난 cursor에 대해 `CURSOR_EXPIRED`를 error event로 보내면 소비자가 최신 상태를 다시 조회해야 한다. 서버는 poll마다 인증·배포·generation을 재검증하고 세션 만료나 폐기 시 연결을 끝낸다. 이벤트 조회용 DB 연결이나 배포 lock을 연결 수명 동안 잡고 있지 않는다.

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
| 기존 PoC runtime (`aip run/migrate/rekey/token`, `diff --against deployed`: `axum`, `deadpool-postgres`, `chrono-tz`, `aes-gcm`) | **제외** | Cargo feature `poc-runtime` (`aip-cli`). 제품 경로 `aip service`는 쓰지 않는다. | `aip-cli/Cargo.toml`, [설계](../plan-docs/backend-capability/optional-runtime-package.md) |
| GCS ADC 어댑터 (Node/Python) | 제외 | `product/providers/`. 소비자 서버 프로젝트가 `@google-cloud/storage` 또는 `google-cloud-storage`를 직접 설치한다. | [providers](providers/README.md) |
| JS/TS SDK `@aip/sdk` | 런타임 의존성 없음 | 없음 | `product/tools/build-sdk.mjs`의 package.json |
| W3C `traceparent` 수신·전파 | 포함. 헤더 파싱과 task-local뿐이라 의존성이 늘지 않는다. | 없음 | `crates/aip-execution/src/trace_context.rs` |
| OpenTelemetry exporter(span 전송) | **제외**. 아직 구현하지 않았다. | Cargo feature 후보 `otel`. 아래 설계 메모. | [설계 메모](#opentelemetry-exporter-설계-메모) |

`aip-cli`의 의존 crate 수는 기본 빌드 293개, `external-brokers` 392개, `poc-runtime` 358개, 둘 다 켜면 450개다(`cargo tree -p aip-cli --locked -e normal --prefix none --features <목록> | sort -u | wc -l`, 2026-10-13 측정). `poc-runtime` 분리 전 기본 빌드는 358개였다.

Python 공급 경로도 같은 구조다. 호출자 SDK는 TypeScript `@aip/sdk` 하나이며 Python SDK 패키지는 없다. Python은 서버 extension 언어이고, 그 worker 스크립트(`spikes/spike-v4-worker/workers/worker.py`)는 `asyncio`, `json` 등 표준 라이브러리만 쓰며 Rust 바이너리에 포함된다. extension이 필요로 하는 서드파티 패키지는 배포자가 venv에 설치하고 `workers.runtime_executable`, `workers.environment`의 `PYTHONPATH`, `read_only_paths`로 지정한다. 선택 어댑터(`product/providers/google_storage.py`)는 공식 SDK를 호출 시점에 import하며 없으면 `PROVIDER_PACKAGE_MISSING`이다. `pyproject.toml`이나 pip extras는 만들지 않았다. 배포 가능한 Python 패키지가 생기는 시점에 다시 정한다.

### OpenTelemetry exporter 설계 메모

구현하지 않은 설계이며 기본 빌드에 의존성을 더하지 않는다. 서버는 이미 요청마다 `traceId`가 든 로그 줄과 `traceparent` 전파를 가지므로, exporter가 없어도 호출자의 tracing 백엔드와 AIP 로그는 trace id로 이어진다. exporter는 그 위에 span을 보내는 일만 더한다.

- 형태: `aip-service`의 Cargo feature `otel`(기본 꺼짐). 의존성은 `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`(http/protobuf, `reqwest` 재사용)로 한정하고 gRPC(`tonic`)는 쓰지 않는다. 켜지 않은 빌드에서 설정이 `otel`을 쓰면 `BROKER_FEATURE_DISABLED`와 같은 방식으로 기동 시점에 거부한다.
- span의 원천: `trace::Trace`가 끝나는 `Drop` 한 곳. 요청 span 하나를 `traceId`, 호출자의 parent span id(지금은 로그에 남기지 않음), route, 상태, `code`, 소요 시간으로 만든다. 경로·본문·principal은 span에도 넣지 않는다(로그 줄과 같은 금지).
- 전송: 로그 줄처럼 유한 큐와 별도 thread로 보내고, 가득 차면 버리며 센다. exporter가 느려도 요청과 연결 허용(permit) 해제를 늦추지 않는다.
- 남은 설계 결정: durable job은 enqueue 요청의 trace에서 이어져야 job 단계의 외부 호출과 로그가 한 trace가 된다. 이를 위해서는 job 행에 `trace_id`를 저장해야 하고 이는 배포 schema 변경(마이그레이션)이라 지금은 하지 않았다. 지금은 job 단계에서 시작한 호출에 `traceparent`가 없다.

패키지가 없는데 설정이 그 기능을 쓰면 **기동 시점**에 거부한다. 기본 빌드에서 `broker` 설정을 주면 DB 접속 전에 `BROKER_FEATURE_DISABLED`로 종료한다(`require_built_features`, `broker_config_is_rejected_when_the_feature_is_not_built` 테스트). PoC runtime 명령도 기본 빌드에서는 `RUNTIME_FEATURE_DISABLED`와 재빌드 명령을 출력하고 종료 코드 1이다(`crates/aip-cli/tests/runtime_disabled.rs`). 선택 패키지를 추가하는 기준은 새 외부 C/C++ 빌드 의존성이나 큰 전이 의존성을 끌어오는지다. PostgreSQL과 이미 포함된 crate만으로 구현되는 기능은 feature로 나누지 않고 서버 설정으로 켠다.

새 직접 의존성의 로컬 Cargo package metadata를 확인했다: `tokio-tungstenite 0.30.0`은 MIT, `futures-util 0.3.34`와 `libc 0.2.189`는 MIT OR Apache-2.0이다. 선택 broker 의존성의 locked metadata는 `lapin 4.12.1`, `rdkafka 0.39.0`, `rdkafka-sys 4.10.0+2.12.1`이 MIT이며 `cmake 0.1.58`은 MIT OR Apache-2.0이다. 전체 transitive 의존성의 별도 특허 검토를 끝냈다는 뜻은 아니다.

## 확인된 미해결 운영 경계

다중 프로세스 schema 분리, 운영 tracing/metrics, tenant 전체 broadcast, S3 누적 quota는 후속 검증·구현 대상이다. 현재 동시 quota·lease 경쟁·부분 요청 disconnect·느린 WebSocket 소비자·pool 취소를 검사하지만, 실제 운영 트래픽의 장시간 부하나 가용성 SLO를 보증하지 않는다. 기존 HTTP listener는 loopback TLS reverse proxy 뒤에 두는 모델을 유지한다.

참고한 공식 자료: [Bubblewrap README](https://github.com/containers/bubblewrap/blob/main/README.md), [AWS multipart 의미와 수명 주기](https://docs.aws.amazon.com/AmazonS3/latest/userguide/mpuoverview.html). 타 프로젝트 소스를 복사하지 않았으며 Bubblewrap은 배포 환경의 별도 도구로 실행한다.
