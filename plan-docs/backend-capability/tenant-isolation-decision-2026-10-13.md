# 테넌트 격리 의사결정 (B03, OI-TN1·OI-TN2) — 2026-10-13

> 상태: **결정됨**(2026-10-13). 창시자 답: "테넌트는 너 추천대로하는게 좋을듯". OI-TN1 = T1-A, OI-TN2 = T2-A 유지(T2-B는 후속 후보), 생성 경로 규칙 = 호출자가 준 Ref 값은 호출자가 읽을 수 있는 행이어야 한다(actor 자기 행은 예외). 규칙은 라이브러리 `spike-v3-write`의 `bundle` create에 구현·테스트했다(`tests/create_ref_visibility.rs`). 아래 4절은 결정 전 권장안을 그대로 둔 기록이고, 결정 결과는 4절 끝의 "결정 결과"에 있다.
> 범위: `aip service` 제품 경로(`spike-v1-fixture` sema, `spike-v2-read` planner, `spike-v3-write`, `spike-v6-transport`). root `aip-runtime`·`aip-pg`의 `tenant via`(D-P18)는 PoC이며 제품 지원으로 세지 않는다.

## 1. 현재 제품 경로의 사실

코드로 확인한 것과 추론을 나눠 적는다.

| 사실 | 근거 |
|---|---|
| 제품 문법에는 `tenant` 선언이 없다. 테넌트는 `rows read when` 행 정책 predicate로 표현한다(`school = actor.school`, `managerOf(actor, club)` 같은 `exists` predicate) | `spike-v1-fixture/fixture/recruitment.aip` |
| 행 정책이 없는 resource는 전부 거부가 기본이다 | `spike-v1-fixture/src/sema.rs` rowRead `{"default":"denyAll"}` |
| 읽기 planner는 루트·traverse 자식·Ref filter 대상 모두에 대상 resource의 rowRead를 건다. 안 보이는 행은 상한도 차지하지 않는다 | `spike-v2-read/src/plan.rs`, `tests/ref_filter_visibility.rs`, `tests/security_authorization.rs` |
| 전이(`/apply`)는 대상 행을 rowRead와 함께 잠그고, 안 보이는 행은 존재하지 않는 행과 같은 `MISSING_TARGET`이다 | `spike-v3-write/src/lib.rs` `judge` |
| job의 `ctx.data.read/apply`는 같은 planner와 전이를 job 주체로 실행한다 | `spike-v6-transport/tests/job_data.rs`(다른 회원 주문 `MISSING_TARGET`) |
| schedule·webhook·outbox trigger는 운영자가 `aip service principal bind`로 등록한 서비스 주체(actor id가 있는 principal)로 실행된다. 제품 경로에는 actor가 없는 데이터 접근 문맥이 없다 | `product/MANAGED-EXECUTION.md` 168행, `src/triggers.rs` |
| 호출자 생성(`expose create`, bundle, compose)은 라이브러리(`spike-v3-write`)에만 있고 HTTP 경로에 연결돼 있지 않다 | `server.rs` 경로 목록, 보안 감사 표 API3 행 |
| 라이브러리 create는 Ref 필드에 호출자가 준 id를 받고, 대상 행의 가시성은 따로 검사하지 않는다. 막는 것은 `expose create`의 `allow`뿐이다 | `spike-v3-write/src/lib.rs` `literal`, `create_row` |
| compose의 생성 값은 상수 Ref를 받지 않고 대상 행 경로에서만 온다. `sameScope`가 대상들의 같은 범위를 강제한다 | 같은 파일 `VALUE_SOURCE_NOT_ALLOWED`, `sameScope` |

추론: 제품 경로의 테넌트 격리는 "모든 업무 resource에 테넌트 조건이 들어간 `rows read when`을 쓴다"는 관례에 기대고, 관례를 빠뜨린 resource는 기본 거부라 노출 대신 사용 불가가 된다. 빠뜨렸을 때 실패 방향이 안전하다는 점은 확인했지만, 조건을 잘못 쓴 경우(예: 다른 경로의 org를 비교)를 checker가 잡지는 못한다.

## 2. PoC의 OI-TN1·OI-TN2를 제품 경로에 옮기면

| PoC 질문 | 제품 경로에서의 형태 |
|---|---|
| OI-TN1 actor 없는 문맥(webhook·핸들러)의 읽기에 테넌트 필터를 걸지 | 제품에는 actor 없는 문맥이 없다. trigger job도 서비스 주체의 행 정책으로 읽는다. 질문은 "여러 테넌트를 다루는 시스템 작업(야간 정산 등)을 어떻게 표현하나"로 바뀐다 |
| OI-TN2 actor를 요청 단위 활성 테넌트에 묶을지 | 여러 workspace에 속한 사용자는 행 정책이 `exists Membership ...`이면 소속 테넌트 전체를 한 요청에서 본다. "지금 이 workspace만"이라는 요청 단위 범위는 호출자 filter로만 표현된다 |

## 3. 선택지

### OI-TN1 (여러 테넌트를 다루는 시스템 작업)

| 안 | 내용 | 장점 | 단점 |
|---|---|---|---|
| T1-A 현행 유지(권장) | 시스템 작업용 actor를 두고 행 정책에 명시적으로 넓힌다. 예: `rows read when org = actor.org or actor.platform = true` | 새 문법 없음. 테넌트 경계를 넘는 권한이 정의에 그대로 보인다(PoC의 `cross tenant` 표식과 같은 역할). 서비스 주체 revoke로 바로 끊는다 | 넓히는 조건을 resource마다 반복한다. predicate 하나(`predicate inOrg(o: Org) = ...`)로 줄일 수는 있다 |
| T1-B 서비스 주체 전용 우회 | trigger job은 행 정책을 건너뛴다 | 작성이 짧다 | 서버 최종 권한 원칙(D-A7)과 job 정책 재검사를 무너뜨린다. 서명된 webhook이라도 payload의 테넌트 id를 믿게 된다. 기각 |
| T1-C `tenant` 선언 이식 + `cross tenant` 수식어 | PoC D-P18을 제품 문법으로 옮긴다 | DB 트리거로 한 transaction 한 테넌트를 강제한다 | 새 개념 두 개. 아래 T2-B와 묶어 판단해야 한다 |

### OI-TN2 (요청 단위 활성 테넌트)

| 안 | 내용 | 장점 | 단점 |
|---|---|---|---|
| T2-A 현행 유지(권장, 당분간) | 소속 테넌트 전체가 보이고, 화면 단위 범위는 호출자 filter(`workspace` Ref filter)로 좁힌다 | 새 개념 없음. Ref filter는 대상 행 정책을 함께 걸어 남의 workspace id로 좁혀도 빈 결과다 | "한 요청은 한 테넌트" 불변식이 없다. 여러 테넌트 행을 한 번에 집계하는 화면을 막지 못한다 |
| T2-B 세션 claim의 활성 테넌트 | access token(또는 `/session` 요청)의 `tenant` 값을 서버가 멤버십으로 검증하고, predicate에서 `session.tenant`로 읽는다 | Slack·Linear식 "현재 workspace" 모델을 그대로 표현. 한 요청 한 테넌트를 서버가 보장 | 새 식 변수와 세션 프로토콜 필드, 멤버십 검증 선언이 필요하다. 계약 지문과 SDK 세션 API가 바뀐다 |
| T2-C `tenant via` 선언(PoC 이식) | resource마다 테넌트 경로를 선언하고 planner가 필터를 자동 삽입 | 조건을 빠뜨리거나 잘못 쓰는 실수를 checker가 잡는다. 테넌트 간 Ref를 DB가 거부 | 행 정책과 의미가 겹친다(`rows read when`과 `tenant via` 두 곳에서 범위를 말함). 지침 §7의 5번(기존 의미 훼손 여부)에 걸린다 |

## 4. 권장안

1. **OI-TN1은 T1-A로 닫기를 제안한다.** 제품 경로에는 이미 actor 없는 데이터 접근이 없고, 여러 테넌트를 다루는 작업은 행 정책에 넓히는 조건을 명시하는 것이 가장 안전한 기본값이다. 코드 변경이 없다.
2. **OI-TN2는 T2-A를 유지하고 T2-B를 다음 결정 후보로 둔다.** T2-B는 공개 계약(세션 필드·식 변수)을 바꾸므로 이번 회차 지시 범위(호환을 깨면 문서로만)에 따라 구현하지 않는다. T2-C는 행 정책과 책임이 겹쳐 권장하지 않는다.
3. **생성 경로 공개 전 필수 조건(결정 필요).** create를 HTTP에 연결할 때 "호출자가 준 Ref 값은 호출자가 읽을 수 있는 행이어야 한다"를 기본 규칙으로 두기를 권한다. 지금은 `allow`가 참조 행을 검사하지 않으면 다른 테넌트의 Task에 Comment를 만들어 그 테넌트 화면에 끼워 넣을 수 있다(라이브러리 경로 코드 추적, HTTP 미노출이라 실제 경로 재현은 없음). 다만 actor resource에 `rows read`가 없는 흔한 정의(`author = actor`)가 이 규칙으로 거부되므로 호환을 깬다. 대안은 (a) actor 자기 행 참조는 예외, (b) `expose create`에 Ref 필드별 출처(`author from actor`)를 선언하게 하는 것이다. 창시자 확인 대상이다.

### 결정 결과 (2026-10-13)

| 항목 | 결정 | 코드 변경 |
|---|---|---|
| OI-TN1 | T1-A. 시스템 작업은 전용 actor(서비스 주체)로 실행하고 행 정책에 넓히는 조건을 명시한다 | 없음 |
| OI-TN2 | T2-A 유지. T2-B(세션 claim의 활성 테넌트)는 공개 계약을 바꾸므로 **후속 결정 후보**로 남긴다 | 없음 |
| 생성 경로 Ref 가시성 | 호출자가 준 Ref 값은 대상 resource의 `rows read`를 통과하는 행이어야 한다. 예외는 actor 자기 행(actor resource의 id가 호출자 actor id와 같은 값). 3절 대안 (b)(`author from actor` 선언)는 채택하지 않았다 | `ensure_refs_readable`(`spikes/spike-v3-write/src/lib.rs`)을 `bundle` create 앞에 둔다 |

구현 메모:
- 안 보이는 대상은 없는 대상과 같은 `MISSING_TARGET`이다(전이와 같은 코드라 존재 여부가 새지 않고 wire 코드도 늘지 않는다).
- compose의 create는 값을 서버가 읽은 대상 행 경로에서만 받고(상수 Ref는 `VALUE_SOURCE_NOT_ALLOWED`) `sameScope`가 범위를 강제하므로 이 검사 대상이 아니다. 회귀 테스트(`v3_2`, `v3_r3`, `create_nullable_values`)가 그대로 통과한다.
- 기존 `v3_2`·`v3_r3` fixture는 Member에 `rows read`가 없어 "관리자가 다른 회원을 ClubMember로 만든다"가 새 규칙에 걸렸다. 두 fixture에 `rows read when school = actor.school`(관리자는 같은 학교 회원을 읽는다)을 더했다. 반례의 의미(업무 규칙 CHECK_FAILED/OK)는 그대로다. 실제 정의가 같은 형태라면 Ref 대상 resource에 읽기 정책이 있어야 한다는 뜻이다.
- HTTP에는 create가 아직 연결돼 있지 않다. 연결할 때 이 규칙이 이미 라이브러리에 있으므로 그대로 따라온다.

## 5. 지침 §7 체크(권장안 기준)

| 질문 | 답 |
|---|---|
| 1. 기존 문법으로 해결할 수 없는가 | T1-A·T2-A는 기존 `rows read when`·predicate·Ref filter로 해결한다 |
| 2. 조합이 더 적절하지 않은가 | 권장안 자체가 조합이다 |
| 3. 반복 활용 | predicate 하나를 여러 resource 행 정책에서 재사용한다 |
| 4. 학습 비용 | 새 개념 없음 |
| 5. 기존 의미 훼손 | 없음 |
| 6. AI 생성 일관성 | 테넌트 표현이 행 정책 한 곳뿐이라 일관성이 높다. 대신 resource마다 조건을 쓰는 것을 잊으면 기본 거부로 드러난다 |
| 7. 변형 요구 | 활성 테넌트(T2-B)·테넌트별 공정성(I08)은 후속 결정이 필요하다 |

## 6. 남는 위험

- 행 정책의 테넌트 조건을 잘못 쓴 경우(다른 경로의 org 비교)를 checker가 잡지 못한다. 선택적 dev-check로 "actor 테넌트 경로와 비교하지 않는 rowRead"를 경고하는 방안이 있다(미구현).
- 테넌트별 rate limit·quota 공정성(I08)은 principal 단위다.
- 감사 기록에 테넌트를 남기는 일은 B09 감사 기록 설계에서 다룬다(행 정책 predicate에서 테넌트를 추출할 수 없으므로 actor·대상 행 id로 남긴다).
