# WP1 (P0-B 제한 산술) 구현 노트

WP2를 같은 브랜치에서 이어 받는 작업자를 위한 메모다. 결정은 [decisions.md](decisions.md) P0-B, 시험은 [test-plan.md](test-plan.md)가 기준이다. 아래는 코드에서 확인한 변경 경로와 함정이다.

## 변경 경로 (한 줄 요약)

| 층 | 어디를 보면 되나 |
| --- | --- |
| Parser·AST | `ast.rs` `Transition.input: Params`, `parser.rs` transition 키 `"input" => self.params_block()`, 효과 update 값이 `additive`를 부름. `hmap.rs` transition 키 목록에 `input`(`.h.ts`·`.h.py`) |
| Sema | `sema.rs` `arith`(기존 `counter` 일반화), `transition_input`(선언 검사, 8개 이하, Int·Decimal non-null), 전이 범위 `tsc`(from·allow·항에서만 `input.p`가 보인다). compose 금지 규칙에 `!x.input.is_empty()` 추가 |
| facts | 리터럴 정수는 `{"increment": n}` 그대로. 새 형태는 `{"arith":{"op":"-","term":{"input"\|"lit"\|"path":…}}}`. 전이 facts의 `input`은 선언한 전이에만 있다(그래서 기존 정의의 facts·지문이 안 변한다) |
| Runtime | `spike-v3-write/src/lib.rs`: `transition_args`(요청 `input` 검증), `ApplyInput.args`, `input_handles`(바인드 핸들), `arith_term`·`arith_op`, `judge_query`·`judge`·`effects`·`delete_rows`가 `input: Option<&ApplyInput>`를 받음. 범위 사후 검사는 `update`에서 `arith`까지 넓히고, 효과 update는 같은 문장 `RETURNING t.id, <범위 밖 여부>`로 확인 |
| MySQL 가드 | `crates/aip-migrate/src/mysql_write.rs` `refuse_limited_arithmetic` → `WIRE.UNSUPPORTED`. `mysql` feature 뒤에 있어 기본 `cargo test -p aip-migrate`에서는 안 돈다 |
| SDK | `spike-v5-sdk/src/lib.rs`(TS 계약 `input: {…}` 타입, range 주석), `python.rs`(`input=` keyword), `sdk/generic.ts`(`ApplyRequest`에 input). `product/python/aip_sdk/__init__.py`는 `apply(request: dict)`가 요청을 그대로 보내므로 수정하지 않았다 |
| 문서 | `syntax_card.rs` → `spec/definition-syntax.md`, 진단 문구 → `spec/definition-diagnostics.md`(둘 다 `diagnostics.rs` 시험이 디스크 파일과 대조) |

## 결정표와 달라진 곳, 덧붙인 판단

- **`input` 값 직접 대입 금지**: `to f = input.p`는 `DEF.UNSUPPORTED`다. 결정표는 `input`을 항으로만 쓰라고 했지만 직접 대입은 말이 없었다. 호출자 값을 그대로 쓰는 길은 `patch`가 따로 있고(검증·expect 모델이 다르다), 두 길이 겹치지 않게 닫았다.
- **효과 update의 `input`**: 결정표의 항 목록에는 효과에서의 `input`이 명시되지 않았지만 `to`와 같은 규칙으로 허용했다(시험 `effect_arith_may_use_a_validated_input_and_rolls_back_with_the_transition`).
- **효과 update 범위 검사**: 효과 대상 resource에는 DDL CHECK가 없어서 `UPDATE … RETURNING`에서 `u.<col> NOT BETWEEN lo AND hi`를 계산해 하나라도 밖이면 `WIRE.BAD_VALUE`로 전체 롤백한다.
- **`ctx.data.apply`의 `input`은 하지 않았다**: `spike-v4-worker/src/write.rs`는 WP3 소유다. 서버는 그 경로에서 `input` 키를 `UNKNOWN_KEY`로 거부하므로 안전하다. 이어서 `ctx_request`가 `input`을 옮기게 하는 일은 work-packages WP2 절에 이미 적혀 있다.
- bundle의 apply 연산은 `input`을 받는다(`bundle_applies_input_per_operation_and_rolls_back_as_a_whole`). `input`은 `apply_input`(lib.rs:1505)이 `transition_args`로 검증해 `ApplyInput.args`에 담고 `apply_op`가 이를 `judge`에 넘긴다. WP2가 `refs`를 해석해 값을 채울 자리도 이 `apply_input`/`apply_op` 경계다.

## 함정

1. **시험 표 공유**: v3 `tests/expr_arith.rs`의 모든 시험이 같은 스키마·같은 `product` 표를 쓴다. `where` 대상 시험이 `stock = 7` 같은 흔한 값을 고르면 병렬로 도는 다른 시험의 행까지 잡아 가끔 실패한다(한 번 겪고 777로 바꿨다). 새 시험의 행 id와 `where` 값은 겹치지 않게 잡는다.
2. **`increment` 분기와 `arith` 분기는 두 곳**: `update`와 `effects` 모두 `v.get("increment").is_some() || v.get("arith").is_some()` 식이다. 새 facts 형태를 더하면 이 둘과 `judge_query`의 범위 검사 위치를 같이 본다.
3. **지문 불변 조건**: facts에 새 키를 더할 때는 쓰는 전이에만 나오게 한다. 정의에 없으면 키 자체를 내보내지 않아야 V-1이 통과한다(`input`이 그렇게 한다).
4. **tsc 시험**: 새 worktree에는 `node_modules`가 없다. `spikes/spike-0-ts`에서 `npm ci`를 하거나 `AIP_TSC`로 tsc 경로를 준다.
5. **PG 시험 환경**: `unset PGHOST PGPORT`, `AIP_TEST_DATABASE_URL`·`AIP_TEST_ADMIN_URL`을 작업 전용 PG 17 컨테이너로 준다. 시험 스키마 이름은 프로세스마다 하나다.
6. **V-1 비교는 기준 바이너리가 필요하다**: 기준 SHA를 별도 worktree와 별도 `CARGO_TARGET_DIR`로 빌드해 `service check`·`core --digest`·`core`·`service gen`(ts, python)을 저장소 정의 전부에 돌려 `diff -r` 한다. 이번 결과는 기존 정의 46개 출력이 동일했고, 차이는 이 묶음이 일부러 바꾼 것(새 문법 fixture 4개, compose 진단 문구 `patch/expect/input`)과 python 생성 panic 로그의 스레드 번호뿐이었다.
7. `facereview-mysql` fixture는 python 생성이 기준과 같은 이유로 panic한다(`python.rs:56`, 이 묶음 이전부터).
