# Track B 소유자 결정 기록

공개 의미·호환성·지원 범위에 걸리는 항목만 둔다. 결정 대기 때문에 독립 작업은 멈추지 않았다. 측정 근거와 재현 명령은 [track-b.md](../track-b.md).

## B-D1. bundle 잠금 순서(LIMITS E9, a1)

### 문제와 재현

같은 행들을 서로 다른 순서로 `target.ids` 전이하는 bundle 둘이 동시에 오면 교착한다. 서버는 `40P01`을 최대 3회 다시 실행하지만 교착 한 번의 감지에 `deadlock_timeout`(1초)이 들고 쓰기 기한이 2초(`WRITE_DEADLINE_MS`)라 한 번 정도만 견딘다. 수정 전 기준 측정(PostgreSQL 17.11, `deadlock_timeout=1s`, 라운드 10회):

| 동시 요청 × 대상 행 | 성공률 | p50 / p99 (ms) | 기한 초과율 |
|---|---:|---|---:|
| 2 × 2 | 100% | 1157 / 1183 | 0% |
| 4 × 2 | 28% | 2012 / 2016 | 72% |
| 8 × 4 | 1% | 2015 / 2023 | 99% |
| 12 × 8 | 0% | 2015 / 2022 | 100% |

전체표와 재현 명령은 track-b.md 2절.

### 이번에 구현한 것: 선택지 B′ (호환 유지 선잠금)

`spikes/spike-v3-write/src/lib.rs`의 `lock_bundle_targets`. bundle이 `target.ids`나 compose로 이름 붙인 행이 둘 이상이면 `bundle_run` 시작에서 `(resource, id)` 순서로 `SELECT ... WHERE id = ANY(..) AND <rows read> ORDER BY id FOR [NO KEY] UPDATE`를 실행한다. 연산은 배열 순서 그대로 실행한다.

| 관찰 가능한 의미 | 바뀌는가 | 근거 |
|---|---|---|
| 연산 실행 순서, `refs`·`target.created` 해석 | 아니오 | 연산 루프를 건드리지 않았다 |
| 정책 판정(`rows read`, `allow`, `from`) | 아니오 | 각 연산의 `judge`가 자기 차례에 그대로 판정한다. 선잠금 문장은 `judge`와 같은 `rows read` 조건을 쓰고 결과 행을 쓰지 않는다 |
| 오류 코드·순서 | 아니오 | 요청 형식이 틀렸거나 없는/안 보이는 행은 선잠금이 건너뛴다. 시험 `the_first_failing_operation_still_decides_the_error_and_nothing_is_kept` |
| 안 보이는 행의 존재 노출·대기 | 아니오 | `rows read` 조건이 없으면 안 보이는 행의 락을 기다린다. 이를 잡는 시험 `rows_the_caller_cannot_read_are_neither_locked_nor_waited_for`(조건을 빼면 실패함을 확인) |
| 롤백 | 아니오 | 같은 트랜잭션 안이다. 측정에서 롤백 위반 0 |
| 잠금 모드 | 아니오 | 행마다 연산이 나중에 요구할 가장 강한 모드(`row_lock`)만 쓴다 |
| 락을 쥐는 시점·기간 | **예** | 이름 붙은 행을 묶음 끝까지 쥔다. 다른 호출자의 같은 행 갱신은 이전보다 일찍부터 기다린다. 앞 연산이 일찍 실패하는 묶음도 뒤 연산 대상 행의 락을 기다릴 수 있다 |
| 한 행만 이름 붙은 묶음 | 아니오 | 순서 문제가 없어 선잠금을 건너뛴다 |

관찰 가능한 응답 의미는 같고 시간 특성(락 보유 구간)만 달라지므로 호환 변경으로 보고 구현했다. 소유자가 이 시간 특성 변경까지 계약으로 받아들일지 확인이 필요하다(추천: 받아들임. 교착 후 1초 지연이 사라지는 이득이 더 크다).

### 효과

같은 격자(동시 2~12 × 행 2~8, `opposite`·`shuffle`)에서 교착 희생자 0, 성공률 100%, 전체 p99 ≤ 61ms(전: 최대 2072ms). 호출자가 순서를 미리 정렬한 대조군(`sorted`)과 같은 수준이다.

### 남은 부분과 선택지

| 선택지 | 내용 | 의미 변화 | 장단점 | 추천 |
|---|---|---|---|---|
| A. 지침만 | LIMITS E9에 `where`·효과가 겹치면 순서를 맞추라고 적는다 | 없음 | 서버 변화 없음. 해당 경로는 교착 가능 | **채택(문서 반영 완료)** |
| B′. 선잠금 | 위 구현 | 시간 특성만 | `target.ids`·compose 교착 해소 | **구현·채택** |
| B″. `where` 대상도 선잠금(시작 시 `where`를 먼저 평가) | 시작 시점의 행 집합을 고정 | **있음**: `where`는 앞 연산이 바꾼 상태를 보고 행을 고르는 의미(예: 앞 연산 뒤 `from`을 만족하는 행). 시작 시 평가하면 다른 행이 선택될 수 있음 | 교착 범위 확대 | 반대 |
| C. 서버가 연산 순서를 `(resource, id)`로 재정렬 | 실행 순서 변경 | **있음**: 같은 행을 두 번 건드리는 묶음(`take` 뒤 `restock`)의 `from` 판정, `refs`·`target.created`(앞 create 뒤에 와야 함), 실패 연산 순서가 모두 바뀜. 측정상 잠금 효과는 B′와 같음(`sorted` 행) | 의미 변경 비용만 크다 | 반대 |
| D. `SET LOCAL deadlock_timeout` 단축 | 교착 감지 50ms | 없음 | PG 15+ 이상 `GRANT SET ON PARAMETER` 또는 슈퍼유저 필요(외부 업무 DB 권한 모델). 측정(서버 설정 50ms): 4건까지는 성공하나 8건 이상에서 성공률 25% 이하로 여전히 기한 초과가 흔함. 재시도 3회가 소진됨 | 보조 수단으로도 불충분, 반대 |
| E. 재시도 횟수·기한 상향 | `SERIALIZATION_RETRIES`, `WRITE_DEADLINE_MS` | 계약 값 변경 | 교착을 숨길 뿐 지연이 늘어남 | 반대(이번에 계약 값은 바꾸지 않음) |

### 코드 변경 범위

`spikes/spike-v3-write/src/lib.rs`: `bundle_target_rows`, `lock_bundle_targets` 추가와 `bundle_run`의 호출 한 줄. 확장 `ctx.data.bundle`·HTTP `/apply` bundle·`bundle_in`이 모두 `bundle_run`을 쓰므로 같은 동작을 받는다. 공개 계약·SDK·생성 코드 변경 없음.

### 기존 소비자 영향

- 응답 코드·값이 바뀌는 경우 없음. 소비자 앱(`product/tests/fixtures/expr-apps`, taskshare, stockdesk)의 bundle 모두 `target.ids`·compose·create 조합이라 영향은 락 시점뿐이다.
- 같은 행을 갱신하는 느린 단독 호출과 겹치면 bundle이 시작 시점에 기다린다(이전에는 그 연산에 도달했을 때). 기한(2초)은 같다.

## B-D2. `SATURATED_REPLAY_BUDGET`(500ms) 등 계약 값

바꾸지 않았다. 측정·권고는 track-b.md 5절.
