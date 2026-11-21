# WP2 (P0-A 단계 간 참조) 구현 노트

WP4(P0-D)를 같은 브랜치에서 이어 받는 작업자를 위한 메모다. 결정은 [decisions.md](decisions.md) P0-A, 독립 검토는 `review-decisions.md` 2절(A-1~A-3, 설계 브랜치 50f2bc27)이다.

## 이 단계에서 한 순서

1. `origin/feat/expr-p0c-ext-create-20261121`(3a6edf5f)을 merge했다. 충돌은 없었다(WP3은 `spike-v4-worker`와 `wp3-notes.md`만 건드렸다).
2. WP1이 미룬 확장 `ctx.data.apply`의 `input` 전달: `write.rs` `ctx_request`가 `input`을 `target`에서 꺼내 요청 키로 옮기고, `worker.mjs`·`worker.py`가 일곱 번째 인자(`input`)를 보내고, `ctx.d.ts`·`aip_ctx.py`가 같은 인자를 선언한다. Node와 Python 확장을 실제 PostgreSQL에서 돌리는 시험은 `tests/expr_ext_apply_input.rs`다(입력이 서버에 닿아 재고가 줄고, `from` 거짓이나 `input` 누락이면 호출 전체가 커밋되지 않는다).
3. P0-A 서버: `spike-v3-write/src/lib.rs`.
4. SDK(TS 빌더·`Created<R>`, Python `created(i)`).

## 변경 경로

| 층 | 어디를 보면 되나 |
| --- | --- |
| 계획(트랜잭션 전) | `bundle_plan(facts, ops, knobs) -> Vec<OpPlan>`(pub). 연산마다 `refs: [(필드, 앞 연산 번호)]`와 `target_created`를 정한다. `bundle_once`는 `begin` 전에, `bundle_in`은 실행 전에 부른다. 판정은 `resolve_created`(`{"created": i}` 하나를 풀어 만든 resource를 돌려준다) |
| 실행 | `bundle_run`이 연산 번호 → 만든 id(`made`)를 들고 간다. create의 `refs`는 `Src::Lit(id, id_kind)`로 `row`에 더해 호출자 리터럴과 같은 `ensure_refs_readable` → `create_row`(allow)를 지난다. apply 대상은 `apply_op(.., created_target)`가 `TargetSpec::Ids(vec![id])`로 쓴다 |
| 단일 `/apply` | `target.created`는 `parse_target`의 `keys(["ids","where"])`가 `WIRE.UNKNOWN_KEY`로 거절한다(묶음 안에서만 앞 연산이 있다) |
| MySQL | 손대지 않았다. `mysql_write.rs`의 create 연산 `keys(["create","values"])`와 대상 `keys`가 새 키를 이미 `UNKNOWN_KEY`로 거절한다(C0-9의 fail-closed) |
| SDK(TS) | `spike-v5-sdk/sdk/generic.ts`: `Created<R>`, `CreateRefs<S>`, `BundleApplyRequest`, `CreateOpWithRefs`, `BundleBuilder`. 런타임 빌더는 `spike-v6-transport/client/typed.ts`의 `buildBundle`(`bundle(b => ...)`) |
| SDK(Python) | `product/python/aip_sdk/__init__.py` `created(i)`(검증 후 `{"created": i}`). `bundle(operations)`는 그대로 연산을 보낸다 |

## 규칙 ①~⑥과 검토 반영

| 규칙·검토 | 구현 | 코드 |
| --- | --- | --- |
| ① 앞 연산만 | `created`는 0 이상 정수이고 현재 연산 번호 미만. 아니면 `BAD_VALUE`(`"0"`, `-1`, 소수, null, bool 포함). `{created: i}` 밖의 키는 `UNKNOWN_KEY`, 객체가 아니면 `BAD_REQUEST`(`values`와 같다) | `resolve_created` |
| ② create여야 하고 `id` 열 | 아니면 `VALUE_SOURCE_NOT_ALLOWED` | 〃 |
| ③ `Ref<R>` 타입 일치 | 필드 타입이 `Ref<만든 resource>`(nullable 허용)가 아니면 `TYPE_MISMATCH`. `Id<..>` 필드는 받지 않는다(결정표 규칙은 `Ref`만 말한다. `literal`은 둘 다 받지만 refs는 더 좁게 갔다) | `bundle_plan` |
| ④ 필드 노출·중복 | `expose create fields`에 없으면 `FIELD_NOT_EXPOSED`, `values`에도 있으면 `BAD_VALUE` | 〃 |
| ⑤ 같은 resource | apply 대상의 resource와 만든 resource가 다르면 `TYPE_MISMATCH` | 〃 |
| ⑥ 트랜잭션 전 | 위 모두 `bundle_plan`에서. 뒤 연산의 오류가 앞 연산 실행 전에 난다(시험 `refs_are_decided_before_the_transaction_…`) | |
| **A-1** `target.created`는 `id` 대상 형식 필요 | `expose apply … target where`만 연 전이는 `TARGET_NOT_ALLOWED`. `created`는 `ids`·`where`와 섞으면 `BAD_VALUE` | `bundle_plan` |
| A-2 `created[]` 번호 ≠ 연산 번호 | `created: i`는 **전체 연산 번호**, 응답 `created[]`는 create 연산만 담는다. 시험 `a_s1_…`이 순서를 고정한다. 문서(WP5)에 `[apply, create Order, create Line refs{created:1}]` 예시를 넣을 것. TS 빌더는 핸들로 가려 준다 | |
| A-3 행 수 | `target.created`는 `bulkMaxRows`가 아니라 1행으로 센다. 시험은 `bulk maxRows 100`인 전이를 19번 불러 선언 상한으로 세면 1900 > 1000으로 막히는 경우다 | `bundle_run` 앞부분 |
| C0-3 권한 | 만든 행이 호출자의 `rows read`에 안 보이면 `MISSING_TARGET`이고 전체 롤백(create 참조·apply 대상 둘 다). create `allow`도 그대로 평가된다 | `a_f7_…` |
| A-F6, A-F8 | 공개되지 않은 create에 `refs`는 `NOT_EXPOSED`, 전이 resource 불일치는 `TYPE_MISMATCH`. internal 전이는 공개 경로에서 `NOT_EXPOSED` | `a_f6_…`, `a_f8_…` |

차이와 판단

- `values`는 refs가 있어도 필수 키다(빈 `{}`면 된다). 빼면 `BAD_REQUEST`다. 빌더는 항상 보낸다.
- TS의 `CreateOpWithRefs`(원시 형식)는 `values`를 `Partial`로 둔다. 참조로 채운 필드를 빼도 되게 하려고 필수 필드 검사를 서버에 넘긴 것이다. 정확한 필수 검사는 빌더 `create(resource, values, refs)`(값 타입에서 `keyof refs`를 `Omit`)에서만 한다.
- legacy id wire(`Id`가 `string`)에서는 `RefTarget`이 never라 `CreateRefs`가 빈 객체 타입이 된다. 빌더로 쓰려면 safe·decimal-string wire여야 한다(서버 규칙은 wire와 무관하게 같다).

## ref 메타데이터와 지문(C0-7)

새 메타데이터를 내보내지 않았다. 참조 필드의 대상 resource는 계약이 이미 가진 `CreateContract`의 필드 타입 `Id<"R">`로 타입 수준에서 읽는다. 그래서 생성 모듈 글자와 계약 지문이 바뀌지 않는다(저장소 정의 46개의 `service check`·`core`·`gen`을 기준 바이너리와 비교해 차이 0, 시험 `refs_add_nothing_to_the_fingerprinted_contract`는 지문 본문에 `refs`·`created`가 없음을 고정한다). 지문 밖 접미부를 새로 만들면 `service gen` 출력이 모든 정의에서 달라져 V-1이 깨진다. 접미부가 필요해지면(예: legacy wire의 참조 대상) 그때 `write_binding` 아래에, 참조 필드가 있는 정의에서만 내도록 한다.

## 멱등·복구(C0-4, I-1~I-3)

저장 요청은 `{"created": i}` 기호 그대로다(서버가 고쳐 쓰지 않는다). `tests/expr_refs_http.rs`가 HTTP로 보인 것:

- 같은 key 재전송은 같은 `created` id와 `replayed: true`로 재생하고 재고는 한 번만 차감한다.
- `refs`만 다른 요청(또는 해석된 id를 `values`로 쓴 요청)과 `input`만 다른 요청은 `IDEMPOTENCY_MISMATCH`다.
- 뒤 연산 실패는 행도 `aip_idem`도 남기지 않고, 같은 key가 새로 실행된다.
- 같은 key 12개 동시 요청은 실행 1회와 재생 11회다.
- 지문이 다른 요청은 `CONTRACT_MISMATCH`다.
- `aip-test-fault-proxy`로 COMMIT 응답을 끊으면 `WIRE.COMMIT_UNKNOWN`이고, 같은 key가 만든 id를 그대로 재생한다.

## 함정

1. **시험 표 공유**: v3 `expr_refs.rs`는 시험마다 다른 actor(`ACTOR` task-local)를 쓰고 행 수는 `owner_id`로 센다. 같은 표의 전체 개수를 세는 시험은 병렬 시험 때문에 흔들린다. `race`가 spawn하는 task에는 `ACTOR.scope`를 다시 씌워야 한다.
2. **actor 자기 행 예외**: create의 `owner: Member`에 다른 멤버를 넣으면 `ensure_refs_readable`이 `MISSING_TARGET`이다(Member에 `rows read`가 없어서). 가시성 시험의 "보이는 부모"는 읽기 정책이 값으로 갈리는 resource(`rows read when title = "shown"`)로 만들었다.
3. **`v16_bundle.rs` 마커 검사가 취약했다**: 오류 출력에서 파일명 등장 횟수를 세고 헤더 주석의 단어까지 마커로 셌다. 타입 메시지가 바뀌자 11 대 10으로 깨져, 마커 줄마다 그 다음 줄에 오류가 있는지 보도록 고쳤다(더 엄격하다).
4. **테이블 이름**: `Order`는 `order`, `OrderLine`은 `order_line`이다(`snake`). `order`는 예약어지만 스키마 한정 이름이라 그대로 쓴다.
5. **RED**: v3 시험은 기능 전체를 되돌리면 `bundle_plan` import가 없어 컴파일에서 실패한다(실패로 센다). 규칙별 RED(타입·앞 연산·create 여부·필드 노출·중복·A-1·resource 일치·가시성·행 수)는 각각 해당 시험 하나만 실패하는 것을 확인했다.
6. **K-3**: 서로 다른 순서로 같은 부모를 만지는 묶음을 4번 겨루게 했고 남는 결과는 성공 또는 `INVALID_STATE`뿐이었다. 교착 자체는 매번 나지 않으므로 이 시험은 "새지 않는다"를 보이는 것이고, 교착 재시도 로직은 기존 `audit_bd_lock_order.rs`가 고정한다.
7. 확장 `ctx.data.apply`가 `target.created`를 보내는 일은 없다(확장은 묶음을 못 만든다). 확장에서 새 행의 id를 쓰려면 `ctx.data.create`가 돌려주는 id를 쓴다.
8. **P0-B 검증의 참고 반례(NULL)**: 물리 컬럼이 사후에 nullable이 되면(외부 DB 채택, 스키마 드리프트) 대상이나 항이 NULL인 산술이 `OK`로 끝나고 NULL이 남았다(`NOT BETWEEN`이 NULL이라 범위 검사를 통과). `update`와 `effects`의 RETURNING 검사에 계산 결과 `IS NULL`을 더해 `WIRE.BAD_VALUE`로 전체를 롤백한다(고정 증감 `increment`도 같다). 검사식 전체는 `COALESCE(.., TRUE)`로 감싸 NULL(판정 불가)을 거절로 읽는다. 가드가 없을 때는 NULL이 `r.get::<bool>`에서 panic했다. 시험은 v3 `expr_arith.rs`의 `a_null_target_or_term_at_run_time_is_bad_value_and_rolls_back`(컬럼에 `DROP NOT NULL`, 대상 NULL·항 NULL·효과 대상 NULL·효과 항 NULL 거절과 롤백, 값이 있으면 통과하는 대조)이고 가드를 각각 빼면 실패한다. 이 시험만 쓰는 `Drift`·`Vault` 표에서 컬럼을 바꾸므로 다른 시험과 겹치지 않는다.
