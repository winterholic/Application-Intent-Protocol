# agent-tenant2 log

## 0. 시작 관찰
- 이전 로그 읽음. 기준선: 골든 사본을 scratchpad/base 에 보관(diff 요약용).
- 현 구조: 트리거는 `<table>__tenant`(참조 무결성, AFTER INSERT OR UPDATE OF 참조컬럼). 앵커 있는 intent는 c.tenant 스캔 필터 + 검사 Step. 시스템 문맥(webhook/retain/consume)은 필터 없음.
- 디스패처는 outbox 1행당 1트랜잭션(`dispatch::tick`), 한 행 안에서 핸들러 여러 개가 같은 tx. 잡은 100건 배치가 한 tx, 스케줄은 `for` 스윕 전체가 한 tx, 규칙은 settle_rules가 명령 tx 안에서 행마다 실행.

## 1. 구현 진행
- 문법/IR: `cross tenant`를 `on Event`, `schedule`, `rule`, `consume` 앞 수식어와 웹훅 핸들러(`cross tenant on "evt"(p) do`)에 허용. IR `cross_tenant`(기본 false 생략) on Reaction::{Event,Rule,Consume}, Schedule, WebhookOn. CORE_IR_VERSION aip-core/0.5 -> 0.6.
- DB: `<table>__tenant_pin` AFTER INSERT/UPDATE/DELETE 트리거(테넌트 범위 엔티티 + 루트) + 공용 함수 `_aip_tenant_pin(t)`. `aip.tenant`, `aip.tenant_cross`(트랜잭션 로컬 set_config).
- 플랜: 컨텍스트 시작 Step `tenant: pin|reset|cross`(Step::Exec, SQL은 플랜에 고정). 테넌트가 하나도 없는 프로그램에는 안 붙음(ariari/shop 골든 불변).
- 예제 saas 확장 후 e2e 1차 통과(saas_end_to_end).

## 2. 실행 문맥별 테넌트 처리 표
| 문맥 | 트랜잭션 단위 | 시작 Step | 쓰기 고정 | 읽기 필터 | 배치 혼합 여부 |
|---|---|---|---|---|---|
| 앵커 있는 command | 호출 1건 | Load, 일치 검사 뒤 `tenant: pin`(앵커 테넌트) | 앵커로 미리 고정 | 앵커 필터(기존) | 해당 없음 |
| 앵커 없는 command | 호출 1건 | 없음 | 첫 쓰기가 고정, 두 번째 테넌트는 MISMATCH | E314 | 해당 없음 |
| internal cross tenant (command, timer.run) | 호출 1건 | `tenant: cross` | 검사 건너뜀(참조 무결성은 유지) | 필터 없음 | 해당 없음 |
| query | read-only | 핀 없음 | 쓰기 없음 | 앵커 필터(기존) | 해당 없음 |
| on Event 핸들러 | outbox 1행 = tx 1개(`dispatch::tick`), 한 행의 핸들러 여러 개가 같은 tx | 핸들러마다 `tenant: pin`(이벤트 앵커) 또는 `reset` | 앵커/첫 쓰기 | 앵커 필터, 앵커 없으면 E314 | outbox는 행 단위라 섞이지 않음. 한 행의 핸들러끼리는 핸들러별 reset |
| cross tenant on Event | 같음 | `tenant: cross` | 검사 건너뜀 | 필터 없음 | |
| webhook 핸들러 | outbox 1행 = tx 1개 | `tenant: reset` | 첫 쓰기가 고정 | 필터 없음(아래 근거) | 행 단위 |
| cross tenant webhook 핸들러 | 같음 | `tenant: cross` | 건너뜀 | 필터 없음 | |
| schedule (for 항목) | for 스윕 전체가 tx 1개(`schedule::run_body`) | 항목 steps 맨 앞 `tenant: pin`(항목 행의 테넌트) | 항목마다 재고정 | 항목 앵커 필터 | 섞임. 항목마다 reset으로 해결 |
| schedule (for 없음) | tx 1개 | `tenant: reset` | 첫 쓰기가 고정 | 스캔은 E314 | |
| cross tenant schedule | 같음 | `tenant: cross` | 건너뜀 | 필터 없음 | 한 문장이 여러 테넌트 갱신 가능 |
| rule | 변경을 일으킨 tx 안에서 `settle_rules`가 행마다 실행 | 행마다 `tenant: pin`(행의 테넌트, cross 스위치는 끔) | 행마다 재고정 | 행 앵커 필터 | 섞임(cross 문맥에서 여러 테넌트 행이 대기). 행마다 reset |
| cross tenant rule | 같음 | `tenant: cross` | 건너뜀 | | |
| retain | 스케줄 tx 1개, DELETE 한 문장이 모든 테넌트 행 | `tenant: cross` | 프레임워크 정의 행 단위 만료라 건너뜀 | 해당 없음 | 행 독립 처리와 같은 결과 |
| job 시작 command | 호출 1건 | Load 뒤 일치 검사(파라미터 둘 이상일 때) | 큐 테이블만 씀 | 앵커 필터 | |
| job 항목 | 100건 배치 = tx 1개, 단계 steps는 항목마다 | 항목 steps 맨 앞 `tenant: pin`(파라미터 앵커) 또는 `reset` | 항목마다 재고정 | progress 스캔과 item guard에 앵커 필터 자동 삽입 | 섞임(앵커 없는 job). 항목마다 reset |
| approval Request/Approve/Reject/Cancel | 호출 1건 | Load 뒤 `tenant: pin`(대상 행) | 대상 테넌트로 고정 | 승인자 집합과 상태 질의에 앵커 필터 | |
| grant link Issue / Redeem | 호출 1건 | Issue는 일치 검사, Redeem은 scope 복원 뒤 `tenant: pin` | scope 테넌트 | 앵커 필터 | |
| verification Request/Verify | 호출 1건 | 대상이 테넌트 엔티티일 때만 앵커(Verify는 pin) | 앵커 또는 첫 쓰기 | 앵커 필터 | |
| consume | 백엔드가 E602로 거부(실행 불가) | 문법, IR, 분석만 | - | - | 확인 못함: 실행 검증 불가 |

## 3. 내가 정한 세부와 근거
- 핀 트리거를 참조 무결성 트리거와 합치지 않았다. 이유: 무결성은 `UPDATE OF 참조컬럼`으로만 발화하지만 핀은 모든 INSERT/UPDATE/DELETE에서 발화해야 한다(상태 컬럼만 고치는 UPDATE도 다른 테넌트 행이면 막아야 함). 합치면 무결성이 모든 UPDATE마다 돌고, 핀만 끄는 negative control이 불가능해진다. 두 트리거는 이름이 `<table>__tenant`(무결성), `<table>__tenant_pin`(핀)이고 둘 다 TENANT_MISMATCH에 대응.
- 핀 대상 테이블은 테넌트 범위 엔티티와 루트(Workspace). 루트를 빼면 핀된 문맥이 다른 테넌트의 루트 행을 지우거나 고치고, 하위 행은 cascade로 사라진다. 루트의 테넌트는 자기 id.
- UPDATE는 NEW 테넌트뿐 아니라 떠나는 OLD 테넌트도 검사(경로 첫 참조 컬럼이 바뀐 경우만 계산). NEW만 보면 A에 핀된 문맥이 B의 행을 A로 끌어오는 UPDATE가 통과한다. 결과로 이전 로그의 "raw clean move"(아무도 참조하지 않는 프로젝트를 다른 워크스페이스로 raw 이동)는 이제 MISMATCH이고 `cross tenant` 트랜잭션에서만 된다. 테스트 기대를 그렇게 바꿨다.
- DELETE에서 부모가 같은 문장 안에서 이미 사라져 테넌트를 계산할 수 없으면(NULL) 검사를 건너뛴다. 루트 삭제 자체가 핀 대상이므로 cascade 자식은 이미 루트에서 걸러진다.
- 핀 Step은 `Step::Exec`(SQL은 플랜에 고정, 새 Step 종류 없음). 테넌트가 하나도 없는 프로그램에는 붙이지 않아 ariari/shop 플랜 골든이 바이트 불변.
- 앵커 있는 command만 Load 뒤에 pin을 둔다. query는 쓰기가 없고 앵커 필터가 있어 두지 않는다. 기존 첫 경로 필드 쓰기 가드(Check)는 남겼다: 핀 트리거와 겹치지만 DB 왕복 전에 실패하고, 트리거만 끈 negative control(Layer::Triggers)의 기존 단언을 보존한다.
- 읽기: actor 없는 문맥의 필터는 "고정된 aip.tenant 동적 필터"를 택하지 않았다. 근거: (1) 노출 대상 actor가 없어 읽은 값이 호출자에게 가지 않는다. (2) 첫 쓰기로 느슨하게 고정되는 문맥에서는 읽기 시점에 핀이 아직 비어 있을 수 있어 필터가 문맥마다 의미가 달라진다. (3) 읽은 값으로 쓰는 것은 핀 트리거가 묶는다. 남는 경로: 웹훅/핸들러가 다른 테넌트 값을 읽어 outbox 이벤트나 notify 수신자 목록으로 내보내는 것(쓰기 아님)은 막히지 않는다. 앵커가 있는 문맥(이벤트, schedule 항목, rule 행, job)은 이미 정적 필터를 받는다.
- cross tenant 문법은 앞 수식어 `cross tenant on|schedule|rule|consume`, 웹훅은 핸들러 앞. `internal`과 같이 쓰면 오류(파서가 'internal' 뒤에 query/command만 허용). IR 키는 `cross_tenant`(false는 직렬화 생략, 기존 프로그램 직렬화 불변). `CORE_IR_VERSION` aip-core/0.6, `ir_version_is_0_6`로 단언 갱신.
- 컨텍스트 reset과 cross 스위치: rule 행은 cross가 아니면 스위치를 끈다(cross 명령 안에서 정산돼도 본문은 자기 테넌트에 묶임). cross 규칙/스케줄이 켠 스위치는 그 트랜잭션 끝까지 남지만 그 뒤에 테넌트 테이블 쓰기가 없다.
- 분석(E314/forms): approval은 대상 행 엔티티가 앵커, job과 grant link는 파라미터, verification은 target 타입. E314 안내문과 코드 설명을 갱신(`spec/diagnostics.md` 재생성).
- facts: 폼 intents에도 `AIP.TENANT.MISMATCH` 규칙을 적용(`form_intents::command`가 `facts::tenant_errors` 호출). saas 계약 골든에 반영.
- 테스트 편의로 `schedule::run_now`(cron 기록 없이 한 번 실행)와 `schedule::run_body`를 분리해 pub으로 노출. 동작은 `maybe_run`과 같다.

## 4. 검증 요약
- (a) 섞인 웹훅 payload(A 태스크 + B 태스크)는 거부, outbox `last_error`에 `AIP.TENANT.MISMATCH`, 두 태스크 모두 OPEN 유지(변경 행 수 0). (b) 같은 테넌트 payload는 성공. (c) `cross tenant` 웹훅 핸들러와 `cross tenant schedule`(RestoreArchived)은 두 테넌트를 갱신. (d) 다른 워크스페이스의 DONE 태스크를 볼 수 있는 carol이 A용 job을 시작해도 job total이 A의 DONE 수와 같고 B의 DONE 태스크는 그대로. (e) 다른 프로젝트(B) 리뷰어인 carol은 A 태스크의 TaskSignoff 승인이 FORBIDDEN, canVote=false, A의 리뷰어 alice는 승인되어 DONE. (f) 이벤트 두 개(A, B)를 같은 outbox에서 드레인해 코멘트가 각각 생성, schedule 스윕이 A와 B의 DONE 태스크를 한 트랜잭션에서 처리, job 배치에 A와 B 항목이 섞여도 DONE(`welcomed` = A 1, B 2), rule Retro가 한 트랜잭션에서 A와 B의 프로젝트를 모두 정산(각 1건).
- 확인 못함: grant link, verification의 e2e 실행(컴파일 계획만 conformance `ok_tenant_forms`로 고정). consume 실행(백엔드가 E602).

## 5. negative control 결과 (상시 테스트 `saas_negative_controls`, 별도 DB)
- 핀 트리거만 끈 구성(`Layer::Pin`): (a)의 섞인 웹훅이 실제로 성공(`mixed_applied`, outbox 오류 없음, 두 워크스페이스 태스크가 DONE). 그 구성에서도 intent 규칙과 참조 트리거는 그대로 막는다(MoveTask, 외부 parent, PinParent). raw 프로젝트 이동은 성공.
- 전체 트리거를 끈 구성(`Layer::Triggers`): 웹훅 침투 성공 + 기존 negative control 결과 유지.
- 문맥 시작 Step(항목별 reset)을 뺀 구성(`Layer::Reset`): 핀은 켜져 있어 섞인 웹훅은 여전히 거부, 그러나 두 테넌트를 훑는 schedule 스윕이 실패하고, 항목이 두 테넌트인 job 배치가 `AIP.TENANT.MISMATCH`로 실패(커밋된 welcome 태스크 0개).
- 둘 다 끈 구성(`Layer::Both`): 웹훅 침투, job이 B 태스크까지 걸어 archived, 다른 워크스페이스 리뷰어의 승인 성공, 기존 probe 전부 성공.
- 구현 변이 확인(코드를 잠시 망가뜨리고 saas_end_to_end 실패 확인 후 원복): (1) `pin_ddl` 호출 제거 -> 웹훅 단언 실패, (2) job의 앵커 필터 제거 -> `the job walked only the tasks of its own workspace` 실패, (3) approval 앵커 제거 -> `a reviewer of another workspace cannot sign off` 실패. 원복 후 2 passed.
- 골든 diff 요약: ariari/shop 실행 계획 골든 바이트 불변(`diff -q` 확인). 변경: saas.plan.json(+핀 DDL, 핀 Step, 신규 intent), sema_ok_tenant_anchored / sema_ok_tenant_cross_internal(핀 DDL과 Step만), 신규 sema_ok_tenant_forms / sema_ok_tenant_cross_reactions. 계약 골든: ariari/shop은 `core_ir` 버전 한 줄만, saas는 새 intent와 `AIP.TENANT.MISMATCH`가 폼 intent에 붙은 것. 클라이언트 골든은 버전 문자열(ariari/shop)과 saas 신규 함수.

## 6. 변경 파일
- 문법/AST/IR: crates/aip-syntax/src/{ast,parser}.rs, crates/aip-sema/src/to_core.rs, crates/aip-ir/src/{lib,analyze,facts,form_intents,tenant,codes,validate}.rs
- 플랜/DB: crates/aip-pg/src/{lib,tenant}.rs, crates/aip-pg/src/plan.rs, crates/aip-pg/src/plan/{approval,job,rule,webhook}.rs
- 런타임: crates/aip-runtime/src/schedule.rs (run_now, run_body)
- 예제: examples/saas/app.aip, examples/{saas,ariari,shop}/client/aip.ts(버전 문자열, saas는 신규 함수)
- 테스트: crates/aip-cli/tests/saas_e2e.rs, crates/aip-sema/tests/core_ir.rs, conformance/sema/{ok_tenant_cross_reactions,ok_tenant_forms,tenant_schedule_scan_without_cross}.{aip,expect}, crates/aip-pg/tests/golden/*.plan.json(saas, sema_ok_tenant_*, 신규 2), crates/aip-cli/tests/golden/*(contract, client), crates/aip-ir/tests/code_sites.snap
- 문서: spec/grammar.md, spec/diagnostics.md(재생성), docs/DECISIONS.md, docs/design/09-verification-log.md

## 7. 남은 TODO
- grant link, verification의 e2e 실행 검증(현재 플랜 형태만 conformance로 고정).
- consume 실행(E602).
- actor 없는 문맥의 읽기 필터(고정된 aip.tenant 기준 동적 필터)는 보류, 근거는 3절. 웹훅/핸들러가 다른 테넌트 값을 읽어 outbox 이벤트나 notify로 내보내는 경로는 막지 않는다.
- 앵커 있는 command마다 pin Step이 SQL 왕복 한 번을 더한다(필요하면 Load와 합칠 수 있음, 측정 안 함).
- 트리거 비용(행마다 테넌트 서브쿼리 2~3개)은 측정하지 않았다.

## 8. 최종 명령 출력
- `cargo fmt --all && cargo clippy -q --workspace --all-targets` -> 출력 0줄(경고 0)
- `cargo test -q --workspace --no-fail-fast` -> passed=111 failed=0
- `./target/debug/aip check-docs docs spec README.md` -> 66 blocks: 59 parsed, 0 failed, 7 skipped (fragments with '...')
- `npx tsc --ignoreConfig --noEmit --strict --target es2022 --lib es2022,dom ../../examples/saas/client/aip.ts` -> tsc_exit=0
- `pkill -f "aip run"` -> 서버를 띄운 적 없음
