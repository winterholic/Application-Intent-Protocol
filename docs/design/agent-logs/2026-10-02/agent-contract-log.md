# agent-contract-log
- [1] 현재 코드로 골든 생성: crates/aip-cli/tests/golden/{ariari,shop}.{contract.json,client.ts}
- [2] 테스트 crates/aip-cli/tests/contract_golden.rs (CLI 바이너리 구동, AIP_UPDATE_GOLDEN=1 때만 재생성). 골든 client.ts == examples/*/client/aip.ts 확인.
- [3] negative control: shop.client.ts 끝에 1바이트 추가 -> 테스트 실패("differs from golden: [shop.client.ts]") -> 원복 -> 통과.
- [4] 분석: 계약의 writes/effects/errors/version 파라미터는 aip-pg 플래너(plan.rs)가 계산, 폼(grant link/verification/approval/job)은 플래너가 intent를 합성함. 이 도출을 이식:
  - aip-ir/src/facts.rs: Stmt 블록 writes/effects/오류코드, 파라미터 NOT_FOUND, 제약조건 위반 오류(DDL 이름 규칙 없이 엔티티별), version 파라미터
  - aip-contract: TypeSpec/Shape JSON, 폼 합성 intent, webhook/job, describe, gen_ts
- [5] IR 부족 판단: 필드 추가 필요 없음(모든 사실을 기존 IR에서 도출 가능). CORE_IR_VERSION 유지(aip-core/0.3).
- [6] 임시 차등 테스트(옛 aip_plan::contract::describe(plan) vs 새 aip_contract::describe(core)): ariari, shop, conformance/sema OK 케이스, + shop에 sequence/slug 필드를 주입한 변형 = 9개 프로그램 모두 serde_json::Value 완전 동일. 이후 임시 테스트(tmp_diff.rs) 삭제.
  커버 못한 경로(확인 못함: 입력 프로그램이 없음): at-most-one/at-least-one 카디널리티, ERASE restrict 정책, query cache, 비-stripe webhook 기본값.
- [7] 호출부 전환: aip-cli contract/gen-ts -> aip_contract::describe/gen_ts(core) (compile_full 신설: core+plan 반환). aip-cli run 이 시작 시 contract = describe(&core) 를 계산해 aip_runtime::Options.contract 로 전달, 런타임 AppState.contract(Arc<Value>)가 /aip/describe 로 서빙. 런타임은 Core IR을 보지 않음. aip-plan 의 contract.rs, tsgen.rs 삭제(테스트 2개는 aip-contract 로 이동).
- [8] 골든: 새 코드 출력 == 골든(바이트 동일), 골든 갱신 없음. 실제 서버 기동(shop, 임시 DB) 후 GET /aip/describe == shop.contract.json 확인(임시 DB 삭제).
- [9] 예제 클라이언트 재생성: examples/{ariari,shop}/client/aip.ts 는 골든 .client.ts 와 동일(내용 변화 없음). tsc --strict 두 예제 통과(tsc-ok-ariari, tsc-ok-shop).

## 관찰: 공개 계약이 클라이언트에 불필요하거나 서버 내부를 드러내는 것 (수정하지 않음)
1. webhooks 섹션 전체: URL, 서명 scheme/header, secret_env(환경변수 이름, 예 STRIPE_WEBHOOK_SECRET), 처리하는 이벤트명. 코드 주석상 "provider 설정자용, 클라이언트용 아님"인데 클라이언트가 읽는 /aip/describe 에 같이 실림.
2. commands[*].writes: 커맨드가 쓰는 엔티티 목록(ariari 에서 25개 엔티티). 내부 데이터 모델 노출. 실제 대상은 테이블이 아니라 엔티티명.
3. commands[*].effects: 확장 연산명(auth.unlink, export.personal, mail.template, s3.put). 사용 중인 외부 서비스가 드러남.
4. commands[*].emits 와 guarantees.events 문구: 내부 이벤트명과 "transactional outbox" 구현 방식.
5. guarantees 문구: "one database transaction", "single snapshot (repeatable read)", round_trips: 1. 저장소/격리수준 구현 세부. 모든 항목에 같은 문구가 반복됨.
6. transport 블록의 고정 문자열(HTTP 경로, 헤더 이름). 계약이 HTTP 개념을 직접 가짐(Core IR 쪽 원칙과는 무관하나 Presentation 계층 안에서도 전송 종류가 하드코딩).
7. errors[*].reason 중 제약조건에서 오는 값(예 {ENTITY}_EXACTLY_ONE, *_TAKEN)과 AIP.CONCURRENCY.CONFLICT, AIP.IDEMPOTENCY.KEY_REUSED: 클라이언트 처리 대상이긴 하나 모든 커맨드에 일괄 부착된 코드가 많아 노이즈.
8. jobs[*].produces(format, expires_seconds): 파일 저장 정책. 클라이언트는 status 의 file URL 만 필요.
9. 계약 version "aip": "0.1" 은 옛 plan IR_VERSION 값을 그대로 이어받은 문자열이라 Core IR 버전(aip-core/0.3)과 무관함.
10. 입력 스키마의 text pattern/min/max, Money/Decimal wire 형식은 클라이언트 검증용이라 적절(노출 대상 아님).

## 중복/드리프트 위험 (TODO 근거)
- aip-ir/facts.rs 의 writes/effects/오류 도출은 aip-pg/plan.rs 가 같은 사실을 SQL 계획 생성 중 따로 계산하는 것과 중복. aip-pg 는 아직 facts 를 쓰지 않음(런타임 실행 계획용 errors/writes 필드가 그대로 남아 있음).
- webhook 기본값(stripe 헤더/시크릿 이름)은 aip-pg/plan/webhook.rs 와 aip-contract/lib.rs 양쪽에 존재.
- 폼이 합성하는 intent(Issue/Redeem/Request/Verify/Approve/Reject/Cancel/Status/job)의 이름·파라미터·오류 코드 규칙이 aip-pg(plan/approval.rs 등)와 aip-contract/forms.rs 양쪽에 존재. 폼의 L3→L2 lowering(OI-IR2) 이후 IR 에 intent 로 내려오면 해소됨.

## 변경 파일
- 신규: crates/aip-contract/{Cargo.toml,src/lib.rs,src/forms.rs,src/shape.rs,src/ty.rs,src/tsgen.rs}, crates/aip-ir/src/facts.rs, crates/aip-cli/tests/contract_golden.rs, crates/aip-cli/tests/golden/{ariari,shop}.{contract.json,client.ts}
- 수정: crates/aip-ir/src/lib.rs(pub mod facts), crates/aip-cli/{Cargo.toml,src/main.rs}, crates/aip-runtime/src/{lib.rs,http.rs}, crates/aip-plan/src/lib.rs(mod 선언 제거), Cargo.lock(자동)
- 삭제: crates/aip-plan/src/contract.rs, crates/aip-plan/src/tsgen.rs
- aip-contract 의존: aip-ir, serde, serde_json 만(cargo tree 확인).

## IR 추가 목록
없음(필드/버전 변경 없음, CORE_IR_VERSION=aip-core/0.3 유지). 새 모듈 aip_ir::facts 는 IR 데이터가 아니라 도출 함수.

## 골든 갱신 여부
없음. 새 코드가 처음부터 바이트 동일.

## 남은 TODO
- aip-pg 가 aip_ir::facts 를 사용하도록 바꿔 중복 제거(그때 facts 와 plan 의 일치를 검증하는 테스트 추가 권장).
- 확인 못함: 입력 프로그램이 없어 차등 검증하지 못한 경로(at-most-one/at-least-one 카디널리티, ERASE restrict 정책, query cache, 비-stripe webhook). 코드는 플래너 로직 대로 이식했으나 골든은 이 경로를 덮지 않음.

## 최종 명령 출력
$ cargo fmt --all && cargo clippy -q --workspace --all-targets
(경고 0, 출력 0줄)
$ cargo test -q --workspace
86 passed, 0 failed (이전 85 + 골든 테스트 1)
