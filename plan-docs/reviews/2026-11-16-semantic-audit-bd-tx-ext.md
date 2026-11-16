# 의미 감사 B/D: 트랜잭션·동시성·확장·외부 효과

- 기준 브랜치: `main`에서 분기한 `audit/semantic-bd-tx-ext-20261116`
- 감사 범위: 제품 서비스가 실제로 호출하는 쓰기 경로와 durable execution의 계약
- 수정 범위: 제품 동작 변경 없이 감사 결과와 최소 재현 테스트만 추가
- 판정 시각: 2026-11-16 (KST)
- 당시 상태: **부분 완료**. 보존 기준선에서 lock-order 테스트 실패는 보이나 실패 결과 전문과 SQLSTATE가 없어 교착 실행 증거로 확정하지 않았다. 해당 시점 세션은 DB 재실행 없이 문서·테스트 판정 기준을 보완했다. 후속 확인과 구현은 §7을 본다.

## 1. 제품 경로와 PoC 구분

이 저장소의 제품 진입점은 `aip service serve`다. `crates/aip-service/Cargo.toml`은 `spike-v6-transport`를 의존하고, `crates/aip-service/src/lib.rs:1697-1710`에서 실제 listener를 구성한다. V6의 `/apply`는 `spikes/spike-v6-transport/src/lib.rs:188-219,246-275`에서 전이·atomic bundle·compose를 하나의 요청 트랜잭션으로 라우팅하고, atomic 경로는 `spike_v3_write::bundle_in`을 호출한다. Durable job은 `spikes/spike-v6-transport/src/durable.rs:1418`에서 같은 V3 write 엔진의 `apply_in_with_wire`를 호출한다.

즉 이 구현은 디렉터리 이름이 `spikes/`인 모듈을 제품 서비스가 직접 연결한다. 그 경로에 붙인 회귀는 단순 prototype 전용 결과로 치부할 수 없다. 다만 이번 새 재현은 내부 `bundle`/`handle`을 직접 호출하며 `aip service serve`의 설정·인증 wrapper 전체를 통과하지는 않는다. 제품 서비스가 같은 함수를 호출한다는 의존 경로를 확인한 범위의 증거다.

## 2. 우선순위 결과

### P2 후보 — atomic bundle의 요청 순서가 교착을 만들 수 있음 (당시 소스상 개연성, 후속 확인은 §7)

- **문제 후보:** 두 atomic 요청이 같은 행을 반대 순서로 여러 단계에서 잠그면 순환 대기가 생길 수 있다. 이는 현재 호출·잠금 코드에서 도출한 재현 가능한 가설이지, 현재 공개 계약 위반 또는 보장된 제품 실패로 판정하지 않는다.
- **영향 후보:** 실제 DB 교착이면 희생 요청은 롤백되어 `WIRE.CONFLICT`로 반환될 수 있다. READ COMMITTED 경로에서 서버 자동 재시도는 확인되지 않았다. 사용자 데이터 손상은 이 기록에서 확인되지 않았다.
- **제품 계약과 결정:** `spec/definition-syntax.md`의 `expect`와 `spec/wire-errors.md`의 `WIRE.CONFLICT`는 stale expect/동시 변경에 따른 rollback 및 호출자 재확인을 정한다. 이 계약은 bundle의 전체 잠금 순서나 순서 무관 실행, 모든 deadlock 자동 재시도를 약속하지 않는다. `docs/design/02-language.md`, `04-safety-matrix.md`, `05-execution-model.md`는 모두 `Proposed`이며 전역 순서·재시도 설명은 현재 구현을 입증하는 결정이 아니다. `docs/DECISIONS.md`에서 bundle 전역 선잠금의 Accepted 결정을 찾지 못했다.
- **소스 추적:** 제품 `aip service`의 V6 `/apply`가 V3 `bundle_in`/`compose_in` 경로를 부른다. V3 `bundle_run`은 연산을 요청 순서대로 처리하고, 각 단계의 `judge_query`가 그 단계 대상만 `ORDER BY id FOR UPDATE`로 잠근다. bundle 전체 대상의 사전 전역 잠금은 이 경로에서 확인되지 않았다. `db_err`는 PG `40P01`/`40001`을 retryable 표식이 붙은 `WIRE.CONFLICT`로 분류한다. 해당 retry loop는 SERIALIZABLE인 apply/bundle/compose에서만 작동한다. 일반 WIRE 코드만으로는 PG 오류를 식별할 수 없다.
- **다른 경로 대조:** 단일 전이는 대상 집합을 `id` 오름차순으로 잠그므로 이 재현의 역순 교착 유형은 없다. `compose`와 V6 write extension의 `bundle_in`도 순차 전이 호출을 공유하므로 동일 유형의 잠금 순서 위험이 남는다. 이 확장 경로의 반례는 이번에 별도 실행하지 않았다.
- **보존 실행 증거:** `/tmp/aip-baseline-engines.log`에는 `audit_bd_lock_order` 테스트의 `FAILED`, `0 passed; 1 failed`, panic 위치 `tests/audit_bd_lock_order.rs:72`, Cargo의 실패 target 문구가 있다. 로그에는 실행 명령 전문·revision 메타데이터·`outcomes` 값·프로세스 exit code가 없고, 테스트 단언은 어떤 원인이든 `WIRE.CONFLICT` 하나면 실패시킨다. 따라서 실제 실행 실패는 보이나 deadlock 원인, 정확한 반환 코드·메시지, PostgreSQL SQLSTATE `40P01` 관측은 **미확인**이다. 이 로그만으로 RED를 교착 재현 성공으로 부르지 않는다. 로그 표식은 테스트 target과 결과만 남기므로 기록 revision은 현재 지정 HEAD `d55dd95`와 동일하다고 확인할 수 없다.
- **테스트 판정 보완:** `audit_bd_lock_order.rs`는 일반 `WIRE.CONFLICT`를 교착으로 오인하지 않도록 V3의 내부 retryable 표식(`is_retryable`)을 구분하고, 예상 business 결과를 `WIRE.INVALID_STATE`로 한정한다. 다만 테스트의 100ms `pause_after_lock_ms`는 스케줄 유도용 sleep이라 양 세션이 첫 잠금을 동시에 가진다는 보장이 아니다. 테스트 자체는 원시 SQLSTATE를 보존하지 않으므로, 정확한 재현은 격리된 PostgreSQL에서 두 세션을 식별해 wait graph/lock wait를 관측하고 서버 오류 로그의 `40P01`과 테스트의 전체 outcomes를 함께 보존해야 한다. 저장소의 `write_extension_atomic.rs:55-68`에 `pg_stat_activity` lock wait polling 선례가 있다. 성공 판정은 한 bundle 성공, 나머지 정상 `WIRE.INVALID_STATE`, 두 행 모두 승자 소유, retryable SQL conflict 표식 없음과 SQLSTATE 40P01 미관측을 각각 기록하는 것이다. 이 세션은 실제 DB 확인을 하지 않는다.
- **수정 권고는 보류:** 현재 증거로 전역 선잠금 구현을 권고하거나 새 제품 보장으로 승격하지 않는다. 테스트가 40P01을 보존된 독립 실행에서 재현한 뒤 lock-order 선택지를 다시 평가한다.
- **수정 난이도:** 중간. 잠금 후보를 계산할 때 전이 순서·행 정책·bulk 상한·생성 후 참조 의미를 보존해야 한다. 대규모 재설계 필요성은 확인되지 않았다.

### 멱등 동시성 — 결함 확인 안 됨, 추가 검증 미완

동일 principal/key는 advisory lock으로 직렬화되고, 저장된 동일 요청은 결과를 재생하며 다른 본문은 `WIRE.IDEMPOTENCY_MISMATCH`로 거부한다 (`spikes/spike-v6-transport/src/lib.rs:307-327`). 커밋 뒤 응답 유실 경로는 기존 `commit_unknown_wire`에서 13건 통과했다. 다만 기준선에 추가했던 12연결 테스트의 두 번째 race가 서로 다른 두 업무 요청(Counter.bump와 Booking.confirm)에 같은 key를 재사용해 12개 모두 mismatch를 받은 것은 테스트 오류다. 본문을 고치면 별도 키를 사용하도록 수정했다. 첫 번째 READ COMMITTED race는 기존 기준선에서 예상한 1회 실행·11회 재생을 통과했으나, 수정 후 두 isolation 경로 전체는 현재 PostgreSQL 환경 제약으로 재실행하지 못했다. 결함으로 세지 않는다.

## 3. 실행 의미 항목별 판정

`검증됨`은 이번 요청의 기존 실제 실행 출력 또는 해당 경로를 겨냥한 기존 실행 테스트가 기준선에서 통과한 경우다. 문서만 존재하거나 이번 세션에서 실행하지 않은 것은 결함 확인으로 올리지 않았다.

| 항목 | 판정 | 코드·기존 검사와 판단 |
|---|---|---|
| 복합 변경 원자성 | 검증됨, 범위 한정 | `bundle_once` 한 트랜잭션과 `bundle_in`/`checks_in` 호출 경로: `spike-v3-write/src/lib.rs:1543-1557,1575-1583`; 실패 시 트랜잭션 drop/rollback. 기존 V3 bundle·commit fault 계열과 V6 HTTP 경로가 있음. 동일 atomic 요청의 전체 서비스 wrapper 변형은 이번에 새로 실행하지 않음. |
| 동시 수정 충돌 | 검증됨, 범위 한정 | 명시 행 잠금과 부재 의존 SERIALIZABLE 재시도는 서로 다른 경로다. 공개 `WIRE.CONFLICT`는 전체 DB rollback 및 상태 재확인을 안내하며 deadlock SQLSTATE 세부를 노출하지 않는다. 기존 `absence_serializable_http` baseline 1건 통과. |
| 잠금 순서·데드락 | **실행 원인 미확인** | 코드상 atomic 단계 순서가 잠금 순서를 결정할 수 있고 baseline test는 실패했으나 결과는 일반 `WIRE.CONFLICT`인지 40P01인지 보존되지 않았다. Proposed 문서의 순서 약속은 현행 제품 계약이 아니다. |
| 낙관적 동시성 | 계약·경로·회귀 대조 완료 | `expect` 선언 필드는 요청마다 모두 필요하고, 요청 값은 `apply_input`에서 타입 검증된 후 잠긴 대상의 UPDATE 조건에 NULL-safe 비교로 들어간다. 기대 값 불일치 시 변경 수 불일치가 `WIRE.CONFLICT`로 되어 transaction 전체를 rollback한다. `general_update.rs`의 stale expect 회귀, `product/tests/general-update.test.mjs`의 wire 결과, `facereview-comments.test.mjs`의 두 동시 편집 중 하나 성공/하나 conflict 기존 회귀가 이 의미를 확인한다. `version`은 버전 필드 증감과 함께 선언할 때 권장되는 낙관적 경합 표식이다. |
| 실패 시 롤백 범위 | 검증됨, DB 범위 | V3 commit-classification tests는 COMMIT 전 실패와 결과 불명 상태를 구분한다. extension atomic test도 callback 오류·deadline·출력 오류 롤백을 겨냥한다. 외부 시스템 효과까지 DB rollback 된다고 보장하지 않는다. |
| commit 후 응답 유실 | 검증됨 | `/apply`는 commit 뒤 연결·응답을 분류하고 같은 key의 재요청으로 결과를 재생한다. 기존 V6 `commit_unknown_wire`: 13 passed. |
| 멱등 키 재사용·결과 재생 | 부분 검증 | 순차 동일 key 재생·다른 본문 거부 경로가 있고 기존 commit-unknown test 통과. corrected 12-connection race는 새로 실행되지 않음. |
| 확장 입력·출력 계약 | 기존 테스트 통과 | 선언된 입력·결과 검증·잘못된 출력 거부의 기존 `write_extension_atomic`(1), `write_extensions_http`(2), `write_extension_commit_classification`(12) 테스트가 기준선 통과. 이는 현재 실행 로그의 결과이며 추가 바운더리 탐색은 미완. |
| 확장의 데이터 접근 범위 | 기존 테스트 통과 | `access`로 허용된 전이/DB 작업만 host callback에 전달되는 경로. `write_extensions_http` 및 `write_extension_atomic` 실행 통과. 모든 드라이버 capability를 포괄했다는 의미는 아님. |
| 확장과 DB 트랜잭션 경계 | 기존 테스트 통과 | write extension은 V3 transition과 audit/idempotency를 같은 tx에 둠. 기준선 `write_extension_atomic` 1건, `write_extension_commit_classification` 12건 통과. 외부 provider는 분리됨. |
| Job 재시도·중복 실행 | 기존 테스트 통과, 조합 공백 | `job_retry` 1건과 `job_breaker` 2건 통과. job API idempotency 및 effect journal이 있음. 여러 worker·provider 결과 불명·lease 만료가 교차하는 모든 조합은 미검증. |
| 외부 API 결과 불명확성 | 기존 테스트 통과 | provider 전송과 DB commit은 원자적이지 않으며 UNKNOWN을 자동 재전송하지 않는 계약. `job_breaker` 2건과 local mock `outbound_webhook` 1건 통과. 실제 외부 전송은 하지 않음. |
| Outbox·Webhook 전달 의미 | 기존 테스트 통과, 중복 허용 | outbox pending dispatch와 서명 webhook/local mock 경로. 기준선 `outbox` 2건, `outbound_webhook` 1건 통과. 외부 consumer의 정확히 한 번 전달을 의미하지 않음. |
| 실패 후 보상·복구 | 계약별 지원 범위 구분 | durable job은 effect journal/idempotency 결과 재생을 지원하고, 결과 불명인 비멱등 호출은 `UNKNOWN`/`WIRE.EFFECT_UNKNOWN`으로 남겨 맹목 재전송하지 않는다. Provider lookup은 그 기능이 선언·구현된 경로에 한정된다. Mongo/Redis write extension의 compensation은 별도 saga log와 `WIRE.COMPENSATION_FAILED` 수동 복구/보상 재시도 안내가 있다. D-P-JOBSTORE 초안은 job saga compensation 미지원을 적으며 현행 durable 분기도 같은 경계를 둔다. design §4의 모든 외부 효과에 대한 컴파일러 생성 saga·일반 lookup 복구 루프는 Proposed이며 제품 보장으로 세지 않는다. |

## 4. 포팅 자산의 실패·우회 유형 분류

다음은 포팅 자산에 명시된 사례 중 이번 감사 축과 직접 관계가 있는 대표 사례다. 전체 포팅 parity가 완료됐다는 판정은 아니다.

| 분류 | 사례와 의미 |
|---|---|
| Implementation | Infisical G2의 암호화 값 저장·읽기·회전 통합은 primitive와 구분해 미구현으로 기록됐다 (`plan-docs/porting/INFISICAL-GAPS.md` §G2). 선언·crypto primitive만으로 제품 동작 완료로 보지 않는 사례. |
| Semantic | FaceReview의 legacy GET `mypage/emotion/summary`는 읽기 호출 중 회원 누계를 바꾸는 부작용을 가진다 (`porting/facereview/ui-overlay/src/api/readAdapter.ts:160`). 포팅 adapter는 이를 write extension으로 실행한다고 명시한다. HTTP GET 의미를 그대로 읽기 전이로 옮기지 않은 명시적 대응이며, 호출 재시도·캐시와 mutation의 차이를 별도 검증해야 한다. |
| Expressiveness | Infisical G14는 임의 깊이 폴더 트리 삭제·순환 조상 검사가 재귀 술어 없이 표현되지 않아 미구현으로 남았다 (`plan-docs/porting/INFISICAL-GAPS.md:92-117`). 일부 자식만 지우는 우회는 의미 동등하지 않다. |
| Abstraction | FlaskBB G-6는 원본 UUID와 내부 PostOrder identity를 분리하고 source sequence mapping/high-water 계약을 둔다 (`porting/flaskbb/PRODUCT-GAPS.md` §G-6). 기존 정수 id를 원본 순서로 간주하거나 DB sequence를 시간으로 오해하지 않도록 별도 저장 추상화를 택했다. 전체 원본 provenance·projection parity는 계속 열린다. |
| Verification | FlaskBB G-14는 실제 PG에서 즉시/지연 FK × apply/bundle/compose 실패와 롤백을 관찰한 뒤 공통 오류 분류를 고쳤다고 기록한다 (`porting/flaskbb/PRODUCT-GAPS.md` G-14; `porting/flaskbb/STATUS.md:43,55`). 초기 source 추론만으로 확정하지 않고 RED/실행 증거로 범위를 좁힌 사례다. FaceReview도 UI adapter 단위 시험, real service 여정, browser journey를 분리해 기록하므로 한 단계 통과를 전체 의미 검증으로 확대하지 않는다. |

## 5. 실행 흔적과 미검증 범위

- 기존 기준선 `/tmp/aip-baseline-engines.log`: lock-order target은 실패(1/1)했지만 원시 outcome·SQLSTATE는 없음. 같은 로그에서 `commit_unknown_wire` 13/13, `job_retry` 1/1, `outbox` 2/2, `outbound_webhook` 1/1, write-extension atomic 1/1 및 commit classification 12/12 통과.
- 전체 baseline 로그에는 서로 다른 engine별 실패가 있다: V3 3 targets(lock-order, latest effect, nullable projection), V4 3 targets(native capability/principal fixtures), V6 1 target(잘못된 key 재사용 race), 그리고 product test sweep의 6 targets. 마지막 그룹에서 로그에 확인되는 이름은 V12–V16이며 실패 원인 전문은 이 보고서에 재분류하지 않았다. 감사와 무관한 latest projection·optional Mongo/Redis·principal fixture 실패를 B/D 결함으로 세지 않았다. 각 숫자는 각각의 Cargo invocation 결과이며 합쳐서 하나의 `6 targets failed`로 표현하지 않는다.
- 이전 세션의 기록된 DB 실행 시도: package 이름으로 지정한 첫 `cargo test -p spike-v3-write ...`는 crate가 독립 workspace라 `package ... is not a member of the workspace`로 테스트 전 종료. 올바른 경로의 테스트 실행은 명시 DSN guard `AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults`에서 중단. 당시 기록된 대안 실패는 Docker socket `unix:///Users/winterholic/.orbstack/run/docker.sock` 부재와 PostgreSQL 17 `initdb`의 `shmget(...): Operation not permitted`다. 이 세션에서는 이 DB 실행·대안을 재시도하지 않았다.
- baseline `audit_bd_idem_race` 실패는 서로 다른 업무 본문에 같은 key를 넣어 모두 `WIRE.IDEMPOTENCY_MISMATCH`를 받은 테스트 오류였다. 현재 파일은 READ COMMITTED `Counter.bump`와 SERIALIZABLE `Booking.confirm` 각 race에 서로 다른 고정 key를 주고, 각 race 안에서는 동일 본문·동일 key를 사용한다. 이번 세션은 assertion을 실행하지 않고 `--no-run` 컴파일만 통과했다.

### 이번 보완 검증

- `cargo fmt --all -- --check` — exit 0.
- `git diff --check` — exit 0.
- `env -u PGHOST -u PGPORT -u PGDATABASE -u PGUSER -u PGPASSWORD -u PGSERVICE -u PGSERVICEFILE AIP_TEST_DATABASE_URL='host=127.0.0.1 port=5433 dbname=aip_test_ci user=runner' AIP_TEST_ADMIN_URL='host=127.0.0.1 port=5433 dbname=aip_test_ci user=runner' cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test audit_bd_lock_order --no-run` — exit 0, executable 생성. DB 연결 및 assertion 실행 없음.
- 같은 exact command의 manifest/test 인자만 `--manifest-path spikes/spike-v6-transport/Cargo.toml --test audit_bd_idem_race --no-run`로 바꿔 실행 — exit 0, executable 생성. DB 연결 및 assertion 실행 없음.
- 첫 V3 `--no-run` compile은 test의 `&str` 비교에서 E0277을 찾아냈다. 참조 비교를 고친 뒤 재컴파일과 format 검사 모두 통과했다.

## 6. 당시 수정안 (코드 변경 아님)

현재 결론은 구현 권고가 아니라 증거 보완이다. lock-order 테스트를 독립 PostgreSQL에서 재현할 때는 두 연결의 lock 획득 단계를 동기화하고, PostgreSQL server log의 원시 `40P01` 및 test outcomes를 보존한다. 동시에 business `WIRE.INVALID_STATE`가 교착 신호로 잘못 집계되지 않는지 확인한다. 이 확인 후에만 전역 선잠금 등 변경안을 검토한다.

낙관적 동시성은 spec·judge/update 경로·기존 회귀로 해소했으며, recovery는 현재 제품별 지원 범위와 Proposed saga를 분리했다. 제품 코드는 수정하지 않았다. C 담당 CI 결과는 직접 받지 않았으므로 미실행이다.

## 7. 구현 후속 — 소유 SQL 트랜잭션의 제한 재시도

이 절은 위 감사 당시의 결론을 덮어쓰지 않고, 이후 확보된 CI 증거와 최소 신뢰성 보완을 추가한다. 전역 선잠금은 여전히 Proposed 설계이며 공개 계약이 아니다. 이 재현에서 데이터 손상은 확인되지 않았다.

### 확인된 원인과 영향

- 기존 회귀 `spikes/spike-v3-write/tests/audit_bd_lock_order.rs`는 두 atomic 요청이 `[30,31]`과 `[31,30]` 순서로 각각 요청 순서대로 전이를 실행하도록 한다. CI43806410294는 head `22683321`에서 `audit-bd-lock-order.log` 결과 `0 passed; 1 failed` 및 `[(1, Err(("WIRE.CONFLICT", true))), (2, Ok(()))]` outcome을 남겼다. retryable 표식 때문에 일반적인 business conflict와 달리 SQL 동시성 실패임을 구분했다.
- 같은 실행의 `postgres-server.log`는 `2026-11-16 04:52:25.979 UTC [394] ERROR: 40P01: deadlock detected`를 기록했다. `aip_bd_lock_order_13748.task`의 두 요청이 같은 `SELECT ... ORDER BY t.id FOR UPDATE OF t`에서 tuple 잠금을 기다렸고, 프로세스 394는 transaction 900(393번 프로세스)을 기다리는 동안 393은 transaction 901(394번)을 기다렸다. 각 bundle의 단계별 잠금이 요청 순서를 따라 역전돼 순환 대기가 생겼다.
- V3 `bundle`은 `SERIALIZABLE`일 때만 기존 3회 bounded retry/backoff를 사용했으므로 READ COMMITTED 40P01은 rollback 뒤 자동 재실행되지 않고 retryable `WIRE.CONFLICT`로 끝났다. 같은 순차 연산을 쓰는 `compose`와 혼합 전이 경로에도 같은 실패 분류가 적용될 수 있다. 이는 공개 계약이 약속한 무교착 보장을 위반한 건 아니다. 원래 계약은 DB 충돌과 전체 rollback을 허용한다.

### 실행 소유권과 재실행 경계

- V3 `apply`, `bundle`, `compose` 래퍼는 `Client`에서 transaction을 열고 해당 요청의 SQL 정책 판정·행 변경·audit/outbox 효과·commit을 소유한다. SQL 오류가 `is_retryable` 표식(40P01/40001)으로 분류된 때에는 전체 transaction이 실패한 뒤 drop/rollback되고, 재실행은 정책 및 현재 행 상태를 다시 평가한다. business refusal, 권한/입력 거절, 일반 `WIRE.CONFLICT`는 retry 표식이 없어 재실행하지 않는다.
- 실제 V6 HTTP `/apply`의 `handle_apply`도 transaction을 시도별로 열며, 업무 변경과 idempotency journal을 함께 커밋한다. 재시도는 같은 principal/key/request로 journal 조회부터 다시 시작한다. 이전 시도의 전체 rollback은 업무 변경과 journal을 함께 제거하므로 성공 결과는 한 번만 저장되며, 기존 권한 필터와 operation 순서를 그대로 다시 평가한다. retry exhausted는 기존 conflict 응답으로 남는다.
- `bundle_in`/`compose_in`은 호출자가 소유한 `Transaction`에만 실행을 보태며 자체 rollback·재시작을 할 수 없다. V6 write extension은 해당 경로를 worker callback, Mongo/Redis saga 효과와 함께 사용하므로 바깥 DB 오류만 보고 callback 전체가 안전하게 재실행됐다고 단정할 수 없다. 이 caller-owned/외부 효과 경로는 자동 재실행에서 제외했다. Durable worker callback도 이 요청에서 재시도 계약을 바꾸지 않았다.
- HTTP `/apply`의 외부 write extension은 `write_extension::apply_with_launch` 경로이며 일반 `handle_apply` retry loop 밖이다. `run_relational_write_extension`의 provider 효과도 별도 callback 경계다. 두 경로 모두 변경하지 않았다. `COMMIT_UNKNOWN`은 COMMIT을 보낸 뒤 응답이 불명확한 경우라 retryable marker를 만들지 않아 자동 재전송되지 않는다.

### 최소 변경 및 영향

- 기존 bounded retry 정책을 `retry_owned_transaction`으로 공유하고, V3 소유 `apply`/`bundle`/`compose`와 V6 소유 SQL-only HTTP `/apply`에서 isolation이 READ COMMITTED여도 retryable conflict를 최대 기존 횟수만큼 backoff 후 재실행한다. 기존 SERIALIZABLE 재시도에도 같은 상한과 경로가 적용된다. 각 경로의 전체 WRITE deadline/cancellation 감시 안에서 대기하므로 deadline이 먼저 끝나면 재시도는 종료되고 COMMIT_UNKNOWN 분류는 유지된다.
- `audit_bd_lock_order` 회귀는 한 승자, business refusal `WIRE.INVALID_STATE` 한 패자, 두 행 모두 승자 소유를 계속 확인한다. 기존 `bundle_http`는 HTTP atomic/compose의 원자성·멱등 재생 계약을 확인한다. retry 변경을 포함한 실제 DB GREEN은 C의 격리 CI에서 실행해야 한다.
- 호환 영향은 제한된 투명 서버 재시도 및 최대 bounded backoff만큼의 요청 지연이다. 상한 소진 시 기존 retryable conflict 계약이 남으며, 모든 deadlock 제거·재시도 성공·무교착을 보장하지 않는다. 권한 결과, operation 순서, 원자성, 멱등 기록, commit 불명 처리는 변경하지 않는다.

### 이번 수정의 검증

- 단위 회귀는 READ COMMITTED와 SERIALIZABLE의 retryable marker를 기존 상한 안에서 허용하고, business refusal 및 상한 초과를 거부한다. 테스트 first RED에서 현재 조건의 결함을 확인한 뒤 구현을 바꿔 GREEN으로 전환했다.
- no-DB 컴파일은 V3 `audit_bd_lock_order`, `latest_effect_values` 및 V6 `bundle_http`, `write_extension_atomic`, `commit_unknown_wire`를 `--no-run`으로 빌드한다. 이는 실제 DB 실행이나 assertion 통과를 뜻하지 않는다.
- DB GREEN 미실행. 격리 CI용 필수 실행 대상은 다음과 같다: `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test audit_bd_lock_order`; 관련 pure DB 영향 회귀 `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test latest_effect_values`; HTTP 소유 경로 `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test bundle_http` 및 `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test absence_serializable_http`; 효과/commit 경계 `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test write_extension_atomic` 및 `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test commit_unknown_wire`. DB 대상은 C가 관리하는 격리 CI이며 이 checkout에서 실행하지 않는다.
