# pass6c log
- baseline: cargo test 86 pass; goldens copied to scratchpad/base
- A: aip-ir에 builtin.rs(webhook_settings, form_enums), form_intents.rs 추가, facts::written_roots/lit_text pub. aip-contract forms.rs/webhooks/enums가 이를 사용. 계약 골든 통과 여부 아래.
- A1 완료: aip-pg plan.rs의 writes/effects/codes 추적(self.writes/effects/codes, self.code) 전부 제거. command()는 facts::command_facts, query()는 facts::query_errors, form 계획은 aip_ir::form_intents의 facts 사용. pg의 written_roots/edited_fields/root/set_root/callee_name/lit_text 중복 삭제(facts의 것을 사용), version_checks는 facts::version_params(VersionParam{of,name,reason})로 교체. lib.rs의 "제약 오류를 command errors에 덧붙이는 루프"는 facts::violation_errors와 중복이라 삭제(런타임은 program.constraint_errors 맵만 사용: aip-runtime/src/exec.rs:79).
  결과 비교: 실행 계획 골든 5개 + 계약 골든 모두 바이트 동일(cargo test --test golden / contract_golden 통과). 즉 pg와 IR 도출 결과의 차이 없음 -> 어느 쪽이 맞는지 판정할 불일치 없음.
- A2: aip-ir/src/builtin.rs::webhook_settings(Webhook)로 pg webhook.rs와 contract webhooks() 단일화. form enum(ApprovalStatus/ApprovalDecision/JobStatus)도 builtin::form_enums로 pg lib.rs, contract 단일화.
- A3: aip-ir/src/form_intents.rs (grant_link/verification/approval/job -> FormIntent{name,params,facts,kind}). contract forms.rs와 pg plan(approval.rs,job.rs,plan.rs)가 이름/파라미터/facts를 여기서 가져옴. 출력 JSON 형태(approval/job status output)는 pg와 contract에 각자 남음(TypeSpec 타입이 달라 이번엔 미통합).
- B1: Type::Json { schema, validated_by } 로 분리(lib.rs). to_core: Json<X> -> schema, `Json validated by p` -> validated_by. validate.rs: schema는 기록 존재(I103), validated_by 머리 필드 존재(신규 AIP-I121). pg dynamic_validations는 validated_by만 사용(이전엔 Json<X>도 동적 검증 후보였던 잠재 오류가 사라짐). 양성 테스트: aip-sema/tests/core_ir.rs named_json_schema_and_dynamic_validation_are_different_types, validate.rs json_schema_record_and_validated_by_field_must_exist. 실행 계획 골든 바이트 동일.
- B2: check.rs에 AIP-E222 "a field can have at most one generator", conformance/sema/two_generators.{aip,expect}. 기존 최대 E221(sort_cases_incomplete)라 다음 번호 E222. conformance 통과.
- B3: CORE_IR_VERSION = aip-core/0.4, core_ir.rs 단언 갱신(테스트명 ir_version_is_0_4).
- C5 negative control (변경 전 코드, Part C 구현 전): contract_golden.rs의 public_contract_carries_no_operator_information 실행 -> FAILED ("ariari: public contract mentions \"webhooks\""), 위 grep으로 shop 계약에도 "secret_env": "STRIPE_WEBHOOK_SECRET", "header": "Stripe-Signature" 존재 확인. operator_view_has_the_webhook_signing_setup도 --operator 미구현이라 FAILED.
- C1: aip-contract에 pub fn operator(core)(webhooks: name/url/source/signature{scheme,header,secret_env}/events, jobs: produces{format,store,bucket,expires_seconds}) 추가. describe()에서 "webhooks" 제거. CLI `aip contract <file> --operator`. 런타임 공개 엔드포인트는 describe 결과만 사용하므로 webhooks 미노출(operator는 어디에서도 서빙 안 함).
- C2: PROTOCOL_VERSION="aip-protocol/0.1" 추가, CONTRACT_VERSION 제거. 계약에 "protocol", "core_ir" 추가, "aip" 제거. TS 생성기는 "aip" 키를 헤더 주석에만 썼음(tsgen.rs generate) -> protocol/core_ir 로 교체. 클라이언트 런타임 코드는 이 값을 쓰지 않음.
- C3 골든 diff 요약 (계약 골든만 AIP_UPDATE_GOLDEN=1로 갱신, 실행 계획 골든은 갱신하지 않음):
  ariari.contract.json / shop.contract.json: 삭제 "aip":"0.1"; 추가 "core_ir":"aip-core/0.4", "protocol":"aip-protocol/0.1"; 삭제 "webhooks"(ariari는 [], shop은 stripe signature 항목 포함 secret_env STRIPE_WEBHOOK_SECRET). 그 외 키(writes/effects/emits/guarantees 등) 변경 없음.
  ariari.client.ts / shop.client.ts: 1행 주석만 변경 "...contract 0.1." -> "...contract aip-protocol/0.1 (core aip-core/0.4)."
  실행 계획 골든(crates/aip-pg/tests/golden/*.plan.json): 변경 없음(Part B-1 포함, 바이트 동일).
- C4: 예제 클라이언트 재생성 후 tsc --strict 두 예제 통과("tsc ariari ok", "tsc shop ok").
- C5: 테스트 contract_golden.rs public_contract_carries_no_operator_information (secret_env, STRIPE_WEBHOOK_SECRET, Stripe-Signature, "webhooks" 부재), operator_view_has_the_webhook_signing_setup. 변경 후 통과.

## 변경 파일
aip-ir: src/lib.rs(Type::Json 분리, 0.4, mod 추가), src/facts.rs(written_roots/lit_text pub, VersionParam), src/builtin.rs(신규), src/form_intents.rs(신규), src/validate.rs(I103 확장, I121, 테스트)
aip-sema: src/check.rs(E222), src/to_core.rs(Json), tests/core_ir.rs(0.4, Json 양성 테스트)
aip-pg: src/plan.rs, src/plan/approval.rs, src/plan/job.rs, src/plan/webhook.rs, src/lib.rs
aip-contract: src/lib.rs(operator, PROTOCOL_VERSION), src/forms.rs, src/tsgen.rs
aip-cli: src/main.rs(--operator), tests/contract_golden.rs, tests/golden/{ariari,shop}.{contract.json,client.ts}
conformance/sema/two_generators.{aip,expect}
examples/{ariari,shop}/client/aip.ts (재생성)

## 남은 TODO
- approval/job status 출력 JSON 형태가 aip-pg(plan/approval.rs, plan/job.rs)와 aip-contract(forms.rs)에 각각 있음(TypeSpec 타입이 달라 이번엔 미통합). 엔진용 enums/records/events 조립도 pg lib.rs와 contract에 중복(enums 값만 builtin으로 통합).
- docs/design/10-philosophy-alignment.md:68 의 secret_env 공개 계약 지적은 이제 해소됨(문서는 건드리지 않음).
- Json<X>가 실제로 record인지 sema 단계 검사는 없음(IR validate에서만 I103로 잡힘): 확인 필요.

## 최종 명령
       0
cargo test --workspace: passed 90 failed 0
