# agent-pass7-log

## 단계 0: 명세/배경 읽기
- 읽음: aip-ir lib.rs/validate.rs, aip-sema to_core.rs, tests/core_ir.rs, 이전 로그, aip-pg shape.rs.
- 관찰: pg shape.rs 에서 list 가 nullable 이 되는 경로는 `field()` 의 `nullable(v, f.optional || guarded)` (Inverse+sub 의 list) 와 `derived()` 의 list + may_be_null. to_core `with_nullable` 이 List 를 그냥 통과시키던 곳이 대응 지점.

## Part A 구현 (1~10)
- aip-ir lib.rs: CORE_IR_VERSION=aip-core/0.2; Type::{RichText{policy}, Event{name}, Symbol}; Node::{Symbol, RecordField, InRangeValue, TypeTest}; Stmt::Update.via: Option<UpdateVia>; FieldKind::RefUnion.on_erase; Counter.via -> store (주석을 "extension 이 보관"으로); Shape::List.nullable.
- validate.rs: I114 가 Event 타입도 검사, I103 에서 `event:` 접두 특례 제거, I109 는 엔티티 전용, 신규 I118(RecordField), I119(Counter store 가 uses 에 선언된 extension 인지; 접두 일치 `store==u`, `u.starts_with(store+".")`, `store.starts_with(u+".")`), TypeTest 는 I116, Update.via/InRangeValue/Symbol 하위 식 순회.
- to_core.rs: Symbol(식별자 인자), RichText policy(`KeyValue(policy, x)`), Update.via(Let 근사 제거), TypeTest(술어가 없고 이름이 엔티티일 때; 술어가 있으면 Pred 우선), RecordField(row_field 소유자가 엔티티가 아니면), Event 타입(바인딩 + `ty_to_ir` 가 sema 의 Record("event:X") 를 Event 로 변환: check.rs 가 이벤트 바인딩 식 타입을 그렇게 표기함), RefUnion.on_erase, Counter.store, Shape::List.nullable(`with_nullable` 가 List 도 처리; pg shape.rs 의 nullable(list) 경로와 일치), InRangeValue(우변 타입이 Range 일 때, 아니면 InSet).
- 해당 구문들의 Proposal 노트 제거. 새 구문 소스에서 Unresolved/Approx/Proposal 노트 0 을 테스트가 강제.
- 관찰: ariari 예제는 이미 `now in period`(Range), `RichText(policy: basic)`, `counter via redis` 를 쓰므로 예제 IR 에도 InRangeValue/RichText.policy/Counter.store 가 나타남(예제 digest 변경).
- 테스트: aip-ir 19 -> 25 (i114_event_type, i109_row_field_is_entity_only, i118_record_field, i119_counter_store(negative: uses 없음/다른 extension 이면 오류), i116_type_test_entity, new_nodes_validate_their_operands), core_ir 13 -> 25 (항목별 양성 테스트 + ir_version_is_0_2 + counter_store_must_be_declared). 기존 dropped_refinement 테스트는 RichText 정책이 이제 보존되므로 `Int(1..5, weird)` 로 바꿈(여전히 Proposal 노트 확인).
- 확인 못함: 항목 8 의 nullable 일치를 두 예제 전 intent 에 대해 자동 비교하는 스크립트는 이번에 다시 돌리지 않음(이전 패스의 ad hoc 비교는 nullable 이 List 에 없을 때 기준). 테스트로는 guarded 인버스 필드 하나만 검증.

## 항목 11 관찰 (검색 쿼리, 판정 아님; 코드 변경 없음)
- aip-pg plan.rs:1182-1184: `FromClause::Call` 은 `self.fail(call.span, "search queries are not available yet")` 로 진단을 내고 variants 를 만들지 않음. plan.rs `query_output` 은 Entity/Param 소스만 엔티티를 얻고 `_ => None` 이라 output = Value::Null.
- sema check.rs:1302-1314: `from X.match(q) r` 의 alias 타입은 `Ty::Entity(search.entity)` (search 선언의 entity). 즉 sema 는 행을 그 엔티티의 행으로 취급.
- to_core: 검색 선언의 entity 로 Ref 타입 alias 를 만들고 output shape 을 그 엔티티 기준 List<Object> 로 생성. 이 shape 이 실제 런타임 결과 행과 일치하는지는 pg 가 실행 계획을 만들지 않으므로 확인 못함: 검증할 실행 경로가 없음.
- 검색 쿼리를 쓰는 예제/conformance 케이스는 없음 (grep: docs/design/08-coverage-catalog.md:452 의 설계 문서 한 곳뿐).
- 같은 설계 문서 예제(08-coverage-catalog.md:452-456)는 `select { id title rank: r.rank }` 로 검색 행에서 `rank` 를 읽음. `rank` 는 엔티티 필드가 아니므로 검색 결과 행이 "순수한 그 엔티티의 행"이 아니라 엔티티 필드 + 검색 점수를 가진 행일 가능성이 있음. sema 는 이 소스를 검사한 적이 없어 이 식의 타입 결과는 확인 못함.

## Part B-1: negative control (현재 코드)
- crates/aip-runtime/src/validate.rs 에 단위 테스트 `decimal_and_money_reject_everything_else` / `..._accept_only_decimal_strings_and_safe_integers` 추가 후 변경 전 코드에서 실행:
  `cargo test -q -p aip-runtime decimal_and_money` -> `"NaN" must be rejected: String("NaN")` (Money 에 "NaN" 이 통과), 두 테스트 FAILED. 숫자 1200 도 정규화 없이 Number 로 통과(`left: Number(1200) right: String("1200")`).
- 관찰: runtime 의 `TypeSpec::Decimal` 은 unit variant 라 precision/scale 정보가 TypeSpec 에 없음 -> 소수 자릿수 초과 거부는 "확인 못함: TypeSpec::Decimal 에 정밀도 없음(aip-plan lib.rs:76), DDL 컬럼도 무제한 numeric (schema.rs:114)". 구현하지 않음.

## Part B 구현 (2~6)
- 2 입력: runtime validate.rs 에 `decimal_text()`. 문자열은 `^-?(0|[1-9][0-9]*)(\.[0-9]+)?$` 수동 검증, JSON number 는 `as_i64()` 이고 |n|<=2^53-1 일 때만 허용(소수/지수 number 는 as_i64 가 None 이라 거부, u64 초과도 거부). 런타임에는 정규화된 문자열(정수 number 는 `to_string()`)이 전달됨. 메시지 `must be a decimal string like "12.50"`. 자릿수 초과 거부는 "확인 못함: TypeSpec::Decimal 에 precision/scale 이 없고 컬럼도 무제한 numeric".
  - 파라미터는 SQL 에서 텍스트로 바인딩되어 `::numeric` 캐스트(sqlexpr.rs marker/cast)라 문자열이 그대로 저장됨.
- 3 출력(변경 지점 전부):
  1) aip-pg/src/select.rs `scalar_json`: Decimal/Money 를 `to_jsonb((v)::text)` 로(+ `pub` 으로 공개). running_sum, 선택 필드, 집계값이 이 경로.
  2) aip-pg/src/plan.rs 커맨드 `returns` 스칼라: Decimal/Money 면 `select::scalar_json`.
  3) plan.rs group by 쿼리: 그룹 키(스칼라 Decimal/Money)와 select 값(`c.json_sql`).
  4) aip-pg/src/sqlexpr.rs 신규 `Compiler::json_sql`: Decimal/Money 를 `::text` 로. 이벤트 payload(`emit`), deferred effect args, notify payload 에 사용. 이것들은 응답은 아니지만 tokio-postgres 가 jsonb 를 serde_json::Value(f64)로 읽어서 같은 정밀도 손실 경로였기 때문에 같이 바꿈. 핸들러 쪽 읽기는 `->>`/`#>>` 텍스트 후 `::numeric` 캐스트라 문자열도 그대로 동작(e2e 전체 통과).
  - serde_json 은 arbitrary_precision 없이 f64 를 거침(Cargo.toml: `serde_json = "1"`), 그래서 숫자 출력으로는 12345678901234567.89 를 보존할 수 없음.
- 4 TS: aip-plan/src/tsgen.rs `decimal|money -> string`(주석 한 줄). 재생성: `./target/debug/aip gen-ts examples/{ariari,shop}/app.aip examples/{ariari,shop}/client/aip.ts` (ariari 의 FinancialLedgerOutput amount/balance 가 string). `npx tsc --ignoreConfig --noEmit --strict ...` 두 예제 모두 오류 없음(`tsc ariari ok`, `tsc shop ok`).
- 5 계약: aip-plan/src/contract.rs `mark_decimal_wire`: describe 문서 전체에서 kind=decimal|money 객체에 `"wire":"decimal_string"` 과 `"format":"^-?(0|[1-9][0-9]*)(\.[0-9]+)?$"` 를 추가(구조 변경 없이 키 2개 추가).
- 6 테스트: runtime validate.rs 허용표(8건 x 2타입)·거부표(25건 x 2타입, NaN/inf/Infinity/1e400/1E5/1.5e3/012/00.5/" 12"/"12 "/+12/"12."/".5"/"1,000"/"-"/""/0x10/1.5/f64::MAX/2^53/-2^53/u64::MAX/true/[]), aip-plan 단위 테스트 2개(contract wire, tsgen string). ariari_e2e 재무 구간: 금액을 문자열("5000","-1500.25","2000")로 바꾸고 잔액 기대값을 문자열(["5499.75","3499.75","5000"], ClubBalance "5499.75")로, 거부 5건(NaN, 1e400, 1.5, Infinity, " 7")이 `AIP.INPUT.INVALID` path=amount 로 실패하는지, 정밀 금액 "12345678901234567.89" 의 저장(`amount::text`)·원장 amount·running balance("12345678901240067.64")·ClubBalance 왕복. 확인 후 해당 기록 삭제.
  - negative control(e2e): select.rs 를 `to_jsonb(v)` 로 되돌리면 `left: [Number(5499.75), Number(3499.75), Number(5000)] right: [String(...)]` 로 실패 확인 후 복구.
  - shop_e2e: shop 의 amount/price 는 `Int` 타입이라 Decimal/Money 가 없음. 변경 없음.

## 변경 파일
- crates/aip-ir/src/lib.rs, crates/aip-ir/src/validate.rs
- crates/aip-sema/src/to_core.rs, crates/aip-sema/tests/core_ir.rs
- crates/aip-runtime/src/validate.rs
- crates/aip-pg/src/select.rs, crates/aip-pg/src/plan.rs, crates/aip-pg/src/sqlexpr.rs
- crates/aip-plan/src/tsgen.rs, crates/aip-plan/src/contract.rs
- crates/aip-cli/tests/ariari_e2e.rs
- examples/ariari/client/aip.ts, examples/shop/client/aip.ts (재생성)

## 남은 근사/미구현
- Decimal(p,s) 자릿수 초과 거부 미구현(TypeSpec 에 정밀도 없음).
- Credential 표현식 타입의 kind 는 sema 가 주지 않아 "" (이전 패스 11번 항목, 이번 범위 아님).
- Spread 불가 케이스 Proposal 노트, 알 수 없는 refinement 옵션 Proposal 노트는 유지.
- 항목 11(검색 쿼리 shape) 은 관찰만, 판정/변경 없음.
- 숫자 정밀도: JSON 으로 나가는 Decimal/Money 는 문자열이 되었으나, 비-Decimal 타입에서 파생된 numeric(예: Int 나눗셈 결과 `::numeric`) 이 Ty::Decimal 로 타입되지 않는 경우는 여전히 number 일 수 있음. 확인 못함: 해당 경로의 sema 타입 전수 조사는 안 함.

## 최종 명령 출력
- `cargo fmt --all && cargo clippy -q --workspace --all-targets` -> 출력 없음(경고 0), exit 0
- `cargo test -q --workspace` -> 합계 passed=83 failed=0 (61 -> 83: aip-ir +6, core_ir +12, runtime +2, aip-plan +2)
- 서버 프로세스: 띄우지 않음(e2e 는 in-process).
