# 진단 코드 레지스트리 작업 로그

## 1. 전수 조사 (관찰)

방법: 변경 전 소스 grep(`AIP-[EWI][0-9]{3}`, `AIP\.[A-Z_.]+`)로 수집한 뒤, 상수화 후 첫 발생 지점을 다시 뽑았다. 줄 번호는 상수화 후 기준이다. 발생 조건은 레지스트리 `explain`과 같다. 심각도: E/I/런타임 = error, W = warning.


### 컴파일 진단 AIP-E

| 코드 | 상수 정의 | 발생 지점 수 | 첫 발생 지점 | 메시지 예 |
|---|---|---|---|---|
| AIP-E100 | codes.rs:94 | 25 | aip-sema/src/check.rs:1320 | query {} needs 'from' (or 'fetch' for external data) |
| AIP-E101 | codes.rs:95 | 12 | aip-sema/src/check.rs:237 | '{}' shadows another name |
| AIP-E102 | codes.rs:96 | 10 | aip-sema/src/check.rs:225 | unknown type for parameter '{}' |
| AIP-E103 | codes.rs:97 | 14 | aip-sema/src/check.rs:386 | {ent} has no predicate '{}' |
| AIP-E104 | codes.rs:98 | 10 | aip-sema/src/check.rs:299 | 'this' is only valid inside an entity declaration |
| AIP-E105 | codes.rs:99 | 1 | aip-sema/src/model.rs:477 |  |
| AIP-E106 | codes.rs:100 | 3 | aip-sema/src/check.rs:1534 | {}.{} is 'via {}', but {target}.{} is not a reference to {} |
| AIP-E110 | codes.rs:101 | 4 | aip-sema/src/check.rs:283 | 'actor' used but no actor is declared |
| AIP-E201 | codes.rs:102 | 59 | aip-sema/src/check.rs:233 | default for '{}' must be {ty}, got {dt} |
| AIP-E202 | codes.rs:103 | 5 | aip-sema/src/check.rs:519 | '{}' is not a value of {en} |
| AIP-E203 | codes.rs:104 | 3 | aip-sema/src/check.rs:842 | {entity}.{} cannot be assigned |
| AIP-E204 | codes.rs:105 | 4 | aip-sema/src/check.rs:228 | Upload parameters are only allowed on commands |
| AIP-E205 | codes.rs:106 | 2 | aip-sema/src/check.rs:906 | insert {entity} is missing {} |
| AIP-E206 | codes.rs:107 | 2 | aip-sema/src/check.rs:1061 | 'each' needs a bounded collection input, got {t} |
| AIP-E207 | codes.rs:108 | 2 | aip-sema/src/check.rs:980 | upsert {ent} by ({}) needs a matching 'unique ({})' constraint |
| AIP-E209 | codes.rs:109 | 3 | aip-sema/src/check.rs:917 | assignment target must be a field, e.g. 'set order.status = PAID' |
| AIP-E213 | codes.rs:110 | 7 | aip-sema/src/check.rs:248 | {what} is multi-valued ({t}); aggregate it or bind one element |
| AIP-E216 | codes.rs:111 | 1 | aip-sema/src/check.rs:1220 | event {} is emitted with a different shape than at line {} |
| AIP-E220 | codes.rs:112 | 2 | aip-sema/src/check.rs:1261 | '{}' is {t}, not a relation; it cannot have a sub-selection |
| AIP-E221 | codes.rs:113 | 1 | aip-sema/src/check.rs:1356 | sort by {} does not cover {} |
| AIP-E222 | codes.rs:114 | 1 | aip-sema/src/check.rs:1487 | a field can have at most one generator ({}.{}) |
| AIP-E301 | codes.rs:115 | 1 | aip-sema/src/check.rs:1169 | {owner} has no 'allow' clause |
| AIP-E306 | codes.rs:116 | 1 | aip-sema/src/check.rs:291 | 'self' is only valid in field visibility/masking of the actor entity |
| AIP-E309 | codes.rs:117 | 2 | aip-sema/src/check.rs:1468 | {}.{} references personal data but does not say what happens when it i |
| AIP-E310 | codes.rs:118 | 1 | aip-sema/src/check.rs:1019 | erase applies to 'personal' entities; {e} is not personal |
| AIP-E311 | codes.rs:119 | 1 | aip-sema/src/check.rs:1147 | cannot tell who to notify from {e}: it has {refs} references to {actor |
| AIP-E312 | codes.rs:120 | 3 | aip-sema/src/check.rs:1575 | {t} has no 'dynamic schema from ...' and cannot validate {} |
| AIP-E501 | codes.rs:121 | 11 | aip-sema/src/check.rs:665 | '{name}' belongs to extension '{ns}', which is not declared |
| AIP-E600 | codes.rs:122 | 1 | aip-pg/src/sqlexpr.rs:188 |  |
| AIP-E601 | codes.rs:123 | 1 | aip-pg/src/plan.rs:76 |  |
| AIP-E602 | codes.rs:124 | 3 | aip-pg/src/lib.rs:59 | '{what}' on {name} is checked but not enforced by this runtime yet |

### 컴파일 진단 AIP-W

| 코드 | 상수 정의 | 발생 지점 수 | 첫 발생 지점 | 메시지 예 |
|---|---|---|---|---|
| AIP-W402 | codes.rs:125 | 2 | aip-sema/src/check.rs:604 | both sides of this comparison are the same expression |
| AIP-W403 | codes.rs:126 | 1 | aip-sema/src/check.rs:1374 | query {} returns an unbounded list |
| AIP-W404 | codes.rs:127 | 1 | aip-sema/src/check.rs:1817 | schedule {} has a time of day but no timezone |
| AIP-W501 | codes.rs:128 | 1 | aip-sema/src/check.rs:1430 | command {} creates rows but is not idempotent; a client retry creates  |
| AIP-W502 | codes.rs:129 | 1 | aip-sema/src/check.rs:2077 | '{}' is not a first-party extension |
| AIP-W602 | codes.rs:130 | 1 | aip-pg/src/lib.rs:67 | {name}.{} is declared encrypted but is stored in plaintext by this run |

### IR 구조 AIP-I

| 코드 | 상수 정의 | 발생 지점 수 | 첫 발생 지점 | 메시지 예 |
|---|---|---|---|---|
| AIP-I101 | codes.rs:133 | 1 | aip-ir/src/validate.rs:94 | aip_core |
| AIP-I102 | codes.rs:134 | 1 | aip-ir/src/validate.rs:162 | unknown enum '{name}' |
| AIP-I103 | codes.rs:135 | 1 | aip-ir/src/validate.rs:164 | unknown record '{name}' |
| AIP-I104 | codes.rs:136 | 2 | aip-ir/src/validate.rs:168 | unknown entity '{entity}' |
| AIP-I105 | codes.rs:137 | 1 | aip-ir/src/validate.rs:179 | collection bound must be greater than zero |
| AIP-I106 | codes.rs:138 | 2 | aip-ir/src/validate.rs:214 | {path}.kind |
| AIP-I107 | codes.rs:139 | 2 | aip-ir/src/validate.rs:242 | {path}.kind |
| AIP-I108 | codes.rs:140 | 2 | aip-ir/src/validate.rs:627 | unknown entity '{en}' |
| AIP-I109 | codes.rs:141 | 1 | aip-ir/src/validate.rs:611 | row field '{entity}.{field}' does not exist |
| AIP-I110 | codes.rs:142 | 2 | aip-ir/src/validate.rs:620 | unknown enum '{enum_name}' |
| AIP-I111 | codes.rs:143 | 5 | aip-ir/src/validate.rs:535 | unknown intent '{intent}' |
| AIP-I112 | codes.rs:144 | 3 | aip-ir/src/validate.rs:469 | '{entity}.{f}' does not exist |
| AIP-I113 | codes.rs:145 | 3 | aip-ir/src/validate.rs:284 | lifecycle field '{entity}.{}' does not exist |
| AIP-I114 | codes.rs:146 | 1 | aip-ir/src/validate.rs:72 | unknown event '{name}' |
| AIP-I115 | codes.rs:147 | 2 | aip-ir/src/validate.rs:660 | unknown entity '{entity}' |
| AIP-I116 | codes.rs:148 | 1 | aip-ir/src/validate.rs:66 | unknown entity '{name}' |
| AIP-I117 | codes.rs:149 | 2 | aip-ir/src/validate.rs:304 | '{entity}.{f}' does not exist |
| AIP-I118 | codes.rs:150 | 1 | aip-ir/src/validate.rs:616 | record field '{record}.{field}' does not exist |
| AIP-I119 | codes.rs:151 | 1 | aip-ir/src/validate.rs:237 | {path}.kind |
| AIP-I120 | codes.rs:152 | 1 | aip-ir/src/validate.rs:267 | ordinal {} is used by another constraint of '{name}' |
| AIP-I121 | codes.rs:153 | 1 | aip-ir/src/validate.rs:259 | {fpath}.ty |

### 런타임 AIP.*

| 코드 | 상수 정의 | 발생 지점 수 | 첫 발생 지점 | 메시지 예 |
|---|---|---|---|---|
| AIP.AUTH.FORBIDDEN | codes.rs:156 | 10 | aip-ir/src/facts.rs:445 |  |
| AIP.AUTH.UNAUTHENTICATED | codes.rs:157 | 7 | aip-ir/src/facts.rs:428 |  |
| AIP.CONCURRENCY.CONFLICT | codes.rs:158 | 3 | aip-ir/src/facts.rs:434 |  |
| AIP.CONFLICT.CAPACITY | codes.rs:159 | 2 | aip-ir/src/facts.rs:418 |  |
| AIP.CONFLICT.IN_USE | codes.rs:160 | 1 | aip-runtime/src/exec.rs:106 | the row is still referenced |
| AIP.CONFLICT.OVERLAP | codes.rs:161 | 2 | aip-ir/src/facts.rs:416 |  |
| AIP.CONFLICT.STALE_VERSION | codes.rs:162 | 2 | aip-ir/src/facts.rs:448 |  |
| AIP.CONFLICT.UNIQUE | codes.rs:163 | 10 | aip-ir/src/facts.rs:402 | {}_TAKEN |
| AIP.IDEMPOTENCY.KEY_REUSED | codes.rs:164 | 2 | aip-ir/src/facts.rs:430 |  |
| AIP.INPUT.IDEMPOTENCY_KEY_REQUIRED | codes.rs:165 | 1 | aip-runtime/src/engine.rs:253 | send an Idempotency-Key header |
| AIP.INPUT.IDEMPOTENCY_KEY_UNSUPPORTED | codes.rs:166 | 2 | aip-runtime/src/dispatch.rs:213 |  |
| AIP.INPUT.INVALID | codes.rs:167 | 16 | aip-ir/src/facts.rs:427 |  |
| AIP.INPUT.REFERENCE_NOT_FOUND | codes.rs:168 | 1 | aip-runtime/src/exec.rs:108 | a referenced row does not exist |
| AIP.INTERNAL | codes.rs:169 | 6 | aip-runtime/src/engine.rs:165 | query has no executable statement |
| AIP.INVARIANT.VIOLATED | codes.rs:170 | 7 | aip-ir/src/facts.rs:414 | {}_{which} |
| AIP.NOT_FOUND | codes.rs:171 | 5 | aip-ir/src/facts.rs:155 | {}_NOT_FOUND |
| AIP.PRECONDITION.FAILED | codes.rs:172 | 15 | aip-ir/src/facts.rs:162 |  |
| AIP.RATE.LIMITED | codes.rs:173 | 1 | aip-runtime/src/engine.rs:84 | at most {} calls per {}s |
| AIP.REQUEST.MALFORMED | codes.rs:174 | 9 | aip-runtime/src/http.rs:134 | bad multipart body: {e} |
| AIP.REQUEST.UNKNOWN_INTENT | codes.rs:175 | 2 | aip-runtime/src/engine.rs:46 | no intent named '{}' |
| AIP.UNAVAILABLE | codes.rs:176 | 3 | aip-runtime/src/engine.rs:58 | database unavailable |

참고: 첫 발생 지점은 grep으로 뽑은 한 줄이라 메시지 예가 비어 있거나 다른 분기의 문장일 수 있다(PRECONDITION.FAILED, E105 등). 정의의 정본은 레지스트리 `explain`이다.

### 관찰: 같은 코드가 다른 의미로 쓰이는 경우
- AIP-E100: 렉서/파서 문법 오류 25곳 중 check.rs:1320는 의미 검사(`query ... needs 'from'`)다.
- AIP-E101: 중복 선언, 섀도잉, 내장 타입 이름, enum 값 중복, actor 둘, 값 없는 enum, 이중 대입, 이중 선택이 한 코드. 같은 계열인 필드 충돌 한 곳(model.rs 암묵 필드 재선언)만 AIP-E105로 분리돼 있다.
- AIP-E102: 알 수 없는 타입/엔티티, Money 통화 누락, 옵션 없는 타입, 그리고 webhook 이벤트 중복 핸들러(check.rs 2039 부근)가 같은 코드.
- AIP-E104: 알 수 없는 이름/intent/flag/partition, `this` 오용, 발행되지 않는 이벤트.
- AIP-E110: actor 미선언, actor 엔티티 미선언.
- AIP-E201: 59곳. 타입 불일치 외에 "'in' needs at least one value", "events do not take spreads", "an approval needs at least 1 approval", "http.webhook needs secret"도 이 코드.
- AIP-E204: touch 대상이 counter 아님, Upload는 command 전용, `+=`/`-=`는 숫자 필드, 세 의미.
- AIP-E213: 다중값이 필요한 곳 외에 webhook 옵션 오류(check.rs 2022, 2025)가 같은 코드.
- AIP-E501: extension 미선언 외에 "not a statement; extension effects are written ns.effect(...)"(check.rs 1079)와 알 수 없는 webhook source.
- AIP-E312: Snapshot 아님, dynamic schema 없음, 엔티티 직접 참조 세 변형.
- 번호 602: AIP-E602와 AIP-W602가 둘 다 "런타임이 아직 강제하지 않음" 계열로 접두만 다르다. 충돌은 아니다(접두가 다른 별개 코드).
- 백엔드 AIP-E600/E601/E602/W602는 aip_plan::Diagnostic(span 대신 path)으로 나오고 `aip check --json`에는 실리지 않는다. aip-plan/src/lib.rs 주석에도 AIP-E600이 있다.
- 런타임: AIP.UNAVAILABLE은 변경 전 `error.rs::status`에 항목이 없어 기본값 409로 응답했다(레지스트리도 409로 기록, 동작 불변). AIP.INPUT.IDEMPOTENCY_KEY_UNSUPPORTED는 dispatch.rs:212에서 내부적으로 삼켜진다. idempotency 코드가 두 네임스페이스(AIP.IDEMPOTENCY.KEY_REUSED, AIP.INPUT.IDEMPOTENCY_KEY_*)로 나뉘어 있다. 변경 전 `retryable`은 코드와 무관하게 호출부가 `.retryable()`을 붙여야 켜졌다(UNAVAILABLE, CONCURRENCY.CONFLICT, RATE.LIMITED만 붙음).
- AIP.PRECONDITION.FAILED: `Step::Check`에서 reason 없으면 "PRECONDITION"을 채운다(exec.rs).

### 관찰: 번호 공백, 미사용, 문서 불일치
- 변경 전 `spec/diagnostics.md`는 없었다. check.rs:3, diag.rs:24 주석과 docs/design/07이 이 파일을 가리켰고, docs/design/10-philosophy-alignment.md:34는 "아직 없다"고 적고 있다(이번에 만들어졌으나 그 문서는 수정하지 않음).
- E 공백(범위 안): 107-109, 208, 210-212, 214-215, 217-219, 302-305, 307-308. E4xx 없음. W 공백: W401 없음(W402부터), W405 이상 없음.
- I101-I121 연속, 공백 없음. validate.rs 머리 주석에서 I116이 I119 뒤에 적혀 순서만 어긋남.
- 미사용(소스에서 발생 지점이 없는 코드): 없음. 레지스트리 전 항목이 소스에서 상수 또는 문자열로 참조됨(테스트 every_registered_code_is_used_outside_the_registry).
- design 문서와 spec/grammar.md의 옛 이름(E-EFFECT, W-SELF, W-FMT, E-ERASE, E-PATH 등)은 소스에 없다. 레지스트리와 대응표를 만들지 않았다.
- 변경 전 소스에서 코드 문자열을 만드는 동적 조합(format! 등)은 없었다. 테스트 파일의 리터럴: aip-sema/tests/core_ir.rs:431(AIP-I119), validate.rs 테스트 모듈, lexer.rs 테스트.

## 2. 레지스트리 위치와 근거
- 위치: `crates/aip-ir/src/codes.rs` (새 크레이트 없음). 컴파일 진단, IR 구조 코드, 런타임 코드를 한 표(`REGISTRY`)에 둔다.
- 근거: 의존 방향이 syntax(무의존), plan(무의존), ir(무의존), sema(syntax, ir), pg(ir, plan), runtime(plan), contract(ir)다. aip-ir는 이미 I코드(validate.rs)와 런타임 오류 분류(facts.rs, contract에 실리는 errors)를 갖고 있어 두 영역을 모두 품고, 무의존이라 어떤 크레이트가 의존해도 순환이 없다. aip-syntax(제안된 위치)는 ir의 I코드와 런타임 코드를 담을 수 없고, 담으려면 ir와 runtime이 syntax에 의존해야 한다.
- 추가한 의존 간선: aip-syntax -> aip-ir, aip-runtime -> aip-ir (둘 다 상수 사용용). aip-plan은 주석에만 AIP-E600이 있어 간선을 추가하지 않았다.
- 항목: code, kind, severity, title, explain, fix, example(.aip, 파서로 검사), http_status, retryable(런타임만), deprecated. 영문 문장(진단 메시지와 같은 언어)으로 적었고, 문서 머리말만 한국어다.
- 상수화: 소스의 코드 리터럴을 `codes::E103`, `codes::CONFLICT_UNIQUE` 같은 상수로 바꿨다(syntax, sema, pg, ir, runtime의 비테스트 코드 전부). 테스트 모듈 안의 리터럴은 코드 번호 고정 가드라 그대로 뒀다.
- 동작 연결: `AipError::status()`는 레지스트리 `http_status`에서, `AipError::new`의 `retryable` 기본값은 레지스트리에서 온다. 레지스트리에 없는 코드는 409/false(변경 전과 같음). 기존 `.retryable()` 호출은 그대로 남아 있다(중복이지만 무해).

## 3. 완전성 테스트 (crates/aip-ir/tests/codes.rs, 7개)
- (a) every_code_in_the_sources_is_registered: crates/*/src, crates/*/tests의 .rs를 텍스트 스캔(`AIP-[EWI]\d{3}`, `AIP\.[A-Z_.]+`)해 레지스트리와 비교. 이 테스트 파일 자신은 스캔에서 제외(가짜 코드 fixture 때문).
- (b) registry_codes_are_unique, entries_are_well_formed(접두/kind, title 한 줄, runtime만 http_status/retryable, severity).
- 추가: every_registered_code_is_used_outside_the_registry(죽은 항목 탐지), removing_a_code_from_the_registry_is_detected(내장 negative control), http_status_and_retry_follow_the_registry, scanner_reads_both_families.
- (c) conformance_codes_are_registered: crates/aip-cli/tests/codes.rs (conformance/sema/*.expect의 코드 21종 전부 레지스트리에 있음).
- 6번: contract_error_codes_are_registered(ariari, shop 계약의 intent별 errors 코드, 12종이 레지스트리의 런타임 코드). contract_golden은 바꾸지 않았고 통과.
- negative control(수동): 레지스트리에서 AIP-W404 항목을 제거하고 실행 -> `codes used in sources but missing from crates/aip-ir/src/codes.rs: ["AIP-W404"] test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.02s ` (복원 후 7개 통과).

## 4. CLI
- `aip explain-code <CODE> [--json]`: 대소문자 무시, 모르는 코드는 실패 종료. 텍스트는 why/fix/example/http status/retryable.
- `aip diagnostics [--markdown|--json]`: 인자 없으면 한 줄 목록.
- 기존 `aip explain <file> <Intent>`는 그대로.
- `aip check --json`: 기존에 `help` 필드(수정 힌트)가 이미 있다(Option, 있을 때만 출력). 레지스트리의 title/fix는 붙이지 않았다. 레지스트리에 있는 코드의 진단에만 `"explain": "aip explain-code <CODE>"` 필드를 맨 뒤에 추가했다(`Diagnostic`의 Serialize를 수동 구현, 기존 필드 순서와 이름 유지). 테스트 check_json_only_adds_explain이 순서를 확인한다.

## 5. spec/diagnostics.md
- `aip diagnostics --markdown > spec/diagnostics.md`로 생성. 머리말에 코드 체계(E/W/I/AIP.*), 번호 영역, 안정성 규칙, 조회 방법.
- 테스트 spec_diagnostics_is_generated_from_the_registry: 파일이 `render_markdown()`과 같음을 확인, `AIP_UPDATE_GOLDEN=1`로 재생성. `aip check-docs`가 문서 안 .aip 예제를 파서로 검사(58 blocks: 51 parsed, 0 failed).

## 6. 결과 요약
### 코드 총계 (레지스트리 기준)
- AIP-E: 31, AIP-W: 6, AIP-I: 21, 런타임 AIP.*: 21. 합계 79.

### 변경 파일
- 신규: crates/aip-ir/src/codes.rs, crates/aip-ir/tests/codes.rs, crates/aip-cli/tests/codes.rs, spec/diagnostics.md(생성물)
- 수정: crates/aip-ir/src/lib.rs(mod codes), crates/aip-ir/src/{facts,form_intents,validate}.rs(상수화), crates/aip-syntax/Cargo.toml(+aip-ir), crates/aip-syntax/src/{diag,lexer,parser}.rs, crates/aip-sema/src/{check,model}.rs, crates/aip-pg/src/{lib,plan,sqlexpr,ddl}.rs, crates/aip-runtime/Cargo.toml(+aip-ir), crates/aip-runtime/src/{error,exec,engine,http,webhook,dispatch,validate}.rs, crates/aip-cli/src/main.rs(explain-code, diagnostics), Cargo.lock(간선 반영)
- 불변 확인: contract_golden, golden(플랜) 테스트 통과.

### 남은 TODO
- docs/design/10-philosophy-alignment.md:34의 "spec/diagnostics.md는 아직 없다" 문장은 낡았다(수정 안 함, 범위 밖).
- 백엔드 진단(E600/E601/E602/W602)은 aip_plan::Diagnostic이라 `explain` 필드가 JSON에 붙지 않는다(현재 `aip check --json`에 나오지 않음). 확인 필요: 다른 출력 경로가 있는지.
- E201, E101, E104 등 과적재 코드의 분리는 하지 않았다(안정성 규칙상 의미를 바꾸지 않기 위해 관찰만 기록).
- 설계 문서의 옛 이름(E-EFFECT 등) 대응표 없음.

### 최종 명령
- `cargo fmt --all && cargo clippy -q --workspace --all-targets` -> 출력 줄 수:        0 (0이면 경고 없음)
- `cargo test -q --workspace` -> passed 103, failed 0
- `aip diagnostics --markdown | diff - spec/diagnostics.md` -> spec 동일
