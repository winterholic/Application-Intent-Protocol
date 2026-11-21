# 동시성·트랜잭션·멱등 재생 반례 탐색 (1.0 하드닝)

- 기준: `origin/main` 0c5d108d 위에서 시작했고, 수정은 78894ef4, cff81a17, 그리고 4.3절의 커밋이다. 측정일 2026-11-21.
- 범위: 동시 차감·증감의 경계, bundle 연산 순서와 교착, refs·compose 실패 시 부분 커밋, 멱등 key 재전송, 확장 `ctx.data.create`·`apply`의 직렬화 충돌, `WORKER_BUSY` 포화 상태의 재생. 권한 정책 반례와 계약 정합성은 다른 작업자 담당이라 제외했다.
- 환경: PostgreSQL 17.11(Docker `postgres:17.11`, `deadlock_timeout=1s`, 호스트 포트 55432), macOS arm64, HTTP는 `listen_with_prototype_deployment_launch`와 `start_with_apply_contract`가 띄운 실제 서버에 소켓으로 요청했다. 확장 worker는 Node, `WorkerIsolation::None`으로 실행했고 로컬 macOS에서 `WIRE.WORKER_FAILED`는 한 번도 나오지 않았다. 격리 모드(샌드박스) 확장은 이 환경에서 시험하지 않았다.
- 기존 보호 장치: 묶음 잠금 교착(dc68e4e6)과 완료된 멱등 응답 재생의 배포 fence(7cdfed93)를 같은 계열의 놓친 경로 기준으로 다시 찾았다.

## 1. 결론

| 구분 | 건수 | 내용 |
| --- | --- | --- |
| 결함, 수정함 | 3 | 줄 생성 뒤의 compose·`target.where` 전이 교착(78894ef4), SERIALIZABLE 확장의 직렬화 실패가 `WIRE.INTERNAL`로 샘(cff81a17), 확장 요청이 worker slot보다 DB 연결을 먼저 잡아 느린 DB에서 `WORKER_BUSY` 상한이 무력화됨(4.3절) |
| 설계 결정 필요, 고치지 않음 | 1 | 같은 행들을 서로 반대 순서로 잠그는 묶음(E9). 5절에 선택지와 영향 |
| 반례를 찾지 못함 | 나머지 | 3절 표의 "통과" 행 |

멱등 재생의 배포 fence는 `/apply` 일반 경로, `/status`, 확장 경로 모두 DB 재생 전에 적용됨을 확인했다. 같은 계열의 놓친 경로로 보이던 PostgreSQL 쪽 재생 경로(`handle_apply`, `/status`, `replay_completed`, 확장 `apply`의 claim 충돌 재생, durable `managed` 경로)는 모두 공통 fence 검사 뒤에 있다(앞의 넷은 시험, durable은 코드 읽기). MySQL relational 경로(`runtime.infrastructure.relational`)는 이 환경이 PostgreSQL뿐이라 시험하지 못했다(`미검증`).

## 2. 재현 방법

모든 시험은 `AIP_TEST_DATABASE_URL`과 `AIP_TEST_ADMIN_URL`이 가리키는 PostgreSQL 17이 있어야 한다. 탐색 시험은 `#[ignore]`이며 프로세스당 하나씩 돌린다(`sqlgen::set_schema`가 프로세스 전역이다).

```
P="cargo test --locked --manifest-path spikes/spike-v6-transport/Cargo.toml --features test-db --test concurrency_probe_http -- --ignored --nocapture --exact"
PROBE_ROUNDS=<반복 횟수> $P <시험 이름>
```

표의 "근거 명령"은 위 `$P` 뒤의 시험 이름이다. 라운드는 서로 독립이며 한 라운드는 모든 호출을 한꺼번에 보낸다.

## 3. 반례 표

"동시성"은 한 라운드에 동시에 보낸 요청 수다. 건수는 모두 관찰한 값이다.

### 3.1 bundle 연산 순서와 교착

| 반례 | 동시성 | 기대 | 실제 | 판정 | 근거 명령 |
| --- | --- | --- | --- | --- | --- |
| 줄 생성(정책이 상품 읽기) → compose가 같은 상품 갱신 | 6건 × 5라운드 | 6건 모두 성공, 수십 ms | 수정 전: 라운드마다 성공 2·1·3·0·1건, 나머지 `WIRE.DEADLINE_EXCEEDED`(2.0s). 수정 후: 5라운드 모두 6건 성공 29~46ms | 결함, 수정 | `a2_line_first_then_compose_on_same_product` |
| 줄 생성 → `target.where` 전이가 같은 상품 갱신 | 6건 × 5라운드 | 같음 | 수정 전: 성공 1·1·0·0·1건, 나머지 `DEADLINE_EXCEEDED`. 수정 후: 5라운드 모두 6건 성공 26~35ms | 결함, 수정 | `a3_line_first_then_where_target_apply` |
| 줄 생성 → `target.ids` 전이(대조군) | 6건 × 5라운드 | 6건 성공 | 5라운드 모두 6건 성공 25~35ms. 수정 전후 같음(dc68e4e6이 막은 경로) | 통과 | `a4_line_first_then_ids_apply_control` |
| 두 상품을 서로 반대 순서로 차감하는 묶음 | 2건(1+1) × 10라운드 | 둘 다 성공 | 9라운드는 교착 1회 뒤 재시도로 둘 다 성공(1.04~1.05s), 1라운드는 18ms | 설계 결정(5절) | `a1_opposite_order_two_product_bundles`(`PROBE_N=2`) |
| 같은 반례 | 3건 × 10라운드 | 셋 다 성공 | 7라운드는 셋 다 성공(약 1.05s), 3라운드는 셋 모두 `DEADLINE_EXCEEDED` | 설계 결정 | 같음(`PROBE_N=3`) |
| 같은 반례 | 6건(3+3) × 10라운드 | 6건 성공 | 6라운드는 1건 성공 + 5건 `DEADLINE_EXCEEDED`, 4라운드는 6건 모두 `DEADLINE_EXCEEDED`(모두 2.02s). 최종 재고는 성공 건수만큼만 줄었다 | 설계 결정 | 같음(`PROBE_N=6`, 기본) |

수정 전 수치는 수정 직전 코드(`0c5d108d` 기준)에서 잰 값이다. 수정한 두 경로의 회귀 시험은 `spikes/spike-v3-write/tests/bundle_lock_order_compose_where.rs`이며, 수정 전에는 compose 라운드 0에서 6건 모두 `WIRE.DEADLINE_EXCEEDED`로 실패했고 수정 뒤에는 10회 반복 실행이 모두 통과했다(각 실행은 compose·where 각 3라운드, 6건 동시).

### 3.2 부분 커밋과 롤백

| 반례 | 동시성 | 기대 | 실제 | 판정 | 근거 명령 |
| --- | --- | --- | --- | --- | --- |
| 줄 정책 거절, compose 뒤 실패, 재고 부족을 섞은 묶음 | 12건 × 8라운드 | 성공한 묶음의 주문·줄·멱등 기록만 남는다 | 8라운드 모두 성공 4건일 때 주문=줄=멱등 기록=4, 실패 묶음의 compose 효과(`seen`)는 성공한 것만 반영, 재고 음수 없음 | 통과 | `f1_failing_bundles_leave_nothing_behind_under_concurrency` |
| 확장 `ctx.data.apply` 뒤 `ctx.data.create`의 유일 제약 충돌 | 4건 × 10라운드(서로 다른 상품 행) | 승자 1건만 반영, 패자의 재고 차감은 취소 | 10라운드 모두 성공 1건 + `WIRE.ALREADY_EXISTS` 3건, 반영된 claim 수 = 차감된 상품 수 | 통과 | `e3_extension_unique_create_race_on_distinct_rows` |
| 확장이 같은 재고 행과 같은 유일 태그를 두고 경합 | 6건(사용자 2명) × 6라운드 | 승자 1건, 재고는 승자 몫만 | 1건 성공, `ALREADY_EXISTS` 2~3건, `WORKER_BUSY` 2건(사용자당 동기 worker 한도 2로 추정), 재고·claim 일치 | 통과 | `e2_extension_contention_distinct_keys` |

### 3.3 동시 차감·증감의 경계

| 반례 | 동시성 | 기대 | 실제 | 판정 | 근거 명령 |
| --- | --- | --- | --- | --- | --- |
| `Int(0..1000)` 상한에서 400씩 더하기(묶음, 단일 apply 두 형태) | 6건 × 6라운드 | 2건 성공, 재고 800 | 두 형태 모두 6라운드에서 2건 성공 + `WIRE.BAD_VALUE` 4건, 재고 800 | 통과 | `n1_concurrent_increment_bounds_and_overflow` |
| i64 최댓값 직전(`max-250`)에서 100씩 더하기(`Int`, 범위 없음) | 6건 × 6라운드 | 2건 성공, 나머지 거절 | 2건 성공 + `WIRE.BAD_VALUE`(숫자 범위 초과) 4건, 값은 `max-50`을 넘지 않음. `WIRE.INTERNAL` 없음 | 통과 | 같음 |
| 재고 5에서 1씩 10건 차감 | 10건 × 6라운드 | 5건 성공, 재고 0 | 6라운드 모두 5건 성공 + `WIRE.INVALID_STATE` 5건, 재고 0 | 통과 | 같음 |

### 3.4 멱등 key 재전송

| 반례 | 동시성 | 기대 | 실제 | 판정 | 근거 명령 |
| --- | --- | --- | --- | --- | --- |
| 같은 key·같은 요청 동시 중복(READ COMMITTED 묶음) | 12건 × 8라운드 | 실행 1회, 나머지 재생 | 8라운드 모두 성공 1 + `replayed` 11, 재고 감소 7(한 번분) | 통과 | `i1_same_key_concurrent_duplicates_read_committed` |
| 같은 key·같은 요청 동시 중복(SERIALIZABLE) | 12건 × 2라운드 | 실행 1회 | 2라운드 모두 성공 1 + `replayed` 11, 방당 확정 예약 1 | 통과 | `i2_same_key_concurrent_duplicates_serializable` |
| 같은 key에 서로 다른 두 요청이 경합(지문 불일치) | 10건(5+5) × 8라운드 | 한 요청만 실행, 다른 요청은 불일치 | 8라운드 모두 성공 1 + 같은 요청 재생 4 + `WIRE.IDEMPOTENCY_MISMATCH` 5, 재고 감소는 3 또는 5(한 요청분)뿐 | 통과 | `i3_same_key_different_requests_race` |
| 응답 유실 후 같은 key 재전송, `/status`, 배포 fence | 순차 1회(결정적) | 재생, 재실행 없음, fence 변경 시 거절 | 응답 유실 뒤 재전송과 `/status` 모두 `replayed:true`, 재고 한 번만 감소. `facts_digest` 변경 후 `/apply`·`/status` 모두 `WIRE.DEPLOYMENT_CHANGED`, marker 삭제 후 모두 `WIRE.SCHEMA_MISMATCH` | 통과 | `i4_response_loss_then_replay_and_fence` |
| SERIALIZABLE 일반 `/apply`, 서로 다른 key 경합 | 6건 × 30라운드 | 확정 1건 이하, `WIRE.INTERNAL` 없음 | 30라운드 합계 성공 30, `WIRE.FORBIDDEN` 150, `WIRE.INTERNAL` 0 | 통과 | `i5_serializable_apply_distinct_keys_never_internal`(`PROBE_ROUNDS=30`) |
| 확장, 같은 key 동시 중복(worker 400ms) | 4건 × 6라운드 | worker 1회 실행, 나머지 재생 | 6라운드 모두 성공 1 + `replayed` 3, worker 시작 1회 | 통과 | `e1_extension_same_key_concurrent_duplicates` |
| SERIALIZABLE 확장, 같은 key 동시 중복 | 4건 × 8라운드 | 실행 1회, 나머지 재생 | 8라운드 모두 성공 1 + `replayed` 3, worker 시작 1회 | 통과 | `e4_serializable_extension_race_and_same_key_duplicates` |
| 완료된 key 재생과 배포 fence(확장) | 기존 시험 | fence 뒤 재생 | 기존 `write_extension_replay_backpressure`가 catalog drift, digest 변경, marker 누락을 모두 막음. 이번 주기에 다시 실행해 통과 | 통과 | `cargo test ... --test write_extension_replay_backpressure` |

### 3.5 확장의 직렬화 충돌과 재시도

| 반례 | 동시성 | 기대 | 실제 | 판정 | 근거 명령 |
| --- | --- | --- | --- | --- | --- |
| SERIALIZABLE 확장(`ctx.data.apply`가 `not exists` 판정 전이)에서 한 방에 4건이 같은 방을 확정 | 4건 × 8라운드(수정 전, 프로브) | 1건 성공, 패자는 재시도 가능한 `WIRE.CONFLICT` | 수정 전: 8라운드 중 7라운드에서 패자가 `WIRE.INTERNAL`(패자 24건 중 12건). PG 로그의 원인은 `UPDATE aip_idem SET result=...`의 `could not serialize access due to read/write dependencies`. 수정 후 회귀 시험 12라운드 × 3회 실행 모두 `WIRE.CONFLICT`만 | 결함, 수정 | `e4_serializable_extension_race_and_same_key_duplicates`, 회귀: `cargo test ... --test write_extension_serializable_conflict` |

회귀 시험 `write_extension_serializable_conflict`는 수정 전에 12라운드 중 7라운드가 실패했고(`WIRE.INTERNAL` 누출) 수정 후 3회 연속 통과했다. 경합이 통계적이므로 12라운드 모두 깨끗해야 통과하게 했다.

### 3.6 `WORKER_BUSY` 포화 상태의 재생

| 반례 | 동시성 | 기대 | 실제 | 판정 | 근거 명령 |
| --- | --- | --- | --- | --- | --- |
| worker 슬롯 4개가 모두 1.2s 점유 중일 때 | 점유 4건 + 관찰 요청 5건 | 완료된 key 재생은 통과, 새 요청은 `WORKER_BUSY` | 완료 key 재전송 `OK(replayed)`, 완료 key `/status` `OK(replayed)`, 진행 중 key `/status` `WIRE.NOT_FOUND`(진행 중과 미존재를 구분하지 않음, 의도), 진행 중 key 재전송 `WIRE.WORKER_BUSY`, 새 key `WIRE.WORKER_BUSY`. 점유가 끝난 뒤 실패한 key를 다시 보내면 기록이 없어 다시 실행(실패는 기록하지 않음) | 통과 | `e5_saturated_slots_replay_status_and_inflight_duplicates` |
| 확장 요청 4건이 멈춘 DB(핸드셰이크 무응답)에서 대기 중일 때 5번째 확장 요청 | 4건 대기 + 1건 | 5번째는 즉시 `WIRE.WORKER_BUSY`, 대기 4건은 기한에 `DB_UNAVAILABLE`, slot 반환 | 수정 전: 5번째가 slot 없이 DB 연결을 기다려 2.0s 뒤 `WIRE.DB_UNAVAILABLE`. 기준 be2c0682에서는 통과, 241ae37f(조기 재생 조회 도입)부터 main까지 실패. 수정 후 통과 | 결함, 수정 | 기존 macOS 전용 시험 `db_connect_lifetime`(`cargo test ... --test db_connect_lifetime`) |
| 확장 요청이 멈춘 SQL 응답을 기다릴 때 | 4건 + 5번째 | 기한 안에 응답, 연결 폐기 | 수정 전(241ae37f~main): 4건 모두 `SQL response remained blocked`(응답 없음). 조기 조회 기한 초과 분기의 `release()`가 멈춘 연결에서 대기한다고 추정했고, 연결을 `drop`으로 폐기하자 통과했다 | 결함, 수정 | 기존 `db_query_lifetime`의 `authenticated_but_silent_sql_releases_slots_and_request_owned_connections` |

## 4. 수정한 결함

### 4.1 줄 생성 뒤 compose·`target.where` 전이의 교착 (78894ef4)

- 원인: `bundle_reference_locks`는 줄 생성 정책이 참조 행에 잡는 `FOR SHARE`를 미리 쓰기 잠금으로 올려 두는데, 뒤 연산이 `target.ids` 전이일 때만 대상 행을 알았다. compose는 `compose_exec`가 대상 행을 `NO KEY UPDATE`로 따로 잠그고, `target.where`는 쿼리가 행을 고른다. 둘 다 줄 생성이 읽은 행에 공유 잠금을 쥔 채 갱신 잠금을 기다리는 N-방향 교착이 되었고, 희생자 한 명마다 `deadlock_timeout` 1초가 들어 쓰기 기한 2초 안에 수렴하지 못했다.
- 수정: compose 대상(id가 계획 단계에 확정)은 id 단위로, `target.where`는 resource 단위로 뒤 연산의 필요 잠금을 예약한다. `where`는 어느 행이 고를지 모르므로 그 resource의 참조 행 모두에 예약해 필요보다 넓게 잠글 수 있다. 같은 묶음 안에서 앞서 만든 참조에만 적용된다.
- 영향: 줄 생성 뒤에 같은 resource의 `where` 전이가 오는 묶음은 참조 행을 더 일찍 쓰기 잠금으로 잡는다. 다른 호출자의 같은 행 갱신은 이 묶음이 끝날 때까지 기다린다(이전에는 교착 뒤 재시도). 연산 실행 순서와 정책 판정 순서는 바뀌지 않는다.
- 한계: 앞 연산이 만든 행 사이의 순서가 아니라 "앞에서 읽은 행을 뒤에서 갱신"하는 경우만 푼다. 5절의 서로 반대 순서 잠금은 그대로다.

### 4.2 SERIALIZABLE 확장의 직렬화 실패가 `WIRE.INTERNAL`로 샘 (cff81a17)

- 원인: 확장 `apply`의 COMMIT 전 단계 오류 분류(`before_commit_failure`)가 연결 끊김 외의 모든 DB 오류를 `WIRE.INTERNAL`로 바꿨다. PostgreSQL은 SERIALIZABLE에서 읽기·쓰기 의존을 알아챈 첫 문장에서 40001을 낸다. 그 문장이 결과 기록 `UPDATE aip_idem`이면 호출자는 재시도 가능한 `WIRE.CONFLICT` 대신 재시도 표지가 없는 내부 오류를 받았다. COMMIT 단계와 `apply_in` 안에서는 이미 `WIRE.CONFLICT`로 분류되고 있었다.
- 수정: 40001과 40P01을 `serialization_conflict`(`WIRE.CONFLICT`, "다시 시도 가능")로 분류한다. 연결 끊김과 그 밖의 오류 분류는 그대로다.
- 영향: 호출자가 보는 코드가 `WIRE.INTERNAL`에서 `WIRE.CONFLICT`로 바뀌는 경우가 있다. 동기 쓰기 확장을 서버가 다시 실행하지 않는 규칙(decisions C0-6)은 그대로이고, 호출자가 같은 key로 다시 보낸다.

### 4.3 확장 요청이 worker slot보다 DB 연결을 먼저 잡음 (WORKER_BUSY 상한 무력화)

- 원인: 241ae37f(완료된 쓰기 재생을 slot보다 먼저)가 확장 요청마다 `pool.acquire_until`과 완료 key 조회를 `extension_slots.try_acquire` 앞에 두었다. 그래서 DB가 느리거나 응답하지 않으면 slot을 쥔 요청이 하나도 없어 `WORKER_BUSY` 상한이 동시 요청 수를 막지 못하고, 상한을 넘은 요청도 `DB_UNAVAILABLE`로 기한(2s)까지 연결을 기다렸다. 기존 macOS 전용 시험 3건(`db_connect_lifetime`, `db_query_lifetime` 2건)이 이 계약을 지키고 있었고 be2c0682에서는 통과, 241ae37f부터 main까지 실패했다. Linux CI는 이 시험들(`#![cfg(target_os = "macos")]`)을 돌리지 않아 놓쳤다.
- 수정(최소): ① slot을 DB 작업 앞에서 먼저 잡는다(`try_acquire`, 못 잡아도 즉시 실패시키지 않는다). ② slot이 없는 포화 요청도 완료 key 재생은 받을 수 있게 하되, 연결 checkout·배포 fence·journal 조회 전체에 500ms(`SATURATED_REPLAY_BUDGET`) 예산만 준다. 재생이 없거나 예산을 넘기면 `WIRE.WORKER_BUSY`다. ③ 조기 조회가 기한을 넘긴 연결은 `release()`로 되돌리지 않고 폐기한다(멈춘 연결에서 reset을 기다리다 응답이 기한을 넘기는 것을 막는다). ④ principal slot이 거절하는 경우 쥐고 있던 연결을 `release()`로 돌려준다(이전에는 폐기해서 건강한 연결을 버렸다).
- 영향: 포화 상태의 재생(7cdfed93이 보장하는 것)은 DB가 500ms 안에 답할 때 유지된다. DB가 그보다 느리면 포화 요청은 재생 대신 `WORKER_BUSY`를 받는다(재시도 가능). 500ms는 이 작업이 정한 값이라 소유자가 조정할 수 있다. 시험: 위 3건과 `write_extension_replay_backpressure`가 통과한다. `db_query_lifetime`의 `silent_operation_sql_after_clock...`는 코드가 아니라 fixture를 고쳤다. 조기 조회가 clock 조회보다 먼저 `aip_idem`을 읽으므로 그 시험의 DB에 journal 테이블이 있어야 하는데 없어서 `relation does not exist`가 `WIRE.INTERNAL`이 되었다. 기대값은 바꾸지 않고 테이블 준비만 추가했다.
- 후속 제안(고치지 않음): 위 세 시험의 `#![cfg(target_os = "macos")]`는 소켓 프록시만 쓰는데 macOS 전용으로 막혀 있다. Linux CI에서도 돌리면 같은 퇴행을 CI가 잡는다. CI 구성은 이 작업의 범위가 아니다.

## 5. 설계 결정이 필요한 것: 반대 순서 잠금 (E9의 남은 부분)

현재 동작: 묶음은 연산 배열 순서대로 대상 행을 잠그므로, 같은 행들을 서로 다른 순서로 건드리는 묶음이 동시에 오면 교착한다. 서버는 40P01을 최대 3회 다시 실행하지만, 교착을 알아채는 데 PostgreSQL `deadlock_timeout`(1초)이 들고 쓰기 기한이 2초라서 실제로는 교착 한 번만 견딘다. 3.1절의 측정이 그 결과다: 2건은 교착 한 번 뒤 둘 다 성공하지만 1초가 들고, 3건 이상은 성공하는 것이 없는 라운드가 흔하다.

`product/LIMITS.md` E9는 "같은 행을 갱신하는 연산을 묶음 앞에 두라"고 안내하지만, 서로 다른 행 둘을 호출자마다 다른 순서로 갱신하는 경우는 지침이 닿지 않는다(`decisions.md` C0-5가 "연산은 배열 순서로 잠근다"를 정했으므로 코드만의 문제가 아니다).

| 선택지 | 내용 | 효과 | 비용·영향 |
| --- | --- | --- | --- |
| A. 문서만 | E9에 "행 집합이 다른 묶음은 모든 호출자가 같은 순서로 쓴다. 어기면 교착 한 번에 1초가 들고 3건 이상 겹치면 기한 초과가 흔하다"와 위 측정을 쓴다 | 사용자가 순서를 맞추면 교착이 없다 | 서버 동작 변화 없음. 호출자 실수는 그대로 기한 초과 |
| B. 시작 시 정렬 선잠금 | 묶음 시작에서 `target.ids`와 compose 대상 행을 `(resource, id)` 순서로 가시성 조건과 함께 잠근다(필요 잠금 중 가장 강한 모드). `where` 대상과 묶음 안에서 새로 만든 행은 해당 없음 | 반대 순서 교착이 id·compose 대상에서 사라진다. E9 지침이 불필요해진다 | 잠금을 묶음 끝까지 더 오래 쥐어 경합이 늘어난다. C0-5와 E9가 바뀐다(소유자 결정). `where` 전이는 여전히 교착 가능. 선잠금도 행 정책(`vis`)을 적용해야 안 보이는 행을 기다리지 않는다 |
| C. 교착 감지 시간 단축 | 트랜잭션마다 `SET LOCAL deadlock_timeout`을 낮춘다(예: 50ms) | 희생자 비용이 1초에서 수십 ms로, 3회 재시도 안에 수렴할 수 있다 | PostgreSQL 15 이상은 `GRANT SET ON PARAMETER`가 필요하고 그 미만은 슈퍼유저만 가능하다. 외부 업무 DB의 권한 모델에 걸린다. 감지 비용과 거짓 감지 부담이 늘어난다 |
| D. 호출 쪽 지침 | SDK·문서가 같은 resource의 `target.ids`를 한 연산에 모으도록 안내(한 연산 안에서는 `ORDER BY id`로 잠가 교착이 없다) | 서버 변화 없음 | 서로 다른 transition을 쓰면 한 연산으로 합칠 수 없다 |

권고: B를 1.0 후보로 검토하고, 결정 전에는 A를 문서에 먼저 반영한다. B는 `bundle_reference_locks`와 같은 계획 단계에서 구현할 수 있고(대상은 계획 단계에 확정), 이 문서의 3.1절 `a1` 시험이 그대로 회귀 시험이 된다. 결정은 소유자 몫이라 이번에는 코드에 넣지 않았다.

## 6. 범위 밖 발견과 미검증

- 범위 밖(권한 정책): 확장 `ctx.data.create`가 유일 제약과 생성 권한(`allow`)이 모두 어긋날 때 경합 결과에 따라 `WIRE.ALREADY_EXISTS` 또는 `forbidden`으로 갈린다. INSERT가 먼저 실행되고 `allow`는 DB 기본값 적용 뒤에 평가되기 때문이다. 생성 권한이 없는 호출자가 숨은 행의 존재를 알아낼 수 있는지는 정책 담당이 판단할 문제라 건드리지 않았다. 탐색 중 `Claim` 생성 `allow owner = actor`에 다른 사용자가 `owner`를 1로 넣어 부딪쳤을 때 6라운드 중 3라운드에서 관찰했다. 시험으로 남기지 않았고 표에도 넣지 않았다.
- 미재현 의심(코드 추론): `handle_apply`의 멱등 key 조회가 `if let Ok(Some(row))`라서 그 조회의 오류(직렬화 실패·연결 끊김)가 버려지고 다음 문장의 `25P02`가 `WIRE.INTERNAL`로 보일 수 있다. SERIALIZABLE 일반 `/apply` 180회 시도(`i5`)에서 `WIRE.INTERNAL`은 없었다. 재현하지 못해 고치지 않았다.
- 미검증: MySQL relational 경로의 재생·fence(이 환경은 PostgreSQL만), 격리 모드 worker(`WorkerIsolation`이 `None`이 아닌 경우), `create`의 absence 판정을 쓰는 확장(`create.<Resource>`) 경합. 마지막은 이번에 쓴 정의가 생성 후 판정이라 자기 행에 막혀 시험이 성립하지 않아 버렸다. 해당 영역은 기존 `absence_serializable_http`가 덮는다.
- 해시 충돌: 멱등 직렬화는 `pg_advisory_xact_lock(hashtext(principal:key))` 32비트 해시를 쓴다. 충돌하면 서로 다른 key가 직렬화될 뿐 결과는 달라지지 않는다고 코드로 판단했고 시험하지 않았다.
- 시험 반복 횟수는 표에 적은 값이다. 교착·경합 결과는 비결정적이라 라운드 사이 편차가 있다(3.1절은 라운드별 분포를 적었다).

## 7. 시험 목록과 결과

- 신규 회귀(커밋됨): `spikes/spike-v3-write/tests/bundle_lock_order_compose_where.rs`, `spikes/spike-v6-transport/tests/write_extension_serializable_conflict.rs`. 4.3절은 기존 시험이 회귀 시험이다(`db_connect_lifetime`, `db_query_lifetime`, 수정 전 실패, 수정 후 통과).
- 탐색(커밋됨, `#[ignore]`): `spikes/spike-v6-transport/tests/concurrency_probe_http.rs`.
- `spike-v3-write` 전체(수정 2건 반영, 4.3절 이전): 통과 107, 실패 0.
- `spike-v6-transport` 전체(4.3절 수정 전): 통과 290, 실패 8, ignored 24. 실패 8건 중 5건은 이 환경에 `tsc`가 없어서(`v12`~`v16` SDK 시험, `No such file or directory`), 3건은 4.3절의 결함(macOS 전용 `db_connect_lifetime` 1건과 `db_query_lifetime` 2건)이었다.
- 4.3절 수정 뒤에는 이 수정이 닿는 확장·slot·수명·fence·commit 계열 35개 시험 바이너리를 다시 돌렸다(아래 작업 보고 참조). 전체를 다시 돌리지 않은 이유는 이 Mac의 부하로 durable job 시험이 매우 느려서다. 4.3절 수정은 `serve_request`의 확장 경로 앞부분만 건드린다.
