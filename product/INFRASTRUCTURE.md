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

SigV4는 Content-Length를 서명해 선언한 크기보다 큰 PUT/part를 막는다. URL 수명은 300초다. presigned URL은 소지자가 사용할 수 있고 발급 이후 principal 폐기가 즉시 URL을 취소하지 않는다. S3 control에는 동시 실행 제한이 있지만 **owner별 S3 합계 quota와 미완료 multipart 자동 정리는 아직 없다**. 배포 시 bucket 정책·수명 주기·비용 제한을 설정해야 한다. provider에서 Content-Length 서명, path-style URL, ETag, CORS 동작을 검증해야 한다. AWS SigV4 공식 fixture 검증과 실제 S3 provider 연동 검증은 서로 다르다. 공급자 자격증명이 없어 후자는 미실행이다.

## SSE와 WebSocket

기존 `events.read/watch` polling을 유지하면서 같은 owner stream과 cursor를 push로 전달한다.

- SSE: `POST /events/subscribe`, Bearer, `Accept: text/event-stream`, JSON `{stream,after,contractFingerprint}`.
- WebSocket: `GET /events/socket`, Bearer와 허용 Origin으로 upgrade, 첫 text frame에 같은 구독 JSON.

SDK의 `readSse(response,signal)`는 streaming UTF-8·multiline·cursor·취소를 처리하며 line/event 버퍼 상한을 적용한다. 소비자 코드가 연결 종료 뒤 마지막 event ID/cursor로 다시 구독해야 한다. `readSse`는 자동 재연결하지 않는다. 서버가 보존 범위를 벗어난 cursor에 대해 `CURSOR_EXPIRED`를 error event로 보내면 소비자가 최신 상태를 다시 조회해야 한다. 서버는 poll마다 인증·배포·generation을 재검증하고 세션 만료나 폐기 시 연결을 끝낸다. 이벤트 조회용 DB 연결이나 배포 lock을 연결 수명 동안 잡고 있지 않는다.

브라우저 기본 WebSocket 생성자는 Authorization header를 지정할 수 없다. 현재 WS는 Bearer header를 지원하는 클라이언트나 인증 gateway에 적합하다. 토큰을 query string에 넣는 우회는 제공하지 않는다. 브라우저는 fetch SSE 또는 기존 polling을 사용할 수 있다. 조직 전체 broadcast나 record별 subscription ACL은 아직 별도 기능이다.

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

`messages.<queue>`가 있는 정의에만 테이블을 생성한다. 기존 배포에는 migrate plan/apply로 추가하며 마지막 queue capability를 제거할 때 테이블을 자동 삭제하지 않는다. Kafka/RabbitMQ 클라이언트 구현은 포함하지 않았다. Rust `Broker`/`BrokerRegistry`는 adapter 경계지만 제품 HTTP queue의 backend 선택 설정으로 연결된 것은 아니다. 외부 broker와 DB의 원자적 commit은 자동 보장하지 않는다.

## Linux 격리와 JS/Python 의존성

`auto`는 macOS에서 network-deny sandbox, Linux에서 Bubblewrap을 선택하고 필요한 도구가 없으면 실행을 거부한다. Linux는 빈 mount namespace에 시스템 runtime·확장 폴더·지정한 dependency 디렉터리만 읽기 전용으로 노출한다. 네트워크와 PID namespace를 분리하고 private tmp를 사용한다. HOME 전체나 `/` mount는 거부한다. Ubuntu CI에서 실제 읽기·쓰기·네트워크 경계를 검사한다. 현재 개발 host가 macOS여서 Linux 실실행 검증은 CI 결과 확인이 필요하다.

Python venv interpreter, 명시적 PYTHONPATH, Node extension 폴더의 node_modules를 사용한다. 패키지 설치는 배포 과정에서 한다. worker가 임의 패키지를 다운로드하지 않는다. AI/RAG용 파서·토크나이저·계산 라이브러리도 이 경로를 사용하고 외부 API는 이름 기반 `ctx.http` endpoint를 통한다. 모델 서버·vector DB URL이나 secret을 프론트 입력으로 받지 않는다. endpoint별 `allow_private_networks:true`로 MSA HTTPS 사설 주소를 명시적으로 허용할 수 있다. DNS 고정·TLS 검증·redirect/proxy 차단은 유지한다.

macOS는 네트워크만 차단하며 filesystem 격리는 보장하지 않는다. worker는 신뢰한 서버 코드다. 의존성 전체 내용은 현재 implementation fingerprint에 들어가지 않으므로 immutable deployment와 lockfile을 사용한다. npm/pip 생태계의 모든 라이브러리 호환성이나 native GPU·subprocess·메모리 상한은 보장하지 않는다. 고위험 코드에는 별도 컨테이너·cgroup·운영 격리가 필요하다.

## 발견과 의존성

인증된 `/capabilities` 응답의 `infrastructure`는 backend 선택, 전송 지원, 공개 URL prefix와 실행 한도를 설명한다. 서버 디렉터리·환경 변수·credential은 노출하지 않는다. 개별 호출의 권한·계약 검사는 계속 수행한다.

새 직접 의존성의 로컬 Cargo package metadata를 확인했다: `tokio-tungstenite 0.30.0`은 MIT, `futures-util 0.3.34`와 `libc 0.2.189`는 MIT OR Apache-2.0이다. 전체 transitive 의존성의 별도 특허 검토를 끝냈다는 뜻은 아니다.

## 확인된 미해결 운영 경계

전역 rate limiting, DB pool, 다중 프로세스 schema 분리, cgroup CPU/메모리 quota, 운영 tracing/metrics, 외부 broker relay, tenant 전체 broadcast, S3 누적 quota는 후속 검증·구현 대상이다. 기능 테스트가 실제 대규모 부하나 장애 주입 검증을 대신하지 않는다. 기존 HTTP listener는 loopback TLS reverse proxy 뒤에 두는 모델을 유지한다.

참고한 공식 자료: [Bubblewrap README](https://github.com/containers/bubblewrap/blob/main/README.md), [AWS multipart 의미와 수명 주기](https://docs.aws.amazon.com/AmazonS3/latest/userguide/mpuoverview.html). 타 프로젝트 소스를 복사하지 않았으며 Bubblewrap은 배포 환경의 별도 도구로 실행한다.
