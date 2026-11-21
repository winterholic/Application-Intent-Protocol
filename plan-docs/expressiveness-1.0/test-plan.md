# 테스트 계획 (P0-A~D)

공통 환경: `unset PGHOST PGPORT`(로컬 셸에 설정돼 있으면 `aip-test-support` build.rs가 strict-db 빌드를 막는다), `CARGO_TARGET_DIR`은 [build-baseline](build-baseline.md) 권장안, `AIP_TEST_DATABASE_URL`·`AIP_TEST_ADMIN_URL`은 작업자 전용 PG 17(포트는 작업자마다 다르게).

새 시험 파일은 묶음별로 따로 만든다(파일 충돌 방지). 이름: `spikes/spike-v1-fixture/tests/expr_<묶음>.rs`(정의), `spikes/spike-v3-write/tests/expr_<묶음>.rs`(PG 런타임), `spikes/spike-v6-transport/tests/expr_<묶음>_http.rs`(HTTP·멱등), `spikes/spike-v4-worker/tests/expr_ext_create.rs`(확장), `spikes/spike-v5-sdk/tests/expr_<묶음>_codegen.rs`(binding).

RED/GREEN 규칙: 새 시험마다 구현 분기를 잠시 되돌리거나 판정 하나를 뒤집어 실패를 한 번 보고(RED), 원복해 통과를 본다(GREEN). 결과는 명령·revision·결과 한 줄만 남긴다.

## 1. 성공 fixture

| ID | 묶음 | 내용 | 기대 | 층 |
| --- | --- | --- | --- | --- |
| A-S1 | WP2 | `[create Order, create OrderLine refs{order:{created:0}} ×2]` | 행 3개, `created[]` 순서 = 연산 순서, 줄의 `order_id` = 첫 id | v3, v6 |
| A-S2 | WP2 | `[create Order, apply Order.submit target{created:0}]` | 같은 트랜잭션에서 상태 전이 | v3 |
| A-S3 | WP2 | Order `check hasLine when exists OrderLine where order = this` + A-S1 | 커밋 통과. 줄 없는 묶음은 `WIRE.CHECK_FAILED`(lib.rs `check` 커밋 검사. `WIRE.INVARIANT_VIOLATED`는 정원·배타 제약용이다. 독립 검토 T-1: SQL 수준 실행으로 근거가 섰고 최종 확정은 PG 통합 시험의 몫) | v3 |
| A-S4 | WP2 | TS 빌더로 A-S1 작성 | `tsc --noEmit` 통과, wire가 `refs` 형식 | v5 |
| B-S1 | WP1 | `transition take { input { qty: Int(1..100) }; from stock >= input.qty; to stock = stock - input.qty }`, qty 3 | stock 10 → 7 | v1 check, v3 |
| B-S2 | WP1 | Decimal(12,2) `balance = balance - input.amount`, amount `"12.50"` | `"100.00"` → `"87.50"` 정확. 실패 fixture: `"-12.50"`·`"0.00"`은 `WIRE.BAD_VALUE`이고 잔액 불변(B-1, 독립 검토) | v3 |
| B-S3 | WP1 | 효과 `update Wallet where owner = this.owner { balance = balance - this.total }` | 지갑 차감과 주문 전이가 한 트랜잭션 | v3 |
| B-S4 | WP1 | Int 항을 Decimal 필드에 `balance = balance + input.points`(Int) | 정확 | v3 |
| B-S5 | WP1 | 기존 리터럴 증감 정의 | facts에 여전히 `{"increment": n}`, 실행 digest 불변 | v1 |
| C-S1 | WP3 | 확장이 `ctx.data.create("Todo", {...})` | 행 생성, id 반환, 호출 전체 커밋 | v4 |
| C-S2 | WP3 | 확장이 부모 create → 받은 id로 자식 create | 둘 다 커밋 | v4 |
| C-S3 | WP3 | 부재 의존 `allow`가 있는 create grant | 확장 트랜잭션이 SERIALIZABLE(`SHOW transaction_isolation`) | v4 |
| D-S1 | WP4 | `atomic:[{compose Project steps[archive]}, {apply Project.rename values{...}}]` | 한 트랜잭션, `applied[]` 2개 | v3, v6 |

## 2. 실패 fixture

| ID | 분류 | 입력 | 기대 코드 |
| --- | --- | --- | --- |
| A-F1 | 타입 오류 참조 | Order를 만든 연산을 `Product` 참조 필드에 | `WIRE.TYPE_MISMATCH` |
| A-F2 | 없는 단계 | `created: 5`(연산 3개), `created: 1`(자기 자신), `created: 2`(뒤 연산), `created: -1`, `created: "0"` | `WIRE.BAD_VALUE` |
| A-F3 | 허용 안 된 출처 | apply 연산·compose 연산·`id` 없는 resource create를 가리킴 | `WIRE.VALUE_SOURCE_NOT_ALLOWED` |
| A-F4 | 미허용 field | `refs`의 필드가 `expose create fields`에 없음 | `WIRE.FIELD_NOT_EXPOSED` |
| A-F5 | 중복 | 같은 필드가 `values`와 `refs`에 모두 | `WIRE.BAD_VALUE` |
| A-F6 | 미허용 resource | create가 공개되지 않은 resource / apply 대상 resource 불일치 | `WIRE.NOT_EXPOSED` / `WIRE.TYPE_MISMATCH` |
| A-F7 | 가시성 | 만든 부모가 호출자 `rows read`에 안 보임 | `WIRE.MISSING_TARGET`, 전체 롤백 |
| A-F8 | internal | `target.created`로 internal 전이 호출 | `WIRE.NOT_EXPOSED` |
| B-F1 | 정의 | `to stock = stock * 2` | `DEF.LEX_UNEXPECTED_CHAR`(현행) |
| B-F2 | 정의 | `stock = stock - input.missing` | `DEF.UNKNOWN_FIELD` |
| B-F3 | 정의 | `stock = stock - a - b`, `stock = other - 1` | `DEF.ARITH_NOT_ALLOWED` |
| B-F4 | 정의 | `input { qty: Int? }`, nullable 대상·항 | `DEF.TYPE_MISMATCH` |
| B-F5 | 정의 | Decimal(12,4) 항을 Decimal(12,2) 필드에, Decimal 항을 Int 필드에 | `DEF.TYPE_MISMATCH` |
| B-F6 | 정의 | 필드 읽기 정책 필드를 항으로. 변형: `allow`·`from`·효과 `when`에서 `input.*`과 가려진 필드를 같은 비교식(`cmp`·`in`·`exists` 안쪽 포함)에(B-2). 서로 다른 비교식의 `and`, 가려지지 않은 필드는 허용 | `DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION` |
| B-F7 | 정의 | `repeat unchanged` + arith | 기존 증감과 같은 오류 |
| B-F8 | 정의 | compose `transitions`에 input 전이 | `DEF.UNSUPPORTED` |
| B-F9 | 요청 | input 키 누락 / 모르는 키 / input 선언 없는 전이에 input | `WIRE.BAD_VALUE` / `WIRE.FIELD_NOT_EXPOSED` / `WIRE.UNKNOWN_KEY` |
| B-F10 | 범위 | 입력 range 밖, Decimal 자릿수 초과 | `WIRE.BAD_VALUE`(DB 전) |
| B-F11 | 오버플로 | 결과가 field range 밖, bigint 초과, numeric(p,s) 초과 | `WIRE.BAD_VALUE`, 전체 롤백, 값 불변 |
| B-F12 | 상태 | `from stock >= input.qty` 거짓 | `WIRE.INVALID_STATE` |
| B-F13 | 부호(B-1) | Int `input`에 range 없음 또는 하한 < 1, 음수·0 리터럴 항(`"-1.50"`, `"0.00"`, `0`) | `DEF.BAD_RANGE` / `DEF.ARITH_NOT_ALLOWED` |
| B-F14 | 부호(B-1) | Decimal `input` `"-100.00"`·`"0.00"`, `this.g`가 음수·0인 행에서 전이와 효과 | `WIRE.BAD_VALUE`(input은 DB 전, `this.g`는 같은 문장의 RETURNING 검사), 전체 롤백 |
| B-F15 | 효과 피연산자(B-3) | 효과의 앞 항을 `this.f`로 씀, 전이 행에 같은 이름 필드가 있음 | `DEF.ARITH_NOT_ALLOWED` / `DEF.AMBIGUOUS_NAME` |
| C-F1 | 미허용 operation | access에 `create.Todo` 없이 `ctx.data.create` | 확장 결과 `WIRE.EXTENSION_ERROR`, cause `ctx.create.access_not_declared`, 롤백 |
| C-F2 | 미허용 field | create 계약 밖 필드 | cause `ctx.create.field_not_exposed`, 롤백 |
| C-F3 | 권한 | create `allow` 거짓 | cause `ctx.create.forbidden`, 롤백 |
| C-F4 | 오류 삼킴 | 확장이 create 실패를 catch하고 성공 반환 | 커밋 안 함(poison) |
| C-F5 | key 형식 | 잘못된 key 문자열 | cause `ctx.create.key_format`, 롤백. C-1 수정이 없으면 통과할 수 없다(RED로 확인 가능, T-3) |
| D-F1 | 중첩 | compose 안에 compose, atomic 안 atomic | 둘 다 `WIRE.UNKNOWN_KEY`. atomic 안 atomic은 `BAD_VALUE`가 아니다(연산 객체가 `create` 분기로 떨어져 `keys(m, ["create","values"])`에 걸린다. 독립 검토 T-2) |
| D-F2 | 크기 | compose 대상×단계 + 연산 행 > 1000 | `WIRE.BULK_LIMIT` |
| X-F1 | MySQL 가드 | arith·refs·중첩 compose facts를 MySQL writer에 | `WIRE.UNSUPPORTED`(mysql feature 시험에서만, 1.0 범위 밖) |

## 3. 롤백·동시성·멱등

| ID | 시나리오 | 기대 | 재사용 |
| --- | --- | --- | --- |
| R-1 | 연산 3이 실패하는 묶음(앞 2개 create) | 행 0개, `aip_idem` 기록 없음, 같은 key 재시도는 재실행 | v6 `bundle_http.rs` 패턴 |
| R-2 | 확장이 create 뒤 throw | 행 0개 | v4 `write_owned_transaction.rs` |
| K-1 | stock 10에 `take(qty=1)` 20개 동시 | 정확히 10개 성공, stock 0, 나머지 `WIRE.INVALID_STATE` | v3 `adv_write_concurrency.rs` |
| K-2 | 같은 지갑 동시 차감(B-S3) 두 주문, 잔액은 하나만 충족 | 하나만 커밋, 잔액 음수 없음. Int 지갑은 range 사후 검사(`WIRE.BAD_VALUE`), Decimal 지갑은 대상의 `check balance >= 0`(`WIRE.CHECK_FAILED`)이 지킨다. `check` 없는 Decimal 지갑은 두 주문이 모두 커밋되어 음수가 되는 음성 시험을 남기고 정의 검사는 `DEF.ADVISORY_EFFECT_NO_LOWER_BOUND`를 낸다(B-4) | v3 `expr_arith.rs`(`k2_…`, `b4_…`) |
| K-3 | 같은 부모에 자식 bundle 두 개 동시 + 부모 전이 | 교착 시 40P01 재시도 후 둘 다 커밋 또는 결정적 거절 | v3 `audit_bd_lock_order.rs` |
| K-4 | 유일 제약이 있는 create를 확장 둘이 동시 | 하나 `WIRE.ALREADY_EXISTS` 원인, 롤백. 호출자가 받는 코드는 C-2 수정 전에는 `EXTENSION_ERROR`, 수정 후 `WIRE.ALREADY_EXISTS`(T-4) | v4 |
| K-5 | 같은 주인의 주문 둘(같은 지갑)을 한 번에 결제(bulk) | `WIRE.EFFECT_TARGET_MISSING`, 지갑·주문 불변. 효과 산술은 호출당 대상 행 하나에 하나씩만 대응한다(B-6) | v3 `expr_arith.rs`(`k5_…`) |
| I-1 | 같은 key 재전송(A-S1, B-S1) | `replayed: true`, 같은 `created` id, 재고 한 번만 차감 | v6 `bundle_http.rs` |
| I-2 | 같은 key, `refs` 또는 `input`만 다름 | `WIRE.IDEMPOTENCY_MISMATCH` | v6 |
| I-3 | COMMIT 응답 유실(fault proxy) | `WIRE.COMMIT_UNKNOWN` → 같은 key 재시도가 `replayed` | v3 `commit_classification.rs`, v6 `commit_unknown_wire.rs`, `aip-test-fault-proxy` |
| I-4 | 커밋 전 연결 끊김 | `WIRE.DB_UNAVAILABLE`, 재시도 재실행, 이중 차감 없음 | v3 `commit_classification.rs` |

## 4. 계약 버전 일관성

| ID | 검사 | 방법 |
| --- | --- | --- |
| V-1 | 기존 정의의 지문·facts·실행 digest 불변 | 4e132564 binary(`/tmp/aip-expr-target/debug/aip`를 보관하거나 같은 SHA로 재빌드)와 새 binary로 저장소 `.aip` 전부를 `aip service check`·`gen` → `executionDigest`·`contractFingerprint` diff 0 |
| V-2 | 골든 계약·client 불변 | `cargo test --manifest-path spikes/spike-v5-sdk/Cargo.toml` 골든 시험, `AIP_UPDATE_GOLDEN` 없이 |
| V-3 | 새 기능 정의만 지문 변화 | input 전이·refs 메타데이터가 있는 fixture에서만 지문이 달라지고, refs 메타데이터는 지문 밖 접미부 |
| V-4 | 구버전 서버 fail-closed | 현행 binary에 `refs`·`input`·중첩 compose 요청 → `WIRE.UNKNOWN_KEY`(이번 세션 실행 확인, 아래) |
| V-5 | 버전 표지 | `node scripts/check-versions.mjs`, `spec/definition-syntax.md`는 `AIP_UPDATE_GOLDEN=1 cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --test diagnostics`로 재생성(syntax_card.rs가 원천) |
| V-6 | 오류 코드 레지스트리 | 새 WIRE 코드 없음을 `spec/wire-errors.md` diff로 확인 |

V-4 실행 증거(4e132564 binary, `/tmp/aip-expr-run`):

```
refs key:         {"code":"WIRE.UNKNOWN_KEY","msg":"create 연산에 알 수 없는 키 `refs`",...}
apply input key:  {"code":"WIRE.UNKNOWN_KEY","msg":"apply 요청에 알 수 없는 키 `input`",...}
compose in atomic:{"code":"WIRE.UNKNOWN_KEY","msg":"create 연산에 알 수 없는 키 `compose`",...}
```

## 5. 묶음별 명령

| 묶음 | 수정 중(좁게) | 기능 완성 후(PG 통합) |
| --- | --- | --- |
| WP1 | `cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --test expr_arith` , `... --test general_update --test diagnostics` | `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test expr_arith --test counters --test general_update --test adv_write_concurrency`, `cargo test --manifest-path spikes/spike-v5-sdk/Cargo.toml --test expr_arith_codegen --test v14_apply_codegen` |
| WP2 | `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test expr_refs` | `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test expr_refs_http --test bundle_http --test commit_unknown_wire`, `cargo test --manifest-path spikes/spike-v5-sdk/Cargo.toml --test expr_refs_codegen --test write_contract` |
| WP3 | `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --lib job_create_tests` | `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --test expr_ext_create --test write_ctx_key_and_rejection --test write_owned_transaction --test write_wire` |
| WP4 | `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test expr_compose_bundle` | `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test expr_compose_bundle_http --test bundle_http` |
| 공통 마감 | V-1~V-6 | `cargo build -p aip-cli --bin aip` 후 `/tmp` 임시 앱으로 serve 실행 1회(A-S1, B-S1, C-S1, D-S1) |
