# 제품 서비스 실행

root의 `aip service`는 caller 정의와 공통 Rust 실행 엔진, 설치형 TypeScript SDK를 연결한다. 기존 `aip run/gen-ts` Core IR 경로와 `aip-prototype` 개발 토큰 경로는 이전 구현이다. `aip run/migrate/rekey/token`은 `cargo build -p aip-cli --features poc-runtime`으로 만든 바이너리에서만 동작하며, 기본 빌드에서는 `RUNTIME_FEATURE_DISABLED`로 거부한다. 제품에서 개발 토큰을 발급하지 않는다.

## 설정

```json
{
  "schema": "aip_my_app",
  "database_url_env": "AIP_DATABASE_URL",
  "listen": "127.0.0.1:8080",
  "wire": "decimal",
  "allowed_origins": ["https://app.example.com"],
  "auth": {
    "issuer": "https://issuer.example.com",
    "audience": "aip-api",
    "jwks": {"kind": "https", "url": "https://issuer.example.com/.well-known/jwks.json"}
  }
}
```

issuer·audience·JWKS URL은 실제 제공자에 맞춰 설정한다. 인증 프로필은 RS256, `typ=at+jwt`, 정확한 API audience와 issuer, `sub/iat/exp`(`nbf`는 선택, 있으면 강제), 최대 900초 수명이다. 이 프로필을 발급하도록 제공자를 설정해야 한다. 로그인·refresh token은 제공자와 앱이 담당한다. 토큰의 role이나 actorId는 AIP 권한으로 사용하지 않는다.

Durable job이나 서버 확장이 필요한 경우에만 선택적으로 `workers`를 추가한다. 생략하면 worker와 HTTP integration이 비활성화되고 SDK 직접 파일 업로드도 사용할 수 없다. `enable_file_uploads`는 SDK route 설정이며 worker의 `ctx.files`는 선언된 capability로 제한된다. 전체 설정 필드와 outbound 제한은 [관리형 durable execution](MANAGED-EXECUTION.md)을 따른다.

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
      "secret_header_env": {"name": "x-api-key", "env": "REPORTS_API_KEY"}
    }]
  }
}
```

`secret_header_env.env`는 서비스 process environment에서 명시적으로 읽는다. 서비스는 `.env`를 자동 로드하지 않는다. endpoint는 startup에서 DNS 검증·고정하고 설정 불일치는 generic `INTEGRATION_CONFIG` 오류로 보고한다. secret이나 URL은 startup 오류에 기록하지 않는다.

운영 환경에서는 마이그레이션 역할과 서버 실행 역할을 나눈다. `migrate_database_url_env`를 추가하고 `aip service provision`으로 인프라 테이블을 미리 만든다. 서버 역할에 줄 권한은 [DB 역할 분리와 최소 권한](DATABASE-ROLES.md)에 있다. 생략하면 역할 하나가 둘 다 맡으며 그 역할은 앱 schema의 소유자다.

DB 연결 문자열은 설정의 환경 변수로 전달한다. 서비스는 `.env`를 자동으로 읽지 않는다. 원격 DB는 `sslmode=require`와 인증서·호스트명 검증이 필요하다. 사설 DB CA는 `database_ca_file`로 설정하며 설정 파일 기준 상대 경로와 최대 64KiB 인증서 PEM만 허용한다. 공개 JWKS 파일을 사용할 때는 `{"kind":"file","path":"./jwks.json"}`이며 역시 설정 파일 기준 경로다.

listener는 loopback HTTP에 한정된다. 외부 HTTPS는 reverse proxy를 구성한다. `allowed_origins`는 정규화된 정확한 HTTPS origin 또는 로컬 HTTP origin만 받는다. 알려지지 않은 설정 키는 오류다. 토큰·개인키·실제 DB 비밀번호를 설정 예제나 로그에 남기지 않는다.

## 초기 배포와 호출

저장소 루트에서 다음 순서로 실행한다. 아래 예시는 인증 제공자와 DB 연결 환경 변수를 구성한 뒤 사용한다.

```sh
cargo build -p aip-cli
./target/debug/aip service check app.aip
./target/debug/aip service init app.aip --config service.json
./target/debug/aip service principal --config service.json bind --subject user-42 --actor 42
./target/debug/aip service gen app.aip --wire decimal --out app.binding.ts
./target/debug/aip service serve app.aip --config service.json
```

actor 행은 실제 앱의 사용자 데이터가 먼저 존재해야 한다. `bind`는 이미 존재하는 actor와 제공자 subject를 연결한다. 같은 subject의 actor 재배정은 거부한다. `revoke --subject user-42`는 연결을 비활성화하고 해당 시각까지 발급한 토큰을 무효화한다. 같은 actor를 다시 활성화해도 폐기 토큰은 살아나지 않는다. 폐기와 같은 초에 발급한 토큰도 제외하므로 다음 초 이후 새 토큰을 사용한다. 없는 subject의 폐기는 `PRINCIPAL_NOT_FOUND`로 실패한다. `bind --not-before <epoch seconds>`로 기준을 높일 수 있고, 기준을 낮추지는 못한다.

SDK는 `npm install --offline <aip-sdk.tgz>`로 별도 소비자에 설치한다. `connect(baseUrl, accessToken, generatedBinding)`는 기존 typed transport·캐시·미확정 쓰기 복구를 사용한다. `wire` 설정과 binding 생성의 `--wire`를 맞춘다. 배포의 wire는 최초 연결 시 영속 고정하며 이후 설정만 바꿔 전환하지 않는다. 이 제한은 멱등 principal namespace가 바뀌어 이미 커밋한 쓰기가 재실행되는 것을 막는다.

### 상태 확인 probe

`GET /health`와 `GET /ready`는 인증 없이 응답한다(HEAD도 허용). load balancer·Kubernetes probe에 쓴다.

| 경로 | 200 조건 | 503 조건 |
|---|---|---|
| `/health` | listener가 요청을 처리함. DB를 확인하지 않음 | 없음 |
| `/ready` | pool에서 DB 연결을 얻고, 저장된 배포 digest가 이 process의 정의와 같음 | `WIRE.DB_UNAVAILABLE`, 다른 배포가 migration한 뒤의 `WIRE.DEPLOYMENT_CHANGED` |

응답은 `cache-control: no-store`이며 digest·버전·DB 주소·오류 원문을 포함하지 않는다. readiness 검사는 2초로 제한한다. 인증 없는 probe가 요청 pool을 고갈시키지 않도록 process당 동시에 하나의 readiness 검사만 DB 연결을 사용한다. 검사가 진행 중일 때 들어온 probe는 줄을 서지 않고 직전 응답(최대 10초 전까지)을 바로 돌려받으므로 probe flood가 load balancer의 probe를 막지 못한다. 한가할 때는 매번 새로 검사한다. 공개 proxy에 두지 않고 내부 listener에서만 probe를 열려면 proxy에서 `/health`·`/ready` 경로를 외부로 노출하지 않는다. probe 경로는 webhook 경로로 설정할 수 없다. readiness가 worker 확장 import나 외부 provider 연결까지 보증하지는 않는다.

선택형 Mongo/Redis read adapter 상태는 MySQL `service serve`의 `listening` JSON과 `POST /capabilities`의 infrastructure catalog에서 확인한다. `configured:false, status:"unavailable"`은 해당 adapter가 설정되지 않았음을 뜻하며 서비스 전체를 unready로 만들지 않는다. 설정된 adapter의 연결/readiness 실패는 listening 전에 startup 오류로 반환된다. 전체 probe 의미는 위 `/ready` 계약 그대로다.

### 서비스 주체와 trigger

스케줄과 webhook은 요청 사용자가 없으므로 운영자가 별도 subject를 bind해 서비스 주체로 쓴다. 예: `aip service principal --config service.json bind --subject svc-scheduler --actor 9000`. 이 주체의 actor에 필요한 행 정책·`allow` 권한만 주고, 사용자 subject와 공유하지 않는다. 설정 형식과 실행 경계는 [관리형 execution의 스케줄과 웹훅 수신](MANAGED-EXECUTION.md#스케줄과-웹훅-수신)을 따른다.

## 변경 배포와 기존 데이터 이관

```sh
./target/debug/aip service migrate next.aip --config service.json
./target/debug/aip service migrate next.aip --config service.json --apply <reviewed-plan-digest>
```

첫 명령은 읽기 전용 계획이다. 변경 분류·SQL·데이터 영향을 검토하고 반환된 digest를 적용 명령에 전달한다. 필요한 경우 `--migration migration.json`을 양쪽에 똑같이 전달한다. backfill/convert는 [마이그레이션 계약](../crates/aip-migrate/README.md)의 정형 JSON만 받으며 임의 SQL은 받지 않는다. 적용 실패는 DDL·데이터·저널을 같은 트랜잭션에서 되돌린다. 커밋 응답이 불명확하면 같은 digest를 재시도해 최신 저널과 catalog로 확인한다.

요청의 실제 SQL 연결은 배포 shared advisory lock을 유지한다. 마이그레이션은 같은 키의 exclusive lock을 사용하므로 진행 중 요청과 직렬화한다. 적용 뒤 이전 정의를 가진 listener는 `WIRE.DEPLOYMENT_CHANGED`로 작업을 거부한다. 새 정의로 서버를 재시작하고 생성 binding을 갱신한다. 구버전 클라이언트를 동시에 지원하는 프로토콜은 이번 구현에 포함하지 않는다.

현재 prototype의 구조 지문 marker가 있는 schema는 다음처럼 명시적으로 이관한다.

```sh
./target/debug/aip service adopt app.aip --config service.json
./target/debug/aip service adopt app.aip --config service.json --apply <reviewed-plan-digest>
```

먼저 기존 prototype을 중지한다. 기존 앱 행과 커밋된 멱등 결과는 유지하고 제품 저널·principal 연결을 만든다. 정의의 DDL과 기존 catalog marker가 다르면 이관을 거부한다. 원래 marker에는 정책 본문이 없으므로 제출한 정의의 execution digest가 새 정책 정본이다. 개발 토큰은 이관하지 않는다. 이전 wire와 같은 제품 설정을 사용한다. 구조 지문 없는 구형 marker나 임의 운영 schema를 자동으로 채택하지 않는다.

## 릴리스 업그레이드와 롤백

새 실행 파일로 바꿀 때는 정의 변경이 없어도 AIP가 만든 schema(durable job, 인프라 테이블)가 달라질 수 있다. 아래 순서를 따른다. 이 순서는 `product/tools/upgrade-drill.mjs`로 이전 릴리스의 실제 binary가 만든 schema를 새 binary로 올리고 되돌리는 것까지 실행해 확인했다([릴리스 게이트](RELEASE-GATE.md)).

1. 서비스를 중지하고 백업한다. 앱 schema와 인프라 schema를 함께 받는다. 인프라 schema 이름은 `aip_infra_` 뒤에 앱 schema 이름의 SHA-256 앞 52자를 붙인 것이다(`printf %s aip_my_app | shasum -a 256 | cut -c1-52`).

   ```sh
   pg_dump -Fc --no-owner --schema aip_my_app --schema aip_infra_<52hex> --file pre-upgrade.dump "$AIP_DATABASE_URL"
   ```

2. 새 binary로 계획을 보고 digest를 적용한다. 계획에는 DROP·TRUNCATE·DELETE가 없어야 하고 `blocked`가 `false`여야 한다. 적용은 저널에 한 행을 더한다.

   ```sh
   ./aip service migrate app.aip --config service.json
   ./aip service migrate app.aip --config service.json --apply <reviewed-plan-digest>
   ./aip service provision --config service.json
   ```

3. 서비스를 시작하고 `/ready`가 200인지 확인한다. 이전 binary가 쓴 행, 멱등 기록, 끝난 job은 그대로 읽힌다.

**적용 전에는 새 binary가 이전 schema를 서빙하지 않는다.** 계획을 적용하지 않고 `serve`를 시작하면 `WIRE.SCHEMA_MISMATCH`로 끝난다. 일부만 올라간 상태로 요청을 받는 일이 없다.

**롤백은 이전 binary로 되돌리는 것만으로 되지 않는다.** 업그레이드를 적용한 schema에 이전 binary를 시작하면 같은 `WIRE.SCHEMA_MISMATCH`로 끝난다(역방향 migration은 없다). 되돌릴 때는 1번의 백업을 복원한다.

```sh
psql "$AIP_DATABASE_URL" -c 'DROP SCHEMA aip_my_app CASCADE' -c 'DROP SCHEMA aip_infra_<52hex> CASCADE'
pg_restore --no-owner --dbname "$AIP_DATABASE_URL" pre-upgrade.dump
```

복원하면 백업 이후 커밋된 쓰기도 사라진다. 클라이언트가 같은 키로 다시 보내면 새 쓰기로 실행되므로(멱등 기록도 함께 되돌아간다) 업그레이드 뒤 트래픽을 받았다면 롤백 전에 그 쓰기를 별도로 확인한다. 이전 binary와 새 schema를 섞어 쓰는 구성은 지원하지 않는다.

## 업무 감사 기록

배포 schema로 시작한 서버(`aip service run`)는 commit된 전이마다 감사 행을 하나 남긴다. 정의에 따로 쓸 것은 없다. 전이 UPDATE와 같은 SQL 문장에서 기록하므로 전이가 rollback되면 감사 행도 없고, 감사 기록이 실패하면 전이도 commit되지 않는다. Hibernate Envers·django-auditlog·PaperTrail이 엔티티 변경을 남기는 것과 같은 역할이다.

| 열 | 뜻 |
|---|---|
| `recorded_at` | 전이 transaction 시각 |
| `actor_id` | 전이를 실행한 주체의 actor. job은 job 주체, schedule·webhook은 서비스 주체 |
| `via` | `apply`(호출자 `/apply`), `extension`(쓰기 확장 `ctx.data.apply`), `job`(durable job `ctx.data.apply`) |
| `request_key` | `apply`는 호출자의 멱등 key, `job`은 `<run id>:<effect key>`로 `aip_effects`와 조인된다. `extension`은 비어 있다 |
| `resource`, `transition`, `target_id` | 바뀐 행 하나 |
| `before`, `after` | 전이의 `to`가 바꾸는 필드만 `{필드: 값}`으로. 다른 필드는 남기지 않는다 |

테이블은 배포 schema가 아니라 서버가 시작할 때 만드는 infra schema(`aip_infra_<hash>.aip_audit`)에 있어 기존 배포를 migrate하지 않아도 된다. `UPDATE`는 trigger가 거부한다. 보존 기한 정리는 운영자가 `DELETE`로 한다. 이 trigger는 실수 방지이지 변조 방지가 아니다. 테이블 소유 role은 trigger를 끄거나 행을 지울 수 있으므로, 규제 감사에는 서비스 실행 role과 소유 role을 나누고 외부 보관소로 복제한다. `target_id`와 Ref 값은 내부 bigint id이며 `wire: decimal` 같은 API 표현으로 바꾸지 않는다. 멱등 재생, 안 보이는 대상, 일부 대상 실패로 전체가 취소된 bulk는 아무것도 바꾸지 않았으므로 기록하지 않는다. 전이의 선언 효과(`update`·`create` 효과가 바꾼 다른 행, `notify` outbox 행)는 따로 기록하지 않는다. 호출자가 쓰는 조회 API는 없다. 운영자는 CLI로 읽는다. 근거 테스트는 `spikes/spike-v6-transport/tests/audit.rs`와 `crates/aip-service/tests/audit_cli.rs`다.

```sh
aip service audit --config service.json list --resource Order --target-id 11   # 한 행의 이력
aip service audit --config service.json list --actor 2 --since 2026-10-01T00:00:00Z --limit 100
aip service audit --config service.json list --request-key 'k-1'               # 멱등 key 또는 <run id>:<effect key>
aip service audit --config service.json list --before-id 3411                  # 이전 쪽의 nextBeforeId부터 이어서
```

필터는 `--resource`, `--target-id`, `--actor`, `--via apply|extension|job`, `--request-key`, `--since`(RFC 3339), `--before-id`이고 모두 AND다. 최신 id 순이며 `--limit`은 1~500(기본 50)이다. 출력은 `{"ok":true,"audit":[{id, recordedAt, actorId, via, requestKey, resource, transition, targetId, before, after}],"nextBeforeId":다음 쪽이 있으면 그 커서, 없으면 null}`이다. 읽기 전용이고 DB 접근 권한이 있는 운영자 도구라 호출자 인증이 없으며, 값이 bind 매개변수로만 들어가 SQL을 조립하지 않는다. 서버가 한 번도 시작되지 않아 테이블이 없으면 `AUDIT_UNAVAILABLE`, 필터 값이 틀리면 `BAD_AUDIT_QUERY`다.

## 요청·operation·job metrics

`{"metrics":{"token_env":"AIP_METRICS_TOKEN"}}`을 설정하면 `GET /metrics`가 Prometheus text 형식(0.0.4)으로 열린다. 설정이 없으면 이 경로는 없는 경로와 같다. scrape는 그 환경 변수 값을 `Authorization: Bearer`로 보내야 하며, 값은 공백 없는 ASCII 32~256자여야 시작된다. 별도 무거운 exporter 라이브러리 없이 표준 HTTP 계측으로 동작한다.

| metric | label | 뜻 |
|---|---|---|
| `aip_requests_total` | `route`, `status`, `code` | 끝난 요청 수. `route`는 고정 이름(`read`, `apply`, `jobs` 등, 그 밖은 `other`), `code`는 실패 응답의 AIP 오류 코드 |
| `aip_request_duration_seconds` | `route` | 요청 수락부터 응답 완료까지(histogram, 5ms~30s) |
| `aip_operations_total` | `operation`, `status`, `code` | 끝난 operation 실행 수. `code`는 실패 응답 코드 |
| `aip_operation_duration_seconds` | `operation` | operation 실행 소요 시간(histogram, 5ms~30s) |
| `aip_jobs_total` | `operation`, `state`, `code` | 끝난 durable job run/attempt 수(`state`: SUCCEEDED, RETRY, FAILED, CANCELLED) |
| `aip_job_duration_seconds` | `operation` | durable job 실행 소요 시간(histogram, 5ms~30s) |

label에는 경로·query·본문·principal을 넣지 않아 호출자 입력이 metric series 수를 늘리지 않는다. 오류율은 `status`가 아니라 `code!=""`로 계산한다. 많은 거부가 HTTP 200 본문의 오류 코드로 전달되기 때문이다.

요청 ID는 응답의 `x-request-id`, JSON 실패 본문의 `requestId`, 요청 로그 줄에 동일하게 기록된다. 오류 본문에는 코드별 `retry`도 있어 운영자가 본문만 받아도 로그와 재시도 지침을 확인할 수 있다. SDK의 실패 객체도 이 정보를 보존한다(Python 예외의 ID 속성은 `request_id`). 미확정 쓰기의 처리 규칙은 [SDK](SDK.md#계약-헤더와-실패-응답)를 따른다. 정적 파일·객체 스트림·managed file·SSE·WebSocket 업그레이드에도 같은 요청 ID 헤더와 상태 로그가 남는다.

W3C `traceparent`가 유효한 헤더 하나로 들어오면 요청 로그의 `traceId`는 호출자의 trace id다. 헤더가 없거나 잘못됐거나 둘 이상이면 서버가 새 trace id를 만든다. 요청 중 외부 HTTP 호출(RAG embedding·answer provider, 객체 저장소)은 같은 trace id, 새 span id, 호출자의 sampled flag를 전달한다. 또한 durable job을 enqueue할 때 이 trace id가 `aip_jobs.trace_id`에 저장되어 worker 실행·재시도 시에도 같은 trace id 컨텍스트가 복원된다.

## OpenTelemetry (OTLP/HTTP) exporter

OTLP 큐가 가득 차 버린 span은 `aip_otlp_spans_dropped_total` 누적 counter로 확인한다.
Prometheus `/metrics`에 항상 노출하고, 1건 이상이면 OTLP `/v1/metrics`에도 monotonic cumulative sum으로 보낸다.
이 값은 큐 포화로 인한 드롭만 세며 process 재시작 때 0이 된다. 주기적 `otlp_spans_dropped` 로그의 배치 카운터가 초기화되어도 누적 값은 유지된다.

`{"otlp":{"endpoint":"http://collector:4318","headers":{},"timeout_ms":5000}}`을 설정하면 OpenTelemetry Collector로 traces와 metrics를 HTTP/JSON 규격으로 내보낸다. 기본값은 off(미설정)이며, 꺼져 있을 때는 단순 `Option` 검사만 수행되어 추가 런타임 비용이 없다.
- `/v1/traces`: HTTP 요청 span(`HTTP <route>`, kind SERVER)과 durable job run span(`JOB <operation>`, kind INTERNAL)을 비동기 bounded queue(4096)를 통해 논블로킹으로 전송한다.
- `/v1/metrics`: 요청·operation·job 카운터 및 지연 histogram 데이터를 2초 간격으로 OTLP JSON 규격으로 전송한다.
- 민감값 배제: span 속성에는 `http.method`, `http.route`, `http.status_code`, `job.operation`, `job.id`, `job.epoch`, `job.state`, `aip.code` 등 사전에 정해진 안전한 식별자만 포함하며, 인증 토큰·query string·요청/응답 payload는 절대 포함하지 않는다.
- 검증: `spikes/spike-v6-transport/tests/otlp.rs`(mock OTLP 수신기를 통한 trace/metrics 수신 확인), `tests/metrics.rs`, `tests/trace_propagation.rs`.

## 확장과 검증 범위

공식 확장은 `workers: {"directory":"./extensions","language":"node","enable_writes":true}`를 명시해 활성화한다. Python은 `language:"python"`이다. 기존 공통 엔진과 정본 구현을 사용하며 서버가 입력·권한·시간·출력·트랜잭션을 검사한다. 기본 worker 격리는 OS별 `auto`이며 macOS는 네트워크 차단, Linux는 Bubblewrap을 사용한다. DB pool, 분산 rate limit, cgroup, 디렉터리 hosting, local/S3/GCS 객체, 브라우저 WebSocket과 사용자 지정 한도는 [서버 인프라 설정](INFRASTRUCTURE.md)을 따른다. 외부 메시지 relay는 [BROKERS](BROKERS.md), 관리형 검색·생성은 [RAG](RAG.md)를 따른다.

실제 PostgreSQL의 소유 테스트 schema, 통제된 HTTPS 서버와 메모리 생성 RSA 키, offline 설치 SDK, 재배치 macOS binary로 검증한다. 외부 IdP 계정은 이 환경에 없어 실제 제공자 연결은 확인하지 못했다([runbook](IDP-RUNBOOK.md)). reverse proxy는 로컬 Caddy로 검증했다(아래). 상수 backfill은 행 수와 무관한 catalog 변경이지만, 한도를 넘는 대형 테이블의 배치 변환·자동 역방향 migration·actor resource 교체·임의 데이터 변환은 지원하지 않는다. 좁은 검증은 다음 명령으로 재실행한다.

```sh
cargo test -p aip-auth -p aip-migrate
cargo test -p aip-cli --test service --test service_boundaries --test service_flow --test adoption_flow
node --test product/tests/*.test.mjs
```

## 업무 resource가 없는 operation 서버

실제 선언·SDK·HTTP·관리형 신원과 실행 제한은 [EXECUTION](EXECUTION.md)을 따른다. `actor principal`과 top-level operation만 있는 정의에도 같은 check→gen→init→principal bind→serve 절차를 사용한다. 업무 resource 테이블은 필요 없지만 운영 인증·배포·권한 검사용 PostgreSQL은 필요하다. worker directory/language를 명시해야 호출할 수 있으며 `enable_writes`는 순수 operation에 필요하지 않다.

배포 fence는 `/operation`과 `/capabilities`에도 적용한다. operation은 실행 전후 allow를 평가하고 반환 직전 현재 principal을 다시 검증한다. principal 명령도 배포에 저장된 wire와 설정 wire가 다르면 거부한다. 관리형 actor로의 전환은 기존 actor resource FK 의미를 바꾸므로 일반 자동 migration에서 차단한다.

## Durable job과 서버 broker

`operation job`은 연결과 분리되어 PostgreSQL에 수락·lease·상태·결과를 기록한다. enqueue/status/cancel/result 및 SDK polling watcher, file/event/HTTP capability 제한과 보존 정책은 [관리형 durable execution](MANAGED-EXECUTION.md)을 따른다. durable schema는 해당 capability를 가진 정의를 init/migrate할 때 만들어진다. 이 기능을 켜도 기존 SQL 동기 operation과 read/apply 요청 경로는 그대로다.

### 실패 job 조회와 재처리

운영자는 모든 owner의 job을 조회하고 `FAILED` job을 다시 큐에 넣을 수 있다. Sidekiq dead set의 retry, SQS DLQ redrive에 해당한다. 두 명령 모두 DB에 직접 연결하므로 서비스 설정의 DB 권한을 가진 운영자만 실행할 수 있다.

```bash
aip service jobs --config service.json list --state FAILED --operation syncOrders --limit 50
aip service jobs --config service.json retry --id 7f3c...
```

- `list`는 최신순으로 id·owner·operation·key·상태·시도 횟수·마지막 오류·`unsettledEffects`(결과를 모르는 외부 효과 수)·`redrivable`을 돌려준다. job 입력·출력·grant는 출력하지 않는다.
- `retry`는 새 job을 만들지 않고 **같은 run**을 `QUEUED`로 되돌린다. id·key·owner·effect journal을 유지하므로 이미 기록된 파일·이벤트·메시지·HTTP·데이터 전이는 반복하지 않고 재생한다. 시도 횟수는 0부터 다시 세고 run 기한은 run에 기록된 시도 횟수로 다시 계산한다. lease epoch는 계속 증가하므로 이전 시도의 worker는 결과를 기록하지 못한다.
- 실행할 때 principal 활성·배포 fence·`allow`를 다시 검사한다. 비활성화한 주체의 job은 재처리해도 다시 실패한다.
- 거부하는 경우는 네 가지다. `FAILED`가 아니면 `JOB_NOT_FAILED`, 결과 보존 기간(24시간)이 지나 입력이 지워졌으면 `JOB_EXPIRED`, 다른 배포에서 수락했으면 `WIRE.DEPLOYMENT_CHANGED`, 결과를 모르는 외부 효과가 있으면 `WIRE.EFFECT_UNKNOWN`이다. 마지막 경우는 provider에서 실제 결과를 확인한 뒤 새 job으로 처리한다. 미확정 효과를 운영자가 확정하는 명령은 아직 없다.
- 재처리는 owner의 active job 상한(100)을 다시 검사하지 않는다. 대량 재처리는 운영자가 나눠서 실행한다.

## TLS reverse proxy 검증

서비스는 loopback HTTP만 연다. 외부 HTTPS는 reverse proxy가 종단한다. `node product/tools/proxy-check.mjs`는 실제 Caddy를 앞에 두고 임시 schema의 서비스에 16개 검사를 실행한다: Authorization·본문 통과, CORS preflight의 Origin 일치와 외부 Origin 거부, `x-request-id` 유지·생성, `X-Forwarded-*`가 인증에 영향을 주지 않음, 1MiB 초과 본문의 서비스 거부, SSE 첫 keepalive가 버퍼링 없이 도착, WebSocket의 Origin 필수·외부 Origin 거부·101 업그레이드·첫 프레임 인증. 필요한 것은 `caddy`(`brew install caddy`), `openssl`, `psql`, 빌드된 `target/debug/aip`다.

최소 Caddy 설정은 `reverse_proxy 127.0.0.1:<서비스 포트>` 한 줄이다. Caddy는 WebSocket 업그레이드와 `text/event-stream` flush를 자동 처리한다. nginx를 쓰면 SSE 경로에 `proxy_buffering off`, WebSocket 경로에 `proxy_http_version 1.1`과 `Upgrade`·`Connection` 헤더 전달이 필요하다. 이 설정은 nginx로 실행해 보지 않았다(`확인 못함: nginx 미설치`).

실제 외부 IdP 연결은 [IdP runbook](IDP-RUNBOOK.md)을 따른다.
