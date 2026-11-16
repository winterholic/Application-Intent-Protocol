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

## 8. Phase4 — 선언형·확장 경계 감사 (2026-11-16)

이 절은 기존 B/D §2–5의 당시 판정과 §7의 제한 재시도 구현을 덮어쓰지 않는 별도 감사다. 목표는 세 포팅 앱의 실제 사용 경로를 현재 live `main`의 선언·SDK·엔진 경계와 비교하는 것이며, 기능 추가나 DSL 변경 제안은 승인하지 않는다.

### 기준 revision, 재사용 증거, 읽기 범위

| 출처 | 확인한 revision·용도 | 재사용 한계 |
|---|---|---|
| live `main` | `git ls-remote origin refs/heads/main` → `6917e476e2037f649a6620ce422cbf0a9ebfb5cb`; 같은 commit object의 `git show -s`도 같은 SHA와 제목 `fix(sema): align nullability with SQL predicates`를 반환했다. 아래 제품 파일은 이 SHA에서 읽었다. | 정본 checkout은 `release/aip-1.0-resume-20261031`의 사용자/다른 작업 변경으로 dirty였고 HEAD는 `c4a0f469c6921b645d6f2ac17cec5d5cfec0fa57`였다. 그 checkout은 편집·빌드·테스트하지 않았다. `python3 .aip-harness/check.py`는 상태와 디스크를 출력했으며 `remote_check: not_run`이다. 따라서 live SHA의 source는 commit object로 검증하고, dirty checkout의 파일을 main 증거로 사용하지 않았다. |
| B/D | 이 문서의 §2–6은 당시 실행/판정, §7은 branch 작업자의 제한된 SQL-owned retry 변경과 no-DB 컴파일을 보존한다. `audit/semantic-bd-tx-ext-20261116` 문서 HEAD는 `af52f1c2c1310911f6bf86a84e12cbf2dc044c94`; §7의 DB GREEN은 없고 C 소유 실행 대기다. | §7의 코드는 live main에 들어간 구현으로 취급하지 않는다. 그 CI 전 단계의 40P01 기록은 `22683321` 회차의 증거이며, retry 후 DB GREEN으로 확대하지 않는다. caller-owned transaction 및 외부 callback의 재실행 한계만 경계 판정에 재사용한다. |
| A/C | A 문서는 `f1c6696` 기반에서 Core 집계 NULL 문제와 service/Core 경계·DB 미실행을 분리한다. C 문서는 `f1c6696` 기반의 별도 branch 기록과 GitHub Actions head `57655d1fdf63ce830ad8b356a37aef345c7a10f0`에서 S-01/S-17 및 CLI baseline 실행을 기록한다. | 그 CI는 명시된 branch head와 PostgreSQL fixture의 결과다. 현재 `6917e476`에서 다시 실행한 결과나 세 앱 전체 의미 동등성으로 세지 않는다. 제품 선언·호출 경로는 아래에 다시 대조했다. C의 “새 결함 0건”은 그 테스트 범위의 관측이지 전체 권한 보증이 아니다. |

현재 main에서 대조한 핵심 제품 파일은 다음과 같다.

- FaceReview: `porting/facereview/ui-overlay/src/api/{readAdapter,writeAdapter,adminAdapter,adminReadAdapter,aipClient}.ts`, `porting/facereview/ui-overlay/README.md`, test definition fixture `product/tests/fixtures/facereview-mysql/facereview.aip`, worker fixtures `product/tests/fixtures/facereview-mysql/extensions/`.
- FlaskBB: `porting/flaskbb/app.aip`, `porting/flaskbb/ui/app.py`, `porting/flaskbb/mail/flaskbb_mail.py`, `product/python/aip_sdk/__init__.py`.
- Infisical: `porting/infisical/app.aip`, `porting/infisical/workers/infisical_jobs.py`, `porting/infisical/tests/test_encryption_contract.py`, `plan-docs/porting/INFISICAL-GAPS.md`.
- 제품 경계 확인: `crates/aip-service/src/lib.rs`, `crates/aip-service/src/mysql_jobs.rs`, TypeScript SDK `product/sdk/index.ts`, Python SDK `product/python/aip_sdk/__init__.py`. AIP service가 일부 `spike-*` 실행 모듈을 직접 의존한다는 사실은 제품 호출 경로 근거이며, `aip-cli` PoC/Core test를 service 실행 증거로 바꾸지 않는다.

FaceReview의 `.aip` 선언은 main에 제품 정의 파일로 포함돼 있지 않다. 여기 인용한 `product/tests/fixtures/facereview-mysql/facereview.aip`는 서비스/확장 시험용 정의다. UI overlay의 실제 SDK 호출은 현재 main에서 확인했지만 그 호출이 운영 배포에 쓰인 정의와 정확히 같은지, 운영 authorization/facts와 DB driver까지 일치하는지는 이 checkout만으로 확인할 수 없다.

### 다섯 판단 단계 적용

아래 각 사례에서 ①기존 primitive 조합, ②primitive 결함, ③공식 확장 경계, ④반복되는 일반 패턴, ⑤신규 표준화 비용 순으로 판단했다. 이 읽기 전용 대조에서는 현재 live main의 Core/service primitive 결함을 새로 재현하지 않았다. “결함”은 현재 source에서 잘못된 동작이 확인된 경우에만 썼고, 미실행·외부 정의 누락은 설계 선택 또는 검증 공백으로 분리했다.

#### FaceReview

1. **기존 조합:** `readAdapter.ts`는 `Video.homeSearch`, `Video.homeVideoAll`, `Video.watchDetail` 같은 `client.extension(name,input)` 호출과 typed SDK `read`/`apply`를 함께 사용한다. `writeAdapter.ts`는 bookmark/like/comment CRUD에 `apply`와 `bundle({atomic:[...]})`, 추천 요청에 `writeExtension`을 사용한다. `adminAdapter.ts`는 선언 transition `apply`와 `approveVideoRequest` job의 `enqueue → status → result`를 나눈다. CRUD·상태 변경·atomic SQL bundle은 기존 primitive로 처리되고, UI의 기존 route envelope 재조립은 TS adapter에서 담당한다.
2. **primitive 결함 판정:** AIP core primitive 결함을 새로 재현하진 않았다. `getVideoDetail`은 `Video.watchDetail` read extension으로 상세 응답을 만든 뒤 `Video.view` transition을 별도 `apply` 요청으로 실행한다. adapter는 거절 응답이나 예외를 warning으로 남기고 상세 응답은 반환한다. 이는 adapter 코드에서 확인되는 read→별도 apply 및 실패 시 부분 성공 동작이다. 원본 read 중 view count 변경과 전체 요청 실패 가능성은 adapter 주석에만 적혀 있으며, 이 감사에서 원본 구현/계약과 대조하지 않았다. 따라서 이는 원본 parity를 확인해야 할 후보이고, 정상·수용된 호환 선택으로 판정하지 않는다. 기존 자산은 `porting/facereview/ui-overlay/tests/readAdapter.test.ts`(호출 순서·warning/상세 반환 단위 테스트), `product/tests/fixtures/facereview-mysql/reference/slice13f_oracle.py`와 `product/tests/fixtures/facereview-mysql/slice13f-golden.json`(포팅 read/apply 대조 자산)이다. 이 자산들은 본 작업에서 실행하지 않았으며 adapter의 실패 시 원본 계약은 여전히 미검증이다. 우선순위는 P2 원본 parity 및 UI/지표 영향 검증 후보로 둔다.

   `getEmotionSummary`는 매 호출 새 key로 `writeExtension('User.myEmotionSummary',...)`를 보낸다. 그러나 새 key만으로 누적 중복을 추론할 수 없다. fixture worker는 `last_finalized_at`/`last_session_id` checkpoint 이후 통계를 요청하고, 기존 checkpoint가 있으며 새 stats checkpoint가 없으면 현재 합계를 그대로 반환하며, 새 합계와 checkpoint는 `expect: {lock_version: ...}`로 적용한다. 이는 코드에 구현된 checkpoint 및 동시 수정 거절 경계이며, 실제 원본 parity 결과는 아니다. 원본과의 parity, 재호출·늦게 도착한 session·동시 호출 결과는 실행해 확인하지 않았다. 기존 비교 자산은 `product/tests/fixtures/facereview-mysql/reference/slice14f_oracle.py`, `product/tests/fixtures/facereview-mysql/slice14f-golden.json`, `product/tests/fixtures/facereview-mysql/slice14f-sessions.json`이며, oracle은 반복 호출·새 session 한 번 반영·동시 호출을 포함한다. 이 자산들은 검증 후보로만 기록한다.
3. **공식 확장:** fixture 선언은 `User.myEmotionSummary`의 write extension(선언 입력/출력과 worker implementation), 여러 `Video.*` read extension, `VideoRequest.recommend`, 그리고 `approveVideoRequest` durable job을 구별한다. TS 호출부는 `extension`, `writeExtension`, `jobs.enqueue/status/result`를 호출한다. 추천·개인화·Mongo 기반 감정 타임라인·외부 영상 provider처럼 relational read/transition만으로 대체할 수 없는 로직은 extension/job에 자연스럽다. 단, approve job 입력은 request id와 관리자가 제공한 제목/채널/길이/category이고 fixture access는 VideoRequest/Video 생성·accept와 Mongo capability다. adapter 주석대로 이 job은 YouTube API 호출을 포함하지 않는다. 운영 선언은 미확인이다.
4. **반복 패턴:** 여러 읽기 함수가 서버 extension의 scalar output에서 JSON text(`videos`, `categories`, `body`, `timeline_data`)를 다시 파싱해 원래 UI DTO로 바꾼다. admin read adapter도 여러 aggregate/extension을 모아 기존 API envelope를 조립한다. 이는 app adapter 층에서 반복되지만 “모든 응답을 JSON scalar로 넣는” 일반 protocol 필요성은 이 사례만으로 입증되지 않는다. SDK typed `read`와 standalone `aggregate`를 이미 사용하는 경로는 별도 유지한다.
5. **신규 표준화 비용:** 공통 DTO/JSON envelope wrapper를 core DSL에 넣으면 transport schema, 타입 생성, nullability, 오류 매핑, 기존 generated SDK 호환을 함께 바꿔야 한다. 현 증거는 한 앱의 route compatibility adapter에 집중돼 있어 비용 대비 이점이 낮다. 반복은 앱 adapter의 작은 helper로 관리하고, 두 번째 독립 앱에서 같은 shape 부족과 실제 소비를 확인하기 전 새 표준화를 P3/보류로 둔다.

#### FlaskBB

1. **기존 조합:** `app.aip`의 resource policy·`expose read/create`·transition·bounded multi-row effect·aggregate를 `ui/app.py`의 Python SDK `read/apply/bundle` 호출이 사용한다. 예를 들어 Topic/Post moderation과 Forum/Topic read projection은 선언형이다. 생성 예로 `/topic` route는 `Topic`과 `Post` id를 먼저 준비하고, `Topic` 생성·`Post` 생성·`Post.publish`·`TopicFirstPost` 생성·`Topic.publish` 다섯 operation을 `client.bundle`에 넣어 `submit_bundle`로 처리한다. 관계·row policy 및 같은 DB 안의 원자성은 custom route를 proxy하지 않고 primitive로 다룬다.
2. **primitive 결함 판정:** 현재 main의 primitive 결함은 재현되지 않았다. Infisical G11은 environment와 root folder를 원본에서는 함께 만들지만 현재 port에서 두 요청으로 나누는 사례다. 반면 FlaskBB `ui/app.py`는 Topic/Post id를 미리 준비해 Topic·Post·TopicFirstPost 생성과 Post/Topic publish를 한 bundle에 넣는다. 따라서 “parent-child 생성은 primitive가 없다”는 일반 결론은 성립하지 않는다. Infisical의 남은 차이는 생성된 Environment ID를 후속 Folder 생성에 전달하면서 한 작업으로 authorization·rollback·idempotency를 보존하는 구체 표현 문제다. 현재 live DB journey로 중간 실패를 재현하진 않아 설계 공백/검증 항목으로 분류한다. FlaskBB G-6의 source order/PostOrder mapping과 `latest` projection 다중 필드 선언은 “추상화가 덜 마음에 든다”는 이유만으로 결함 취급하지 않는다. 원본 정수 순서 보존 문제와 선언/테스트 mismatch가 별도 증명돼야 한다.
3. **공식 확장:** `app.aip`의 `sendActivationEmail`·`sendPasswordResetEmail` job은 bounded input/output, `allow`에서 actor 본인 주소/사용자명을 제한하고, `access http.mail`, `effect managed`, deadline, `implementation`을 선언한다. `mail/flaskbb_mail.py`가 외부 메일 실행을 구현한다. 현재 main의 실제 SDK enqueue 호출은 `tests/mail-jobs.test.mjs`의 `alice.jobs.enqueue` journey에서 확인된다. `ui/app.py`에는 production UI에서 이 두 job을 enqueue하는 호출이 없어 운영 UI 연결은 미확인이다. 외부 메일은 SQL transaction에 원자적으로 묶이지 않는다. `effect managed`는 job 실행 경계·기록을 뜻하지 SMTP와 DB의 분산 원자성을 뜻하지 않는다. 이외 forum CRUD는 확장보다 선언형이 적절하다.
4. **반복 패턴:** `ui/app.py`에는 같은 resource별 query/filter/response shaping 호출이 여러 route에서 반복되고 `.aip`에는 유사한 ownership/policy predicates와 per-resource budget이 있다. 이 반복은 typed client/helper와 도메인 predicate 재사용으로 줄일 수 있는 앱 계층 패턴이다. 이름만 다른 요청을 하나의 범용 DSL operation으로 합칠 근거는 아니다.
5. **신규 표준화 비용:** FlaskBB가 보여 주듯 id를 caller가 미리 정할 수 있는 관련 생성은 현재 atomic bundle로 처리된다. Infisical의 G11은 server-generated Environment ID를 후속 Folder input으로 넘겨야 하는 사례여서, 일반 graph-create를 도입하면 임시 ID 참조, 각 row의 `allow`, create ordering, 실패 원자성, 반환 ID, 재시도·idempotency를 명세해야 한다. 현재 확인된 미충족 수요는 Infisical 한 곳이며 공통 primitive가 최소인지 입증되지 않았다. 우선순위 P2 source/DB 재현 조사, 즉시 primitive 추가/기존 호출 변경 없음.

#### Infisical

1. **기존 조합:** `app.aip`는 Org/Project/Environment/Folder/Secret/PersonalSecret 관계, `rows read`와 write predicates, uniqueness, CRUD transitions, 제한된 multi-row delete, read projection, `secretReminder` job을 선언한다. 이 구조·authorization slice는 기존 DSL로 표현된다. secret value 자체는 활성 정의에서 의도적으로 제외돼 있다.
2. **primitive 결함 판정:** 실행 가능한 primitive 결함은 확인하지 않았다. G2의 암호화 저장·읽기·버전 이력·키 회전 통합은 구현되지 않았으며 main의 `test_encryption_contract.py`는 experimental encrypted fixture의 check advisory와 init/serve 차단, 금지된 modifier 거절을 확인하는 정적/경계 test다. 이는 암호 primitive 존재와 제품 feature 완료를 혼동하지 않는 명시적 scope다. G14의 임의 깊이 force-delete, G16의 시간 산술/position 재정렬은 현재 표현의 부족 또는 명시적 기능 미포팅이고, G11 root folder 두 번째 create는 부분상태 경계다. 이 셋은 본 감사에서 실행된 service DB 재현이 아니므로 결함으로 승격하지 않는다.
3. **공식 확장:** `secretReminder`는 input `Secret.Id`, output count, actor/secret visibility `allow`, `access read.Secret, Secret.remind`, `effect managed`, deadline, Python implementation을 선언한다. 구현(`workers/infisical_jobs.py`)은 reminder 상태를 기록하는 흐름이며 주석의 선언대로 메일/Slack 전달은 포함하지 않는다. main의 enqueue 호출은 `tests/live_harness.py` 및 `tools/baseline_client.py`/live DB test harness에서 확인되고, 사용자 UI의 production enqueue 경로는 확인되지 않았다. secret value 암호화는 이 worker 확장이 해결하는 문제가 아니다. 값 저장·복호화는 data-plane에서 호출되는 provider/crypto capability와 인가·저장·감사·키 lifecycle 계약이 필요하며 별도 설계/통합 검증 범위다.
4. **반복 패턴:** canReadProject/canWriteProject 같은 경계가 여러 resource와 transition에서 반복되고, project→environment→folder→secret 트리의 동일 authorization 조건이 누적된다. 공통 predicate 재사용은 선언 수준에서 이미 보인다. 반복 delete shapes(Secret + PersonalSecret 등)는 관계별 cascade와 보존 조건이 다르므로 단순 문법 축약의 대상이라고 단정하지 않는다.
5. **신규 표준화 비용:** encryption primitive를 활성화하면 serialization, searchable/indexed 조회, 비밀값 노출 정책, 감사·journal 평문, unique 제약, rotation/retry, rollback 및 구버전 호환까지 영향을 받는다. 현재 `docs/design/13-field-encryption.md`는 Proposed이고 활성 정의는 runtime gate로 거부된다. 비용은 높고 파급이 커 P1은 설계 승인/위험 표면 닫기, P0 변경이나 gate 해제는 없음. 재귀 delete와 time arithmetic도 업무 의미·상한·복구를 먼저 정하지 않고 일반 연산자를 추가할 근거는 없다.

### 확장 계약과 DSL 경계

| 경로 | 입력·출력 | 인가 | 데이터베이스 범위·transaction | 외부 효과 |
|---|---|---|---|---|
| 선언형 `read`/`apply`/`bundle` | `.aip`의 resource field, filter, select, transition 입력·결과. TS/Python SDK가 wire 응답을 소비한다. | resource `rows read`, transition `allow/from`, operation의 명시 access. | service가 SQL 요청을 수행한다. `bundle`/owned apply의 DB 변경·audit/idempotency는 같은 요청 transaction 경계 안에서 처리한다. | 직접 외부 provider 호출을 뜻하지 않는다. |
| read extension (FaceReview) | TS adapter가 이름·input을 보내고 scalar output을 JSON text로 풀어 기존 UI DTO에 맞춘다. fixture operation declaration은 테스트 정의다. | 정의의 `allow`/`access`가 경계여야 하나 production definition은 이 checkout에서 미확인이다. | fixture implementation은 relational DB 및 Mongo read 등 선언 capability를 쓸 수 있다. read와 별도 `apply`를 호출하는 watch count 경로는 한 transaction이 아니다. | 외부 YouTube/provider 호출 여부는 operation 별도. approve job 주석은 YouTube 호출을 제외한다. |
| write extension (FaceReview) | `writeAdapter`/`readAdapter`가 input과 idempotency key를 보낸다. `myEmotionSummary`의 response `body`는 JSON text다. | 실제 운영 operation의 `allow`/`access`는 현재 checkout 미확인. UI의 member guard는 서버 인가 대체물이 아니다. | V6 HTTP extension 경로는 일반 owned `/apply` retry와 다르며 외부 callback을 통째로 재실행하지 않는다. fixture worker는 checkpoint 이후 새 통계가 없으면 현재 합계를 반환하고, 합계·checkpoint 변경은 `expect lock_version`으로 적용한다. DB callback atomicity는 구현 contract에 제한된다. | 매 호출 새 key라는 사실만으로 중복 누적을 뜻하지 않는다. worker의 원본 대비 반복 호출·새 세션·동시 호출 parity 및 production definition은 이 감사에서 미검증이다. |
| durable job (FaceReview / FlaskBB / Infisical) | 선언의 typed input/output, deadline, implementation name. FaceReview admin client는 enqueue→status→result, Python은 SDK `jobs` client를 제공한다. | enqueue 가능한 operation과 worker access/principal은 별도 policy 계약이다. Fixture에서 admin/service 호출을 실제 운영 정의로 일반화하지 않는다. | job state/journal 및 worker 실행 tx는 DB 변경과 조정되지만 분리된 외부 시스템과 원자 commit이 되지 않는다. caller-owned transaction callback은 자동 재시도에서 제외한다(§7). | FlaskBB mail은 SMTP/provider 외부 효과. Infisical reminder는 outbox 상태 기록뿐이고 mail/Slack 전달은 하지 않는다. |

따라서 DSL의 기본선은 관계·권한·조건부 상태 전이·요청 단위 SQL atomicity·제한된 aggregate/read 계획이다. 외부 provider, 비관계 계산, arbitrary legacy response 조립, 장기 실행 작업은 선언된 access/입출력/실행 효과 경계를 가진 공식 extension/job 쪽이 자연스럽다. Extension은 업무 controller를 무제한 proxy하는 통로가 아니며, 이름/input/output이 선언과 맞고 actor/access·deadline·idempotency·외부 효과의 재실행 특성을 별도로 검토해야 한다.

SDK와 worker 언어도 구분한다. FaceReview overlay의 TypeScript와 FlaskBB/Infisical의 Python 코드는 공식 SDK client이며, worker implementation은 `.aip`의 `implementation` 이름으로 연결된다. current main `crates/aip-service/src/mysql_jobs.rs` worker bundle은 `.py`와 `.mjs`를 포함한다. 따라서 TS client 호출은 TS server extension implementation이 존재한다는 증거가 아니다. FlaskBB·Infisical은 Python worker 파일과 operation declaration을 확인했으며, FaceReview fixture의 worker implementations는 Python 파일이다.

### 과도한 조합, 의미 통합 위험, 우선순위

| 사례 | 분류 | 재현성·호환 영향 | 우선순위/현재 조치 |
|---|---|---|---|
| FaceReview GET 화면 호출 후 read+view apply 두 요청 | adapter 코드상 확인된 별도 요청·부분 성공; 원본 parity 미검증 | 실패 응답/예외에서 warning 후 detail을 반환한다. adapter 주석은 원본의 read 중 view count 변경 및 전체 요청 실패 가능성을 말하지만 독립 원본 근거는 확인하지 않았다. `readAdapter.test.ts`는 adapter 단위 자산이며 `reference/slice13f_oracle.py`·`slice13f-golden.json`은 포팅 비교 자산이다. 이번엔 실행하지 않았다. | P2 후보: 원본 계약 및 UI/지표 영향 검증. 호환 차이를 정상·수용된 것으로 판정하지 않음. |
| FaceReview legacy GET의 `myEmotionSummary` write extension | checkpoint와 optimistic lock이 있는 write; 원본 parity 미검증 | 매 호출 새 key는 확인되나 그 자체로 중복 누적을 뜻하지 않는다. worker는 `last_finalized_at`/`last_session_id` checkpoint, 새 stats 없음 반환, `expect lock_version`을 사용한다. 기존 `reference/slice14f_oracle.py`·`slice14f-golden.json`·`slice14f-sessions.json`에 반복·늦은 session·동시 호출 자산이 있으나 이번엔 실행하지 않았다. | P2 후보: 기존 오라클 자산으로 원본 대비 반복/신규/경합 계약을 검증. 중복 누적 주장은 보류. |
| Infisical Environment + root Folder 두 요청 | 앱 한 곳의 expressiveness/verification 후보 | existing G11은 중간 상태를 기록한다. FlaskBB `ui/app.py`의 bundle은 caller-preassigned Topic/Post id로 두 row와 TopicFirstPost를 만들고 Post/Topic을 publish하는 반례다. 이번엔 Infisical current-main DB/SDK journey에서 두 번째 요청 실패를 재현하지 않았다. | P2 조사. generated-ID threading·Folder authorization·rollback·retry contract를 재현하기 전 새 compound primitive 없음. |
| FlaskBB 다수 `latest`/projection 및 Infisical bounded multi-delete | 선언 조합의 복잡성, 현재 source상 의도 | 반복은 읽히기 어렵고 상한/순서/NULL 의미를 함께 담지만 특정 잘못된 SQL/결과를 현재 실행으로 재현하지 않았다. 단순 축약은 field별 기준·row policy·부분 삭제 거절을 합쳐 의미를 바꿀 수 있다. | P2 readability/source-map 및 계약 시험. 선언 제거/통합 없음. |
| Infisical 암호화 미포함·G14 재귀 삭제·G16 시간/position 의미 | 범위 결정 또는 미충족 capability, 실행 결함으로 미확정 | encrypted runtime gate는 의도된 fail-closed 경로. 삭제·시각 산술은 적용 경로에서 실행해 parity를 검증하지 않았고, current docs는 proposed/unsupported로 분리한다. | G2는 P1 설계/제품 승인과 역호환 분석, G14/G16은 요구별 우선순위 산정. gate 해제·권한 완화 없음. |
| C aggregate source policy, A Core aggregate NULL | 검증된 경계 주의와 이전 revision의 Core 결함 후보 | C S-01/S-17 실행은 정확한 branch head의 PG test다. A Core NULL 결과는 service path 결함으로 확인되지 않았다. `callerFilter` aggregate가 row policy를 자동 상속한다고 가정하지 말아야 하나 이번 단계에서 새 exploit/제품 결함을 재현하지 않았다. | P1 기존 C의 service `/read` aggregate matrix, A Core↔service 계약 비교. 선언을 한 의미로 자동 통합하지 않음. |

현재 확인한 반복 패턴은 “새 범용 프레임워크”보다 앱 adapter/SDK helper에서 먼저 처리할 성격이다. Parent-child atomic creation 자체는 FlaskBB에서 기존 bundle로 확인됐지만 Infisical의 generated-ID 전달과 같은 의미라고 볼 수 없다. operation 결과 JSON을 richer type로 바꾸거나 extension을 일반 handler로 통합하는 변경은 generated SDK 및 기존 definition/wire 호환 영향을 조사하지 않아 이번 결론에서 제안하지 않는다.

### 검증 및 미실행 범위

- live main 대조: `git ls-remote origin refs/heads/main` 출력 SHA와 `git show -s --format='%H %s' 6917e476e2037f649a6620ce422cbf0a9ebfb5cb`의 commit SHA가 같다. 주요 제품 파일을 그 commit으로 `git show`/`git grep`하여 호출·선언 이름을 확인했다.
- 문서의 과거 상태 대조: A/C 이전 head `f1c6696`, C CI head `57655d1…`, B/D 문서 head `af52f1c…`, B/D 후속 CI evidence head `22683321…`를 서로 다른 revision으로 기록했다. 과거 문서의 실행 결과를 `6917e476`의 신규 실행으로 옮기지 않았다.
- 읽은 호출/선언: `readAdapter.ts`의 `Video.watchDetail`→`apply Video.view`, `User.myEmotionSummary` write extension; FaceReview의 기존 비교 자산 `porting/facereview/ui-overlay/tests/readAdapter.test.ts`, `product/tests/fixtures/facereview-mysql/reference/slice13f_oracle.py`, `slice13f-golden.json`, `reference/slice14f_oracle.py`, `slice14f-golden.json`, `slice14f-sessions.json`은 경로만 확인했으며 이번엔 실행하지 않음; `writeAdapter.ts`의 atomic bundle 및 `VideoRequest.recommend`; `adminAdapter.ts`의 `approveVideoRequest`; FaceReview test fixture의 대응 operation/implementation; FlaskBB `ui/app.py`의 Topic+Post atomic bundle, mail job definition/implementation 및 `tests/mail-jobs.test.mjs`의 enqueue call; Infisical `secretReminder` definition/implementation 및 test/tool enqueue call; Infisical root folder G11과 G2/G14/G16 경계.
- 실행하지 않은 것: 어떤 앱의 DB GREEN, service/HTTP/SDK journey, 브라우저, worker/provider, CI 재실행, `initdb`, clone/target/container/DB 생성 모두 미실행이다. DB GREEN은 C 소유로 남겼고 실행 중 CI를 조회·취소하지 않았다. source audit이므로 실행을 하지 않았으며, 구현 의도·production FaceReview definition 및 실제 운영 actor/access는 unverified다.
- 제품 code, DSL 문법, main 및 dirty release checkout은 수정하지 않았다. 이 절은 감사 브랜치의 문서 전용 변경이다.
