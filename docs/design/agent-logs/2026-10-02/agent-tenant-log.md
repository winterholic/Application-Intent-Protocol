# agent-tenant log

## 0. 관찰 (구현 전)
- `internal` intent는 현재 어디서도 호출할 수 없다: `Engine::call`이 internal을 REQUEST_UNKNOWN_INTENT로 거부하고, `dispatch.rs`의 `timer.run`(at ... run)도 `engine.call`을 거치므로 internal 대상이면 실패한다. cross tenant intent를 실제로 돌리려면 신뢰된 진입점이 필요하다 -> `Engine::call_internal` 추가 + timer.run이 그것을 쓰도록 변경 예정.
- 엔티티 집합 스캔은 `aip-pg/src/sqlexpr.rs::begin_set` 한 곳(+ `plan.rs::list_sql`이 query main FROM을 직접 만듦)으로 모인다. 필터 삽입 지점으로 적합.
- 가시성 정책(docs/design/04-safety-matrix.md #8, 02-language.md:136): 보이지 않는 행은 없는 행과 구분되지 않아 NOT_FOUND로 통일.

## 1. 구현 단계 (컴파일러/IR)
- IR: `crates/aip-ir/src/tenant.rs` 신설(경로 해석 `path`, 루트 `roots`, 앵커 `checked_params`/`anchor`, 경로 종속 판별 `bound`, 이벤트 앵커 `event_anchor`). Query/Command에 `cross_tenant: bool`(기본 false는 직렬화 생략) 추가, `CORE_IR_VERSION` aip-core/0.4 -> 0.5, `core_ir.rs` 단언 테스트(ir_version_is_0_5)와 계약/클라이언트 골든의 버전 문자열 한 줄 갱신(계약 골든 내용은 이 한 줄 외 불변).
- 코드: AIP-E313(tenant 경로 불량), AIP-E314(앵커 없는 테넌트 스캔), AIP-E315(internal 아닌 intent의 cross tenant), 런타임 AIP.TENANT.MISMATCH(HTTP 404). `analyze.rs`에 규칙, `codes.rs` 레지스트리에 title/explain/fix/example.
- 문법: `internal cross tenant query|command ...` (`cross`는 예약어 추가). internal 없이 `cross tenant`만 쓰면 파싱은 되고 E315로 거부.
- pg: `aip-pg/src/tenant.rs`(행/아이디의 테넌트 SQL, 참조 무결성 트리거 DDL). `sqlexpr.rs::begin_set`와 `plan.rs::list_sql`에 필터 삽입, `tenant_steps`(Load 뒤 일치 검사 Step, `CheckKind::Tenant`), 첫 경로 필드 쓰기 가드. E602에서 tenant 제거.
- 런타임: `Engine::call_internal`(신뢰 진입점) 추가, `timer.run`이 사용. `CheckKind::Tenant` -> AIP.TENANT.MISMATCH.
- 기존 골든(ariari/shop plan, sema_* plan)은 diff 없음 확인(`diff -rq`로 비교, 신규 파일만 추가됨).

## 2. 검증 (e2e + 변이 확인)
- `crates/aip-cli/tests/saas_e2e.rs`: `saas_end_to_end`(정상 흐름, A 사용자의 B id 접근 거부와 행 수 불변, 양쪽 소속자의 침투 probe 전부 거부) / `saas_negative_controls`(같은 probe를 계층별로 끄고 공격이 성공함을 단언).
- negative control 결과(테스트로 상시 실행, 별도 DB):
  - 의도 규칙 끔(모든 intent를 cross tenant로 컴파일, 트리거 유지): MoveTask로 A의 Task가 B Project로 이동 성공, 앵커 아닌 write(MoveToMyDefault)도 성공, `MyTasks(A)`에 B 행(carol-in-B) 노출, `OpenTaskCount(A)`가 두 워크스페이스 합계. 반면 외부 부모 참조(CreateTask parent, PinParent)는 트리거가 여전히 거부.
  - 트리거 끔(의도 규칙 유지): 파라미터/쓰기 가드가 막는 공격(MoveTask, CreateTask parent, MoveToMyDefault, 목록/집계)은 여전히 거부, 그러나 PinParent(파라미터에 없는 B Task를 parent로 저장)와 raw 쓰기(경로 중간 이동으로 하위 행 부모가 다른 테넌트에 남음)는 성공.
  - 둘 다 끔: 모든 probe가 실제 공격으로 성공(예: 외부 parent가 있는 child 행이 1개 생성됨). cross tenant 아카이브와 하위 참조 없는 프로젝트 이동은 어느 구성에서도 성공.
- 구현 변이 확인(코드를 잠시 망가뜨리고 saas_end_to_end가 실패하는지 본 뒤 원복, 최종 재실행 `2 passed`):
  1. 필터 조건을 `true`로: 실패 메시지 `B's row leaked into A's list: ["carol-in-A", "carol-in-B"]`.
  2. `tenant::ddl` 호출 제거(트리거 미생성): `the database refuses the cross-tenant parent: Ok(())`.
  3. Tenant Check Step(파라미터 일치, 쓰기 가드) 제거: 첫 MoveTask probe의 MISMATCH 단언에서 실패.

## 3. 내가 정한 설계 세부와 근거
- 오류 선택: 런타임 `AIP.TENANT.MISMATCH`, HTTP 404, 재시도 불가. 근거: 기본 정책이 "보이지 않는 행은 없는 행과 구분되지 않는다(NOT_FOUND 통일)"(04-safety-matrix #8, 02-language:136). 403이면 상태 코드만으로 "존재하지만 남의 것"이 드러난다. 다만 새 code를 따로 둔 이유는 클라이언트가 "다른 테넌트를 섞었다"를 고칠 수 있는 입력 오류로 구분해야 하기 때문이다. 노출 범위: Load가 가시성을 먼저 적용하므로(안 보이면 NOT_FOUND) MISMATCH는 호출자가 이미 볼 수 있는 두 행에 대해서만 나온다. 트리거가 non-param 참조에서 내는 MISMATCH는 "그 id가 다른 테넌트에 존재함"을 알릴 수 있는 좁은 누수이며 상태 코드(404)는 같다. 확인 못함: 이 좁은 누수를 NOT_FOUND로 완전히 합칠지는 사용자 판단.
- 컴파일 오류 코드: E313(경로 불량), E314(앵커 없는 스캔), E315(cross tenant 오용). 의미 규칙이므로 `aip-ir/src/analyze.rs`에 두고 conformance 케이스(tenant_scan_without_anchor, tenant_path_not_required, tenant_cross_not_internal, ok_tenant_anchored, ok_tenant_cross_internal)로 IR만으로 재현됨을 `ir_json_alone_yields_the_semantic_codes`가 확인.
- 테넌트 범위의 "스캔" 정의: `tenant via`가 붙은 엔티티만. 루트(Workspace)는 앵커로는 인정되지만 스캔 필터 대상이 아니다(내 워크스페이스 목록 같은 질의가 가능해야 함). Membership은 일부러 테넌트 범위로 두지 않았다(한 사람이 여러 테넌트에 속하므로 actor-테넌트 매핑은 테넌트 데이터가 아님).
- 앵커: 엔티티 타입(범위 엔티티 또는 루트) 파라미터. 필터 기준은 선언 순서상 첫 필수(옵션/기본값 없음) 파라미터, 일치 검사는 선택 파라미터와 집합 파라미터까지 포함(집합은 안에서도 한 테넌트여야 함). 필수 앵커가 없고 선택 파라미터만 있으면 스캔은 E314(fail closed).
- 필터 불필요 추론: 파라미터는 일치 검사로 같은 테넌트다. 테넌트를 가진 엔티티 사이 참조는 트리거가 테넌트 교차를 막으므로, 파라미터에서 출발해 (테넌트 엔티티 -> 테넌트 엔티티) 참조 필드나 그 역참조(`project.tasks`, `workspace.projects`)로만 도달한 집합은 항상 같은 테넌트다. 따라서 `ir::tenant::bound`가 true인 집합에는 필터를 넣지 않는다. 출발점이 파라미터가 아니거나(엔티티 스캔, 로컬, actor의 역관계) 중간에 테넌트 없는 엔티티를 지나면 필터가 들어간다(액터 `actor.tasks`처럼 테넌트 없는 엔티티가 범위 행을 참조하는 경우 대비). EXPLAIN으로 확인: `count(workspace.projects p ...)`에는 테넌트 조건이 없고 `count(Task t ...)`에는 있음.
- 쓰기 가드: 경로의 첫 참조 필드에 쓰는 insert/update/set/toggle/upsert는 새 값의 테넌트가 앵커 테넌트와 같은지 먼저 검사(`CheckKind::Tenant`). 이유: DB 트리거는 행 자신의 참조끼리만 비교하므로 "첫 경로 필드를 다른 테넌트의 행으로 바꾸면 행이 그 테넌트로 이사"하는 것을 못 막는다(derived tenant). 이 경우를 막는 것은 의도 단위 규칙뿐이다.
- 트리거 설계: 엔티티별 `<table>__tenant_check(rid, deep, depth)` 함수 + AFTER INSERT OR UPDATE OF <테넌트 참조 컬럼> 트리거. 검사: (1) 자기 참조의 테넌트 일치 (2) UPDATE 시 자신을 참조하는 범위 행의 테넌트 일치 (3) UPDATE 시 경로가 이 행에서 시작하는 하위 행을 재귀 검사(깊이 16 제한). 인서트는 (1)만. 경로 첫 필드는 구성상 일치라 (1)에서 생략.
- actor 없는 문맥의 기본값(관찰 결과): rule은 걸린 행, schedule 본문은 `for` 항목 행(항목 선택 자체는 전 테넌트 스윕), on Event는 이벤트 필드 중 테넌트를 가진 엔티티를 참조하는 첫 필드가 앵커(없으면 범위 스캔 E314). webhook/consume/retain은 페이로드를 읽기 전엔 테넌트를 알 수 없는 시스템 문맥이라 필터 없이 실행(= 암묵적 cross tenant). 이 기본값은 웹훅이 테넌트 테이블을 건드릴 수 있는 열린 구멍이므로 사용자 판단이 필요한 항목이다.
- 문법: `internal cross tenant query|command`. `cross`를 예약어로 추가. 순서는 `internal` 다음 `cross tenant`(D-P18 표기와 같음).
- 런타임: internal intent가 어디서도 호출 불가였던 점을 바로잡기 위해 `Engine::call_internal`을 추가하고 `timer.run`(at ... run)이 쓰게 했다. HTTP는 여전히 internal을 거부(REQUEST_UNKNOWN_INTENT).
- 계약 오류 목록(facts): 파라미터가 서로 다른 테넌트를 섞을 수 있거나(검사 Step 존재) 범위 엔티티에 쓰는 intent에 `AIP.TENANT.MISMATCH`를 올린다(쓰기만 있는 delete도 포함되는 보수적 표시). cross tenant intent는 제외.

## 4. 변경 파일
- 신규: crates/aip-ir/src/tenant.rs, crates/aip-pg/src/tenant.rs, crates/aip-cli/tests/saas_e2e.rs, examples/saas/app.aip, examples/saas/client/aip.ts, conformance/sema/{tenant_scan_without_anchor,tenant_path_not_required,tenant_cross_not_internal,ok_tenant_anchored,ok_tenant_cross_internal}.{aip,expect}, 골든: crates/aip-pg/tests/golden/{saas,sema_ok_tenant_anchored,sema_ok_tenant_cross_internal}.plan.json, crates/aip-cli/tests/golden/saas.{contract.json,client.ts}
- 수정: crates/aip-ir/src/{lib,analyze,codes,facts}.rs, crates/aip-syntax/src/{parser,ast}.rs, crates/aip-sema/src/{to_core,lower}.rs, crates/aip-ir/src/validate.rs, crates/aip-pg/src/{lib,ddl,plan,sqlexpr}.rs, crates/aip-pg/src/plan/rule.rs, crates/aip-plan/src/lib.rs, crates/aip-runtime/src/{engine,exec,dispatch}.rs, crates/aip-contract/src/lib.rs, crates/aip-cli/tests/{common/mod,golden,contract_golden,codes}.rs, crates/aip-sema/tests/core_ir.rs, crates/aip-ir/tests/code_sites.snap(재생성), spec/grammar.md, spec/diagnostics.md(재생성), 버전 문자열만: crates/aip-cli/tests/golden/{ariari,shop}.{contract.json,client.ts}, examples/{ariari,shop}/client/aip.ts

## 5. 남은 TODO / 한계
- L3 폼(approval, job, grant link, verification, subscribe, projection)에서 만든 intent는 테넌트 필터·검사를 받지 않는다(Compiler.tenant 없음, analyze도 forms에는 앵커 규칙 미적용).
- 앵커 없는 intent(예: CreateWorkspace처럼 파라미터 없이 새 루트를 만들고 하위 행을 넣는 흐름)가 서로 다른 루트를 가진 행을 여러 개 쓰는 것은 막지 않는다. 행 내부 참조 일치는 트리거가 보장한다.
- webhook/consume/retain은 시스템 문맥(필터 없음). 위 3절 참조.
- 트리거의 하위 행 재귀 검사는 큰 서브트리를 옮길 때 비용이 든다(드문 연산으로 가정). 데이터 순환 깊이는 16으로 제한.
- `Stmt::Set`/`Update`의 첫 경로 필드 가드는 앵커가 있을 때만 적용(앵커 없는 intent는 비교 대상이 없음).
- 구현 한계: 검증 못함: `consume`은 기존대로 E602라 테넌트 문맥을 시험하지 못함. event handler 앵커(on Event)와 schedule/rule 앵커는 컴파일되지만 e2e로 실행 검증하지 않았다(미실행: saas 예제에 해당 reaction 없음).

## 6. 최종 명령 출력
- `cargo fmt --all && cargo clippy -q --workspace --all-targets` -> 출력        0줄(경고 0)
- `cargo test -q --workspace` -> passed=110 failed=0
- `./target/debug/aip check-docs docs spec README.md` -> 66 blocks: 59 parsed, 0 failed, 7 skipped (fragments with '...')
- `npx tsc --ignoreConfig --noEmit --strict ... examples/saas/client/aip.ts` -> tsc_exit=0
- `pgrep -f "aip run"` -> 서버를 띄운 적 없음
