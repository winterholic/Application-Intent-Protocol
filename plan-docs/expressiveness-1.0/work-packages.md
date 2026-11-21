# 구현 묶음 (수직 단위)

각 묶음은 Parser → AST → Sema → Runtime → SDK → 시험을 끝까지 가진다. 공통 파일은 한 번에 한 묶음만 고친다. 시험 파일은 묶음마다 새 파일을 만든다([test-plan](test-plan.md)). 문서는 WP5가 마지막에 한 번에 고친다.

## 순서와 병렬

```
작업자 1:  WP1 (P0-B 산술) ──▶ WP2 (P0-A 단계 참조) ──▶ WP4 (P0-D compose in bundle)
작업자 2:  WP3 (P0-C 확장 create)            (WP1과 병렬, 파일 공유 없음)
마지막:    WP5 (문서·생성 문서·호환 표)       (WP1~4 병합 뒤)
```

- **직렬**: WP1 → WP2 → WP4. 셋 다 `spike-v3-write/src/lib.rs`와 `spike-v5-sdk/sdk/generic.ts`를 고친다. WP1이 먼저인 이유: `input` 키가 apply 연산 형식을 바꾸고, WP2의 `refs`·`target.created` 판정이 그 위에 얹힌다.
- **병렬 가능**: WP3은 `spike-v4-worker/src/write.rs`만 고친다. 다른 묶음은 이 파일을 건드리지 않는다(아래 소유권). WP1과 동시에 시작한다.
- WP1과 WP2는 사용자 계획대로 한 작업자가 한 브랜치에서 순차로 한다. WP4는 WP2 병합 뒤.

## 파일 소유권

| 파일 | WP1 | WP2 | WP3 | WP4 | WP5 |
| --- | --- | --- | --- | --- | --- |
| spike-v1-fixture/src/ast.rs | ● | | | | |
| spike-v1-fixture/src/parser.rs | ● | | | | |
| spike-v1-fixture/src/hmap.rs (`.h.ts`·`.h.py` 키) | ● | | | | |
| spike-v1-fixture/src/codes.rs | ● | | | | |
| spike-v1-fixture/src/sema.rs | ● | | | | |
| spike-v1-fixture/src/syntax_card.rs → spec/definition-syntax.md | ● | | | | |
| spike-v3-write/src/lib.rs | ● | ●(WP1 뒤) | | ●(WP2 뒤) | |
| spike-v5-sdk/src/lib.rs, python.rs | ● | ●(WP1 뒤) | | | |
| spike-v5-sdk/sdk/generic.ts | ● | ●(WP1 뒤) | | ●(WP2 뒤) | |
| spike-v6-transport/client/typed.ts | | ● | | | |
| spike-v6-transport/src/lib.rs (`bundle_tags`) | | | | ● | |
| product/python/aip_sdk/__init__.py | ● | ●(WP1 뒤) | | | |
| spike-v4-worker/src/write.rs | | | ● | | |
| crates/aip-migrate/src/mysql_write.rs | ● | | | | |
| product/LIMITS.md, SUPPORT.md, SDK.md, GENERAL-UPDATE.md, MANAGED-EXECUTION.md, COMPATIBILITY.md, CHANGELOG.md | | | | | ● |

`●(X 뒤)`는 같은 파일을 X 병합 뒤에 이어서 고친다는 뜻이다. 동시에 두 브랜치가 같은 파일을 열지 않는다.

## WP1. P0-B 제한 산술 (직렬 1번)

| 층 | 수정 | 파일:함수 |
| --- | --- | --- |
| Parser | transition 키 `input` 블록(Params 파서 재사용), 효과 `update` 값이 `additive`를 부르게 | parser.rs:554 근처 transition 키 분기, :573 `"update"`, :1040 `additive` 재사용 |
| AST | `Transition.input: Params` | ast.rs:143 |
| 다른 형식 | `.h.ts`·`.h.py` transition 키 목록에 `input`, 효과 값 산술 | hmap.rs:311, host 파서 |
| Sema | `counter`(:520) 일반화: 항 = 리터럴·`input.p`·`this.g`, Int/Decimal 규칙, NULL 배제. transition 스코프에 `sc.input`(:418 경로). 효과 update 대입에 같은 규칙. compose 금지 규칙(:1363)에 input 전이 추가. facts: 리터럴은 `increment` 유지, 새 형태 `arith` | sema.rs:520, :557, :1162, :1363 |
| 진단 | 메시지·help 갱신, syntax card 행 추가 | codes.rs:84·:434, syntax_card.rs |
| Runtime | `apply_input`(:1414)에 `input` 바인딩, `judge_query`(:379) allow/from에 input 값, `update`(:548) `arith` SET, range 사후 검사(:571) 확대, `effects`(:795) update 대입의 arith와 range 검사 | spike-v3-write/src/lib.rs |
| MySQL 가드 | `arith`·input을 만나면 `WIRE.UNSUPPORTED` | mysql_write.rs:477, :704 |
| SDK | TS `ApplyRequest`에 input, 계약 생성(지문은 input 전이 있는 정의만 변화), Python helper `input=` | spike-v5-sdk/src/lib.rs:17·:51, python.rs:167, sdk/generic.ts |
| 시험 | v1 `tests/expr_arith.rs`, v3 `tests/expr_arith.rs`, v5 `tests/expr_arith_codegen.rs` | B-S1~5, B-F1~12, K-1, K-2, I-1, I-2, V-1, V-2 |

- 좁은 시험(수정 중): `cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --test expr_arith --test general_update --test diagnostics`
- PG 통합(완성 후): `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test expr_arith --test counters --test general_update --test adv_write_concurrency` 와 `cargo test --manifest-path spikes/spike-v5-sdk/Cargo.toml --test expr_arith_codegen --test v14_apply_codegen`
- 예상 빌드 비용(측정 기준, [build-baseline](build-baseline.md)): sema 수정 뒤 v1 시험 약 1초, v3 PG 시험 재빌드 2~5초, aip 바이너리 재링크 약 2.5초. 콜드 시작이면 v3 테스트 첫 빌드 약 6초.

## WP2. P0-A 단계 간 참조 (직렬 2번, WP1 뒤)

| 층 | 수정 | 파일:함수 |
| --- | --- | --- |
| Parser·AST·Sema | 없음(정의 문법 변화 없음). 생성 계약에 ref 필드의 대상 resource 메타데이터만 필요 | - |
| Runtime | `bundle_ops`(:1578)에서 `refs`·`target.created` 정적 판정(C0-2, P0-A 규칙 ①~⑥). `bundle_run`(:1603)에서 `created_rows[i]`를 `Src::Lit`로 해석해 `ensure_refs_readable`·`create_row`에 넣고, apply 대상은 `TargetSpec::Ids` | spike-v3-write/src/lib.rs |
| SDK(TS) | 타입 `refs`, `Created<R>` 핸들, 빌더 `client.bundle(b => ...)`. ref 메타데이터는 `write_binding` 접미부(지문 밖) | sdk/generic.ts:63~78, spike-v5-sdk/src/lib.rs `write_binding`, spike-v6-transport/client/typed.ts:27 |
| SDK(Python) | `bundle()`이 `refs` 전달, `created(i)` 도우미 | product/python/aip_sdk/__init__.py:440 |
| 시험 | v3 `tests/expr_refs.rs`, v6 `tests/expr_refs_http.rs`, v5 `tests/expr_refs_codegen.rs`, TS 부정 타입 시험 `sdk/expr-refs-types.ts`(type-neg.ts 형식) | A-S1~4, A-F1~8, R-1, K-3, I-1~4, V-3 |

- 좁은 시험: `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test expr_refs`
- PG 통합: `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test expr_refs_http --test bundle_http --test commit_unknown_wire` 와 `cargo test --manifest-path spikes/spike-v5-sdk/Cargo.toml --test expr_refs_codegen --test write_contract`
- 예상 빌드 비용: v3 수정 뒤 v6 시험 재빌드 약 2.5초(측정), v6 콜드 첫 빌드 약 13초(측정).

## WP3. P0-C PG 쓰기 확장 create (병렬)

| 층 | 수정 | 파일:함수 |
| --- | --- | --- |
| Parser·AST·Sema | 없음. sema.rs:2217이 `create.<R>` grant를 이미 검증한다 | - |
| Runtime | `run_write_in`(:513)에 `op == "create"` 분기: grant 확인 → `job_create_request`(:705) → `spike_v3_write::bundle_in` → `{"created":[...]}` 응답. `write_needs_serializable`(:162)이 `create.<R>` grant의 `exposeCreate.isolation`을 보게. `ctx_request`가 `input` 키도 옮기게(WP1의 ctx.data.apply input 지원. WP1 병합 전에는 서버가 `UNKNOWN_KEY`로 거부해 안전) | spike-v4-worker/src/write.rs |
| SDK | worker.mjs·worker.py의 `create`는 이미 있다. 변경 없음 | - |
| 시험 | v4 `tests/expr_ext_create.rs`, 확장 fixture `tests/fixtures/expr-ext-create/*.mjs` | C-S1~3, C-F1~5, R-2, K-4 |

- 좁은 시험: `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --lib job_create_tests`
- PG 통합: `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --test expr_ext_create --test write_ctx_key_and_rejection --test write_owned_transaction --test write_wire`
- 예상 빌드 비용: v4 콜드 첫 빌드 약 9초(측정), 이후 약 2초.
- `spike-v3-write`의 공개 함수 `bundle_in`만 부른다. WP2가 `bundle_run` 내부를 바꿔도 시그니처는 그대로이므로 충돌이 없다. WP2 병합 뒤 WP3 시험을 한 번 다시 돌린다.

## WP4. P0-D bundle 안 compose (직렬 3번, WP2 뒤)

| 층 | 수정 | 파일:함수 |
| --- | --- | --- |
| Parser·AST·Sema | 없음(WP1이 compose의 input 전이 금지를 이미 넣는다) | - |
| Runtime | `bundle_run`에 `compose` 연산 분기(`compose_plan`·`compose_exec` 재사용), 행 수 합산, 한 단계 중첩만. `bundle_needs_serializable`(:1346)이 compose 연산에 `compose_needs_serializable` 적용. `refs`가 compose 연산을 가리키면 `VALUE_SOURCE_NOT_ALLOWED` | spike-v3-write/src/lib.rs |
| Transport | `bundle_tags`가 중첩 compose 태그 포함 | spike-v6-transport/src/lib.rs:251 |
| SDK | `BundleOp`에 `ComposeRequest` | sdk/generic.ts:67 |
| 시험 | v3 `tests/expr_compose_bundle.rs`, v6 `tests/expr_compose_bundle_http.rs` | D-S1, D-F1, D-F2, A-F3(compose 참조) |

- 좁은 시험: `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test expr_compose_bundle`
- PG 통합: `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test expr_compose_bundle_http --test bundle_http`

## WP5. 문서·호환 (마지막)

LIMITS E1·E2·E7 갱신(E3~E6 유지), SUPPORT 4·16, SDK 묶음 절, GENERAL-UPDATE(input), MANAGED-EXECUTION(ctx.data.create), COMPATIBILITY "1.0.x에서 더한 정의 문법" 표, CHANGELOG. `spec/definition-syntax.md`는 WP1이 syntax_card.rs로 재생성한 것을 그대로 쓴다. 마감 시험: test-plan V-1~V-6과 `/tmp` 임시 앱 serve 실행 1회.

## 묶음 공통 완료 조건

1. 묶음의 새 시험 파일이 RED(구현 분기 되돌림) → GREEN을 보였다.
2. 좁은 시험과 PG 통합 시험이 통과했다(명령·revision·결과 한 줄 기록).
3. V-1(기존 정의 digest·지문 불변)을 묶음마다 돌린다. 하나라도 바뀌면 병합하지 않는다.
4. 전체 워크스페이스 시험은 돌리지 않는다. CI가 맡는다.
