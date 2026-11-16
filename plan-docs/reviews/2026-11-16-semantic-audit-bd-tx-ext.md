# 의미 감사 B/D: 트랜잭션·동시성·확장·외부 효과

- 기준 브랜치: `main`에서 분기한 `audit/semantic-bd-tx-ext-20261116`
- 감사 범위: 제품 서비스가 실제로 호출하는 쓰기 경로와 durable execution의 계약
- 수정 범위: 제품 동작 변경 없이 감사 결과와 최소 재현 테스트만 추가
- 판정 시각: 2026-11-16 (KST)
- 상태: **부분 완료**. 기존 PostgreSQL 기준선에서 lock-order 회귀의 RED를 관측했지만, 현재 세션에서는 테스트 DSN이 제공되지 않아 교정된 동시 멱등 회귀와 후속 확인을 실행하지 못했다.

## 1. 제품 경로와 PoC 구분

이 저장소의 제품 진입점은 `aip service serve`다. `crates/aip-service/Cargo.toml`은 `spike-v6-transport`를 의존하고, `crates/aip-service/src/lib.rs:1697-1710`에서 실제 listener를 구성한다. V6의 `/apply`는 `spikes/spike-v6-transport/src/lib.rs:188-219,246-275`에서 전이·atomic bundle·compose를 하나의 요청 트랜잭션으로 라우팅하고, atomic 경로는 `spike_v3_write::bundle_in`을 호출한다. Durable job은 `spikes/spike-v6-transport/src/durable.rs:1418`에서 같은 V3 write 엔진의 `apply_in_with_wire`를 호출한다.

즉 이 구현은 디렉터리 이름이 `spikes/`인 모듈을 제품 서비스가 직접 연결한다. 그 경로에 붙인 회귀는 단순 prototype 전용 결과로 치부할 수 없다. 다만 이번 새 재현은 내부 `bundle`/`handle`을 직접 호출하며 `aip service serve`의 설정·인증 wrapper 전체를 통과하지는 않는다. 제품 서비스가 같은 함수를 호출한다는 의존 경로를 확인한 범위의 증거다.

## 2. 우선순위 결과

### P2 — atomic bundle의 요청 순서가 교착을 만들 수 있음 (확인됨)

- **문제:** 하나의 atomic 요청이 두 행을 `[30,31]` 순서로 잠그고, 동시 요청이 같은 행을 `[31,30]` 순서로 잠그면 양쪽이 서로의 두 번째 행을 기다릴 수 있다.
- **영향:** PostgreSQL이 한 트랜잭션을 교착 희생자로 중단한다. 해당 요청은 모두 롤백되지만 보통의 READ COMMITTED 명령은 자동 재시도하지 않아 정상 업무 경쟁이 `WIRE.CONFLICT`로 끝날 수 있다. 데이터 손상은 확인되지 않았다.
- **근거/원인:** 설계는 `docs/design/05-execution-model.md` §3.1에서 한 명령의 전체 행 잠금을 전역 순서로 획득한다고 약속한다. 반면 `spikes/spike-v3-write/src/lib.rs:1586-1617`의 `bundle_run`은 연산을 요청 순서대로 돌고, 각 `apply_op`의 `judge_query`는 개별 전이 대상만 `ORDER BY t.id FOR UPDATE`로 잠근다 (`:401-416,1431-1450`). bundle 전체 대상 행의 선행 전역 잠금 단계가 없다. 두 경로는 V6의 제품 `/apply`에서 호출된다.
- **다른 경로 대조:** 단일 전이는 대상 집합을 `id` 오름차순으로 잠그므로 이 재현의 역순 교착 유형은 없다. `compose`와 V6 write extension의 `bundle_in`도 순차 전이 호출을 공유하므로 동일 유형의 잠금 순서 위험이 남는다. 이 확장 경로의 반례는 이번에 별도 실행하지 않았다.
- **권고(구현 안 함):** bundle/compose의 대상 집합을 쓰기 전에 계산해 리소스 전역 순위와 id 순으로 잠근 다음 단계들을 실행한다. 정책상 잠그면 안 되는 행이 대기하거나 노출되지 않도록 현재 행 가시성·권한 필터를 유지하는 계획이 필요하다. 구현은 중간 난이도이며 전체 재설계는 필요해 보이지 않는다.
- **RED:** 기존 `/tmp/aip-baseline-engines.log`에서 `cargo test --test audit_bd_lock_order -- --nocapture` 실행 대상이 `atomic_bundles_touching_the_same_rows_in_opposite_order_do_not_deadlock ... FAILED`, `0 passed; 1 failed`로 종료했다. 기존 테스트의 실패 위치는 `tests/audit_bd_lock_order.rs:72`이고, 그 단언은 결과에 `WIRE.CONFLICT`가 하나라도 있으면 실패한다. 이 테스트는 실제 PostgreSQL 두 연결을 쓴다. 현재 파일은 `spikes/spike-v3-write/tests/audit_bd_lock_order.rs`다.
- **수정 난이도:** 중간. 잠금 후보를 계산할 때 전이 순서·행 정책·bulk 상한·생성 후 참조 의미를 보존해야 한다. 대규모 재설계 필요성은 확인되지 않았다.

### 멱등 동시성 — 결함 확인 안 됨, 추가 검증 미완

동일 principal/key는 advisory lock으로 직렬화되고, 저장된 동일 요청은 결과를 재생하며 다른 본문은 `WIRE.IDEMPOTENCY_MISMATCH`로 거부한다 (`spikes/spike-v6-transport/src/lib.rs:307-327`). 커밋 뒤 응답 유실 경로는 기존 `commit_unknown_wire`에서 13건 통과했다. 다만 기준선에 추가했던 12연결 테스트의 두 번째 race가 서로 다른 두 업무 요청(Counter.bump와 Booking.confirm)에 같은 key를 재사용해 12개 모두 mismatch를 받은 것은 테스트 오류다. 본문을 고치면 별도 키를 사용하도록 수정했다. 첫 번째 READ COMMITTED race는 기존 기준선에서 예상한 1회 실행·11회 재생을 통과했으나, 수정 후 두 isolation 경로 전체는 현재 PostgreSQL 환경 제약으로 재실행하지 못했다. 결함으로 세지 않는다.

## 3. 실행 의미 항목별 판정

`검증됨`은 이번 요청의 기존 실제 실행 출력 또는 해당 경로를 겨냥한 기존 실행 테스트가 기준선에서 통과한 경우다. 문서만 존재하거나 이번 세션에서 실행하지 않은 것은 결함 확인으로 올리지 않았다.

| 항목 | 판정 | 코드·기존 검사와 판단 |
|---|---|---|
| 복합 변경 원자성 | 검증됨, 범위 한정 | `bundle_once` 한 트랜잭션과 `bundle_in`/`checks_in` 호출 경로: `spike-v3-write/src/lib.rs:1543-1557,1575-1583`; 실패 시 트랜잭션 drop/rollback. 기존 V3 bundle·commit fault 계열과 V6 HTTP 경로가 있음. 동일 atomic 요청의 전체 서비스 wrapper 변형은 이번에 새로 실행하지 않음. |
| 동시 수정 충돌 | 부분 검증 | 단일 행 잠금과 absence 의존 SERIALIZABLE·재시도 경로가 있음. 기존 `absence_serializable_http` 1건 통과. 다중 행 bundle 역순은 아래 lock-order 결함. |
| 잠금 순서·데드락 | **결함 확인** | 위 P2. 설계의 전역 순서 약속과 atomic 실행 순서가 불일치. |
| 낙관적 동시성 | 의미 불명확 / 검사 공백 | 요청 `expect` 검사가 동시성 제어에 어떻게 대응하는지 별도 코드·제품 계약 정합성 판정을 끝내지 못했다. 잠금은 pessimistic이며, 별도 version/ETag 경로의 보장으로 확대하지 않는다. |
| 실패 시 롤백 범위 | 검증됨, DB 범위 | V3 commit-classification tests는 COMMIT 전 실패와 결과 불명 상태를 구분한다. extension atomic test도 callback 오류·deadline·출력 오류 롤백을 겨냥한다. 외부 시스템 효과까지 DB rollback 된다고 보장하지 않는다. |
| commit 후 응답 유실 | 검증됨 | `/apply`는 commit 뒤 연결·응답을 분류하고 같은 key의 재요청으로 결과를 재생한다. 기존 V6 `commit_unknown_wire`: 13 passed. |
| 멱등 키 재사용·결과 재생 | 부분 검증 | 순차 동일 key 재생·다른 본문 거부 경로가 있고 기존 commit-unknown test 통과. corrected 12-connection race는 새로 실행되지 않음. |
| 확장 입력·출력 계약 | 기존 테스트 통과 | 선언된 입력·결과 검증·잘못된 출력 거부의 기존 `write_extension_atomic`(1), `write_extensions_http`(2), `write_extension_commit_classification`(12) 테스트가 기준선 통과. 이는 현재 실행 로그의 결과이며 추가 바운더리 탐색은 미완. |
| 확장의 데이터 접근 범위 | 기존 테스트 통과 | `access`로 허용된 전이/DB 작업만 host callback에 전달되는 경로. `write_extensions_http` 및 `write_extension_atomic` 실행 통과. 모든 드라이버 capability를 포괄했다는 의미는 아님. |
| 확장과 DB 트랜잭션 경계 | 기존 테스트 통과 | write extension은 V3 transition과 audit/idempotency를 같은 tx에 둠. 기준선 `write_extension_atomic` 1건, `write_extension_commit_classification` 12건 통과. 외부 provider는 분리됨. |
| Job 재시도·중복 실행 | 기존 테스트 통과, 조합 공백 | `job_retry` 1건과 `job_breaker` 2건 통과. job API idempotency 및 effect journal이 있음. 여러 worker·provider 결과 불명·lease 만료가 교차하는 모든 조합은 미검증. |
| 외부 API 결과 불명확성 | 기존 테스트 통과 | provider 전송과 DB commit은 원자적이지 않으며 UNKNOWN을 자동 재전송하지 않는 계약. `job_breaker` 2건과 local mock `outbound_webhook` 1건 통과. 실제 외부 전송은 하지 않음. |
| Outbox·Webhook 전달 의미 | 기존 테스트 통과, 중복 허용 | outbox pending dispatch와 서명 webhook/local mock 경로. 기준선 `outbox` 2건, `outbound_webhook` 1건 통과. 외부 consumer의 정확히 한 번 전달을 의미하지 않음. |
| 실패 후 보상·복구 | 부분 / 의미 범위 제한 | durable job의 알려진 실패는 재시도·분기 가능하고 UNKNOWN은 멈춘다. saga가 필요한 선행 외부 효과의 lookup·compensation 전체 복구는 현재 제품 경로에서 판정을 마치지 못했다. 설계 문서만 근거로 구현 완료 처리하지 않음. |

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

- 기존 기준선 `/tmp/aip-baseline-engines.log`: lock-order test 실패(1/1); `commit_unknown_wire` 13/13, `job_retry` 1/1, `outbox` 2/2, `outbound_webhook` 1/1, write-extension atomic 1/1 및 commit classification 12/12 통과.
- 전체 baseline 로그에는 서로 다른 engine별 실패가 있다: V3 3 targets(lock-order, latest effect, nullable projection), V4 3 targets(native capability/principal fixtures), V6 1 target(잘못된 key 재사용 race), 그리고 product test sweep의 6 targets. 마지막 그룹에서 로그에 확인되는 이름은 V12–V16이며 실패 원인 전문은 이 보고서에 재분류하지 않았다. 감사와 무관한 latest projection·optional Mongo/Redis·principal fixture 실패를 B/D 결함으로 세지 않았다. 각 숫자는 각각의 Cargo invocation 결과이며 합쳐서 하나의 `6 targets failed`로 표현하지 않는다.
- 현재 세션의 재실행 시도 1: `cargo test -p spike-v3-write --test audit_bd_lock_order -- --nocapture`는 `package ... is not a member of the workspace`로 테스트 전에 종료했다. 올바른 crate 경로에서 다시 시도했다.
- 현재 세션의 재실행 시도 2: `cargo test --test audit_bd_lock_order -- --nocapture` (`spikes/spike-v3-write`): build script가 `AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults`로 종료했다. dedicated DSN이 설정되지 않았다.
- 대안 확인: `docker ps`는 `unix:///Users/winterholic/.orbstack/run/docker.sock` 없음으로 실패. 설치된 PostgreSQL 17로 임시 클러스터를 `/private/tmp`에 초기화하려 했으나 `initdb` bootstrap의 `shmget(...): Operation not permitted`로 종료했다. `--set=shared_memory_type=mmap` 지정으로 다른 구성을 한 번 더 시도해도 bootstrap이 같은 `shmget` 오류를 냈다. 임시 인스턴스는 시작되지 않았고 DB 테스트는 실행되지 않았다.
- corrected `audit_bd_idem_race.rs`는 source 수정만 했으며 **컴파일·실행 미검증**이다. 앞서 잘못된 key 사용으로 실패한 기록을 제품 실패 증거로 사용하지 않는다.

## 6. 수정안 (코드 변경 아님)

`bundle_run`이 operation list 전체의 write target과 순위를 수집할 수 있도록 transition별 target resolution을 분리한다. 모든 대상은 정책상 보이는 행만, 한 명령의 전역 순위와 id 순으로 선잠금한다. 그 뒤 기존 연산 순서로 정책·precondition·effects를 실행하되 다시 잠금을 획득하지 않거나 이미 획득한 잠금을 재사용한다. `bundle_in`·`compose_in`·write extension·durable job 호출에서 같은 계획을 공유한다. 테스트는 두 연결의 역순 multi-row bundle에서 성공 1개와 business refusal 1개, 전체 rollback을 확인하고, 숨김 행 누출·혼합 리소스·where 상한·같은 행 중복 요청을 반례로 둔다.

`2026-11-16` 검증에서 수정 전 RED는 확보됐지만 위 후보의 GREEN, corrected idempotency race, 전체 `cargo test --no-fail-fast`, port 별 독립 evidence 대조 및 원격 push SHA 검증은 완료되지 않았다. 제품 코드는 수정하지 않았다.
