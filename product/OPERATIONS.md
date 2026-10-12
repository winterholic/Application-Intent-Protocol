# 제품 서비스 실행

root의 `aip service`는 caller 정의와 공통 Rust 실행 엔진, 설치형 TypeScript SDK를 연결한다. 기존 `aip run/gen-ts` Core IR 경로와 `aip-prototype` 개발 토큰 경로는 이전 구현이다. `aip run/migrate/rekey/token`은 `cargo build -p aip-cli --features poc-runtime`으로 만든 바이너리에서만 동작하며, 기본 빌드에서는 `RUNTIME_FEATURE_DISABLED`로 거부한다. 제품에서 개발 토큰을 발급하지 않는다.

## 설정

```json
{
  "schema": "my_app",
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

issuer·audience·JWKS URL은 실제 제공자에 맞춰 설정한다. 인증 프로필은 RS256, `typ=at+jwt`, 정확한 API audience와 issuer, `sub/iat/nbf/exp`, 최대 900초 수명이다. 이 프로필을 발급하도록 제공자를 설정해야 한다. 로그인·refresh token은 제공자와 앱이 담당한다. 토큰의 role이나 actorId는 AIP 권한으로 사용하지 않는다.

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
| `/ready` | pool에서 DB 연결을 얻고, 저장된 배포 digest가 이 process의 정의와 같음 | `DB_UNAVAILABLE`, 다른 배포가 migration한 뒤의 `DEPLOYMENT_CHANGED` |

응답은 `cache-control: no-store`이며 digest·버전·DB 주소·오류 원문을 포함하지 않는다. readiness 검사는 2초로 제한한다. 인증 없는 probe가 요청 pool을 고갈시키지 않도록 process당 동시에 하나의 readiness 검사만 DB 연결을 사용한다. probe 경로는 webhook 경로로 설정할 수 없다. readiness가 worker 확장 import나 외부 provider 연결까지 보증하지는 않는다.

### 서비스 주체와 trigger

스케줄과 webhook은 요청 사용자가 없으므로 운영자가 별도 subject를 bind해 서비스 주체로 쓴다. 예: `aip service principal --config service.json bind --subject svc-scheduler --actor 9000`. 이 주체의 actor에 필요한 행 정책·`allow` 권한만 주고, 사용자 subject와 공유하지 않는다. 설정 형식과 실행 경계는 [관리형 execution의 스케줄과 웹훅 수신](MANAGED-EXECUTION.md#스케줄과-웹훅-수신)을 따른다.

## 변경 배포와 기존 데이터 이관

```sh
./target/debug/aip service migrate next.aip --config service.json
./target/debug/aip service migrate next.aip --config service.json --apply <reviewed-plan-digest>
```

첫 명령은 읽기 전용 계획이다. 변경 분류·SQL·데이터 영향을 검토하고 반환된 digest를 적용 명령에 전달한다. 필요한 경우 `--migration migration.json`을 양쪽에 똑같이 전달한다. backfill/convert는 [마이그레이션 계약](../crates/aip-migrate/README.md)의 정형 JSON만 받으며 임의 SQL은 받지 않는다. 적용 실패는 DDL·데이터·저널을 같은 트랜잭션에서 되돌린다. 커밋 응답이 불명확하면 같은 digest를 재시도해 최신 저널과 catalog로 확인한다.

요청의 실제 SQL 연결은 배포 shared advisory lock을 유지한다. 마이그레이션은 같은 키의 exclusive lock을 사용하므로 진행 중 요청과 직렬화한다. 적용 뒤 이전 정의를 가진 listener는 `DEPLOYMENT_CHANGED`로 작업을 거부한다. 새 정의로 서버를 재시작하고 생성 binding을 갱신한다. 구버전 클라이언트를 동시에 지원하는 프로토콜은 이번 구현에 포함하지 않는다.

현재 prototype의 구조 지문 marker가 있는 schema는 다음처럼 명시적으로 이관한다.

```sh
./target/debug/aip service adopt app.aip --config service.json
./target/debug/aip service adopt app.aip --config service.json --apply <reviewed-plan-digest>
```

먼저 기존 prototype을 중지한다. 기존 앱 행과 커밋된 멱등 결과는 유지하고 제품 저널·principal 연결을 만든다. 정의의 DDL과 기존 catalog marker가 다르면 이관을 거부한다. 원래 marker에는 정책 본문이 없으므로 제출한 정의의 execution digest가 새 정책 정본이다. 개발 토큰은 이관하지 않는다. 이전 wire와 같은 제품 설정을 사용한다. 구조 지문 없는 구형 marker나 임의 운영 schema를 자동으로 채택하지 않는다.

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

테이블은 배포 schema가 아니라 서버가 시작할 때 만드는 infra schema(`aip_infra_<hash>.aip_audit`)에 있어 기존 배포를 migrate하지 않아도 된다. `UPDATE`는 trigger가 거부한다. 보존 기한 정리는 운영자가 `DELETE`로 한다. 이 trigger는 실수 방지이지 변조 방지가 아니다. 테이블 소유 role은 trigger를 끄거나 행을 지울 수 있으므로, 규제 감사에는 서비스 실행 role과 소유 role을 나누고 외부 보관소로 복제한다. `target_id`와 Ref 값은 내부 bigint id이며 `wire: decimal` 같은 API 표현으로 바꾸지 않는다. 멱등 재생, 안 보이는 대상, 일부 대상 실패로 전체가 취소된 bulk는 아무것도 바꾸지 않았으므로 기록하지 않는다. 전이의 선언 효과(`update`·`create` 효과가 바꾼 다른 행, `notify` outbox 행)는 따로 기록하지 않는다. 조회 API·CLI는 아직 없고 SQL로 읽는다. 근거 테스트는 `spikes/spike-v6-transport/tests/audit.rs`다.

## 요청 metrics

`{"metrics":{"token_env":"AIP_METRICS_TOKEN"}}`을 설정하면 `GET /metrics`가 Prometheus text 형식(0.0.4)으로 열린다. 설정이 없으면 이 경로는 없는 경로와 같다. scrape는 그 환경 변수 값을 `Authorization: Bearer`로 보내야 하며, 값은 공백 없는 ASCII 32~256자여야 시작된다. 별도 exporter 의존성이 없어 기본 빌드에 포함된다.

| metric | label | 뜻 |
|---|---|---|
| `aip_requests_total` | `route`, `status`, `code` | 끝난 요청 수. `route`는 고정 이름(`read`, `apply`, `jobs` 등, 그 밖은 `other`), `code`는 실패 응답의 AIP 오류 코드 |
| `aip_request_duration_seconds` | `route` | 요청 수락부터 응답 완료까지(histogram, 5ms~30s) |

label에는 경로·query·본문·principal이 들어가지 않으므로 호출자가 series 수를 늘릴 수 없다. AIP의 많은 거부는 HTTP 200 본문의 `code`로 오므로 오류율은 `status`가 아니라 `code!=""`로 계산한다. job 상태별 수, DB pool 대기, outbox 적체 같은 내부 상태 metric과 OpenTelemetry trace 전송은 아직 없다. trace exporter는 외부 SDK 의존성이 커서 [선택적 패키지](INFRASTRUCTURE.md#선택적-패키지-구조) 후보로 둔다. 요청 상관 ID는 응답의 `x-request-id`와 요청 로그 줄이 함께 가진다. 근거 테스트는 `spikes/spike-v6-transport/tests/metrics.rs`다.

## 확장과 검증 범위

공식 확장은 `workers: {"directory":"./extensions","language":"node","enable_writes":true}`를 명시해 활성화한다. Python은 `language:"python"`이다. 기존 공통 엔진과 정본 구현을 사용하며 서버가 입력·권한·시간·출력·트랜잭션을 검사한다. 기본 worker 격리는 OS별 `auto`이며 macOS는 네트워크 차단, Linux는 Bubblewrap을 사용한다. DB pool, 분산 rate limit, cgroup, 디렉터리 hosting, local/S3/GCS 객체, 브라우저 WebSocket과 사용자 지정 한도는 [서버 인프라 설정](INFRASTRUCTURE.md)을 따른다. 외부 메시지 relay는 [BROKERS](BROKERS.md), 관리형 검색·생성은 [RAG](RAG.md)를 따른다.

실제 PostgreSQL의 소유 테스트 schema, 통제된 HTTPS 서버와 메모리 생성 RSA 키, offline 설치 SDK, 재배치 macOS binary로 검증한다. 외부 IdP 계정과 실제 reverse proxy 배포는 이 환경에서 실행하지 않았다. 대형 테이블 무중단 변경·자동 역방향 migration·actor resource 교체·임의 데이터 변환은 지원하지 않는다. 좁은 검증은 다음 명령으로 재실행한다.

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
- 거부하는 경우는 네 가지다. `FAILED`가 아니면 `JOB_NOT_FAILED`, 결과 보존 기간(24시간)이 지나 입력이 지워졌으면 `JOB_EXPIRED`, 다른 배포에서 수락했으면 `DEPLOYMENT_CHANGED`, 결과를 모르는 외부 효과가 있으면 `EFFECT_UNKNOWN`이다. 마지막 경우는 provider에서 실제 결과를 확인한 뒤 새 job으로 처리한다. 미확정 효과를 운영자가 확정하는 명령은 아직 없다.
- 재처리는 owner의 active job 상한(100)을 다시 검사하지 않는다. 대량 재처리는 운영자가 나눠서 실행한다.
