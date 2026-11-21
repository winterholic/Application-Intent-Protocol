# WP4 (P0-D bundle 안 compose) 구현 노트

새 대화로 이어받아도 되도록 작업 중에 바로 적는다. 결정은 [decisions.md](decisions.md) P0-D, 독립 검토는 `review-decisions.md` 5절(D-1~D-5, 설계 브랜치 50f2bc27)이다. 앞 단계는 [wp1-notes.md](wp1-notes.md), [wp2-notes.md](wp2-notes.md).

## 변경 경로

| 층 | 어디를 보면 되나 |
| --- | --- |
| 계획(트랜잭션 전) | `spike-v3-write/src/lib.rs` `bundle_plan(facts, ops, knobs, wire)`. 시그니처가 `ops: &'a [Value]`, `wire`를 받고 `OpPlan<'a>`를 돌려준다. 첫 순회가 행 수 합계(`BUNDLE_MAX_ROWS`)와 compose 연산의 `compose_plan`을 한꺼번에 처리한다(검토 D-3). 행 수 계산은 `bundle_run`에서 여기로 옮겼다 |
| 실행 | `bundle_run`의 `compose` 분기가 `compose_exec(tx, facts, plan, ..)`를 그대로 부른다. 새 실행 경로는 없다. 결과(`applied`, `created_rows`)는 연산 순서대로 이어 붙인다 |
| 응답 경계(D-4) | `BundleResult.ops: Vec<OpSpan { kind, applied: Range, created: Range }>`가 연산마다 `applied[]`·`created[]`의 구간을 남긴다. `compose`는 단계 수만큼 applied를, create 단계마다 대상 수만큼 created를 만들어 연산 번호와 항목 번호가 어긋나기 때문이다 |
| 격리(D-2) | `bundle_needs_serializable`에 `op.get("compose")` 분기 → `compose_needs_serializable(facts, op)` |
| 전송 | `spike-v6-transport/src/lib.rs` `bundle_tags`가 `compose_tags`(top-level compose와 같은 규칙)를 쓴다(D-1). `run_write`는 compose 연산이 있는 묶음에만 `ops: [{kind, applied:[시작,끝), created:[시작,끝)}]`를 더한다 |
| 암호화(D-5) | `encryption.rs` `reject_unsupported_bundle_ops`: 암호화가 켜진 서버는 묶음 안의 compose와 `refs`를 `WIRE.UNSUPPORTED`로 거절한다(변환기가 이 연산들을 건너뛰어 평문이 들어가거나 범위 id가 "0"이 되는 것을 막는다). `input`은 거절하지 않았다(`values`가 아니라 암호화 대상이 아니다) |
| MySQL | `mysql_write.rs`는 `BundleResult { ops: vec![] }`만 맞췄다. 묶음의 create 연산 `keys(["create","values"])`가 compose 연산을 이미 `UNKNOWN_KEY`로 거절한다(fail-closed) |

## 규칙과 판단

- compose 연산의 판정은 연산 키로 한다: `apply` 키가 있으면 apply, 없고 `compose` 키가 있으면 compose, 아니면 create. 그래서 `apply`와 `compose`를 같이 쓴 객체는 apply로 보고 `apply_op`가 `UNKNOWN_KEY`로 거절한다.
- 중첩 거절은 새 코드가 아니라 기존 키 검사가 한다: compose 단계는 `transition`·`create`뿐이라 단계 안 compose는 `create 단계` `keys` 검사에서, 묶음 안 `atomic`은 create 분기 `keys`에서 `WIRE.UNKNOWN_KEY`다(test-plan T-2).
- `refs`·`target.created`가 compose 연산을 가리키면 P0-A의 `resolve_created`가 `VALUE_SOURCE_NOT_ALLOWED`로 거절한다(compose 연산은 `create` 키가 없다). 추가 코드는 필요 없었다.
- 입력·patch·expect가 있는 전이는 정의 단계가 compose에 못 넣게 막는다(WP1). 손으로 만든 facts가 열어도 `compose_exec`가 `WIRE.BAD_VALUE`로 거절한다(시험은 facts를 고쳐 이 두 번째 방어선을 본다).
- `bundle_plan`은 P0-A의 `refs` 판정도 하므로 compose 계획이 refs 판정보다 먼저 돈다. compose의 `NOT_EXPOSED`·`STEP_NOT_ALLOWED`가 앞 연산 실행 전에 난다.

## 함정

1. **compose_plan이 `wire`를 받는다**: ids 파싱 때문에 `bundle_plan`에도 `wire`를 더했다. 호출처는 `bundle_once`(Legacy), `bundle_in`(요청의 wire), `tests/expr_refs.rs`다.
2. **`ops` 응답 키는 compose가 있을 때만 나간다**: 기존 묶음의 응답은 글자까지 그대로다(저장된 멱등 결과 재생도 마찬가지). 구 클라이언트가 `applied[]`·`created[]`만 읽어도 깨지지 않는다. `BundleResult.ops`는 항상 채우므로 라이브러리 사용자(확장 `ctx.data.create` 경로)는 구간을 쓸 수 있다.
3. **v3 시험 표 공유**: `expr_compose_bundle.rs`도 시험마다 다른 actor(`ACTOR` task-local)를 쓰고 행 수는 `owner_id`로 센다. `race`가 spawn하는 task에 `ACTOR.scope`를 다시 씌워야 한다. 격리 수준 시험은 `slot`·`log` 표의 INSERT 트리거(`seen` 표)로 `transaction_isolation`을 읽는다.
4. **격리 표지의 출처**: compose의 `isolation`은 그 compose가 부르는 전이와 create의 `allow`가 부재 의존(`not exists`)이면 `serializable`이다(`isolation.rs`). 시험은 `Slot`의 create `allow free(this.owner)`(안에 `not exists`)로 만들었다.
5. **TS 타입**: `generic.ts`의 `BundleOp`·`BundleRequest`·`BundleResult`·`BundleBuilder`에 세 번째 타입 인자 `Cp`(기본 `{}`)를 더했다. 두 인자만 쓰는 기존 코드는 그대로 컴파일된다. `BundleBuilder.compose(request)`는 핸들을 돌려주지 않는다. `BundleResult.ops?`는 `BundleOpSpan[]`이다. 부정 타입 시험은 `sdk/expr-refs-types.ts`(compose 마커 3개 추가, 계약 fixture에 `expose compose` 추가)이고 v6 TS 실행 시험은 `client/expr-compose-bundle-http.test.ts`다(생성 모듈 `generated-expr-compose.ts`는 시험이 쓴다).
6. **Python SDK**: `bundle` docstring에 compose 연산과 `ops`만 적었다. 연산은 이미 그대로 전달되므로 코드는 바꾸지 않았다.
7. **P0-A 시험 수정**: `tests/expr_refs.rs`의 `a_f3_…`에 있던 "compose 연산은 UNKNOWN_KEY 또는 VALUE_SOURCE_NOT_ALLOWED" 확인은 compose가 이제 실제 연산이라 `expr_compose_bundle.rs`의 `nesting_input_transitions_and_references_to_a_compose_are_refused`로 옮겼다(compose를 가리키는 `refs`·`target.created`는 `VALUE_SOURCE_NOT_ALLOWED`).

## 시험과 RED

- v3 `tests/expr_compose_bundle.rs`(7개): D-S1(apply → compose → apply와 구간), 대표 시나리오(input 차감 + compose, 어느 단계 실패든 전체 롤백), D-2(표지와 실제 `transaction_isolation`), D-3(계획 오류와 합산 행 수가 앞 연산 전에), D-F1·D-F2·A-F3(중첩·compose 참조·입력 전이), 반대 순서 묶음 경합, compose 없는 묶음의 구간.
- v6 `tests/expr_compose_bundle_http.rs`(1개, 안에 TS 실행 포함): 구간·tags·재생·`IDEMPOTENCY_MISMATCH`·`ops` 없음(compose 없는 묶음)·롤백·계획 오류 코드·격리·TS 빌더.
- RED(각각 해당 시험만 실패): D-2 분기 제거, 계획 오류 무시(D-3), compose를 1행으로 세기, 구간 기록 제거, compose 연산 건너뛰기, `bundle_tags`의 compose 분기 제거(D-1), 응답 `ops` 제거, 암호화 거절 제거(D-5).
- V-1: 기존 정의 46개의 check·core·digest·gen 출력이 기준 바이너리와 동일(차이는 `facereview-mysql` python 생성 panic 로그의 스레드 번호 하나).

## 남은 것 / 참고

- compose 대상이 많은 묶음은 교착 확률이 올라간다(검토 5절). 연산 배열 순서가 곧 잠금 순서라는 점을 WP5 문서에 적을 것. 서버는 40P01을 최대 3회 재시도한다.
- 응답 `ops`의 구간은 `[시작, 끝)` 두 수다(검토의 목록 예시 대신 구간으로 했다. 데이터를 중복하지 않는다).
