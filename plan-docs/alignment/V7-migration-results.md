# V7. 마이그레이션 사전 검사 실험 결과

> 상태: 검증 필요 (2026-10-04, Codex r8 반영). 의제 11(Migration)·[E](E-technical-risks.md) RK-10을 작게 실행한 기록이다. 마이그레이션 정책·승인 절차 결정이 아니다. 기존 PoC의 `aip-pg/src/evolve.rs`·`aip-contract/src/compat.rs`와 비교하지 않았다.
> 위치: `../../spikes/spike-v7-migrate/`. V1 facts 두 버전을 비교하고, 로컬 PostgreSQL `aip_v7` schema의 실제 데이터로 판정한다(테스트 끝에 삭제).

## 실제 실행 출력

명령: `cd spikes/spike-v7-migrate && cargo test --offline -q -- --nocapture`

```text
r8 전이 allow 확대: ["SecurityReview"] 적용 false {"part":"transitions","resource":"Recruitment"}
r8 nullable 범위 강화: ["Blocked"] 적용 false {"field":"internalNote","range":[2,100],"resource":"Recruitment","violatingRows":1}
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
```

원래 9사례 + 측정 수치 단언 + Codex r8 반례 10사례 + 위반 없는 불변식 추가(적용 가능) 대조를 한 테스트에서 돈다.

고장 주입: 정책 확대 판정을 끄면 확대가 적용 가능으로 분류되어 실패(r8 전). 분류 밖 차이를 잡는 부분을 끄면 전이 allow 확대·unique 추가가 빈 변경/적용 가능이 되어 실패(r8 후). 복원 후 통과.

## 1. 분류

| 분류 | 뜻 | 자동 적용 |
|---|---|---|
| Safe | 데이터·호출자 영향 없음 | 가능 |
| Breaking | 기존 화면 요청이 거부되거나 결과가 줄어듦. 데이터 손실 없음 | 가능(계약 변경 기록 필요) |
| SecurityReview | 누군가 새로 볼 수 있게 됨, 또는 필드 정책 변화 | 불가 |
| Destructive | 데이터 손실(필드·resource 제거, 타입 변경) | 불가 |
| Blocked | 기존 데이터가 새 규칙을 어김 | 불가(데이터 정리 먼저) |

정적 비교 뒤 DB로 판정을 보강한다. **명시적으로 분류한 부분(필드, 공개 select·filter·sort, 행·필드 정책, 불변식, enum) 밖의 실행 facts 차이는 의미를 모르므로 모두 SecurityReview다**(관계 공개, budget·루트 조회, 집계, 전이·쓰기 공개, 확장, unique, check, 기존 predicate·access·limit 본문). 행 정책 변경은 측정 결과와 무관하게 SecurityReview이고, 측정(현재 회원과 익명이 옛·새 정책으로 보는 행, 평가 시각 명시)은 검토 근거로 붙는다.

## 2. 결과 (같은 seed 데이터)

| 변경 | 판정 | 근거 |
|---|---|---|
| Club에 nullable 필드 추가 | Safe | |
| Club에 필수 필드 추가 | Blocked | 기존 행 3개, 채울 값 없음 |
| Recruitment 공개 select에서 views 제거 | Breaking | 옛 화면 요청 `select: ["id","views"]`가 새 계약에서 `FIELD_NOT_EXPOSED` |
| Club 행 정책 축소(같은 학교만) | SecurityReview(측정: 4쌍 잃음, 새로 보는 행 0) | 측정 0은 확대 없음의 증명이 아니다(r8 F8: 시간이 지나면 새로 보이는 정책 반례) |
| Club 행 정책 확대(모두) | SecurityReview(측정: 6쌍 새로 보임) | |
| internalNote 필드 정책 제거 | SecurityReview | |
| 초안은 동아리당 1개 불변식 추가 | Blocked | 동아리 10에 초안 2개(위반 그룹 1) |
| 데이터가 쓰는 enum 값(DRAFT) 제거 | Blocked | 그 값을 쓰는 행 2개 |
| 안 쓰이는 enum 값(REJECT) 제거 | Breaking | 쓰는 행 0. 호출자 계약은 바뀜 |

정의가 쓰는 enum 값(CLOSED, close 전이의 목표)을 제거하면 새 정의 자체가 V1 의미 검사에서 거부된다. 마이그레이션 검사까지 오지 않는다.

## 2.1 Codex r8 반영

[codex-v7-r8](../reviews/codex-v7-r8.md): P1 8, P2 5. 핵심은 "diff가 보지 않는 facts 부분이 바뀌면 변경 없음 = 자동 적용 가능"이었다.

| 발견 | 반영 |
|---|---|
| F1~F6 P1: predicate 본문, traverse, 집계 sourceAccess·access 본문, 전이 allow·쓰기 공개, budget 추가(루트 조회 공개), 기존 불변식 변경·unique 추가가 빈 diff | 분류 밖 차이는 모두 SecurityReview. 기존 predicate·access·limit 이름의 본문 변경·제거도 SecurityReview. 새 이름 추가는 그것을 쓰는 변경에서 판정 |
| F7 P1: nullable 필드의 범위 강화가 Safe | 범위 변경은 새 범위를 어기는 기존 값 수를 셈(예: 1행 → Blocked) |
| F8 P1: 측정상 확대 0을 축소로 확정, 시각 고정 | 행 정책 변경은 항상 검토. 평가 시각을 인자로 받고 결과에 기록 |
| F9 P2: NULL 그룹 측정이 DB 유일 인덱스와 다름 | DB처럼 NULL 그룹은 세지 않고, nullable per면 `nullGroupsNotLimited` 표시 |
| F10 P2: enum 사용 행을 필드별로 중복 계산 | resource별 서로 다른 행 수 |
| F11 P2: 숫자 조건 불변식에서 panic | 측정 SQL에 매개변수 전달 |
| F12 P2: 새 필드를 쓰는 정책과 필드 동시 추가에서 panic | 측정 불가(`measurable: false`)로 기록, 검토로 남김 |
| F13 P2: 측정 수치를 회귀 검증하지 않음 | 기존 행 3, 잃은 쌍 4·새로 보는 쌍 0·6, 위반 그룹 1, enum 사용 행 2·0 단언 |

## 3. 드러난 것

- **권한 확대 여부는 정책 식만 비교해서는 알기 어렵다. 현재 데이터 측정은 확대를 찾는 데는 쓰이지만, 확대가 없다는 증명은 못 한다**(미래 데이터, 시간 의존 조건). 그래서 정책 변경은 측정과 함께 사람 검토로 보낸다. 자동 적용은 "분류된 안전 변경만"으로 좁혀야 했다.
- 비교기가 facts 구조를 다 알지 못하면 빈 diff가 위험한 허가가 된다. 기본값을 "모르면 검토"로 둬야 했다.
- 불변식 추가·enum 제거·필수 필드 추가는 적용 전에 위반 개수를 셀 수 있었다. 데이터 정리 자체는 사람이 정해야 한다.

## 4. 하지 않은 것

실제 DDL 적용·rollback, 데이터 이전 스크립트, 필드 이름 바꾸기(현재는 제거+추가로 보임), 구버전 SDK와의 호환 기간, 기존 PoC 마이그레이션 자산과 비교, 승인 기록.

## 변경 이력

- 2026-10-04 V7 마이그레이션 사전 검사 실행 결과.
- 2026-10-04 Codex r8 반영: 분류 밖 차이는 검토, 정책 변경은 항상 검토, 범위·NULL·enum·매개변수·측정 불가 처리, 수치 단언.
