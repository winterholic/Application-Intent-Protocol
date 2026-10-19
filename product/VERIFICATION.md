# 제품 통합 검증 (2026-10-17)

## 2026-10-19 최신 실행 범위

- WBS 8.18.2: main `11cf652`의 [Linux worker jobs 39214468985](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39214468985)가 성공했다. 진단 21개·암호화 정규화 10개·migrate gate 1개·transport gate 1개를 원시 로그에서 확인했다. 암호화 정의 정규화와 실행 차단 경계가 해당 task의 범위이며 제품 저장 통합 완료로 세지 않는다.
- WBS 8.18.3 증분: DEK metadata의 최초 생성·scope 분리·caller rollback·KEK-only rewrap·DB 제약 회귀 7개를 추가했다. 로컬의 식별자 검사 1개, execution/service/CLI(MySQL 포함)/transport/prototype clippy와 기존 버전 보존을 포함한 잠금 파일 9개 metadata 검사를 확인했다. PostgreSQL을 쓰는 나머지 6개와 실제 provision 최소권한 증분은 Linux CI 대기다. 로컬 `initdb`의 기존 공유 메모리 권한 거부를 재시도하지 않았다.
- WBS 8.3: `73cb6bd`의 [Linux worker jobs 39212001257](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39212001257)에서 `job_breaker` 2개, OTLP 9개가 통과했다. OTLP 중 실제 DB/worker 경로는 HTTP→job 2개와 Node/Python 중첩 실행 1개다. 중첩 `ctx.jobs.run`은 별도 durable row를 enqueue하지 않고 root lease 안에서 실행된다. root→middle→leaf의 고유 span·실제 부모·동일 trace/root job.id·DB run 1개·민감 canary 배제를 확인했다. 과거 아래의 "중첩 job이 enqueue한 job" 서술은 이 구현 의미와 다르며 당시 미검증 기록으로 남긴다.
- WBS 8.5: `a8eb690` Verify 39210323292의 장부 25개와 [Object storage 39210323219](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39210323219)의 실제 TLS MinIO 6개가 통과했다. 기존 무장부 객체/두 번째 보류 종료 뒤 도착한 PUT의 명시적 confirm 복구, owner 격리, 중복 계상 방지, quota 초과 관측의 보존을 확인했다. 자동 목록/이벤트 발견과 provider 하드 상한은 여전히 보장하지 않는다.
- WBS 8.16: [Database contracts 39207996436](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39207996436)의 SCRAM 최소권한 1개·password helper 10개·MySQL/MariaDB live fence 각 5개를 확인했다. 아래의 CI 대기 서술은 당시 이력이다.
- WBS 8.18.1: `a8eb690` Verify의 all-features 구간에서 crypto 17개가 Linux 통과했다. WBS 8.18.2 준비 단계는 로컬 parser 164개·진단 21개·암호화 정규화 10개와 runtime/migrate gate 각 1개가 통과했다(164개에 진단/정규화 포함). 5형식 동일 facts/digest, Text/Email/Url/Decimal·owner/ref metadata, 실제 CLI check/gen, TS/Python binding 동일 지문과 암호 metadata 비노출을 확인했다. 개발 검사 옵션과 관계없이 서비스 기동과 migration/adopt는 암호화 metadata를 거부한다. 이는 실제 셀 저장·읽기·회전·저널 통합 완료가 아니다.
- `a8eb690` 전체 Verify는 `job_breaker`의 두 테스트가 한 프로세스에서 다른 schema 이름을 써 실패했다. `62ac64c`에서 같은 이름을 재사용하도록 고쳤고 위 Linux 작업에서 두 테스트 모두 통과했다. 해당 전체 Verify를 성공으로 세지 않는다. 최신 전체 회귀는 별도로 확인한다.

## 포팅 공통 HTTP 전송 전 실패 분류 (2026-10-19)

- 닫힌 loopback 포트 연결이 `Transport`가 되어 durable effect를 UNKNOWN으로 남기는 반례를 재현했다. 자동 retry·redirect·환경 proxy를 끈 broker의 `send`에서 `is_connect()`인 오류만 `NotSent`로 분리한다. durable host는 `WIRE.DELIVERY_FAILED`와 intent 제거로 응답한다. 연결 이후 전송/응답 손실은 불확정을 유지한다.
- 실제 연결 거부와 **전체 요청 본문 수신 후 응답 손실**을 구분하는 regression을 포함해 `aip-execution --test integrations` **11 passed**. execution 및 transport의 관련 all-targets clippy 통과. Node/Python job의 같은 run redrive·효과 key 재생과 수신 후 UNKNOWN 유지 회귀를 추가했으며 그 실DB 실행은 다음 Linux CI에서 확인한다. 로컬 DB 실행으로 합산하지 않는다.

## WBS 8.16 최소 권한 fixture와 DB 계약 CI (2026-10-19)

- PMS의 과거 수정 완료 서술과 달리 현재 main에는 runtime role의 비밀번호 지정과 DSN password 치환이 없었다. 기존 admin 비밀번호가 유지되는 음성 대조를 재현하고 keyword/URI DSN에서 role·password를 함께 바꾸는 helper와 특수 문자·중복 query password 회귀를 추가했다. `aip-test-support` **10 passed**, 관련 CLI/helper all-targets clippy 통과.
- 최소 권한 fixture는 `LOGIN PASSWORD`와 실제 `current_database()`의 CONNECT 권한을 쓰며, SCRAM fixture에서는 틀린 비밀번호가 CLI `DB_CONNECT`로 거부되는지 확인한다. `Database contracts`에 전용 PostgreSQL 17 SCRAM fixture와 MySQL 8.4/MariaDB 11.4 live fence·drift·capability 검사를 연결했다. 실DB 결과는 CI 확인 전이므로 아직 완료로 세지 않는다.
- WBS 8.2는 main `9d2c39f`의 [Deployment checks](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39205855579) 원시 로그에서 Caddy **16/16**, OIDC/TLS **18/18**, 실제 Keycloak **12/12** 통과를 확인했다. 인증된 operation의 선언 출력 본문과 무인증·주체 revoke 거부까지 검사했다. 실제 운영 IdP 계정·운영 배포 검증은 아니다.

## WBS 8.2 IdP/TLS CI와 동시성 회귀 재현 (2026-10-19)

- `Deployment checks`에 기존 Caddy proxy 검사뿐 아니라 전체 OIDC/TLS fixture와 실제 Keycloak smoke를 연결했다. native fixture는 host loopback upstream과 소유 TLS 파일 경로를 렌더링한다. 렌더러 양성·음성 테스트 **2개 통과**, 실제 `caddy adapt` 성공, Node·shell 문법 검사 통과. 전체 DB/API 검사는 Linux CI에서 확인하며 로컬 실행으로 합산하지 않는다.
- main `b4ce7ed`의 Linux Verify는 `v3_r3`에서 생성/철회 결과 `(WIRE.CHECK_FAILED, OK)`로 실패했다. 테스트가 고정 150ms 뒤 철회를 시작해 생성 검사가 먼저 실행됐다는 보장이 없었다. 커밋의 deferred trigger를 시험용 advisory lock으로 멈추고 `pg_blocking_pids`로 생성의 검사 완료와 철회의 실제 근거 행 잠금 대기를 확인하도록 바꿨다. 제품 잠금·정책을 완화하지 않았다. 새 테스트의 실DB 실행은 Linux CI 확인 전이다.

## WBS 8.4 Python binding의 엄격 타입·출력 계약 (2026-10-19)

- `mypy --warn-return-any`에서 생성된 operation helper 7개의 `Returning Any` 오류를 재현했다. TypedDict 반환 전에 같은 공개 scalar descriptor로 실제 객체·키·null·타입·범위·ID/enum/Decimal/날짜를 검사한다. 정상·잘못된 소비자 타입 검사도 추가해 input/return 타입이 Any로 빠지는 것을 막는다. 검사기 미설치는 실패로 처리한다.
- Python **3.9.6 / 3.10.21 / 3.11.16 / 3.12.14 / 3.13.15 / 3.14.7** 각각 SDK **25개 통과**, ResourceWarning·unclosed socket 출력 없음. 각 runtime에서 생성 module import와 별도 venv의 wheel 설치·repo 밖 import를 검사했다. mypy 검사는 host Python 3.14.7 / mypy 2.4.0으로 실행했으며 각 runtime 자체에서 mypy를 돌렸다고 합산하지 않는다.
- 생성기 회귀 **9개 통과**: 실제 생성된 Python helper를 safe/decimal 두 wire에서 실행해 잘못된 출력 24종을 거부했다. 관련 SDK crate all-targets clippy와 fmt 통과. 재빌드한 CLI로 생성한 operation fixture의 mypy 출력은 `Success: no issues found in 1 source file`이다.
- 이전 package build/egg-info 산출물은 Git HEAD의 SDK와 byte 일치를 확인하고 제거했다. 새 wheel은 소유 임시 경로에서 만들고 설치·검증 후 제거한다. CI는 mypy 2.4.0을 명시적으로 설치하고 ResourceWarning을 오류로 올린다.

## WBS 8.15 OTLP 큐 드롭 계측과 제품 job 연결 (2026-10-19)

- 기존 Linux Verify `da5817e`의 원시 로그에서 `otlp.rs` **6 passed, 0 failed**와 HTTP enqueue→저장 trace→worker→collector 연결 2개를 확인했다. 이 검사는 서버가 내보낸 span을 받으며 테스트가 span을 대신 만들어 성공시키지 않는다.
- 큐 포화는 이전에 latency만 확인했다. `aip_otlp_spans_dropped_total` 누적 counter를 Prometheus와 OTLP monotonic cumulative sum에 추가했다. 로그 배치 카운터 초기화와 분리하고, 단일-thread runtime에서 5000개를 enqueue해 queue 4096개를 넘는 **904개 드롭**을 검사한다. 실제 collector 수신도 확인했다.
- 로컬 `cargo test --lib otlp::tests`: **4 passed**. queue saturation **1 passed**, 실제 OTLP collector counter 수신 **1 passed**, transport all-targets clippy `-D warnings` 통과. 이 검사는 DB를 쓰지 않는다.
- 실제 job E2E에는 collector의 `job.id`와 enqueue 결과 ID 일치 검사를 추가했다. 이 추가 단언의 Linux/DB 실행은 다음 Verify CI에서 확인하며 로컬 sandbox의 DB 실행으로 합산하지 않는다.

## WBS 8.13 prototype CLI·검증 게이트 복구 (2026-10-19)

- 현행 prototype에서 `--locked` 실패와 CLI 2개 실패를 재현했다. 공유 생성기는 `OperationBinding`을 내보내는데 prototype SDK 진입점에 이 타입이 없었다. SDK의 누락된 binding 타입을 내보내고, 이전 `PrototypeBinding` 문자열만 요구하던 검사를 현재 계약에 맞췄다. 실제 SDK를 import하는 TypeScript 호출부 컴파일 검사도 함께 통과한다.
- `cargo test --manifest-path prototype/Cargo.toml --test cli --locked --target-dir target`: **13 passed, 0 failed**. `cargo clippy --manifest-path spikes/spike-v2-read/Cargo.toml --all-targets --locked --target-dir target -- -D warnings`와 prototype fmt, `git diff --check`도 통과했다. 이 CLI 검사는 DB에 연결하지 않는다.
- prototype lock 파일에 누락된 현재 경로 의존성을 동기화하고 `tools/verify.sh`에 prototype CLI를 추가했다. 별도 workspace인 prototype도 `debug = "line-tables-only"`로 맞춰 공유 target에 전체 DWARF 빌드를 중복하지 않는다.
- 앞선 main `da5817e`의 Verify 포함 5개 CI는 성공했다. 원시 Verify 로그에서 새 PostgreSQL Bool 정렬·NULL·커서·행 정책 검사와 전체 `AIP core verification passed.`를 확인했다. 이 prototype 변경의 CI 결과와는 구분한다.

## WBS 9.3~9.5 패키지 배포와 WBS 8.14 TLS 후속 (2026-10-19)

- main `db358fa`의 [App packages](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39189959354)는 Linux x64에서 경계 검사 73개, release 두 앱 이동·SDK 소비자 검사 1개, native/Docker/복원/worker lifecycle 1개가 모두 통과했다. 원본 입력과 중간 bundle을 지운 설치 경로의 binary·정의·SDK·확장을 사용했다. 검사 harness 및 TypeScript/Python 시험 도구는 checkout에 있다.
- lifecycle은 인증 HTTP read/apply, SIGTERM 종료와 재기동, 잘못된 migration digest 거부, nullable 필드 추가와 데이터·멱등 기록 보존, 구버전 정의의 schema fence 거부, 변경 전 dump 복원 후 이전 패키지 조회·멱등 처리를 확인한다. Linux에서는 별도로 non-root/read-only/cap-drop Docker image의 build·ready·인증 조회·정상종료·재생성과 외부 DB/설정 보존, 네이티브 Node/Python 격리 worker와 durable job을 두 wire로 검사했다. 운영 배포나 Docker 안의 worker 성공을 뜻하지 않는다.
- 같은 CI의 Docker namespace 사전 probe는 `docker exec <run-owned-container> bwrap --unshare-user --unshare-net --ro-bind / / -- /bin/true`가 exit 1, `No permissions to create a new namespace`였다. 따라서 이 **기본 Docker 정책 안**의 worker 격리는 지원 조건 미충족이다. 별도 네이티브 Linux worker 실행은 통과했다. privileged·무격리로 우회하지 않았다.
- macOS arm64에서 관련 Node 검사 75 passed / 0 failed. `event-checkin`과 `folder-notes` 예제도 실제 release package 생성 → 입력 삭제 → install → 중간 bundle 삭제 → 설치 경로 check를 통과했다. macOS의 Linux 전용 Docker/worker 단계는 `NOT RUN`으로 출력한다.
- 추가 실행한 [Deployment checks](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39189659128)는 큰 본문에서 Caddy 502가 나 15/16이었다. 직접 TCP 회귀 시험도 수정 전 `ConnectionReset`으로 1/2 실패했다. 응답 전송 후 write-half FIN과 최대 200ms/2MiB의 bounded discard를 추가했다. 요청 본문 상한을 늘리거나 초과 본문을 실행하지 않는다.
- `cargo test --offline --locked --manifest-path spikes/spike-v6-transport/Cargo.toml --target-dir target --test rejected_body_close --test v8_headers`는 수정 후 2+2 passed / 0 failed, 같은 library/두 test의 clippy `-D warnings`와 fmt도 통과했다. 이 두 socket 시험은 DB를 호출하지 않으며 명시적인 비접속 시험 DSN을 guard에 전달했다. 처음 실행의 DSN 미설정 build guard 실패는 행동 RED로 세지 않았다.
- 원본 worker 설정을 잠시 제거한 로컬 Caddy 진단에서는 큰 본문 검사가 PASS였으나 SSE가 `WIRE.NOT_AVAILABLE`로 실패해 전체는 15/16이었다. 이 진단 변경은 되돌렸다. **원래 worker 설정의 최종 TLS 16개와 TCP 수정 후 전체 main CI는 재검증 전**이다. 앞선 성공을 최종 SHA 성공으로 바꾸어 적지 않는다.

## WBS 9.2 앱별 release 패키징 (2026-10-19, macOS arm64, Codex 자체 검증)

- `node --test product/tests/app-package.test.mjs product/tests/app-manifest.test.mjs product/tests/package-boundaries.test.mjs product/tests/sdk-package.test.mjs`: 58 passed, 0 failed. 앱 두 종류의 원본 제거 후 무결성, 보조 확장 파일 포함, target/feature·lockfile·경로·credential·출력 보존, 동시 입력 변경 및 실패 정리를 검사했다.
- `node --test product/tests/app-package-e2e.test.mjs`: 1 passed, 0 failed. 실제 host release 엔진으로 Event/Review 두 앱을 패키징하고, 출력 이동 및 원본 앱 삭제 후 체크섬·실제 CLI check/gen·tarball 설치·엄격한 NodeNext binding 검사·SDK import를 각각 실행했다. Python 확장도 번들 입력으로 포함했지만 이 검사는 worker 실행·HTTP·DB E2E가 아니다.
- 추가한 실패 테스트에서 내장 `.aip` 예제 변경을 감지하지 못하는 경우를 재현하고 source snapshot 범위를 보완했다. SDK 검사의 전역 npm cache 쓰기 거부는 이 Codex sandbox의 `EPERM`이며, 소유 임시 cache로 바꿔 실행·회수했다. 호스트 전체의 npm 장애로 판단하지 않았다.
- `App packages` workflow는 Ubuntu의 release 이동 검증을 추가한다. 이 로컬 기록 시점에는 원격 CI를 아직 실행하지 않았다. 운영 배포·기동 자동화(9.3)와 Docker(9.4), HTTP/worker/DB 패키지 검증(9.5)은 이 결과에 포함하지 않는다.
- SDK scratch/npm cache와 테스트 앱·번들은 각 테스트의 `finally`/종료 hook으로 제거한다. 별도 clone/worktree는 만들지 않았고 기존 엔진 `target` 한 곳을 재사용했다.

운영 IdP·TLS reverse proxy 및 실제 broker(Kafka, RabbitMQ)·S3 호환 object storage(MinIO) 연동의 로컬 컨테이너 기반 종단간(E2E) smoke 검증 기록이다. 원 작업(커밋 `cbaa401`), 독립 교차 검증(`결과.md`), 그리고 후속 IdP 원인 진단(`idp-server-config-diagnostic.md`)의 실행 결과를 시점·환경·커밋별로 병기한다. CA 코드와 문서 보완은 독립 검증 PASS다. IdP 전체 smoke는 독립 교차 검증에서 FAILED(17개 중 3개 실행, 14개 미실행)였고, 아래 WBS 8.x 통합 회차에서 원래 설정으로 17 passed였다. 후자는 자체 실행이며 독립 검증은 남아 있다. (작업 기준 부모 커밋: `4e8e2126a91bd15e21afd870fe5373c0ccba9adb`).

## WBS 8.x 통합 회차 (2026-10-17, Claude 구현·통합 / GPT 독립 검증 미실시)

Codex 작업 세션이 취소된 7개 브랜치를 이어받아 main에 통합한 회차의 기록이다. 모든 DB 의존 테스트는 이 Run이 `initdb`로 새로 만든 PostgreSQL 17.11(`127.0.0.1:55432`, DB `aip_test_claude`)에서만 실행했다. 컨테이너는 IdP·Caddy smoke에만 썼다. 이 회차의 구현과 통합은 Claude가 했고, 별도 문맥의 GPT 독립 검증은 아직 하지 않았다. 아래 PASS는 같은 Run의 자체 실행 결과이며 독립 검증을 대신하지 않는다.

| 브랜치 | 처리 | 실행 결과 |
|---|---|---|
| `codex/db-test-isolation-20261017` | 병합. 누락된 경로를 추가로 막음 | 병합 시점: 루트 workspace 402 passed, 0 failed, 5 ignored, Node 가드·DB 테스트 26 passed. 최종 트리 전체 결과는 아래 "최종 전체 실행" |
| `fix/wbs-8.1-serializable-concurrency` | 병합. 내용이 위 브랜치 버전에 포함되어 그 버전을 유지 | `absence_serializable_http` 5/5 통과, `SERIALIZATION_RETRIES=0` 변이는 3/3 실패 |
| `fix/wire-commit-unknown-20261017` (8.9) | 병합 | `commit_unknown_wire` 통과. 수정 전 `lib.rs`로 되돌리면 `WIRE.INTERNAL`(`retry: later`)로 실패 |
| `codex/wbs-8-6-cli-lint-cleanup-20261017` (8.6) | 병합 | `clippy -p aip-cli --all-targets -D warnings`가 main에서는 오류 5개 타깃(12·41·39·40·91건), 병합 후 통과. 변경 테스트 6개 타깃 통과 |
| `feature/wbs-8.4-python-sdk` (8.4) | 병합 | `python_sdk_test` 23 OK(생성 모듈 import·mypy, 깨끗한 venv wheel 설치 포함), spike-v5 46 passed, aip-cli `service` 4 passed, `service-smoke` 1 passed. 저장소의 `.aip` 21개 모두 생성되고 mypy 통과 |
| `feature/wbs-8.2-8.3-recovery-20261017` (8.2/8.3) | 일부 병합. `AIP_BINARY` 검사, `smoke-otlp.sh`, 큐 포화 테스트만 반영 | IdP·TLS smoke 원래 설정(workers 포함) 17 passed, 0 failed. OTLP 5 passed, 큐 포화 테스트 10/10 통과 |
| `fix/otlp-status-code-standard` | 병합하지 않음. 코드는 이미 main에 동일하게 있음 | `otlp.rs`·`trace.rs`는 main과 동일, 나머지 차이는 이후 가드·8.9 변경뿐. 원 브랜치의 문서 변경은 정정된 STATUS/VERIFICATION을 되돌리므로 가져오지 않음 |

### 최종 전체 실행

통합 마지막 코드 커밋 `d9ca0d8faad5672c87ddcd7423286b341eb31f54`에서 같은 Run 전용 PostgreSQL로 한 번에 실행했다. 이 뒤의 커밋은 문서만 바꾼다.

| 명령 | 결과 |
|---|---|
| `cargo test --workspace --locked --no-fail-fast` | 404 passed, 0 failed, 5 ignored |
| `cargo test --manifest-path spikes/spike-v{2-read,3-write,4-worker,5-sdk,6-transport,7-migrate,11-dev-checks}/Cargo.toml --locked --no-fail-fast` | v2 102, v3 33, v4 62(1 ignored), v5 46, v6 184, v7 1, v11 13 passed, 모두 0 failed |
| `cargo test --manifest-path prototype/Cargo.toml --locked --no-fail-fast` | 35 passed, 2 failed. 실패는 `cli`의 `gen_emits_a_typed_binding_...`, `generated_binding_resolves_...`이며 main에서도 같다 |
| `node --test product/tests/{postgres-guard,general-update,uuid-external,facereview-comments,service-smoke}.test.mjs` | 27 passed, 0 failed |
| `python3 -m unittest product.tests.python_sdk_test` | 23 tests OK |
| `cargo clippy --workspace --lib --bins --locked -- -D warnings`, `cargo clippy -p aip-auth -p aip-migrate -p aip-service -p aip-cli --all-targets --locked -- -D warnings`, `cargo fmt --all --check` | 모두 통과 |

### 이번 회차에서 고친 문제

- DB 가드가 불완전했다. `spike_v2_read::connect()`와 `DB_URL`, `127.0.0.1:5432` 하드코딩 relay(spike v6 `db_*_lifetime`, prototype `db_startup`), 사용자 없는 proxy DSN, prototype `lifecycle`/`serve`, `product/tests/uuid-external.test.mjs`가 기본 엔드포인트를 쓰고 있었다. 모두 명시 DSN으로 바꾸고 `spikes/spike-v2-read/tests/test_database_default.rs`로 `connect()`가 DSN 없음·기본 DSN을 거부함을 고정했다.
- Node 가드는 URI query 키를 대소문자 구분으로 비교했다. 실제 libpq(`psql`)는 `HOSTADDR`·`HOST`·`SERVICE`·`SERVICEFILE` 대문자 키를 `invalid URI query parameter`로 거부하고 소문자 `hostaddr`만 적용하므로 우회 경로는 아니었다. Rust 가드와 같게 정렬하고 회귀 테스트를 추가했다. tokio-postgres의 대소문자 처리는 소스 읽기로만 확인했다.
- db-test-isolation 브랜치는 spike v2/v3/v4/v5/v7/v11과 prototype의 `Cargo.lock`을 갱신하지 않아 `--locked` 빌드가 실패했다. 갱신했다.
- CI 워크플로가 `aip_test_ci` DB를 만들지 않거나(brokers, worker-jobs-linux) DSN을 주지 않았다(deploy-checks). 맞췄다. GitHub Actions에서는 실행하지 못했다.
- `wire_codes::no_code_is_written_without_its_area`가 main(0eb0ce5)에서 이미 실패하고 있었다. metrics·OTLP의 job 상태 `RETRY`·`PENDING`·`RUNNING`이 허용 목록에 없었다. 목록에 추가했다.
- 8.9 수정 전 코드는 이미 커밋된 쓰기를 `WIRE.INTERNAL`과 `retry: later`로 돌려줬다. 응답 단절 시 `WIRE.COMMIT_UNKNOWN`으로 바꿨다.

### 사고: 공유 PostgreSQL 접속

위 가드 누락을 알기 전에 spike v2/v3/v4 전체와 spike v6 일부, prototype `db_startup`을 실행했다. 이 테스트들은 DSN에 포트가 없어 이 Mac의 공유 PostgreSQL(`127.0.0.1:5432`)에 접속했다. 대부분 자기 이름의 스키마를 만들고 지우는 테스트이고 `db_startup`은 인증만 중계했지만, 접속 사실과 중간 실패로 남은 스키마가 있는지는 확인하지 못했다. 공유 DB 접속은 금지 대상이라 확인용 접속도 하지 않았다. 인지한 즉시 실행을 멈추고 사용자에게 알림을 보냈다. `spike-v4-worker/extensions/recruitment.{mjs,py}`는 worker가 DB에 직접 닿는지 보는 격리 probe로, 5432에 TCP 연결만 시도한다. 그 probe는 의도된 fixture라 바꾸지 않았다.

### 이번 회차에도 남은 것

- 당시 `prototype` 테스트 `cli`의 2개(`gen_emits_a_typed_binding_...`, `generated_binding_resolves_...`)는 main에서도 `OperationBinding` 미export로 실패했다. **2026-10-19 후속에서 수정·13개 통과**, 위 WBS 8.13 기록 참조.
- 당시 `cargo clippy --all-targets`는 spike-v2-read의 `needless_lifetimes` 때문에 실패했다. **2026-10-19 현행 v2 all-targets `-D warnings` 통과**, 위 WBS 8.13 기록 참조.
- `prototype/Cargo.lock`은 main에서 `--locked` 검사가 이미 실패했고 이번에 갱신했다.
- 당시 8.3 큐 포화 테스트는 호출 latency만 확인했다. **2026-10-19 후속에서 누적 드롭 counter 및 collector 수신 검사를 추가**, 위 WBS 8.15 기록 참조.
- 8.9는 연결이 닫힌 경우(`is_closed`)만 `COMMIT_UNKNOWN`으로 분류한다. 그 밖의 연결 오류 형태는 실행해 보지 않았다. (다음 절 "WBS 8.9 분류 보강"에서 실행했다.)
- GitHub Actions 워크플로 변경, 실제 Kafka·RabbitMQ·MinIO smoke, Linux 격리는 이번 회차에 실행하지 않았다.

## WBS 8.9 분류 보강 (2026-10-18, Claude 구현 / 독립 검증 대기)

대상은 `spikes/spike-v6-transport`의 `/apply` 쓰기 경로다. 이 회차의 구현과 실행은 같은 Run의 Claude이고, 별도 문맥의 독립 검증은 아직 없다. DB 테스트는 이 Run이 만든 PostgreSQL 17.11 컨테이너(`127.0.0.1:32786`, DB `aip_test_wbs89`)에서만 실행했고 공유 `127.0.0.1:5432`에는 접속하지 않았다. 저장소의 DSN 가드(`aip-test-support`)가 켜진 상태다.

### 바꾼 분류

| 연결 단절 시점 | 수정 전 | 수정 후 |
|---|---|---|
| COMMIT 전송 전(UPDATE 직후, 멱등 INSERT 직후) | `WIRE.INTERNAL`, `retry: later` | `WIRE.DB_UNAVAILABLE`. 커밋되지 않았고 같은 키로 다시 보내면 새로 실행된다 |
| COMMIT 호출 시점에 연결이 이미 닫힘(COMMIT이 나가지 못함) | `WIRE.COMMIT_UNKNOWN`(표지를 먼저 올림) | `WIRE.DB_UNAVAILABLE`, 표지 유지 안 함 |
| COMMIT 전송 뒤 응답 손실(FIN, RST, 서버가 COMMIT을 받지 못함, backend 종료) | `WIRE.COMMIT_UNKNOWN` | 같다 |
| COMMIT 응답을 해석할 수 없음(`Kind::Parse`), 예상 밖 메시지(`Kind::UnexpectedMessage`) | `WIRE.INTERNAL` | `WIRE.COMMIT_UNKNOWN` |
| 서버가 거절한 COMMIT(23514, 23505, 40001, 40P01) | `BAD_VALUE`, `ALREADY_EXISTS`, `CONFLICT` | 같다. `COMMIT_UNKNOWN`으로 넓히지 않는다 |

판정은 `connection_failed`(SQLSTATE가 없고 `is_closed`이거나 Io·UnexpectedMessage·Parse 문구)이다. 서버 SQLSTATE가 있는 오류와 로컬 파라미터 직렬화 오류는 연결 오류로 보지 않는다. COMMIT 전 단계에서 쓰기 엔진이 오류 원본을 지운 거절은 연결이 이미 닫혔고 코드가 `WIRE.INTERNAL`일 때만 `DB_UNAVAILABLE`로 바꾼다.

### 실행 결과

| 명령(`spikes/spike-v6-transport`, `/tmp/aip-wbs89-classify-20261018`) | 결과 |
|---|---|
| 새 테스트를 먼저 쓰고 분류 코드는 수정 전과 같은 상태로 `cargo test --locked --test commit_unknown_wire` (red) | 11개 중 5 failed: 전송 전 단절 2개(`WIRE.INTERNAL`), 이미 닫힌 연결(`COMMIT_UNKNOWN`), 해석 불가·예상 밖 응답 2개(`WIRE.INTERNAL`) |
| 수정 후 같은 명령 (green) | 11 passed, 0 failed |
| 변이 1: `connection_failed`를 `is_closed`만으로 되돌림 | 2 failed (`unparseable_*`, `unexpected_message_*`) |
| 변이 2: COMMIT 전 `is_closed` 검사 제거 | 1 failed (`commit_on_an_already_closed_connection_*`) |
| 변이 3: 전송 전 코드를 `WIRE.INTERNAL`로 되돌림 | 3 failed (전송 전 단절 2개, 이미 닫힌 연결) |
| 변이 4: `connection_failed`를 항상 true로 넓힘 | 2 failed (`definite_commit_rejections_*`, `connection_failure_predicate_*`) |
| `cargo clippy --all-targets --locked -- -D warnings` (spike-v6) | 통과. 수정 전에는 `commit_unknown_wire.rs` 105·202·221줄의 3개(`manual_range_contains`, `bool_assert_comparison` 2개)로 실패 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` (루트) | 통과(수정 전에도 통과. spike는 루트 workspace가 아니다) |
| `cargo fmt --all --check` (루트) | 통과. 수정 전 main에서도 `spike-v6-transport/tests/otlp.rs` 포맷 차이로 실패하고 있어 그 파일만 포맷했다 |
| `cargo test --locked --no-fail-fast` (spike-v6 전체) | 187 passed, 7 targets failed(아래) |

전체 실행의 실패 7개 타깃은 수정 전 main에서도 같은 테스트가 같이 실패했다. `v12_typed`, `v13_idwire`, `v14_apply`, `v15_values`, `v16_bundle`은 `tsc`가 이 환경에 없어 파일을 찾지 못하는 오류(`NotFound`)이고, `--lib`는 `pool::tests::checkout_is_bounded_by_pool_capacity_and_acquire_deadline`(30ms 기한)이다. 이 pool 테스트는 단독 실행 3회는 통과했다. `write_isolation`은 pool checkout 기한 초과(`WIRE.DB_UNAVAILABLE`)로 실패하며 원인은 조사하지 않았다. 이 항목들은 이번 변경과 무관한 사전 실패다.

### 미검증·남은 것

- `Kind::Io`는 연결 task 안에서만 생기고 호출자에게는 `Closed`로 전달된다. RST를 보내는 테스트도 호출자에게는 `Closed`로 보였다. 따라서 `Io` 자체의 호출자 쪽 재현은 못 했다. 표시 문구 비교는 tokio-postgres 0.7.18 기준이며, 그 문구가 바뀌면 해석 불가·예상 밖 응답 테스트가 깨지도록 했다.
- 서버가 보내는 SQLSTATE 08·57P01(관리자 종료) 오류는 분류에 넣지 않았다. `pg_terminate_backend` 재현에서도 호출자는 FATAL 대신 `Closed`를 받아 그 경로를 만들 수 없었다.
- COMMIT 호출 직전의 `is_closed` 검사와 실제 전송 사이의 짧은 경합에서 닫히면 `COMMIT_UNKNOWN`(보수적 쪽)으로 보고된다.
- 이 보강은 `/apply` 핵심 경로(`apply_attempt`)만 고쳤다. 같은 COMMIT 분류가 필요할 수 있는 다른 경로는 이 절에서 실행하지 않았다(다음 절 "WBS 8.9 나머지 COMMIT 경로"에서 실행했다): `write_extension.rs`(`classify_write_error("커밋", ..)`), `spike-v4-worker/src/write.rs`, `spike-v3-write/src/lib.rs`의 COMMIT, `durable.rs`.
- COMMIT 전 단계에서 쓰기 엔진이 오류 원본을 지운 거절은 서버 FATAL(57P01 등)이면 `WIRE.INTERNAL`로 남는다.
- GPT 독립 검증 전이다. 완료가 아니라 "독립 검증 대기"다.

## WBS 8.9 나머지 COMMIT 경로와 spike-v6 사전 실패 (2026-10-18, Claude 구현 / 독립 검증 대기)

앞 절의 분류 규칙을 `write_extension.rs`, `spike-v4-worker/src/write.rs`, `spike-v3-write/src/lib.rs`, `durable.rs`에 적용하고, spike-v6의 사전 실패 세 가지를 조사했다. 이 회차의 구현과 실행은 같은 Run의 Claude이고 별도 문맥의 독립 검증은 아직 없다. DB 테스트는 이 Run이 만든 PostgreSQL 17.11 컨테이너(`aip-pg-wbs89b-1018003521-51030`, `127.0.0.1:32787`, DB `aip_test_wbs89b_1018003521_51030`)에서만 실행했고 공유 `127.0.0.1:5432`에는 접속하지 않았다. 저장소의 DSN 가드(`aip-test-support`)가 켜진 상태다. 컨테이너는 이 Run이 띄웠고 작업이 끝난 뒤 정리했다.

### spike v3·v4를 고친 근거

세 spike는 실험 코드지만 제품 경로다. `aip-service`와 `aip-auth`, `aip-cli`가 `spike-v6-transport`를 path dependency로 쓰고, v6는 다시 v3·v4를 쓴다. README도 "독립 spike의 검증된 엔진을 path dependency로 재사용한다"고 적는다. 그래서 문서화만 하지 않고 고쳤다.

### 공통 함수

a5774b3의 판정(`connection_failed`, `lost_before_commit`, 거절 재분류, COMMIT 전송과 분류)은 v6 `lib.rs`에만 있었다. 의존 방향이 v6, v4, v3 순이라 가장 아래인 `spike-v3-write`로 옮겼고 v4와 v6는 그것을 쓴다.

| 함수 | 역할 |
|---|---|
| `connection_failed(&Error)` | SQLSTATE가 없고 `is_closed`이거나 Io·UnexpectedMessage·Parse 문구인 오류 |
| `lost_before_commit()` | COMMIT 전에 연결을 잃은 실패(`WIRE.DB_UNAVAILABLE`) |
| `before_commit_rejection(&tx, Reject)` | 쓰기 엔진이 오류 원본을 지운 `WIRE.INTERNAL` 거절이 연결이 닫힌 뒤의 것이면 `DB_UNAVAILABLE`로 바꿈 |
| `send_commit(tx, on_send)` | 연결이 이미 닫혔으면 `NotSent`, COMMIT을 보낸 뒤 연결 오류면 `Unknown`, 서버가 SQLSTATE로 거절하면 `Rejected(e)`. `on_send`는 보내기 직전에만 불려 결과 불명 표지를 올린다 |

v6 `/apply`(`commit_attempt`)도 같은 `send_commit`을 쓰도록 바꿨다. 앞 절의 `commit_unknown_wire` 11개는 리팩터링 뒤에도 그대로 통과했다.

### 경로별 적용

| 경로 | COMMIT 전 연결 끊김 | COMMIT 전송 뒤 끊김·해석 불가 | 서버 거절 |
|---|---|---|---|
| v3 `apply`·`bundle`·`compose`(`commit`, 세 진입점) | `WIRE.DB_UNAVAILABLE`(수정 전 `WIRE.INTERNAL`) | `WIRE.COMMIT_UNKNOWN`(수정 전 `WIRE.INTERNAL`) | 기존 코드 유지 |
| v4 `invoke_write` | `WIRE.DB_UNAVAILABLE`. 확장이 ctx 쓰기 중 연결을 잃은 경우도 포함(수정 전 `WIRE.EXTENSION_ERROR`) | `WIRE.COMMIT_UNKNOWN`(수정 전 `WIRE.INTERNAL`) | 기존 코드 유지 |
| v6 `write_extension::apply` | `WIRE.DB_UNAVAILABLE`(수정 전 `WIRE.INTERNAL`) | `WIRE.COMMIT_UNKNOWN`(수정 전 `WIRE.INTERNAL`) | 기존 코드 유지 |
| v6 `durable.rs` 쓰기 요청 COMMIT 두 곳(job 접수·파일·메시지 handler, trigger 접수) | 같다(이미 `DB_UNAVAILABLE`). COMMIT이 나가지 못한 경우만 `DB_UNAVAILABLE`로 바꿈(수정 전 `COMMIT_UNKNOWN`) | `WIRE.COMMIT_UNKNOWN`(같다) | 기존 `COMMIT_UNKNOWN`을 유지(넓히지도 좁히지도 않음) |
| v6 `durable.rs` managed effect journal COMMIT(데이터 apply·파일·이벤트·메시지) | `WIRE.DB_UNAVAILABLE`. 이제 job이 이 코드로 재시도된다(수정 전에는 확장 오류로 `FAILED`) | `WIRE.COMMIT_UNKNOWN`으로 job 종료(수정 전 `FAILED`와 `WIRE.EXTENSION_ERROR`) | `WIRE.DB_UNAVAILABLE`(기존) |

durable.rs의 나머지 COMMIT 지점은 바꾸지 않았다. 이유는 지점별로 다르다.

- 읽기 전용이거나 principal 확인만 하는 트랜잭션(`guard`, 하트비트, 읽기 op, 파일 get, 재생 조회)과 만료 정리(GC)는 쓰기 결과를 호출자에게 보고하지 않거나 멱등이다.
- 워커 내부에서 로그만 남기는 지점(job 결과·취소·admission 기록)은 이미 오류를 모두 같은 로그로 남기며 임대 만료가 복구한다.
- HTTP effect의 journal(PENDING·UNKNOWN·SUCCEEDED 표시) COMMIT은 `status` 응답이 PENDING·UNKNOWN 행을 `WIRE.EFFECT_UNKNOWN`으로 덮어 보여 주는 별도 장치가 있다. 이 장치를 대체하지 않으려고 바꾸지 않았다.
- 자식 job·operation 결과 기록(`state=$3,result=$4`)과 업무 거절 기록(`reject_data_effect`)은 재현하지 않아 바꾸지 않았다. 후자는 데이터가 바뀌지 않는다.

managed effect journal의 변경은 제품 동작 변경이다. 두 가지가 바뀐다. 하나는 COMMIT을 보낸 뒤 응답을 잃으면 job이 확장 오류가 아니라 `WIRE.COMMIT_UNKNOWN`으로 끝나는 것이다(`FAILED`이지만 쓰기는 커밋됐을 수 있고, 자동 재시도하지 않는다). 다른 하나는 `WIRE.DB_UNAVAILABLE`을 job host가 확장에 넘기지 않고 job 시도 실패로 올려, 이미 있던 재시도 규칙(`retryable`)이 적용되는 것이다. `product/MANAGED-EXECUTION.md`의 "DB 장애는 아무것도 commit하지 않았으므로 ... 다시 시도할 수 있다"는 기존 문서 문장을 코드가 처음으로 지킨다. 확장이 `catch`로 DB 장애를 직접 처리하던 job은 이제 그 오류를 받지 못하고 시도 전체가 재시도된다. 이 동작 변경은 독립 검증에서 따로 봐야 한다.

### 테스트 도구

장애 지점마다 같은 TCP proxy가 필요해서 `crates/aip-test-fault-proxy`를 새로 만들었다. 테스트 DB 가드(`aip-test-support`)가 허용한 endpoint 앞에만 서고, 한 번만 장애를 일으킨다. 장애는 COMMIT 성공 응답 삭제(FIN·RST), COMMIT을 서버에 전달하지 않고 닫기, COMMIT 응답을 다른 메시지로 바꾸기, 지정 문장이 끝난 직후 닫기가 있고, COMMIT 단계 장애는 지정한 문장을 보낸 연결에서만 일으킬 수 있다(서버 배경 작업의 COMMIT이 장애를 먼저 쓰지 못하게 한다). 앞 절의 `commit_unknown_wire` 테스트는 자체 proxy를 그대로 쓴다.

### 실행 결과

모든 명령은 `/tmp/aip-wbs89b-20261018`(이 회차 worktree)에서 실행했다. spike 명령은 해당 `spikes/<crate>` 디렉터리에서 `cargo test --locked`다.

| 대상 | red | green |
|---|---|---|
| v3 `commit_classification`(`apply`·`bundle`·`compose`) | 14개 중 12 failed(`WIRE.INTERNAL`, 구현 전). 통과 2개는 서버 거절 코드 유지와 판정 함수 대조 | 구현 뒤 14 passed. 이후 compose 3개와 BEGIN 단절 1개를 더해 18 passed |
| v4 `commit_classification` | 9개 중 8 failed(`write.rs`를 HEAD로 되돌려 실행). 통과 1개는 서버 거절 코드 유지 | 9 passed. BEGIN 단절을 더해 10 passed |
| v6 `write_extension_commit_classification` | 10개 중 8 failed(`write_extension.rs`를 HEAD로 되돌려 실행). 통과 2개는 서버 거절 코드 유지와, v4 수정이 이미 해결한 확장 쓰기 중 단절 | 10 passed. BEGIN 단절과 확장 세션 설정 단절을 더해 12 passed |
| v6 `durable.rs` 단위 `commit_request_on_an_already_closed_connection_is_db_unavailable` | 스텁을 수정 전 동작으로 두고 1 failed(`COMMIT_UNKNOWN`) | 통과 |
| v6 `durable_commit_classification`(HTTP `/jobs/enqueue`, 4개) | 처음부터 4 passed. 수정 전 코드도 이 단절 지점들은 이미 규칙대로였고, 이 테스트는 그 동작을 고정한다 | 4 passed |
| v6 `durable_job_commit_loss`(job의 데이터 apply COMMIT 손실 2개, 전이 UPDATE 직후 단절 1개) | 3개 모두 failed(`WIRE.EXTENSION_ERROR`로 끝남. COMMIT 응답 손실 시에도 데이터는 커밋됨) | 3 passed(`COMMIT_UNKNOWN` 2개, 재시도 뒤 `SUCCEEDED`·시도 2회·쓰기 1회 1개) |
| v6 `commit_unknown_wire`(앞 절) | 리팩터링 뒤에도 11 passed | BEGIN 단절 1개를 더해 12 passed |
| 전체: v3, v4, v6 각 `cargo test --locked --no-fail-fast` | | v3 51 passed, v4 72 passed(1 ignored), v6 217 passed, 모두 0 failed |

### 변이 확인

분류를 되돌리거나 넓히는 변이를 하나씩 넣고 관련 스위트(v3·v4·v6)를 다시 실행했다. 20개 변이가 모두 실패로 잡혔다(killed). 처음 실행에서 살아남은 M9와 M12는 해당 지점을 겨냥한 테스트(v4의 `START TRANSACTION` 단절, v6 `write_extension`에서 v4가 보내는 세션 설정 문장 단절)를 더한 뒤 잡혔다. M19도 같은 방식으로 v6 `commit_unknown_wire`에 `START TRANSACTION` 단절 테스트를 더해 확인했다.

| 변이 | 잡은 스위트와 실패 수 |
|---|---|
| M1 `connection_failed`에서 문구 판정 제거 | v3 2, v4 2, v6 4 |
| M2 `send_commit`의 이미 닫힘 검사 제거 | v3 3, v4 1, v6 1, v6 lib 2 |
| M3 `send_commit`이 전송 뒤 연결 오류를 서버 거절로 취급 | v3 8, v4 5, v6 13 |
| M4 `before_commit_rejection`이 거절을 바꾸지 않음 | v3 3, v4 2, v6 2 |
| M5 `connection_failed`가 항상 참 | v3 2, v4 1, v6 4 |
| M6 v3 `begin_failure`가 항상 내부 오류 | v3 2 |
| M7 v4 ctx 쓰기 연결 단절을 바꾸지 않음 | v4 1, v6 1 |
| M8 v4 `invoke_write`의 거절 재분류 제거 | v4 1 |
| M9 v4 `invoke_write`의 트랜잭션 시작 단절을 분류하지 않음 | v4 1 |
| M10 v6 `write_extension`의 COMMIT 전 오류가 항상 `INTERNAL` | v6 4 |
| M11 v6 `write_extension`이 전송 뒤 불명을 `DB_UNAVAILABLE`로 냄 | v6 5 |
| M12 v6 `write_extension`이 준비 단계 거절을 재분류하지 않음 | v6 1 |
| M13 v6 `commit_attempt`의 `NotSent`를 `COMMIT_UNKNOWN`으로 냄 | v6 1 |
| M14 durable 쓰기 요청이 전송 뒤 불명을 `DB_UNAVAILABLE`로 냄 | v6 3 |
| M15 durable 쓰기 요청이 `NotSent`를 `COMMIT_UNKNOWN`으로 냄 | v6 lib 1 |
| M16 durable effect가 전송 뒤 불명을 `DB_UNAVAILABLE`로 냄 | v6 2 |
| M17 durable effect가 `NotSent`를 `COMMIT_UNKNOWN`으로 냄 | v6 lib 1 |
| M18 v4 job host가 `COMMIT_UNKNOWN`을 넘기지 않음 | v6 2 |
| M19 v6 `/apply`의 트랜잭션 시작 단절을 분류하지 않음 | v6 1 |
| M20 v4 job host가 `DB_UNAVAILABLE`을 넘기지 않음 | v6 1 |

### 검사 명령

| 명령 | 결과 |
|---|---|
| `cargo fmt --all --check` (루트) | 통과 |
| `cargo clippy --workspace --lib --bins --locked -- -D warnings` | 통과 |
| `cargo clippy -p aip-auth -p aip-migrate -p aip-service -p aip-test-fault-proxy -p aip-test-support --all-targets --locked -- -D warnings` | 통과 |
| `cargo clippy -p aip-cli --features poc-runtime --bins --locked -- -D warnings` | 통과 |
| `cargo clippy --all-targets --locked -- -D warnings` (spike-v6) | 통과 |
| `cargo clippy --lib --test commit_classification --locked -- -D warnings` (spike-v3, spike-v4) | 각각 통과 |
| `cargo clippy --all-targets --locked -- -D warnings` (spike-v3, spike-v4) | 실패. main(`a5774b3`)에서도 같다. v3는 `general_update` 테스트의 참조 오류 1건, v4는 lib 테스트의 `marker_processes` 미사용과 길이 비교 오류 2건이며 이번 변경 파일이 아니다 |
| `cargo test --workspace --locked --no-fail-fast` | 405 passed, 0 failed, 5 ignored |
| `cargo test -p aip-test-support --features strict-db --locked` | 7 passed, 0 failed |
| `cargo test --locked --no-fail-fast` (spike-v3, spike-v4, spike-v6) | v3 51 passed, v4 72 passed(1 ignored), v6 217 passed, 모두 0 failed |

워크스페이스 전체 실행에서 `aip-cli`의 `least_privilege`가 처음에 실패했다(`DB_CONNECT`). 이 Run의 컨테이너가 비밀번호 인증(`scram-sha-256`)만 받아 비밀번호 없는 `CREATE ROLE ... LOGIN` 역할이 접속하지 못한 것이고, 코드 변경 전 main(`a5774b3`)에서도 같은 실패였다. 이 Run 전용 컨테이너의 `pg_hba.conf`를 `trust`로 바꿔 다시 실행했더니 통과했고(위 405 passed), 공유 DB는 건드리지 않았다.


### spike-v6 사전 실패 조사

| 항목 | 판단 | 조치와 결과 |
|---|---|---|
| `write_isolation` | 제품 결함이 아니라 테스트 지원 함수 결함이다. `aip_test_support::database_url_with_option`이 URI의 option 값을 form 인코딩(`append_pair`)해 공백을 `+`로 썼다. libpq와 tokio-postgres는 URI query의 `+`를 공백으로 읽지 않아 서버가 `+default_transaction_isolation`이라는 이름의 설정으로 받고 `unrecognized configuration parameter`로 접속을 거부했다. 서버 pool이 연결을 못 얻어 `WIRE.DB_UNAVAILABLE`(pool checkout 오류)이 났다. `postgres://` 형식 DSN에서만 생기고 키워드 형식 DSN은 영향이 없다 | 값을 퍼센트 인코딩(공백은 `%20`)으로 쓰고 기존 query를 유지하도록 고쳤다. 단위 테스트(`uri_option_values_encode_spaces_as_percent_20`)는 수정 전 코드에서 failed, 수정 뒤 passed. `write_isolation` 1 passed. 같은 함수를 쓰는 `aip-migrate`의 `migration_history`도 main에서 2 failed(`DB_CONNECT`)였고 수정 뒤 3 passed |
| pool 30ms 타임아웃 | 시간에 민감한 불안정 테스트다. `acquire_timeout`이 30ms인 pool에서 첫 `acquire()`가 새 연결을 만들면 연결 시간이 같은 30ms 기한을 쓴다. 부하가 있으면 첫 호출이 `Timeout`으로 먼저 실패했다(`pool.rs:237`의 `unwrap`). 두 번째 호출은 슬롯이 잡혀 있어 성공할 수 없으니 이 시간 의존은 불필요하다 | 연결을 idle 목록에 미리 넣어 첫 checkout이 연결을 만들지 않게 하고 두 번째 호출만 30ms 기한을 받게 했다. 최소 대기 시간 단언을 더했다. spike-v6 lib 테스트 전체를 연속 실행해 수정 전 8회 중 4회 실패, 수정 뒤 12회 중 0회 실패였다(둘 다 같은 부하의 다른 변이 실행이 겹친 상태) |
| v12~v16 `tsc` 부재 | 환경 문제다. 이 테스트들은 `spikes/spike-0-ts/node_modules`의 `tsc`를 쓰는데 그 디렉터리는 추적되지 않는 설치물이라 새 worktree에는 없다. 주 checkout에는 있다 | 전역 설치는 하지 않았다. 저장소가 정한 경로 `npm ci --prefix spikes/spike-0-ts --ignore-scripts`로 설치(typescript 7.0.2)한 뒤 `v12_typed`, `v13_idwire`, `v14_apply`, `v15_values`, `v16_bundle` 5개가 모두 passed. 코드 변경은 없다 |

### 미검증·남은 것

- `Kind::Io`의 호출자 쪽 직접 재현, 서버가 보내는 SQLSTATE 08·57P01 오류의 분류는 앞 절과 같이 미실행이다.
- `write_extension.rs`와 v4 `invoke_write`의 "COMMIT 호출 직전에 이미 닫힘" 검사는 `send_commit` 하나로 합쳤고, 그 분기는 v3의 세 경로(일시 정지 knob과 `pg_terminate_backend`), v4, v6 `commit_attempt`, durable 단위 테스트가 본다. `write_extension` 경로에서 이 분기에만 닿는 재현은 만들지 못했다(확장 뒤 멱등 INSERT가 먼저 닫힌 연결을 만난다).
- `durable.rs`의 자식 job 결과 기록과 업무 거절 기록, GC·워커 내부 지점은 실행하지 않았다. managed effect journal에서 COMMIT 응답을 잃은 job을 운영자가 어떻게 확정하는지(명령)는 아직 없다.
- 서버 거절 COMMIT(제약 위반·직렬화 실패)을 `durable.rs`의 쓰기 요청 경로가 `COMMIT_UNKNOWN`으로 내는 것은 기존 동작을 유지한 것이다. 확정 rollback인데 결과 불명으로 보고하는 보수적 분류이며, 바꿀지는 정하지 않았다.
- managed effect journal의 job host 변경(`DB_UNAVAILABLE` 넘김, `COMMIT_UNKNOWN` 종료)은 제품 동작 변경이다. 이 변경이 다른 job 시나리오(HTTP effect, 중첩 job, 메시지)에 미치는 영향은 v6 전체 스위트가 통과한 범위까지만 확인했고 운영 환경은 미실행이다.
- 다른 crate의 직접 COMMIT(`aip-runtime`, `aip-execution`, `aip-migrate`, `aip-service`의 `tx.commit()`)은 이번 범위가 아니라 분류를 확인하지 않았다. 대부분 `map_err(|_| ...)`로 오류를 종류와 상관없이 자기 코드(`COMMIT_UNSETTLED`, `JOBS_UNSETTLED` 등)나 `WIRE.COMMIT_UNKNOWN`으로 한 번에 내므로, COMMIT 전 단절을 `DB_UNAVAILABLE`로 가르는 규칙이 필요한지는 별도로 봐야 한다.
- GPT 독립 검증 전이다. 완료가 아니라 "독립 검증 대기"다.

## WBS 8.10 독립 검증 결함 6건 수정 (2026-10-18, Claude 구현 / 독립 검증 대기)

GPT 독립 검증(기준 HEAD `7585ec5`)이 찾은 결함이다. 상세 재현 보고서는 가드가 읽기를 막아 읽지 못했고 요청에 적힌 요약으로 작업했다. DB는 이 Run의 PostgreSQL 17 컨테이너(`127.0.0.1:32801`)만 썼고, Keycloak·MinIO·Caddy는 이 Run이 띄운 컨테이너다. 모두 같은 Run의 구현과 실행이며 독립 검증 전이다.

| # | 결함 | 수정 | 증거 | 커밋 |
|---|---|---|---|---|
| 1 | 실제 Keycloak 26.2.5 토큰에는 `nbf`가 없어 거부됨 | `nbf`를 선택 claim으로 바꿈. 있으면 여전히 강제하고(미래 값, 음수, null, 문자열은 거부) `iat` 필수·미래 `iat` 거부·900초 상한은 그대로다. 근거: RFC 7519 선택 claim, RFC 9068 필수 목록에 없음, `iat`가 사용 시작을 이미 묶음. runbook 갱신 | 실제 Keycloak 컨테이너 smoke(`scripts/smoke-keycloak.sh`, TLS와 HTTPS JWKS, `aud` 배열, 암호화 키가 섞인 JWKS): 수정 전 12개 중 2 FAIL(세션과 operation), 수정 뒤 12개 모두 PASS. `cargo test -p aip-auth` 15·2·11 passed | `3b0925f` |
| 2 | IdP smoke의 17번째 확인이 HTTP 200만 보고 본문 `WIRE.BAD_REQUEST`를 놓침 | 선언된 operation(`length`)을 호출해 본문 `ok:true`와 `output.length===3`을 검사하고, 토큰 없는 같은 호출이 본문에서 `WIRE.UNAUTHENTICATED`인지 검사 | `scripts/smoke-idp-proxy.sh`: `[SUCCESS] All 18 checks passed!` | `99e1052` |
| 3 | Python 코드젠이 `typing.Never`·`NotRequired`(3.11+)와 import 시 평가되는 `A \| B`·`X \| None`(3.10+)을 써 3.9/3.10에서 ImportError | `NoReturn`, `Union`, `Optional`, 버전 분기한 `NotRequired`(3.11 미만은 런타임 stand-in, 정적 검사는 `typing_extensions`). 소스 검사 시험 추가(수정 전 코드에서 실패) | `uv run --python 3.9`(3.9.6)·`3.10`(3.10.21)·`3.13`(3.13.15)에서 `python -m unittest product.tests.python_sdk_test` 각각 `Ran 23 tests ... OK`. 처음 두 시도는 3.9에서 실제로 실패해 `Union`·`Optional`까지 넓혔다 | `5b7f23c` |
| 4 | trace E2E가 손으로 만든 span이라 동어반복이고 job span에 부모가 없음 | 요청 span id를 요청 시작 때 고정하고 enqueue 때 trace와 함께 기존 `trace_id` text 컬럼에 `<trace>-<span>`으로 저장(스키마 변경 없음, trace만 적힌 기존 행은 그대로 읽힘). job span의 `parentSpanId`가 그 요청 span이다. trace id fallback이 span 크기(16자) id를 쓰던 것도 고침. 손으로 span을 내보내는 시험을 지우고 HTTP enqueue부터 서버가 내보낸 span을 읽는 시험 2개로 교체 | `cargo test --test otlp`(spike-v6): 6 passed. 새 시험 2개는 이전 코드에서 `parentSpanId` 없음으로 실패. spike-v6 전체 227 passed·0 failed, `aip-execution` 124 passed | `dc95dc9` |
| 5 | `cargo clippy --features poc-runtime --all-targets` 14개 타깃 실패 | 8곳(불필요한 borrow 3, `unwrap` 3, 미사용 변수 2)을 고쳐 `tools/verify.sh`의 clippy 단계를 `--all-targets`로 올림 | `cargo clippy -p aip-cli --features poc-runtime --all-targets --locked -- -D warnings` 통과, 고친 e2e 5개 모음 4+9+15+1+4 passed | `791367b` |
| 6 | `prototype/tools/*.py` 기본 DSN이 `host=localhost dbname=postgres` | 공통 가드(`dedicated_db.py`)가 명시적 host·고포트(5432 제외)·user·`aip_test_*` DB, `PG*` 변수 미설정을 요구. 기본값 없이 `--db-url` 또는 `AIP_TEST_DATABASE_URL`만 받음. 묶음 README 예시도 전용 DSN 변수로 변경 | `python3 -m unittest product.tests.dedicated_db_test` 5 OK(13개 거부 DSN 포함). 인자 없이 실행 시 `refusing PostgreSQL target: no DSN given`, 옛 기본값은 `the port must be explicit` | `a8b0d0b` |

### 확인하지 못했거나 남은 것

- 3의 3.11·3.12는 실행하지 않았다(3.9·3.10·3.13만). 3.9/3.10에서 `NotRequired` 키는 런타임에서 선택 여부를 구분하지 못한다(정적 검사에서만).
- 4의 중첩 job(job이 enqueue한 job)이 부모를 이어받는 경로는 코드로 연결했지만 시험하지 못했다. `report` job은 이 시험 구성에서 파일·이벤트 설정이 없어 실패했고, 확인한 것은 `count` job의 요청 span 부모다.
- 6의 `prototype/tools/smoke.py`는 가드를 통과한 뒤 TypeScript 컴파일에서 `OperationBinding`이 prototype SDK에 없다는 기존 오류로 실패한다. 이번 변경과 무관해 고치지 않았다.
- 1은 Keycloak만 확인했다. Auth0·Entra ID는 계정이 없어 미실행이다. 발급 토큰의 `aud` 문자열 형태와 배열 형태 중 배열만 실제 provider로 봤다.
- main의 `spike-v6-transport`는 `cargo clippy --all-targets -D warnings`가 `src/server.rs:724`의 `items after a test module`로 실패한다. 다른 트랙의 변경이라 건드리지 않았다.

## WBS 8.5 보존 정리와 S3 owner quota (2026-10-18, Claude 구현 / 독립 검증 대기)

DB 테스트는 이 Run이 띄운 PostgreSQL 17 컨테이너(`aip-claude-wbs8-pg-1018`, `127.0.0.1:32801`, DB `aip_test_wbs8_1018`)에서만 실행했고 공유 5432에는 접속하지 않았다. 객체 저장소 live 테스트는 이 Run이 만든 임시 CA의 TLS MinIO 컨테이너(`aip-claude-wbs8-minio`, `127.0.0.1:32802`, 이미지 `aip-minio-fixture:local`, 서버 `RELEASE.2025-09-06T17-38-46Z`)에서 실행했다. AWS S3와 GCS 실계정은 미실행이다.

### 구현한 것

| 항목 | 결과 | 위치 |
|---|---|---|
| S3 owner별 합계 quota | 서명한 PUT·multipart part 크기를 `aip_infra_<digest>.aip_object_ledger`에 예약하고 owner 합계(기본 10GiB·1,000개, `objects.max_total_bytes`·`max_objects`)에 넣어 검사한다. 넘으면 URL 없이 `WIRE.QUOTA_EXCEEDED`. 같은 객체 재서명은 이전 예약을 대체하고 owner마다 advisory lock으로 동시 예약이 합계를 넘지 못하게 한다 | `crates/aip-execution/src/object_ledger.rs`, `spikes/spike-v6-transport/src/object_transport.rs` |
| 만료 업로드 정리 | 서명 PUT은 발급 65분 뒤, multipart는 part 서명이 24시간 없으면 서버가 `HEAD`로 저장소를 확인한다. 객체가 있으면 실제 크기로 확정, multipart는 없으면 저장소에서 abort하고 예약을 푼다. 서명 PUT은 첫 부재에서 풀지 않고 24시간 보류한 뒤 한 번 더 확인한다(2026-10-18 후속, [릴리스 게이트](RELEASE-GATE.md)). 서명 DELETE는 저장소가 객체 없음을 확인할 때까지 quota를 유지한다. 모든 인스턴스가 60초마다 돌되 행별 임대와 `version` 비교로 재서명과 겹쳐도 새 예약을 지우지 않는다 | 같은 파일의 `reconcile_loop`, `S3Store::head_object` |
| multipart 보호 | 장부에 없는 upload id의 part 서명과 complete는 `WIRE.NOT_FOUND`. 완료 객체의 id와 지어낸 upload id로 abort를 호출해도 그 객체의 quota가 풀리지 않는다 | `ObjectLedger::abort_multipart` |
| outbox 정리 | relay가 넘긴 행·dispatch 기록 보존 기간을 설정으로 열었다(`retention.outbox_dispatched_days`, 기본 30). handler가 없는 topic의 행은 `retention.outbox_unhandled_days`를 정할 때만 지운다(기본 보존). handler가 있는 topic의 미전달 행은 지우지 않는다 | `spikes/spike-v6-transport/src/durable.rs`(`collect_expired`) |
| events 정리 | 개수 상한(stream당 1,000)에 더해 `retention.events_days`를 정하면 그 기간이 지난 이벤트를 지우고 replay 하한을 올려 옛 cursor는 `WIRE.CURSOR_EXPIRED`를 받는다. 10분마다 최대 1,000 stream | `crates/aip-execution/src/events.rs`(`expire_older_than`) |

기존 문서의 "`aip_outbox`는 아직 자동 정리하지 않으며"는 틀렸다. O-20(`b68fa7b`)이 handler가 있을 때 dispatch 30일 뒤 정리를 이미 넣었다. 이번 회차는 그 기간을 설정으로 열고 handler 없는 topic과 events를 더했다. `MANAGED-EXECUTION.md`와 `INFRASTRUCTURE.md`를 고쳤다.

### 실행 결과

| 명령 | 결과 |
|---|---|
| `cargo test -p aip-execution --test object_ledger --locked` | 17 passed. 장부 예약·대체·동시 8개 예약·만료 해소·stale 해소 거부·multipart 부담·abort 경계·서명 DELETE |
| `cargo test -p aip-execution --test events --locked` | 13 passed(신규 3: 나이 기준 정리, stream 수 상한, 잘못된 설정) |
| `cargo test --locked --test object_ledger_s3` (spike-v6, 실제 HTTP 서버와 PostgreSQL, 저장소 연결 없음) | 4 passed, 3 ignored |
| 같은 명령에 `-- --ignored`, MinIO 환경 변수(`scripts/test-minio-object-storage.sh`가 설정) | 3 passed. 서명 PUT 업로드 뒤 만료 확정, 미사용 5MiB 예약 해소, 서명 DELETE 미사용·사용 두 경우, multipart 두 part 완료와 유휴 upload의 저장소 abort(abort 뒤 part PUT이 404) |
| `cargo test --locked --lib retention_tests` (spike-v6) | 4 passed. 기본은 handler 없는 topic 보존, 설정하면 나이로 삭제, id 순서로 2,500행 삭제하고 젊은 40행에서 멈춤, dispatch 기간 설정 |
| `cargo test -p aip-service --lib` 신규 2개 | S3 quota 기본값·거부값, retention 기본값·범위 |
| `cargo test --workspace --locked --no-fail-fast` | 427 passed, 0 failed, 5 ignored |
| `cargo test --locked --no-fail-fast` (spike-v6) | 226 passed, 0 failed, 3 ignored(live MinIO) |
| `cargo fmt --all --check`, spike-v6 `cargo fmt --check` | 통과 |
| `cargo clippy --workspace --lib --bins --locked -- -D warnings`, 제품 crate와 spike-v6 `--all-targets` | 통과 |

변이 확인: 합계 검사 제거(3 실패), stale resolve의 version 비교 제거(1), abort가 아무 행이나 지움(1), events 정리가 replay 하한을 올리지 않음(1)이 모두 잡혔다. reconciler가 multipart abort를 하지 않는 변이는 live 테스트의 abort 뒤 PUT 확인(200 대 404)에서 잡혔다.

전체 실행 중 `aip-execution` `messages`의 `owner_checks_scope_claim_ack_and_dlq_and_expiry_cleanup_is_bounded`가 한 번 실패했다(TTL 25ms 시험으로 부하에 따라 흔들린다). 단독으로 3번 연속 통과했고 이번 변경은 messages 모듈에 닿지 않는다. 전체 재실행에서는 통과했다.

### 한계·미검증

- 장부가 생기기 전에 올린 객체는 quota에 세지 않는다. 장부가 필요한 객체를 소급하려면 bucket을 목록 조회해야 하는데 구현하지 않았다.
- (2026-10-18 후속으로 바뀜) 첫 확인 뒤에 끝나는 PUT은 이제 보류 후 확정된다. 두 번째 확인(약 25시간) 뒤에 끝나는 PUT은 여전히 세지 않는다. bucket 수명 주기 규칙이 필요하다. 상세는 [릴리스 게이트](RELEASE-GATE.md)의 "후속 증분".
- 업그레이드 중에 장부가 생기기 전에 시작한 multipart는 part 서명과 complete가 `WIRE.NOT_FOUND`다(재시작해야 한다).
- quota는 서명한 크기 기준이다. provider 비용이나 bucket 전체 용량을 대신하지 않는다.
- `aip_events.created_at`에 index가 없어 events 정리는 10분마다 테이블을 한 번 훑는다. 앱 migration 지문을 바꾸지 않으려는 선택이다. 매우 큰 events 테이블의 비용은 측정하지 않았다.
- GCS(`gcs_hmac`)는 같은 코드 경로이지만 실제 계정으로 실행하지 않았고 MinIO로만 검증했다. AWS S3의 `HEAD`·multipart abort 응답 차이는 미확인이다(MinIO는 존재하지 않는 upload의 abort에도 성공을 돌려줘 `NotFound` 경로는 live로 재현하지 못했다).
- 독립 검증 전이다. 완료가 아니라 "독립 검증 대기"다.

## WBS 8.11 8.9 후속: clippy 정리와 Kind::Io 시험 (2026-10-18, Claude 구현 / 독립 검증 대기)

DB 테스트는 이 Run이 띄운 PostgreSQL 17 컨테이너(`aip-claude-wbs8-pg-1018`, `127.0.0.1:32801`, DB `aip_test_wbs8_1018`)에서만 실행했고 공유 5432에는 접속하지 않았다.

- `commit_unknown_wire` 자체의 clippy는 이미 깨끗했다(`cargo clippy --all-targets --locked -- -D warnings`, spike-v6). 8.9 절에 남긴 사전 실패는 이 파일이 아니라 spike-v3·v4 테스트였고 셋을 고쳤다: v3 `general_update`의 불필요한 참조, v4 `cleanup_tests`의 `marker_processes`(Linux 테스트에서만 쓰여 비Linux에서 dead code라 `cfg(target_os = "linux")`), v4 `grant_tests`의 `len() != 0`. 동작은 바꾸지 않았다.
- `Kind::Io`: 호출자의 COMMIT 호출에서는 여전히 재현하지 못한다. 대신 접속 단계에서 가짜 서버가 시작 메시지를 읽고 RST로 닫게 해 실제 `Kind::Io` 오류(`error communicating with the server`)를 만들고 `connection_failed`가 이를 연결 오류로 판정하는지 고정했다(`io_error_during_startup_is_a_connection_failure`). Io 문구 판정을 지운 변이에서 이 테스트가 실패했다(12 passed, 1 failed).
- COMMIT 전 단절 시험은 8.9 회차에 이미 있다(`disconnect_while_beginning_the_transaction_is_a_safe_retry`, `..._after_update_...`, `..._after_idempotency_insert_...`, `commit_on_an_already_closed_connection_...`). 새로 더하지 않았다.

| 명령 | 결과 |
|---|---|
| `cargo clippy --all-targets --locked -- -D warnings` (spike-v3, v4, v6 각각) | 모두 통과 |
| `cargo test --locked --test commit_unknown_wire` (spike-v6) | 13 passed |
| `cargo test --locked --no-fail-fast` (spike-v3, spike-v4) | 모두 0 failed |

## Memoir Folder rename 실DB AIP 검증 (2026-10-17)

Infisical은 공식 공개판 `188cf58173019bf0fe59972ed1c7ef715bec979c`에서 프로젝트 권한이 normalized Membership→MembershipRole 관계를 거쳐 결정됨을 확인했다. 기본 Member의 Settings 편집도 `ee/` 코드에서만 확인할 수 있었고, [root LICENSE](https://github.com/Infisical/infisical/blob/188cf58173019bf0fe59972ed1c7ef715bec979c/LICENSE)는 MIT Expat, [`ee/LICENSE.md`](https://github.com/Infisical/infisical/blob/188cf58173019bf0fe59972ed1c7ef715bec979c/backend/src/ee/LICENSE.md)는 별도 상용 라이선스다. AIP 현재 정책 DSL로 그 관계 평가를 그대로 표현하려면 큰 계약 확장이 필요해 이 실험은 기각했다. Infisical source service/API/DB를 실행하거나 운영 서비스·secret에 접속한 검증은 없으며, Infisical 적용으로 계산하지 않는다.

대체 후보는 기록에 있던 `port-trials/memoir-notes`이며, 그 시험의 Note create/favorite/delete와 겹치지 않는 Folder rename 한 업무를 택했다. [기계적 source 비교·제외 범위](tests/fixtures/memoir-folder-rename/README.md)에 따라 Memoir committed source revision `26396070917ee065b988b0de296baf54f6266322`의 owner-scoped update와 UUID Folder model을 AIP resource/read/apply에 연결했다. 원본 저장소 worktree에는 36개 변경 파일이 있어 건드리거나 pull하지 않았다. 이 commit과 local `origin/main` ref는 같지만 원격은 fetch하지 않아 freshness는 확인되지 않았다.

전용 PostgreSQL DB마다 synthetic source-shaped `memoir_source`와 AIP-owned schema를 생성했다. 원본 서비스/API/DB는 시작하지 않았다. 따라서 결과는 **코드 기준 적용 / 실DB AIP 검증**이며 original E2E 동등성이나 Memoir 전체 포팅을 뜻하지 않는다.

| 실제 실행 | 결과 | 확인한 범위 |
|---|---|---|
| `CARGO_TARGET_DIR=/Users/winterholic/development/projects/aip/aip/target AIP_TSC=/Users/winterholic/development/projects/aip/aip/spikes/spike-0-ts/node_modules/.bin/tsc node --test product/tests/memoir-folder-rename.test.mjs` | 4 passed, 0 failed, 0 skipped | CLI init/migrate/provision/bind/serve, 생성 TS binding strict typecheck, SDK offline build/install. Owner가 UUID folder를 읽고 `name/sortOrder`를 `Research/3`→`Engineering/7`로 바꿈. 무관 actor는 read 결과 0행, apply `WIRE.MISSING_TARGET`; 거부 후 새 연결 조회와 실제 AIP DB 값은 계속 `Engineering/7`. |

합성 원본형 row는 변경 전후 `Research|3`으로 유지됐고 AIP-owned row만 `Engineering|7`이 됐다. 테스트 종료 뒤 전용 DB와 runtime role을 제거했다. 초기에 발생한 parser/fixture SQL/ref ID 오류는 fixture를 바로잡은 후 회귀가 통과했으며 AIP core gap은 발견되지 않아 코어 제품 코드는 수정하지 않았다. 원본 `@RequiresPlan` entitlement, 이름 공백 validation 경계, parent folder 흐름, source API/DB와 전체 Note model은 범위 밖이다.

## WBS A: 외부 IdP·TLS reverse proxy 운영 smoke (2026-10-17)

`conformance/idp-proxy/idp/server.mjs`는 저장소 자체의 경량 RSA/JWKS/JWT fixture이며, 실제 Keycloak·Entra·Google 연동이나 RFC 9068 전체 conformance 적합성은 미검증이다. Caddy TLS reverse proxy(자체 서명 SAN 인증서) 환경에서 `aip service` 운영 인증 및 프록시 헤더 처리를 검증했다.

| 실행 시점 및 환경 | 실행 명령 | 결과 | 확인한 범위 및 상태 |
|---|---|---|---|
| 원 작업 (`cbaa401`, 2026-10-17 02:56) | `bash scripts/smoke-idp-proxy.sh` (`node conformance/idp-proxy/smoke-check.mjs`) | 17 passed, 0 failed (`[SUCCESS] All 17 checks passed!`) | Caddy HTTPS 앞단 통신, Origin/CORS 허용·차단, `x-request-id` 보존 및 자동 생성, 루프백 직접 연결(헤더 위조) 차단 정책, OIDC IdP HTTPS JWKS 검증 기반 정상 토큰 인증 및 읽기, 토큰 만료(`WIRE.TOKEN_EXPIRED`), 잘못된 aud/iss(`WIRE.UNAUTHENTICATED`), 위조 서명/지원하지 않는 typ 거부, 미등록 sub 거부 |
| 독립 교차 검증 (`cbaa401`, 2026-10-17 03:07, 별도 DB `aip_flash_verify_20261017`) 및 후속 진단 | `AIP_TEST_DATABASE='...' bash scripts/smoke-idp-proxy.sh` | exit 1 (failed) | IdP health/discovery/JWKS 3 assertion 통과 후 `aip service` 시작 단계에서 `SERVER_CONFIG`로 중단됨. 후속 진단에서 실패 stage는 `worker_runtime_probe`로 확인됨: macOS worker 격리(`isolation=auto`) 아래 `node --version` 프로브가 exit status 71 (`stderr_class=permission_or_sandbox_denial`)로 종료. 이는 nested Mac Run 실행 환경의 제약이며, 어떤 syscall/프로파일 규칙이 거부됐는지는 특정되지 않음(정확한 원인은 불명). 이를 production 부재로 일반화하지 않으며, 프록시·CORS·request ID·JWT 검증 등 남은 14개 assertion은 여전히 미실행 상태임. |

- 사설/자체 서명 CA 환경 지원을 위해 `crates/aip-auth`가 `SSL_CERT_FILE` 우선, `AIP_AUTH_CA_PEM` fallback으로 환경 변수를 인식해 webpki 루트 인증서 풀에 추가하도록 지원한다. 파일 읽기/파싱 실패 시 `.ok()`로 조용히 무시하고 기본 루트로 계속하는 기존 동작을 유지한다. 두 회귀 테스트는 실제 local TLS/JWKS 요청에서 `new_explained`와 공유하는 lookup helper의 CA 로딩 경로, CA 부재·잘못된 CA 경로·hostname mismatch 거부를 검증했다. 공개 `new_explained` wrapper의 환경 조회 위임은 코드 경로로 확인했다.

## WBS B: 실제 Kafka·RabbitMQ 및 MinIO object storage smoke (2026-10-17)

실제 Kafka(v4.1.0 KRaft), RabbitMQ(v4.2), MinIO(S3 호환 HTTPS) 컨테이너를 구동하고 broker 발행·소비·재시도·redrive 및 object storage 업로드·다운로드·권한 경계를 실제 통신으로 검증했다. 원 작업과 독립 교차 검증 모두 4개 테스트 통과를 확인했다.

| 실제 실행 명령 (`scripts/smoke-brokers-storage.sh` 원문 일치) | 결과 | 확인한 테스트 및 범위 |
|---|---|---|
| `cargo test --locked -p aip-execution --features external-brokers --test brokers_live -- --ignored --nocapture` | 명령군 2 passed, 0 failed | `real_kafka_confirm_and_import_round_trip` (Kafka 발행·소비 왕복 및 dead-letter redrive), `real_rabbitmq_confirm_and_import_round_trip` (RabbitMQ 발행 확인 및 DLX quarantine 재시도) |
| `cargo test --locked -p aip-service --features external-brokers broker_worker_recovers -- --ignored --nocapture` | 명령군 1 passed, 0 failed (31 filtered out) | `brokers::tests::broker_worker_recovers_from_database_disconnect_without_losing_delivery_budget` (DB 연결 단절 후 broker 복구 복원력, audit_cli/jobs_cli 등 0개 타깃 제외) |
| `cargo test -p aip-execution --test cloud_storage minio_https_actual_object_and_multipart_contract --locked -- --ignored --nocapture` | 명령군 1 passed, 0 failed (3 filtered out) | `minio_https_actual_object_and_multipart_contract` (MinIO TLS S3 호환 다중 파트 업로드·다운로드 및 읽기 권한 분리) |
| **전체 스크립트 실행**: `bash scripts/smoke-brokers-storage.sh` | **총계 4 passed, 0 failed** | 상기 세 명령군을 순차 실행하여 4개 live 통합 검사 모두 통과 (필터된 0개 타깃은 집계에서 제외) |

- `crates/aip-execution/tests/brokers_live.rs` 및 `crates/aip-service/src/brokers.rs` 내 하드코딩되어 있던 `user=runner` 연결 문자열을 `AIP_TEST_DATABASE` 및 `AIP_TEST_DATABASE_URL` 환경 변수를 우선 참조하도록 수정했다.
- MinIO 도커 환경은 `conformance/brokers-storage/minio/Dockerfile`에서 고정 릴리스 태그(`RELEASE.2025-09-06T17-38-46Z`) 및 SAN 인증서 진입점을 구성했다. 독립 검증에서는 기존 로컬 이미지(`aip-minio-fixture:local`)가 재사용되어 source build/download 경로는 미실행되었다. '공식 이미지 배포 중단'은 확인된 근거가 없으므로 단정에서 제외한다.

# 제품 통합 검증 (2026-10-06)

## MySQL 8.4 읽기·쓰기·작업 저장소 실행 검증 (2026-10-18, Claude 구현 / 독립 검증 대기)

범위는 [MySQL adapter 계약](../plan-docs/porting/MYSQL-ADAPTER-CONTRACT.md)의 "읽기 경로", "쓰기 경로", "작업 저장소" 절이다. 실행 환경은 macOS arm64, Docker의 `mysql:8.4`(8.4.11)와 `mariadb:11.4`(11.4.13)를 `scripts/mysql-live-fixture.sh`로 loopback·tmpfs에 띄운 격리 컨테이너, 격리 PostgreSQL 17이다. 업무 DB·`.env`는 쓰지 않았다. 모든 라이브 시험은 `#[ignore]`이고 `-- --ignored` 또는 `--include-ignored`로 실행하며, 대상 URL이 없으면 통과가 아니라 실패한다.

| 명령(요약) | 결과 |
|---|---|
| `cargo test -p aip-migrate --features mysql -- --include-ignored mysql --skip every_semantic_ddl` (MySQL) | 20 passed(드라이버 라이브 13 포함) |
| 같은 명령 (MariaDB URL) | 20 passed |
| `cargo test -p aip-migrate --features mysql --test mysql_fence_verification -- --include-ignored` (MySQL+MariaDB URL 동시) | 8 passed. 두 엔진 모두 |
| `cargo test -p aip-migrate --features mysql --test mysql_drift_matrix -- --ignored` | 1 passed: 35개 DDL 변경 중 의미 있는 32개가 지문을 바꾸고 트리거·view·comment 3개가 바꾸지 않음(알려진 한계로 고정) |
| `cargo test -p aip-migrate --features mysql --test mysql_jobs -- --ignored` | 3 passed: job 상태 기계, 40 run×8 worker 동시 claim, outbox 릴레이 |
| `cargo test -p aip-migrate --features mysql --test mysql_capability_gate -- --ignored` | 2 passed: MySQL 8.4 통과, MariaDB 11.4 거부(LATERAL, FOR SHARE OF) |
| `cargo test -p aip-service --features mysql --test mysql_read_http --test mysql_write_http -- --ignored --test-threads 1` | 읽기 1 + 쓰기 5 passed |
| `cargo test -p aip-cli --features mysql --test mysql_adoption_flow -- --ignored` | 1 passed: 실제 `aip` 바이너리로 plan/apply/bind/serve/JWT 읽기·쓰기/drift |
| `cargo test -p aip-migrate -p aip-service` (기본 feature) | 121 passed, 1 ignored |
| `cd spikes/spike-v2-read && cargo test` (PostgreSQL) | 109 passed. 방언 도입 뒤에도 PostgreSQL SQL이 같다 |
| `cd spikes/spike-v6-transport && cargo test --no-fail-fast` | 65 target 통과, 5 target 실패(`v12_typed`·`v13_idwire`·`v14_apply`·`v15_values`·`v16_bundle`). 다섯 모두 `tsc` 부재(`Os { code: 2, NotFound }`)로 기존 사전 실패와 같다 |
| `cargo clippy -p aip-migrate -p aip-service -p aip-cli --features mysql,aip-cli/mysql --all-targets -- -D warnings` | 경고 0 |

정정(2026-10-18 후속): 위 표의 `mysql_adoption_flow`·`mysql_read_http`·`mysql_write_http`·`mysql_jobs` 결과는 서버 능력 게이트(`check_server_profile`)를 넣기 전 실행이다. 게이트 뒤에는 `mysql_adoption_flow`가 실패하는 상태로 푸시됐다. 시험 런타임 계정에 `CREATE TEMPORARY TABLES`가 없어 probe가 서버를 "InnoDB·SKIP LOCKED·CHECK 없음"으로 오판했고, `mysql_jobs`는 두 시험이 같은 schema를 병렬로 지워 서로 깨뜨렸다. 고친 뒤 같은 명령을 다시 실행했다: `mysql_adoption_flow` 1 passed, `mysql_read_http` 1 + `mysql_write_http` 5 passed, `mysql_capability_gate` 3 passed(권한 부족 계정이 서버 결함이 아니라 권한 부족으로 거부되는 시험 추가), `mysql_jobs` 3 passed(직렬화), `mysql_fence_verification` 8 passed, `mysql_drift_matrix` 1 passed. 런타임 계정은 업무 schema에 `CREATE TEMPORARY TABLES`가 필요하다(세션 안에서만 보이는 probe 표 하나를 만든다).

2026-10-18 후속 실행(같은 컨테이너, 같은 명령 형식): `AIP_TEST_JOBS_URL`을 MariaDB로 두고 `cargo test -p aip-migrate --features mysql --test mysql_jobs -- --include-ignored`를 실행하면 3 passed(상태 기계·8 worker 동시 claim·outbox 릴레이)이고, MySQL 8.4에서도 3 passed다. MariaDB에서 claim의 `SKIP LOCKED`를 빼면 `mysql_concurrent_claims_never_hand_a_run_to_two_workers`가 실패한다. `mysql_capability_gate`는 4 passed(job 저장소 프로파일은 MariaDB를 통과시키고 서비스 프로파일은 계속 거부). `mysql_write_http`는 6 passed(`where` target, `sameScope`, 단일 `update` 효과 추가). 이 항목의 "MariaDB는 job 저장소를 실행하지 않았다"는 서술은 이로써 낡았다.

요청당 catalog 비용(측정, `product/tests/fixtures/facereview-mysql/reference/big.py`): 펜스는 캐시 없이 요청마다 바인딩된 모든 표의 catalog를 읽는다. 표 하나에 약 8.8ms(중앙값), 10개 13.5ms, 30개 22.5ms, 60개 35.7ms, 120개 62.6ms로 표당 약 0.45ms씩 선형으로 늘었다(p95는 중앙값보다 3~4ms 위). FaceReview 9개 표는 읽기 요청당 약 13.7ms다. 조건은 debug 빌드, 같은 호스트의 loopback MySQL 8.4, 표당 열 10개, 요청 100회 단일 실행이다. 운영 환경의 지연과 같다고 주장하지 않으며, 표 수가 수십 개를 넘는 배포에서는 캐시 없는 fail-closed 설계의 비용이 눈에 띈다는 것이 확인된 내용이다.

환경 한계: `aip-cli`의 `least_privilege`는 비밀번호 인증 PostgreSQL에서는 역할 접속이 실패하고, trust 인증 PostgreSQL 컨테이너에서는 통과한다(이번 변경과 무관). 시험 DB 비밀번호와 계정은 모두 이 시험을 위해 만든 일회용이다.

변이 확인(보호 장치를 하나씩 끄고 시험이 실패하는지 확인): 용량 잠금, 키 잠금, 매칭 행 수 확인, SERIALIZABLE(`mysql_write_http`), 후보 행 잠금, lease epoch 검사, epoch 증가(`mysql_jobs`)는 각각 시험을 실패시켰다. 쓰기 판정의 `FOR UPDATE`만은 끄면 통과하며, 뒤의 `UPDATE`가 같은 행 잠금과 `from` 재확인을 해서 결과가 같기 때문이다.

미실행·미검증: MariaDB의 읽기·쓰기 경로(능력 부족으로 거부), HTTP `/jobs/*`·job 실행, `compose`, 확장 쓰기, OIDC 외부 IdP로 UUID actor를 묶는 CLI 경로의 IdP 연동(UUID actor 바인딩 자체는 FaceReview 시험이 실행), 다른 OS. 이 문장은 이후 실행으로 일부 갱신됐다(아래 후속 단락).

## MySQL catalog 변환 계층과 fail-closed 경계 (2026-10-17)

AIP external introspection은 현재 PostgreSQL 전용이다. MySQL URL 거부 guard는 유지하면서, `aip-migrate::mysql_catalog::normalize_table`에 driver 비의존 INFORMATION_SCHEMA 행→중립 manifest 변환을 추가했다. 원본 `facereview-inventory.json`의 11개 SQLAlchemy 모델과 PostgreSQL catalog/외부 채택 경로를 대조해 컬럼 타입·PK/인덱스 순서의 작은 범위로 제한했다. 아래 catalog 행들은 synthetic 회귀 fixture이며 실제 DB 관측이 아니다.

| 실행 | 결과 | 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/aip-target cargo test -p aip-migrate --test mysql_catalog --test mysql_adapter --locked --offline` | 9 passed, 0 failed | synthetic PK/index/FK 순서·unsigned·enum·tinyint(1) alias·nullable/default/generated·NULL default 모호성·varchar 길이 진단·잘못된 metadata 거부, 그리고 기존 URL fail-closed/credential 숨김 |
| `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/aip-target cargo test -p aip-migrate --test mysql_catalog --test mysql_adapter --test migrate --locked --offline` | 38 passed, 0 failed | catalog 8, 기존 MySQL URL guard 1, migrate regression 29 |
| `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/aip-target cargo test -p aip-migrate --lib --locked --offline` | 1 passed, 0 failed | migration library 회귀 |
| `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/aip-target cargo clippy -p aip-migrate --all-targets --locked --offline -- -D warnings` | exit 0 | 변경 crate 전체 target lint |
| `cargo fmt --all --check` | exit 0 | workspace Rust formatting |
| `python3 tools/doccheck.py` | 14 aip blocks checked, 0 findings | 제품 문서 선언 검사 |
| `python3 tools/check_plan_docs.py` | 172 markdown files checked, 0 errors | plan-docs 검사 및 negative controls |
| `git diff --check` | exit 0 | 공백 오류 없음 |
| `CARGO_NET_OFFLINE=true cargo build -p aip-cli --locked --offline && node --test product/tests/uuid-external.test.mjs` | build exit 0; PostgreSQL UUID owned/external 통합 1 passed, 0 failed | 기존 PG introspect/adopt, runtime DML role, TS/Python SDK, 두 wire, drift 거부. 새 MySQL converter를 호출하지 않음 |

`mysql_catalog` 테스트 입력은 테스트 코드에서 만든 synthetic catalog rows다. DB에서 관측한 값으로 표시하거나 취급하지 않는다. MySQL 8.4는 `INFORMATION_SCHEMA.COLUMNS.COLUMN_DEFAULT = NULL`을 명시적 `DEFAULT NULL`과 DEFAULT 절 부재 양쪽에 반환하므로, manifest는 이를 `NullOrAbsent`로 남기고 `SHOW CREATE TABLE` 등 추가 판별을 요구하는 diagnostic을 붙인다 ([공식 MySQL 8.4 문서](https://dev.mysql.com/doc/refman/8.4/en/information-schema-columns-table.html)). converter는 typed columns/index rows와 FK 입력을 받으며, FK manifest는 composite 순서·source/target·constraint identity·actions를 보존한다. 잘못된 순서/누락/미지원 action은 거부하고 `CHAR`/`VARCHAR` 길이 축약은 diagnostic으로 남긴다. 원본 11개 RDB 모델 선언은 [FK 기준 manifest](../plan-docs/porting/facereview-fk-manifest.json)에 정리했다. 기존 댓글 PostgreSQL fixture는 Comment의 두 참조만 보여 주며 11개 전체의 catalog 관측치나 동등성 근거가 아니다. 진단 항목은 소비자가 검토해야 하며 unsupported 타입은 manifest에 명시한다.

이번 후속 변경은 중복/누락 ordinal·없는 index column·혼합 `NON_UNIQUE`·`VARCHAR` 길이 진단 음성 회귀를 추가했다. 수정 전 duplicate ordinal, missing index column, mixed uniqueness, missing length diagnostic은 각각 원래 오류 또는 손실을 재현했고 수정 뒤 통과했다. composite FK 순서/분리, schema/table/column target, action 보존과 잘못된 순서·누락·알 수 없는/`SET DEFAULT` action 거부도 synthetic 입력으로 통과했다.

driver 추가, MySQL 실 DB 연결·catalog/read/write, MariaDB 실행은 미실행이다. adapter는 미지원 상태다. 이전 `cargo info mysql_async` 단계는 crate 다운로드 뒤 nested worker sandbox에서 Cargo cache open이 `Operation not permitted`로 막혔다. 이는 해당 실행 환경의 cache 접근만 말하며 production의 MySQL 가용성 원인으로 일반화하지 않는다. cache 경로 변경·직접 다운로드·브라우저/권한 우회는 하지 않았다. 다음 단계는 허용 의존성 공급이 가능한 실행 환경에서 driver와 read-only query를 연결하고 같은 manifest 회귀를 실제 catalog row로 대조하는 것이다.

이번 catalog/FK 구현 source revision: `28bb463f4bb73f461ff813ec8da3f57ad726e165` (코드·테스트·원본 FK manifest 커밋).

운영 인증·일반 마이그레이션·root 제품 통합을 사용자 요청 범위로 구현했다. AIP의 caller read/direct apply와 서버의 최종 권한, 같은 정의·같은 SDK·Node/Python 확장을 유지한다. 앱별 endpoint나 별도 실행 엔진을 추가하지 않았다.

## 페이스리뷰 원본 Socket.IO 부분 기준선 (2026-10-17)

고정 원본 backend `7948b7e721837fdb73c427cb54e12cad4ec2d5af`를 기존 격리 seed에서 실행해 허용된 `http://localhost:5173` Origin으로 Engine.IO polling handshake를 열었다. 실제 빈 `watch_frame` 입력은 `Missing required fields`로 거부됐고 client transport 종료를 확인했다. 결과는 [기계 판독 보고서](../plan-docs/porting/facereview-socket-baseline.json), 재현 gate는 `tools/facereview_socket_baseline.py`와 `tools/test_facereview_socket.py`에 있다.

| 실행 | 결과 | 범위 |
|---|---|---|
| `/tmp/aip-facereview-baseline-venv/bin/python -m unittest discover -s tools -p 'test_facereview_socket.py'` | 4 passed, 0 failed | 고정 source/Origin/transport/응답·부분 범위, scope 과장 및 민감 필드 거부 |
| `/tmp/aip-facereview-baseline-venv/bin/python tools/facereview_socket_baseline.py --source /tmp/aip-face-comment-ui/backend --inventory plan-docs/porting/facereview-inventory.json --output /tmp/aip-face-comment-socket-r2.json` | `ok=true`, connect pass, transport closed, `Missing required fields` | 실제 원본 polling handshake와 빈 입력 거부 |

격리 재실행 서버 로그는 client transport 종료 뒤 `handle_disconnect()`의 필수 `message` 인자가 없어 `TypeError`가 난 것을 보였다. 원본 callback은 실패했으며 이 원본 결함은 AIP 갭으로 분류하지 않았다. WebSocket transport는 Python `websocket-client` 부재로 미실행이고, 프레임·ML·Mongo timeline·worker/provider·AIP 대조는 미실행이다. 이 단위는 AIP 본체 변경이나 제품 갭 발견으로 계산하지 않는다. 상세 환경은 [페이스리뷰 포팅 추적](../plan-docs/porting/README.md)을 따른다.

## 내부 metrics·OTLP exporter·durable job trace 연결 검증 (2026-10-17)

WBS 항목인 "내부 metrics·OTLP exporter·durable job trace 연결"의 3개 범위를 구현하고 검증했다.

| 실제 실행 | 결과 | 확인한 범위 |
|---|---|---|
| `cargo test --test otlp --manifest-path spikes/spike-v6-transport/Cargo.toml --target-dir target` | 2 passed, 0 failed | mock in-process OTLP/HTTP 수신기(/v1/traces, /v1/metrics)를 통한 trace/span 수신, HTTP 요청 span과 durable job run span의 동일 trace_id 연계, duration histogram 수신, 민감값(인증 토큰·payload) 배제 |
| `cargo test -p aip-execution` | 80 passed, 0 failed | `aip_jobs.trace_id` 스키마 바인딩·보존(18 tests), files(5), integrations(10), messages(15), objects(11), rag(16), rate_limit(4), trace_context(4) |
| `cargo test -p aip-service` | 33 passed, 0 failed | 제품 설정의 `otlp` 파싱 및 validation, unit tests(31), audit_cli(1), jobs_cli(1) |
| `cargo fmt -- --check && cargo fmt --manifest-path spikes/spike-v6-transport/Cargo.toml -- --check` | exit 0 | workspace 및 transport spike 코드 포맷 검사 |
| `cargo clippy --workspace --lib --bins --locked -- -D warnings` | exit 0 | workspace 라이브러리/바이너리 clippy 검사 |
| `cargo clippy -p aip-auth -p aip-migrate -p aip-service --all-targets --locked -- -D warnings` | exit 0 | 제품 서비스 크레이트 전체 타깃 clippy 검사 |
| `cargo clippy --manifest-path spikes/spike-v6-transport/Cargo.toml --all-targets --locked -- -D warnings` | exit 0 | transport spike 전체 타깃 clippy 검사 |
| `cargo deny --manifest-path Cargo.toml check && cargo deny --manifest-path spikes/spike-v6-transport/Cargo.toml check` | exit 0 | 라이선스, 금지 crate, advisories, sources 보안 검증 |

미검증:
- 실제 remote OTLP Collector 실연결은 미실행. 별도 격리 검증에서는 로컬 Collector 0.120.1가 실제 제품 HTTP span과 `export_job_span` wire를 수신·파싱해 HTTP 200→`Ok`, 405→`Error`, job `SUCCEEDED`→`Ok`, `FAILED`/`CANCELLED`→`Error`, `RETRY`→`Unset`을 확인했다. Job 검증은 synthetic Lease와 제품 emitter를 사용했으며 PostgreSQL enqueue/claim 또는 worker lifecycle은 포함하지 않는다.

## 기존 durable job schema의 trace_id upgrade 후속 (2026-10-17)

출시 차단 결함: `4e8e2126a91bd15e21afd870fe5373c0ccba9adb` 이전에 생성한 `aip_jobs`에는 `trace_id`가 없다. 기존 metadata의 DDL digest가 현행 생성 DDL과 다를 때 planner가 `verified_marker()`에서 거부해 upgrade plan을 만들지 못했고, 그 상태의 Store enqueue/claim은 PostgreSQL `42703`을 반환했다. 이 원인 경로는 기존 재현의 실행 결과에 코드 경로 검토를 결합한 것이다.

수정은 이전 job DDL digest를 정확히 재구성해 해당 marker만 별도 인식한다. facts/generator version·journal·실제 catalog digest 검사는 유지하고, 실제 `trace_id`가 이미 있거나 다른 drift·미지 digest가 있으면 계속 거부한다. 허용된 schema는 plan에 nullable 컬럼 추가를 기록하고 기존 advisory fence·transaction·rollback·target verification·metadata/journal commit을 거친다. 범용 migration version 체계는 추가하지 않았다.

| 실행 | 결과 | 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=<Run temp> cargo build --locked --offline -p aip-cli` | exit 0 | 제품 `aip service` CLI 빌드 |
| `AIP_TRACE_UPGRADE_DATABASE_URL=<Run-owned PostgreSQL> AIP_SERVICE_CLI=<built aip> CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=<Run temp> cargo test --locked --offline -p aip-migrate --lib trace_id_upgrade_tests::explicit_parent_job_schema_upgrade_adds_trace_id_and_rejects_drift -- --ignored --exact --nocapture` | 1 passed, 0 failed | test가 실제 `aip service init` 및 `aip service migrate` plan/apply/repeated apply CLI를 호출. 기존 NULL job claim·retry, 신규 enqueue/claim·idempotency replay·trace retry 보존, actor/principal/idempotency rows와 facts/wire metadata 보존, unknown digest·추가 drift·이전 digest와 실제 컬럼 불일치 거부 |

위 표와 이 문단은 구현 worker의 최초 자체 검증 기록이다. 테스트는 별도 Run 소유 PostgreSQL 컨테이너의 임의 loopback 포트와 전용 DB를 사용했다. 부모 job 구조는 독립 재현에 기록된 이전 DDL(현재 생성물에서 `trace_id`만 제거)과 그 정확한 DDL/catalog marker로 구성했다. SQL을 직접 실행해 컬럼을 성공시킨 결과는 PASS로 세지 않았고, SQL 조작은 과거 schema 재현 fixture 준비에만 썼다. 최초 자체 회귀에서는 적용 후 실패를 강제한 rollback 및 제한된 DB role 권한 음성 경로를 실행하지 않았다. 그 실행 시점에는 apply의 advisory lock·단일 transaction·목표 검증·metadata/journal commit 경로는 코드 경로 확인에 그쳤다.

이후 고정 source SHA `9bc78de3654c163e29f3cb369ec21ff496499e85`를 별도 worktree와 별도 Run 소유 PostgreSQL에서 독립 검증했다. **이 SHA의 DB upgrade 범위 `verification_outcome=passed`**다. CLI `init` → `migrate plan` → `apply` → repeated `apply`를 통해 기존 NULL trace job과 신규 enqueue/claim/retry/replay, actor/principal/idempotency/facts/wire 보존, drift·미지 digest 거부를 확인했다. DDL 실패 주입은 제품 apply를 `MIGRATION_FAILED`로 끝냈고 컬럼 미추가·metadata/catalog 불변·journal 1→1 rollback을 확인했다. 제한 role은 `DB_APPLY`의 `journal 제약 정리 실패` 단계에서 거부됐으므로 ALTER 권한 단계까지 도달한 검사로 확대하지 않는다.

원시 독립 로그에는 최종 PASS 전의 두 번의 관측 helper 실패도 남아 있다. 첫 시도는 제한 role 정리 중 종속 객체 때문에 helper가 종료됐고, 다음 시도는 rollback 구조 비교식의 자기 비교 오류로 실패했다. helper를 바로잡고 원복한 최종 반복은 exact regression `1 passed; 0 failed`였고, 위의 rollback 및 제한 role 관측을 출력했다. 이전 코드의 원래 결함은 commit `4e8e212` 기준 planner `WIRE.SCHEMA_MISMATCH`와 Store `42703`이며, 수정 후 최종 독립 PASS와 시점을 구분한다.

OTLP 회귀 4개는 이 candidate에서 실행했다. Collector 실연결은 새로 실행하지 않았다. `661024d6d23629b2eb276105eaba22144f0ff401`와 비교한 `durable.rs`, `lib.rs`, `otlp.rs`, `trace.rs`, `tests/otlp.rs` 다섯 파일이 모두 byte-identical이므로 이전 Collector 실행 근거만 재사용한다. 실제 worker E2E는 미실행이고, 제한된 worker runtime probe의 기존 sandbox denial을 넘어 확인하지 않았다. plan-docs의 `D-development-experience.md` 관련 기존 broken links 12개는 별도 상태로 유지하며 이번 문서/DB 검사 통과로 바꾸지 않는다. 이 판정은 DB upgrade 한정이다. 전체 WBS와 AIP 출시 완료는 미완이고, production 배포도 하지 않았다.

## 페이스리뷰 댓글 적용 시험 (2026-10-15)

사용자가 AIP 미완료 작업을 이어가며 페이스리뷰 등 개인 프로젝트·공개 저장소로 시험하는 방식을 허용했다. [페이스리뷰 댓글 slice](tests/fixtures/facereview-comments/README.md)를 먼저 선택했다. 원본 `new-facereview` HEAD `7948b7e721837fdb73c427cb54e12cad4ec2d5af`의 모델·서비스·입력 schema를 읽고 PostgreSQL 복사 구조를 만들었다. 원본 MySQL·MongoDB와 운영 데이터를 수정하지 않았다.

| 실제 실행 | 결과 | 범위 |
|---|---|---|
| `node --test product/tests/facereview-comments.test.mjs product/tests/general-update.test.mjs product/tests/uuid-external.test.mjs` | 22 passed, 0 failed, 0 skipped | 페이스리뷰 19개 하위 시나리오와 상위 test, 기존 Memoir 일반 수정, UUID owned/external·두 wire |
| `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --locked --target-dir target` | 33 passed, 0 failed, 0 ignored | 쓰기 엔진 전체 unit/integration regression. 새 scalar literal test 2개 포함 |
| `cargo clippy --manifest-path spikes/spike-v3-write/Cargo.toml --lib --locked --target-dir target -- -D warnings` | exit 0 | 변경 엔진 library lint. 기존 needless borrow 한 곳도 정리 |
| `cargo build -p aip-cli --locked` | exit 0 | 위 HTTP/SDK 시험 전에 최신 CLI 재빌드 |
| `cargo fmt --manifest-path spikes/spike-v3-write/Cargo.toml --check`, `python3 tools/doccheck.py` | exit 0, 14 aip blocks checked, 0 findings | 변경 crate 형식과 문서의 제품 선언 블록 |

외부 PG의 `Text(1..500)` 생성에 빈 content가 저장되는 실패를 실제 HTTP로 재현했다. 신규 unit test에서도 범위 검증 누락과 Time/Url 생성 미지원으로 2개가 실패했다. 생성·patch·expect를 같은 scalar/range parser에 연결한 뒤 위 결과를 얻었다. 영상이 나중에 삭제되면 일반 댓글 조회에서 남는 앱 정책 누락도 음성 시나리오로 재현하고 선언에 영상 가시성 조건을 추가했다.

환경은 macOS arm64·로컬 PostgreSQL 17.11·메모리 생성 JWT key·설치 TS SDK다. Python은 생성 helper와 로컬 SDK package 경로를 사용했다. 테스트는 새 DB와 자체 runtime 역할만 만들고 종료 시 정리한다. 원본 MySQL에 직접 붙이기, 원본 frontend/API 호환, 관리자·좋아요/북마크·감정 분석·외부 provider·worker 격리·운영 배포는 검증하지 않았다. `tools/verify.sh`에 새 시험을 연결했지만 이번 회차에 전체 core script를 실행한 것은 아니다. 자세한 적용 차이와 다음 우선순위는 slice README를 따른다.

## Linux 격리 worker/job smoke (2026-10-14)

Ttakkari 안의 macOS에서는 `sandbox-exec`가 `Operation not permitted`로 차단되어 worker가 시작되지 않았다. 이를 대신해 `.github/workflows/worker-jobs-linux.yml`을 추가하고 GitHub Actions Ubuntu 24.04에서 기본 격리(`auto` → Bubblewrap)가 실제 적용되는 경로를 실행했다. 새 workflow는 별도 PostgreSQL 17 service를 사용한다.

| 실행 | 결과 | 확인한 범위 |
|---|---|---|
| GitHub Actions [Linux worker jobs #38511151991](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38511151991), commit `1d5a666` | success | Ubuntu 24.04, PostgreSQL 17, Bubblewrap 경계 14 passed; 격리 경계의 파일·쓰기·네트워크 차단 테스트 포함 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test job_retry --test triggers --test outbox` (workflow step) | 6 passed, 0 failed | Node/Python의 job 성공·`retryLater(afterMs)` 후 재실행, cron schedule, HMAC 서명 webhook, `notify` outbox 행에서 durable job 실행 |
| `node --test product/tests/service-smoke.test.mjs` (workflow step) | 1 passed, 0 failed | 설치 SDK와 제품 CLI의 실제 PostgreSQL smoke; Node/Python worker의 job 성공을 두 Id wire에서 확인 |

위 통합 테스트들은 각각 Node와 Python worker를 직접 순회한다. webhook 검사는 서명된 inbound webhook의 job 수락·실행·중복 전달, cron은 서비스 주체 job 실행, outbox는 commit된 notify 전이 뒤 정확히 하나의 성공 job을 확인했다. GitHub Actions의 후속 push run #38511151991은 전체 workflow success다. 앞서 수동 dispatch run #38510801999도 세 검증 step을 모두 통과했지만, 동시 main push가 마지막 정리 단계 중 들어와 workflow 상태는 cancelled로 기록됐다. 결과 기준은 전체가 success인 #38511151991이다.

로컬 컨테이너 대체 경로는 사용하지 못했다: Docker·Colima·Podman 실행 파일이 없었고, 설치된 OrbStack CLI도 Ttakkari sandbox에서 `~/.orbstack` 초기화가 `Operation not permitted`로 거부됐다.

macOS의 Seatbelt 정책 자체는 Linux 결과로 검증되지 않았다. 일반 macOS Terminal에서 로컬 PostgreSQL과 SDK test dependency가 준비된 상태로 다음 명령을 한 번 실행하면 실제 Node/Python worker와 job smoke를 확인할 수 있다. 이 스크립트는 임시 HOME/npm cache와 새 임시 schema를 사용한다.

```sh
cd /Users/winterholic/development/projects/aip/aip
npm ci --prefix spikes/spike-0-ts --ignore-scripts --no-audit --no-fund # node_modules/.bin/tsc가 없을 때만
bash scripts/verify-macos-worker-smoke.sh
```

기대 출력은 `✔ the installed SDK completes authenticated reads, cache, writes and operations for both Id wires`, `ℹ pass 1`, `ℹ fail 0`이다. Ttakkari 안에서는 이 macOS 검사를 실행하지 못했으며, Terminal에서 위 스크립트를 실행하기 전까지 macOS sandbox 정책은 미검증이다.

## 계약 응답·내보내기 보완 (2026-10-14)

CD-13·CD-2·CD-7을 반영한 회차다. `/read`·`/apply`·`/status`의 계약 헤더 누락과 지문 불일치를 구분하고, JSON 실패 본문과 SDK에 요청 ID·재시도 지침을 연결했다. 미확정 쓰기 오류에도 마지막 서버 지침이 남으며 기존 멱등 키 복구를 유지한다. `aip contract`는 compact가 기본이고 `--pretty`가 같은 JSON 의미를 보존한다. 최상위 오류 표의 참조를 확장된 계약과 비교해 코드·reason 손실이 없는지 확인했다.

| 실행 | 결과 | 범위 |
|---|---|---|
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test …` | 선택한 14개 대상의 26개 테스트 통과 | `error_body`, `v13_headers`, `wire_codes`, `v13_idwire`, `bundle_http`, `v12_typed`, `v14_apply`, `v8_auth`, `response_connection`, `v10_session`, `v9_lifetime`, `configured_server`, `v6_e2e`, `absence_serializable_http`. 실제 PG·HTTP·SDK 경계 포함 |
| `node --test` (verify.sh의 SDK·transport 단위 테스트 16개 파일) | 107 passed, 0 failed | 캐시·세션·쓰기 복구·계약·관리 API·실패 metadata |
| `python3 -m unittest product.tests.python_sdk_test` | 21 tests, OK | Python 호출 SDK. 기존 네트워크 경계 테스트에서 ResourceWarning은 남음 |
| `npm_config_cache=/private/tmp/aip-E-npm-cache node --test product/tests/sdk-package.test.mjs` | 5 passed, 0 failed | offline pack/install·실제 소비자·strict 타입. 임시 npm cache를 사용해 기본 cache의 실행 환경 쓰기 제한을 피함 |
| `cargo test -p aip-cli --locked --test contract_golden` | 4 passed, 0 failed | compact·pretty의 의미 일치, 코드·reason 보존, 생성 TS 골든과 운영 정보 분리 |
| `cargo test -p aip-ir --test codes --locked` | 8 passed, 0 failed | 오류 레지스트리와 방출 위치 pin |
| `cargo clippy --workspace --lib --bins --locked -- -D warnings`, transport의 `clippy --all-targets` | exit 0 | 제품 실행 코드와 transport 테스트 lint. 새 CLI 계약 테스트도 별도로 lint |

전체 통과를 주장하지 않는다. `bash tools/verify.sh`는 `service_flow`의 macOS worker 기동에서 실패했다. `/usr/bin/sandbox-exec -p '(version 1)(allow default)' /usr/bin/true`도 `sandbox_apply: Operation not permitted`로 exit 71이고, `webhook_oversize`는 worker runtime probe가 같은 메시지로 실패함을 직접 확인했다. 격리를 끄거나 테스트를 건너뛰는 변경은 하지 않았다. 추가로 시도한 CLI 전체 테스트 target lint는 기존 테스트의 `unwrap_used` 경고 때문에 통과하지 않았다. 이 회차의 실행 환경으로 worker 전체·외부 provider·실배포 검증을 대신할 수 없다.

main 통합 뒤 `python3 tools/check_plan_docs.py`는 `markdown files checked: 171; errors: 0`을 출력했고, `aip-ir codes` 8개와 CLI 계약 골든 4개도 다시 통과했다.

문서는 SDK·운영·wire 오류·결정 기록·CLI 출력 안내를 갱신했다. 초기 설계 대조와 개발 경험 비교는 과거 표본의 관측이라는 점을 명시하고 PoC 파일 확장자 링크를 고쳤다. 문서 전체의 내용 감사를 완료한 것은 아니다. CD-1 SafeNumber 단일 wire·시퀀스 상한, migration `requiresReview` 강제, 구조 정리와 예제·AI 평가 후속은 이 회차에 구현하지 않았다.

## 운영 문제 점검 회차 (2026-10-13)

worker 격리, 실배포 미검증 항목, 낮은 심각도 보안 항목, 의존성을 점검한 회차의 기록이다. 항목별 수정 전 실패·조치·커밋은 [보안 감사](../plan-docs/reviews/2026-10-13-security-audit.md)의 S-34 이후 행과 6.2절에 있다.

| 방금 실행한 명령 | 확인한 범위 |
|---|---|
| `node product/tools/proxy-check.mjs` (Caddy 2.11.7, 자체 서명 인증서, 임시 schema의 실제 서비스) | 16개 통과: Origin·CORS, Authorization·본문, `x-request-id` 유지·생성, `X-Forwarded-*`, 1MiB 초과, SSE keepalive 무버퍼, WebSocket Origin 필수·101·첫 프레임 인증 |
| `cargo test -p aip-auth --lib idp_tests` | mock HTTPS IdP: discovery의 issuer·jwks_uri, 겹침 회전, kid 교체, 폐기된 kid, 위조 kid, IdP 장애 중 캐시 키, JWKS 파일 변경 감지. 실제 IdP는 확인 못함: 계정 없음 |
| `AIP_BACKFILL_ROWS=12000000 cargo test -p aip-migrate --test large_backfill -- --ignored --nocapture` | 합성 1200만 행: 이전 방식은 20초 statement 한도에서 취소되고 읽기를 20.06초 막음. 상수 backfill은 적용 0.81초, 최대 읽기 대기 0.76초. 1200만 행 Text→Int 변환 4.5초 |
| `cargo test -p aip-migrate`, `--test online_backfill` | 테이블 재작성·행 갱신 없음, 적대적 값 인용, 잠금 대기 2초 뒤 `DB_LOCK`과 같은 digest로 재적용, 순환 migration, UTC 변환 |
| `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --lib` | worker 후손 프로세스 정리(손자 3종), macOS 샌드박스(읽기·쓰기·fork·spawn·network), Node 실행. 이중 fork 데몬 테스트는 한계를 기록한 `#[ignore]` |
| `bash tools/dependency-check.sh` | `cargo audit`·`cargo deny`를 10개 manifest에, `npm audit`를 SDK에 실행해 모두 통과(`rsa` RUSTSEC-2023-0071은 `deny.toml`의 근거 있는 예외) |

미실행: 실제 외부 IdP, nginx, 두 번째 PG 인스턴스·다중 서버 인스턴스.

## Python 호출 SDK 회차 (2026-10-13)

| 방금 실행한 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `python3 -m unittest product.tests.google_storage_provider_test product.tests.python_sdk_test` | 11 passed | 계약 지문 header·불일치, 응답 유실 뒤 같은 key 재전송, 재시도의 실행 전 거부를 미확정으로 유지, 세션 principal 범위, 관리 경로 |
| `node --test product/tests/service-smoke.test.mjs` | 1 passed | 실제 JWT·PostgreSQL 서비스에서 worker 없음·Node·Python worker × decimal·safe wire로 Python SDK의 발견·읽기·TS 쓰기의 같은 key 재생·`/status`·지문 고정 거부, worker 실행에서 operation·job |

## 백엔드 기능 포괄성 보완 회차 (2026-10-13)

job의 업무 데이터 접근, schedule·webhook·outbox trigger, health probe를 추가한 회차의 기록이다. macOS, 로컬 PostgreSQL에서 실행했다. 같은 시간에 다른 작업자가 같은 PostgreSQL에서 검증을 돌렸을 수 있다. 첫 v6 실행에서 `extension_lifetime`의 worker PID 기록 시간 초과가 한 번 났고, 단독 재실행 3회와 이후 전체 실행 2회에서는 통과했다. 부하에 민감한 기존 타이밍 테스트로 보고 기록만 남긴다.

| 방금 실행한 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true bash tools/verify.sh` | Rust 599 passed, 2 ignored, 0 failed. Node 105 passed, 0 failed | root workspace(새 cron·서명 단위 테스트 포함 안 됨), V1/V2/V3/V5 suite, 제품 lint·fmt, SDK package |
| `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --locked --target-dir target --no-fail-fast` | 52 passed, 0 failed | worker ctx 변경(`ctx.data.read`, `apply` key) 뒤 기존 Node/Python 회귀 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --no-fail-fast` | 114 passed, 0 failed | 신규 `job_data`, `triggers`, `outbox`, `health`, cron·서명 단위 테스트와 기존 transport 전체 |
| `node --test product/tests/service-smoke.test.mjs` | 1 passed | 재배치 binary·설치 SDK·두 wire·Node/Python worker |

신규 e2e가 실제 PostgreSQL에서 확인한 것: 행 정책으로 숨은 행에 대한 job 전이가 `WIRE.MISSING_TARGET`, 선언 없는 전이는 job 실패, 같은 key 재호출과 commit 뒤 worker crash 재시도에서 전이를 다시 실행하지 않고 journal 재생(Node·Python). webhook 정상·재전송·위조·무서명·주체 비활성(503). 두 instance가 같은 schedule slot과 같은 outbox 행을 경쟁해도 job 하나. 배포 digest 변경 뒤 `/ready` 503. 독립 컨텍스트 검토가 지적한 서명 밖 header key, 서비스 주체 보존 quota, outbox head-of-line, DST 반복 시간, grace 하한, `/ready` pool 사용, outbox id 재사용 7건을 고친 뒤 위 명령을 다시 실행했다. 다른 작업자의 커밋(`826028f`까지) 위로 rebase한 뒤 같은 명령을 다시 실행해 verify Rust 635 passed·2 ignored·0 failed, Node 108 passed, v4 53 passed, v6 114 passed, smoke 1 passed를 확인했다. AWS·GCP·실제 provider webhook 전송, Linux worker에서의 신규 e2e는 실행하지 않았다.

## resource 독립 operation 회차 (2026-10-10)

이 절은 2026-10-10 resource 독립 동기 operation 회차의 고정 검증 기록이다. 당시 기준 문법은 [EXECUTION](EXECUTION.md)에 적었다. 그 뒤 추가된 durable job, managed file/event, HTTP broker의 현재 계약은 [관리형 durable execution](MANAGED-EXECUTION.md)을 따른다. 이 절의 명령 결과를 새 변경의 실행 증거로 간주하지 않는다.

| 방금 실행한 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true bash tools/verify.sh` | Rust 487 passed, Node 72 passed, 실패·ignore·skip 0 | root workspace와 V1/V2/V3/V5 독립 suite, 제품 lint·fmt, SDK package, 마지막 CLI build. Rust 수치는 제품만의 테스트 수가 아님 |
| `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --locked --target-dir target` | 34 passed, 0 failed/ignored | 공통 worker 호출 리팩터링 뒤 기존 Node/Python·framing·ctx·읽기/쓰기·기한·wire 회귀 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target` | 43 passed, 0 failed/ignored | 새 operation 네 조합, 기존 HTTP·인증·CORS·DB 기한·배포/WRITE 복구·세션 결과 보존 |
| `node --test product/tests/service-smoke.test.mjs` | 1 passed, 0 failed/skipped | 재배치 binary·설치 SDK strict 소비자, 실제 JWT·owned PG schema, Node/Python × safe/decimal operation·Unicode·범위 거부·타입 음성 대조 |
| root와 수정한 다섯 spike의 `cargo fmt … --check`, `bash -n tools/verify.sh`, `node --check product/tools/smoke.mjs`, 문서 상대 링크 검사 | exit 0 | 포맷·shell/JS 문법·로컬 문서 링크. 외부 링크 가용성 보장은 아님 |

먼저 재현한 실패는 실행 중 principal revoke 뒤 output 반환, safe 배포에 decimal 설정으로 큰 actor 등록, 안전 정수 초과 operation 입력 수락, SDK에서 표현할 수 없는 범위 metadata 수락이다. 수정 후 같은 음성 대조를 포함해 위 suite를 실행했다. revoke 테스트는 worker가 시작 파일을 쓴 것을 확인한 뒤 폐기해 수락 이전 거부와 구분한다. 기존 `principal` 이름의 실제 resource와 새 관리형 mode의 전환도 migration에서 차단한다.

첫 설치 smoke는 기존 CLI 실행 파일의 오래된 `dependencies: []` 생성 계약과 새 SDK 타입이 달라 실패했다. 현재 소스로 CLI를 build한 뒤 같은 pack/install/typecheck/runtime 검사가 통과했다. package 도구가 현재 source에서 binary를 빌드했다는 provenance를 보장하는 것은 아니므로 실행 순서를 SDK 문서에도 명시했다.

독립 Sol 검토의 실행 중 폐기·정수·배포 wire 지적을 부모가 실제 테스트로 재현해 보완했다. 마지막 재검토는 소스/문서 대조이며 별도 테스트 실행으로 합산하지 않는다. Luna는 legacy identity·패키징과 다음 Job의 PoC 재사용 경계를 읽기 전용으로 검토했다. 이번 회차 Claude Code의 새로운 독립 실행 검토는 미실행이며 과거 감사의 상호 검토 기록과 구분한다.

당시 미실행/미구현: Linux worker 격리, 동기 요청 disconnect의 프로세스 회수 실측, durable Job·파일·외부 효과·전체 tenant 경로·운영 provider 연결·부하/성능 측정. 업무 테이블 0개 실행은 운영 PostgreSQL이 없는 배포를 의미하지 않는다. durable 회차는 아래와 같이 별도 검사해야 하며, provider 실서비스 exactly-once나 Linux filesystem 격리로 확대 해석하지 않는다.

## 관리형 durable execution (2026-10-11)

현재 구현 계약은 [MANAGED-EXECUTION](MANAGED-EXECUTION.md)에 정리했다. 아래는 macOS arm64와 로컬 PostgreSQL에서 현재 source로 실행한 결과다. Cargo 명령에는 `CARGO_NET_OFFLINE=true`, `CARGO_TARGET_DIR=target`을 적용했다.

| 실행 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `bash tools/verify.sh` | exit 0, `AIP core verification passed` | workspace 테스트·제품 lint·SDK 검사·v1/v2/v3/v5 독립 suite·마지막 CLI build. 새 Job/file/event/HTTP store, durable principal, migration upgrade, SDK Job 회귀 포함 |
| `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --locked` | exit 0, 실패 0 | 기존 worker 회귀와 Node/Python managed context, 선언 capability 및 안정 key 검사 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked` | exit 0, 실패 0 | 전체 HTTP transport 회귀와 새 durable 통합 시나리오 4개. 재기동·취소·owner 격리·HTTP 불확실성·중첩 예산·자식 실패 재생을 두 언어에서 검사 |
| `node product/tools/package.mjs --out /tmp/aip-managed-release-20261011-b` | `ok:true`, 파일 8개 | 현재 CLI, 설치 SDK tarball, Node/Python 예제와 source manifest 조립 |
| `node product/tools/smoke.mjs --bin <artifact>/bin/aip --sdk-package <artifact>/sdk/aip-sdk-0.1.0.tgz --workers node --extension-dir <artifact>/extensions` 및 `--workers python` | 두 명령 exit 0, 각 `ok:true` | 설치한 SDK strict 타입 검사와 실제 JWT/PG 실행. Node/Python × decimal/safe 네 조합에서 Job `SUCCEEDED`, Unicode 길이 3, 같은 key의 같은 id 재생. 기존 read/cache/write/revoke·동기 operation도 검사 |
| artifact에서 `shasum -a 256 -c SHA256SUMS`, source root에서 `shasum -a 256 -c <artifact>/SOURCE_SHA256SUMS` | artifact 8개 및 source 200개 일치 | 로컬 무결성. 공개 배포 서명이나 commit provenance 보장은 아님 |
| root 및 수정한 여섯 spike의 `cargo fmt … --check`, `bash -n tools/verify.sh`, `node --check product/tools/smoke.mjs`, 문서 상대 링크 검사 | exit 0, 문서 17개 중 누락 링크 0 | 포맷·문법·로컬 문서 연결 |

검증 중 순수 operation이 금지된 context 호출 오류를 잡고 성공을 반환할 수 있는 회귀를 기존 음성 테스트로 확인해 차단했다. 설치 소비자의 선택적 Job API와 smoke 결과 타입도 strict 검사에 맞췄다. 저장소 이동으로 드러난 v12 테스트의 개발자 절대 경로는 `CARGO_MANIFEST_DIR` 기준으로 바꾸고 해당 타입 테스트와 전체 v6 suite를 다시 실행했다. 이동 전 경로가 남은 로컬 crate 빌드 캐시는 재생성했다.

HTTP 통합 검사는 통제된 loopback provider fixture를 사용한다. 같은 안정 key 사이에 다른 capability 호출을 끼워도 provider mutation이 한 번만 전송되는 경우, 응답 유실 후 `WIRE.EFFECT_UNKNOWN`, 전송 중 취소 후 `WIRE.CANCELLATION_UNCONFIRMED`를 확인했다. 이 결과는 실제 운영 provider의 exactly-once 보장이 아니다. 실시간 경로는 cursor polling이며 SSE/WebSocket은 미구현이다. 외부 IdP·운영 provider·대용량 object storage·Linux worker 격리·부하 및 성능 측정은 미실행이다.

## 첫 공개 뒤 반복 검증 회차 (2026-10-06)

이 회차는 기존 구현의 재검증과 입력 경계·빌드 안내 보완이다. caller 표현이나 서버 권한, 최종 문법·wire 선택을 변경하지 않는다. 이전 실행 묶음은 당시 source 기록이며 이번 source 변경을 포함하지 않는다.

| 명령/실행 | 결과 | 확인한 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true bash tools/verify.sh` | Rust 230 passed, Node 53 passed, 실패·skip 0 | 전체 workspace 테스트, SDK cache/session/pending/contract 단위 테스트, offline 설치 SDK, 전체 fmt, 실행 코드와 제품 3 crate lint, 마지막 제품 build |
| `cargo test -p aip-cli --test service_boundaries --locked --offline` | 5 passed, 0 failed | FIFO를 정의·설정·migration·DB CA로 넣었을 때 유한 시간 내 오류. 크기 제한·origin·출력 원본 보호 |
| `node --test product/tests/service-smoke.test.mjs` | 1 passed, 0 failed | 재배치 binary·설치 SDK, 두 wire, 실제 JWT/PG read/write/replay/revoke, Node/Python 확장 |
| node_modules가 없는 임시 checkout에서 `npm ci --prefix spikes/spike-0-ts --offline --ignore-scripts --no-audit --no-fund` 후 SDK build | 두 명령 exit 0 | 기존 설치 디렉터리를 공유하지 않고 lockfile과 npm cache로 compiler·타입 의존성을 준비 |
| `actionlint .github/workflows/verify.yml`, `bash -n tools/verify.sh` | exit 0 | workflow 문법과 shell 문법 |
| SDK compiler가 없는 임시 checkout에서 verify script 실행 | exit 1, 설치 명령 출력 | 검사 누락을 성공으로 보고하지 않는 음성 대조 |
| 각 독립 `Cargo.toml`의 `cargo test --manifest-path … --locked --offline` | V1 43·V2 33·V3 5·V4 32·V5 22·V6 40·V7 1·V11 9·prototype 37, 실패·ignore 0 | 정의 상한, 실제 SQL 정책, 쓰기·worker 원자성, 요청/DB/worker 기한, 응답 유실 복구, CORS·두 wire·Node/Python 확장 |
| [GitHub Linux CI #37482679158](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/37482679158), commit `06ff8d8` | success、Rust 230 passed、Node 53 passed、실패·skip 0 | Ubuntu x86_64, Rust 1.99.0, Node 26, PostgreSQL 17, 새 checkout 의존성 설치와 core 검증 |

FIFO 회귀는 수정 전 `waited for a FIFO writer`로 실패했다. SDK 회귀는 수정 전 `BUILD_FAILED`와 기대한 `SDK_TOOLCHAIN_MISSING`이 달라 실패했다. 두 반례를 확인한 뒤 해당 동작만 보완했다. 별도 9개 파일의 포맷 변경은 변경 전 source를 rustfmt한 결과와 바이트 단위로 대조했다.

V7의 최초 `--locked` 실행은 오래된 lockfile 때문에 테스트 시작 전에 실패했다. 현재 V2 읽기 엔진의 TLS 의존성 39개를 기록하되 기존 locked 패키지 버전은 제거·교체하지 않았다. 갱신 뒤 V7의 `--locked` 검사는 1 passed다. 이 회차 Rust 합계 452개는 workspace 230개와 독립 경로 222개의 합계이며, 제품 서비스 452개로 표현하지 않는다. Node 53개와 제품 smoke 1개도 서로 다른 검증 범위다.

Root `cargo clippy --workspace --all-targets --offline -- -D warnings`는 기존 CLI 통합 테스트의 `unwrap_used` 진단으로 실패했다. 실행 코드 `--lib --bins`와 제품 3 crate `--all-targets` 검사는 exit 0이다. 전체 테스트 lint 통과로 확대하지 않는다. Linux CI는 위 실제 실행 결과로 확인했으며 macOS worker 및 외부 운영 배포 검증을 포함하지 않는다.

보완 목록과 자동 검증의 경계는 [VALIDATION](VALIDATION.md)을 따른다. 아래 표와 bundle 해시는 이전 제품 조립 회차의 기록이다.

## 현재 제품 경로의 실제 검증

| 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `cargo test -p aip-auth -p aip-migrate -p aip-service --offline` | 39 passed, 0 failed | JWT/JWKS/TLS·actor 연결, DDL 계획·변환·backfill·rollback·동시 적용·drift·wire 원자성·이관 재시도 |
| `cargo test -p aip-cli --test service --test service_boundaries --test service_flow --test adoption_flow --offline` | 8 passed, 0 failed | 실제 JWT read/apply·재기동·live 정책 변경 거부·토큰 폐기와 재활성화·기존 서버 쓰기 결과 이관·설정 및 출력 보호 |
| `node --test product/tests/sdk-package.test.mjs product/tests/service-smoke.test.mjs` | 5 passed, 0 failed | offline SDK 설치·strict 타입 음성 대조·JWT·캐시·쓰기·멱등 재생·공백 경로 재배치·Node/Python 확장 |
| `cargo clippy -p aip-auth -p aip-migrate -p aip-service --all-targets --offline -- -D warnings` | exit 0 | 제품 조립 crate·인증·마이그레이션과 해당 테스트 경고 없음 |
| `cargo build -p aip-cli --offline` | exit 0 | root CLI 실행 파일 생성 |

재배치 확인은 [최종 묶음](dist/2026-10-06T11-19-25-091Z/README.md)의 binary·SDK tarball·확장 파일을 저장소 밖 공백 경로에 복사해 실행했다. 기본 설정·Node·Python 각각 decimal/safe 두 wire에서 실제 JWT/PG 흐름을 실행한다. SDK 읽기·캐시 hit·쓰기 후 무효화·같은 키 재생·인증 철회를 확인한다. 확장 검사는 이미 적용한 Event에 `Event.confirm`을 호출해 count 0과 같은 키 재생을 확인한다. 새 행 변경과 확장 rollback의 엔진 검증은 아래 기존 worker/전송 회귀 범위와 구분한다.

묶음의 SHA256SUMS 8개 파일과 SOURCE_SHA256SUMS 181개 source 입력을 메인이 다시 비교했으며 mismatch 0이다. 마지막 `cargo build`의 binary와 묶음 binary도 바이트 단위로 일치한다. Cargo 테스트가 다른 dependency feature 조합의 실행 파일을 target 경로에 복원할 수 있어, 최종 build를 테스트 뒤 실행하고 묶음을 새로 생성했다.

## 함께 확인한 기존 엔진 회귀

2026-10-05 이어서 작업 중 `cargo test --manifest-path <각 Cargo.toml> --offline`을 실행했다. V1 43·V2 33·V3 5·V4 32·V5 22·V11 9·V6 40·prototype 37개가 각각 실패 0이다. 기존 root CLI의 contract_golden/golden/codes/diff 19개도 실패 0이다. 이 숫자는 현재 제품 47개에 합쳐 제품 표면 전체를 보증하는 수치로 사용하지 않는다.

원격 DB TLS 연결의 오류 타입 변경에 맞춰 기존 V4/V6 테스트의 진단 분기를 갱신했다. 운영 JWT의 4KiB 한도에 맞춰 V6 기존 Bearer 경계 테스트를 공개 MAX_TOKEN+1로 바꿨다. 실제 오류·경계 검사 자체를 제거하지 않았다. V1/V2/V6과 제품 crate의 수정 파일 포맷을 확인했다. root CLI 전체 fmt 검사는 이번 작업에서 수정하지 않은 encrypt_e2e.rs의 기존 포맷 차이를 보고하므로, root CLI는 이번 수정 파일을 직접 rustfmt 검사했다.

## 독립 검토와 제한

Sol high가 별도 문맥에서 운영 중 구 정책 실행, 폐기 토큰 재활성화, 없는 신원 폐기 성공, 이관 멱등 namespace 누락, wire 고정의 별도 트랜잭션 문제를 발견했다. 메인은 실제 RED/GREEN과 코드로 대조했다. 최종 Sol 검토는 같은 SQL 연결의 배포 잠금·wire 포함 승인 지문·원자적 metadata 기록·폐기 cutoff를 읽고 추가 필수 결함을 확인하지 못했다. 마지막 독립 검토에서 DB 테스트를 재실행한 것은 아니다.

검증 환경은 현재 macOS arm64, 로컬 PostgreSQL, 통제된 HTTPS 인증서 fixture와 메모리 생성 RSA 키다. 외부 실제 IdP 계정·TLS reverse proxy 배포·다른 OS·대형 테이블 무중단 변경은 미실행이다. actor resource 교체·임의 SQL 변환·자동 역방향 migration·구버전 client 공존은 지원하지 않는다. 최종 작성 형식·Id 표현·쓰기 조합·라이선스의 미결정을 승인으로 승격하지 않는다.

실행 절차는 [OPERATIONS](OPERATIONS.md), 설계 관문은 [서비스](../crates/aip-service/README.md)·[인증](../crates/aip-auth/README.md)·[마이그레이션](../crates/aip-migrate/README.md)·[정의 한계](../spikes/spike-v1-fixture/PRODUCTION-LIMITS.md)·[정책 실행 한계](../spikes/spike-v2-read/PRODUCTION-RUNTIME.md)를 따른다.
