# agent-lowering-log (to_core / validate / aip core)

## 단계 1: 명세 읽기 (aip-ir lib.rs, forms.rs, ast.rs, model.rs, check.rs, lower.rs, shape.rs, schema.rs)
관찰:
- check.rs::name(): 이름 해석 순서는 (1) scopes 의 vars (param/let/bind/alias/event binding), (2) row 필드 (scope.row, 안쪽 스코프부터), (3) 대문자 이름이면 enum 값, (4) 그 외는 AIP-E104. config/flag 이름을 expression 에서 해석하는 분기는 없음 (flag(X) 인자도 같은 경로로 E104).
- lower::expand 는 Expose 를 지우지 않고 file.decls 에 파생 Query/Command 를 덧붙임. 따라서 하강 시 Expose 는 건너뛴다 (파생 intent 가 이미 있음).
- Shorthand `FieldAssign::Named{value:None}` 은 check.rs 가 Expr 없이 `name()` 만 호출하므로 a.types 에 타입이 없음. 타입은 대상 필드 타입으로 대체.
- Kw::SelfRow 의 sema 타입은 Bool (check.rs).
- Duration 초 환산은 aip-pg sqlexpr::duration_seconds 와 동일 (월=30일, 년=365일) 규칙 사용 예정.

## 단계 2: to_core.rs 초안 작성 + 컴파일
- crates/aip-sema/Cargo.toml 에 aip-ir 의존 추가, lib.rs 에 `pub mod to_core;` 연결.
- API: `to_core(a) -> (Program, SourceMap)`, `to_core_with_notes(a) -> (Program, SourceMap, Vec<LowerNote>)` (NoteKind: Unresolved/Approx/Proposal/Info).
- 스코프 스택(Param/Local + row)은 check.rs 의 push/set_expr/bind 호출 위치를 그대로 따라감.
- 결과: `cargo build -p aip-sema` 통과.

## 단계 3: validate 구현 (crates/aip-ir/src/validate.rs)
- 코드 AIP-I101..I117 (I101 버전, I102 enum, I103 record/event:, I104 Ref/RefUnion/Snapshot, I105 Set/List max=0, I106 Ref 필드 target, I107 Inverse target/via, I108 Field 식, I109 RowField, I110 EnumValue, I111 Callee, I112 insert/upsert/toggle, I113 lifecycle, I114 event, I115 Pred, I116 엔티티 참조, I117 constraint 필드).
- negative control: `valid_program_has_no_diagnostics`(양성 대조) + 규칙당 1개 이상 테스트(각 테스트는 "진단이 정확히 1개이고 해당 코드"를 요구).
- 결과: `cargo test -p aip-ir` -> `test result: ok. 19 passed; 0 failed`

## 단계 4: CLI `aip core <file> [--digest]`
- crates/aip-cli/src/main.rs: `Cmd::Core` + `lower_core()` (Check/compile 와 같은 방식으로 진단 출력, 오류면 FAILURE). 출력 전 `validate`, 진단이 있으면 stderr + 종료코드 1.
- 실행: `aip core examples/shop/app.aip --digest` -> `sha256:80cd942a...` (exit 0); sema 오류 파일은 `error AIP-E102 ...` 후 exit=1.

## 단계 5: 테스트 crates/aip-sema/tests/core_ir.rs (13개)
- (a) 두 예제: validate 0, Unknown 타입 0, Unresolved 노트 0. (b) JSON 왕복 == 원본. (c) 두 번 하강 digest 동일. (d) 주석/빈 줄만 다른 소스 digest 동일 + 대조(소스맵은 달라야 함). (e) enum 값 추가 / Text 범위 변경 -> digest 다름.
- 추가: 이름 종류(Param/Local/RowField/EnumValue/Pred/Field.entity), 설탕 제거(spread 펼침, shorthand), 선언 타입 refinement 유지, Idempotency/Shape::None, 탐지기 negative control, 정합 conformance 케이스 하강 후 validate 0.
- Unknown 타입 식: ariari 0, shop 0 (위치 목록 없음).

## 관찰 (판정 아님)
- aip-pg shape.rs 출력과 비교: 두 예제 intent 84개의 output 구조(필드 집합, 중첩, nullable)가 IR Shape 와 모두 일치 (ad hoc 스크립트, 커밋 안 함; json 타입은 pg 쪽이 `other` 로 표기해서 동일 취급).
- Expression `ty` 는 sema 타입이라 refinement 없음. 선언 위치(Field/Param/Record 필드/event 필드/config/fn ret)만 refinement 유지. shorthand `x` 와 `x: x` 가 같은 IR 이 되도록 바인딩 타입도 refinement 를 제거해서 보관 (처음엔 shorthand 만 refinement 가 붙어서 digest 가 달라졌고, 수정 후 shop digest 가 3e57... -> 80cd... 로 바뀜).
- check.rs 는 fn 본문, consume, projection, outbound webhooks, impersonate, upcast, config 기본값을 검사하지 않음: 이 부분의 expression 타입은 Unknown 이 되어 to_core 가 리터럴/필드/호출 정도만 보조 추론. 예제에는 해당 구문 없음 (conformance 케이스에도 안 걸림, 확인 못함: 다른 코퍼스).
- check.rs 는 config/flag 이름을 expression 에서 해석하지 않음(E104). to_core 는 해석 순서 끝에 config/flag 를 Config 로 하강하는 확장을 둠 (현재 sema 가 오류로 막으므로 실제로는 도달 안 함).
- Expose: lower::expand 후에도 file.decls 에 남음. 파생 intent 만 하강하고 Expose 자체는 건너뜀 (Info 노트).
- `Update ... via path alias` 는 IR 에 필드가 없어 앞에 Let 을 붙여 근사 (예제에는 없음, 파서 테스트에만 존재).
- 기존 clippy 경고 2건이 aip-ir 타입 정의(large_enum_variant: Form, Constraint)에 있었음 -> 타입 모양은 바꾸지 않고 `#[allow(clippy::large_enum_variant)]` 속성만 추가 (직렬화 영향 없음).

## 결과 명령
- `cargo fmt --all && cargo clippy -q --workspace --all-targets` -> 경고 0 (출력 없음, exit 0)
- `cargo test -q --workspace` -> 합계 passed=61 failed=0 (기존 29 + aip-ir 19 + core_ir 13)

## 하강하지 못했거나 근사한 구문
1. `actor X via auth.oidc(kakao)`: 식별자 인자(kakao, google)는 Text 리터럴로 근사 (Callee 인자에 symbol 표현 없음).
2. `RichText(policy: basic)` (ariari 45:9, 335:9): 옵션 policy 는 IR Type::RichText 에 필드가 없어 버려짐 (노트 Proposal).
3. `update ... via path alias`: 앞선 Let 로 근사.
4. `Is` 타입 테스트(`x is Entity`, 액터 엔티티): Pred{name=엔티티명} 로 근사 + 노트. (예제에는 없음.)
5. `x in <Range 값>`: InSet 로 하강 (InRange 는 `[lo, hi)` 리터럴 전용). (예제 사용 여부 확인 못함.)
6. 레코드 체크의 bare 필드: RowField{entity: <레코드명>} 로 표현 (validate 가 레코드도 허용).
7. 이벤트 바인딩 타입: Type::Record{name: "event:<Event>"} 로 표현 (validate 가 events 와 대조).
8. Shape: IR List 에 nullable 이 없어 pg 쪽 `nullable` 마크(가드 필드의 list)는 버려짐. 검색(`from X.match()`) 쿼리는 pg 가 output=Null 인데 IR 은 검색 엔티티 기준 shape 를 생성.
9. Counter via: sema 는 `redis` 같은 확장 이름을 via 로 가지는데 IR 주석은 "inverse relation via" 라고 설명함 (validate 는 이 via 를 검사하지 않음).
10. RefUnion 의 `on erase`: IR 에 필드 없음 (예제에서 사용 안 함, 코드에 노트 있음).
11. Credential 타입의 kind: sema Ty::Credential 에 kind 가 없어 expression 타입에서는 kind="" (선언 타입은 보존).

## IR 변경 제안 목록
- Arg/Node: 심볼 인자(`auth.oidc(kakao)`) 표현 (예: Node::Symbol{name}).
- Type::RichText{policy: Option<String>} (또는 refinement 일반화).
- Stmt::Update.via: Option<(Expr, String)>.
- Node::TypeTest{row, entity} (`actor is Member` 류).
- Node::RowField 가 record 를 가리킴 -> RecordField 노드 분리 또는 문서화.
- Type::Event{name} (현재 Record{"event:X"}).
- FieldKind::RefUnion.on_erase.
- Shape::List.nullable.
- Counter.via 의미 정리 (확장 이름 vs inverse relation).
- Node::InSet 과 Range 멤버십의 구분 (필요하면 InRangeValue).

## 미해석 이름 목록
없음 (ariari, shop, 정합 conformance 케이스 전부 Unresolved 노트 0).
