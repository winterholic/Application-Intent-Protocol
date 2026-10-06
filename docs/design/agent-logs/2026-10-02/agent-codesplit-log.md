# codesplit 진행 로그

- 시작: sema 소스 백업 scratchpad/sema-src-before, 발생 지점 추출 sites.tsv (E101 E102 E104 E110 E201 E204 E213 E312 E501, 총      118곳)

## 1. 발생 지점 전수 (과부하 9개 코드, 119곳; 줄 번호는 변경 전 기준)

| 위치 | 옛 코드 | 새 코드 | 의미 그룹 | 메시지 |
|---|---|---|---|---|
| check.rs:225 | E102 | E102 | 알 수 없는 타입·엔티티(선언하기) | unknown type for parameter '{}' |
| check.rs:228 | E204 | E208 | 허용되지 않는 문맥 | Upload parameters are only allowed on commands |
| check.rs:233 | E201 | E201 | 타입 불일치 | default for '{}' must be {ty}, got {dt} |
| check.rs:237 | E101 | E101 | 이름 충돌/중복(이름 바꾸기) | '{}' shadows another name |
| check.rs:248 | E213 | E213 | 다중값을 단일값 자리에 | {what} is multi-valued ({t}); aggregate it or bind one element |
| check.rs:253 | E201 | E201 | 타입 불일치 | {what} must be {want}, got {t} |
| check.rs:283 | E110 | E110 | actor 미선언 | 'actor' used but no actor is declared |
| check.rs:299 | E104 | E208 | 허용되지 않는 문맥 | 'this' is only valid inside an entity declaration |
| check.rs:307 | E110 | E110 | actor 미선언 | 'authenticated' used but no actor is declared |
| check.rs:320 | E201 | E201 | 타입 불일치 | cannot negate {t} |
| check.rs:338 | E201 | E201 | 타입 불일치 | 'in' list item is {ti}, expected {elem} |
| check.rs:342 | E201 | E210 | 비어 있음/최소 미만 | 'in' needs at least one value |
| check.rs:352 | E201 | E201 | 타입 불일치 | range bound is {bt}, value is {t} |
| check.rs:364 | E201 | E201 | 타입 불일치 | right side of 'in' must be a collection or range, got {other} |
| check.rs:374 | E201 | E201 | 타입 불일치 | {tt} cannot be in a collection of {el} |
| check.rs:392 | E201 | E201 | 타입 불일치 | 'is' applies to entity rows, not {other} |
| check.rs:422 | E201 | E201 | 타입 불일치 | 'latest ... by' needs a time or number, got {bt} |
| check.rs:445 | E201 | E212 | 허용되지 않는 형태 | count takes no ': body'; filter with 'where' |
| check.rs:452 | E201 | E201 | 타입 불일치 | cannot aggregate {t} |
| check.rs:468 | E104 | E104 | 알 수 없는 이름(선언/철자) | unknown partition '{}' |
| check.rs:478 | E201 | E201 | 타입 불일치 | if branches differ: {a} vs {b} |
| check.rs:528 | E201 | E201 | 타입 불일치 | '{}' is an entity type, not a value |
| check.rs:533 | E104 | E104 | 알 수 없는 이름(선언/철자) | unknown name '{}' |
| check.rs:592 | E201 | E201 | 타입 불일치 | cannot compare {tl} with {tr} |
| check.rs:600 | E201 | E201 | 타입 불일치 | {tl} has no order; only numbers, times, texts and 'ordered' enums compare with < > |
| check.rs:629 | E201 | E201 | 타입 불일치 | cannot apply '{}' to {a} and {b} |
| check.rs:638 | E201 | E201 | 타입 불일치 | cannot multiply/divide {tl} and {tr} |
| check.rs:665 | E501 | E501 | extension 미선언(use 추가) | '{name}' belongs to extension '{ns}', which is not declared |
| check.rs:687 | E201 | E211 | 인자 개수 | {name} takes {n} argument(s), got {} |
| check.rs:694 | E201 | E212 | 허용되지 않는 형태 | same() takes a binder: same(items x: x.field) |
| check.rs:725 | E201 | E201 | 타입 불일치 | snapshot() takes an entity row, got {other} |
| check.rs:735 | E104 | E104 | 알 수 없는 이름(선언/철자) | unknown flag '{}' |
| check.rs:741 | E104 | E104 | 알 수 없는 이름(선언/철자) | unknown function or relation '{name}' |
| check.rs:750 | E201 | E211 | 인자 개수 | {name} takes {} argument(s), got {} |
| check.rs:756 | E213 | E213 | 다중값을 단일값 자리에 | argument '{}' of {name} is multi-valued ({got}) |
| check.rs:759 | E201 | E201 | 타입 불일치 | argument '{}' of {name} must be {want}, got {got} |
| check.rs:777 | E201 | E201 | 타입 불일치 | expected a set of rows, got {other} |
| check.rs:823 | E102 | E102 | 알 수 없는 타입·엔티티(선언하기) | unknown entity '{}' |
| check.rs:853 | E201 | E201 | 타입 불일치 | {entity}.{} is {want}, got {got} |
| check.rs:859 | E101 | E101 | 이름 충돌/중복(이름 바꾸기) | '{}' assigned twice |
| check.rs:875 | E201 | E201 | 타입 불일치 | spread field '{}' is {rt}, but {entity}.{} is {} |
| check.rs:895 | E201 | E201 | 타입 불일치 | only records can be spread, got {other} |
| check.rs:941 | E204 | E204 | 필드 종류에 안 맞는 연산 | '+='/'-=' need a numeric field; {ent_name}.{} is {} |
| check.rs:944 | E213 | E213 | 다중값을 단일값 자리에 | value for {ent_name}.{} is multi-valued; aggregate it |
| check.rs:946 | E201 | E201 | 타입 불일치 | {ent_name}.{} is {}, got {got} |
| check.rs:949 | E201 | E201 | 타입 불일치 | {ent_name}.{} is not optional and cannot be set to null |
| check.rs:969 | E201 | E212 | 허용되지 않는 형태 | 'insert ... from' creates several rows and cannot be bound with 'as' |
| check.rs:1010 | E201 | E201 | 타입 불일치 | only entity rows can be deleted, got {t} |
| check.rs:1053 | E213 | E213 | 다중값을 단일값 자리에 | 'when' condition is multi-valued |
| check.rs:1079 | E501 | E502 | 효과가 아닌 호출 문장 | '{}' is not a statement; extension effects are written ns.effect(...) |
| check.rs:1093 | E201 | E201 | 타입 불일치 | 'into' target must be an s3.Object field, got {tt} |
| check.rs:1109 | E501 | E501 | extension 미선언(use 추가) | reserve/confirm/release come from extension 'inventory' |
| check.rs:1116 | E104 | E104 | 알 수 없는 이름(선언/철자) | unknown intent '{}' |
| check.rs:1126 | E201 | E201 | 타입 불일치 | personal data export is for the actor entity |
| check.rs:1140 | E501 | E501 | extension 미선언(use 추가) | '{}' belongs to extension '{ns}', which is not declared |
| check.rs:1194 | E201 | E212 | 허용되지 않는 형태 | events do not take spreads; name each field |
| check.rs:1200 | E501 | E501 | extension 미선언(use 추가) | broker '{}' is not a declared extension |
| check.rs:1234 | E101 | E101 | 이름 충돌/중복(이름 바꾸기) | '{}' selected twice |
| check.rs:1295 | E201 | E201 | 타입 불일치 | 'from {}' needs an entity parameter, got {other} |
| check.rs:1299 | E104 | E104 | 알 수 없는 이름(선언/철자) | 'from {}': no such parameter; write 'from Entity alias' |
| check.rs:1308 | E104 | E104 | 알 수 없는 이름(선언/철자) | '{}' is not a search declaration |
| check.rs:1367 | E201 | E201 | 타입 불일치 | 'sort by {} of {{...}}' needs an enum parameter |
| check.rs:1390 | E204 | E204 | 필드 종류에 안 맞는 연산 | 'touch' needs a counter field, '{}' is not one |
| check.rs:1393 | E204 | E204 | 필드 종류에 안 맞는 연산 | 'touch' needs a counter field path |
| check.rs:1409 | E213 | E213 | 다중값을 단일값 자리에 | let {} is multi-valued; use same(...), the ... or an aggregate |
| check.rs:1493 | E201 | E201 | 타입 불일치 | 'on delete/erase' only applies to references; {} is {} |
| check.rs:1496 | E201 | E201 | 타입 불일치 | 'set null' needs an optional field ('{}?') |
| check.rs:1525 | E501 | E501 | extension 미선언(use 추가) | mask function '{}' comes from an undeclared extension |
| check.rs:1549 | E201 | E201 | 타입 불일치 | 'tree' needs a reference to {} itself |
| check.rs:1554 | E501 | E501 | extension 미선언(use 추가) | counter via '{}' needs 'use {}' |
| check.rs:1557 | E201 | E201 | 타입 불일치 | counter fields are Int |
| check.rs:1561 | E201 | E201 | 타입 불일치 | sequence/slug/position fields are Text |
| check.rs:1575 | E312 | E302 | dynamic schema 없음 | {t} has no 'dynamic schema from ...' and cannot validate {} |
| check.rs:1580 | E312 | E312 | validated by는 Snapshot 필드 | '{}' validates against {t}, which can change later; pin it with Snapshot<{t}> |
| check.rs:1587 | E312 | E312 | validated by는 Snapshot 필드 | 'validated by {}' must name a Snapshot field of {} |
| check.rs:1595 | E201 | E201 | 타입 불일치 | default of {} must be {}, got {t} |
| check.rs:1617 | E201 | E201 | 타입 불일치 | lifecycle needs an enum field; {} is {other} |
| check.rs:1625 | E110 | E110 | actor 미선언 | 'visible to actor' needs an actor declaration |
| check.rs:1640 | E201 | E201 | 타입 불일치 | dynamic schema needs a List<Record> field, {} is {other} |
| check.rs:1660 | E201 | E201 | 타입 불일치 | '{}' cannot be part of a unique constraint |
| check.rs:1695 | E201 | E201 | 타입 불일치 | no overlap needs a Range field; {} is {other} |
| check.rs:1722 | E501 | E501 | extension 미선언(use 추가) | actor provider '{}' needs 'use {ns}' |
| check.rs:1725 | E110 | E102 | 알 수 없는 타입·엔티티(선언하기) | actor entity '{}' is not declared |
| check.rs:1744 | E102 | E102 | 알 수 없는 타입·엔티티(선언하기) | unknown type for field '{}' |
| check.rs:1766 | E201 | E201 | 타입 불일치 | relation {} is declared to return {}, but its body is {t} |
| check.rs:1775 | E201 | E201 | 타입 불일치 | relation {} must be a condition (Bool) or declare its result type, got {t} |
| check.rs:1798 | E104 | E104 | 알 수 없는 이름(선언/철자) | event '{}' is never emitted or declared |
| check.rs:1828 | E201 | E201 | 타입 불일치 | retain ... after needs a time, got {t} |
| check.rs:1877 | E501 | E501 | extension 미선언(use 추가) | '{}' needs 'use {ns}' |
| check.rs:1899 | E201 | E201 | 타입 불일치 | 'grants' must name a relation that returns a membership row |
| check.rs:1934 | E201 | E201 | 타입 불일치 | approvers must be {} rows or rows with exactly one {} field |
| check.rs:1944 | E201 | E212 | 허용되지 않는 형태 | approvers need an alias (e.g. 'ClubMember m where ...') |
| check.rs:1947 | E201 | E210 | 비어 있음/최소 미만 | an approval needs at least 1 approval |
| check.rs:2008 | E501 | E104 | 알 수 없는 이름(선언/철자) | unknown webhook source '{via}' (known: {}) |
| check.rs:2010 | E501 | E501 | extension 미선언(use 추가) | '{via}' needs 'use {}' |
| check.rs:2018 | E201 | E214 | webhook 옵션 | '{}' must be a string literal |
| check.rs:2022 | E213 | E214 | webhook 옵션 | unknown webhook option '{}' (known: {}) |
| check.rs:2025 | E213 | E214 | webhook 옵션 | webhook options are named (e.g. secret: \"ENV_NAME\") |
| check.rs:2031 | E201 | E214 | webhook 옵션 | http.webhook needs secret: \"<ENV VAR NAME>\" (the secret itself never goes in source) |
| check.rs:2039 | E102 | E101 | 이름 충돌/중복(이름 바꾸기) | event '{}' is handled twice |
| check.rs:2044 | E201 | E201 | 타입 불일치 | a webhook payload is a record or Json |
| check.rs:2055 | E104 | E104 | 알 수 없는 이름(선언/철자) | unknown intent '{}' |
| model.rs:196 | E101 | E101 | 이름 충돌/중복(이름 바꾸기) | '{}' is a built-in type name |
| model.rs:198 | E101 | E101 | 이름 충돌/중복(이름 바꾸기) | duplicate type name '{}' (first declared at line {}) |
| model.rs:207 | E101 | E107 | actor 중복 선언 | only one actor declaration is supported |
| model.rs:215 | E101 | E101 | 이름 충돌/중복(이름 바꾸기) | duplicate value '{}' in enum {} |
| model.rs:219 | E101 | E210 | 비어 있음/최소 미만 | enum {} has no values |
| model.rs:228 | E101 | E101 | 이름 충돌/중복(이름 바꾸기) | duplicate relation '{}' |
| model.rs:233 | E101 | E101 | 이름 충돌/중복(이름 바꾸기) | duplicate function '{}' |
| model.rs:265 | E101 | E101 | 이름 충돌/중복(이름 바꾸기) | duplicate operation name '{}' (first declared at line {}) |
| model.rs:286 | E102 | E501 | extension 미선언(use 추가) | type '{}' comes from extension '{ns}', which is not declared |
| model.rs:308 | E102 | E108 | 타입 옵션 오류 | Money needs a currency: Money(KRW) |
| model.rs:314 | E102 | E108 | 타입 옵션 오류 | type '{other}' takes no options |
| model.rs:324 | E201 | E201 | 타입 불일치 | Range<{inner}> is not supported; ranges are over time, date or numbers |
| model.rs:335 | E201 | E201 | 타입 불일치 | Snapshot<{other}>: only entities can be snapshotted |
| model.rs:340 | E102 | E102 | 알 수 없는 타입·엔티티(선언하기) | unknown generic type '{other}' |
| model.rs:351 | E201 | E201 | 타입 불일치 | '{other}[]' is only valid for entities (inverse relations) |
| model.rs:358 | E102 | E102 | 알 수 없는 타입·엔티티(선언하기) | unknown entity '{}' in union reference |
| model.rs:398 | E102 | E102 | 알 수 없는 타입·엔티티(선언하기) | unknown type '{n}' |

### 판단 근거 (그룹 결정)
- 원래 의미 유지: E101 이름 충돌, E102 알 수 없는 타입/엔티티, E104 알 수 없는 이름, E110 actor 미선언, E201 타입 불일치(59곳 중 45+3곳 남음), E204 touch/+= 필드 종류, E213 다중값, E312 Snapshot 필드 아님, E501 extension 미선언.
- 합친 것: "알 수 없는 타입"과 "알 수 없는 엔티티"는 수정이 모두 "선언하거나 철자 수정"이라 E102 하나로 둠. actor 엔티티 미선언(E110에서 옴)도 같은 이유로 E102로 이동. webhook 이벤트 중복 핸들러(E102에서 옴)는 "한 스코프에서 같은 이름 둘, 하나를 지운다"라 E101로 이동. 알 수 없는 webhook source(E501에서 옴)는 `use`가 아니라 알려진 목록 중 하나를 고르는 문제라 E104로 이동. 확장 타입의 미선언 extension(model.rs:286, E102에서 옴)은 수정이 `use` 추가라 E501로 이동. 필드 충돌(E105)은 이미 분리돼 있어 그대로.
- 새 코드를 만든 것: E107 actor 중복(수정이 "삭제"이고 이름 변경이 아님), E108 타입 옵션(Money 통화 누락/옵션 없는 타입), E208 문맥 오류(Upload 비-command, this 엔티티 밖), E210 비어 있음/최소 미만('in' 빈 목록, 값 없는 enum, approval 0), E211 인자 개수, E212 허용되지 않는 형태(count body, same binder, 이벤트 spread, insert from + as, approvers alias), E214 webhook 옵션(알 수 없는 옵션, 이름 없음, 문자열 리터럴 아님, http.webhook secret 누락. E201/E213에서 모음), E302 snapshot 대상에 dynamic schema 없음(수정이 E312와 다름), E502 네임스페이스 없는 호출 문장.
- 판단이 갈리는 곳(관찰): E201에 남은 'on delete/erase은 참조에만', "'tree'는 자기 참조", 'unique에 inverse 불가', 'set null은 optional' 등은 필드 종류/제약 위반 성격이라 E204 계열로 볼 여지가 있으나 "선언한 타입이 한정자와 안 맞는다"로 보고 E201에 남김. E210/E212는 "수정이 문서화된 형태로 다시 쓰기"라는 약한 공통점으로 묶은 그룹이다(분리하면 지점 1~2개짜리 코드가 늘어남).
- E110 중 3곳(283, 307, 1625)만 남음. E204는 3곳.
- E100의 `query ... needs 'from'`(의미 검사)는 이번 범위(목록의 과부하 코드)에 없어 건드리지 않음.

## 2. 레지스트리
- 새 항목 9개 추가(E107 E108 E208 E210 E211 E212 E214 E302 E502), 기존 9개 항목의 title/explain/fix를 좁힘(E312 title을 "validated by does not name a snapshot field"로, E204는 title 유지). 번호 공백 확인 후 사용: 107,108 / 208,210,211,212,214 / 302 / 502 (E5xx는 공백이 없어 끝 다음 번호). 남은 공백: E109, E215, E217-219, E303-305, E307-308.
- 주의: 코드 의미를 좁히는 일은 "공개 코드의 의미 불변" 규칙과 충돌할 수 있다. 사용자의 지시로 진행했고, 의미를 바꾸는 쪽이 아니라 섞여 있던 것을 새 코드로 빼는 방향이다. E102/E101/E104/E501로 옮긴 4곳은 해당 코드가 이미 가리키던 의미에 합류시킨 것.

## 3. conformance
- 코드가 바뀌는 기존 케이스: upload_on_query (E204 -> E208). 나머지 기존 케이스(E201, E204 없음, E213, E312, E501, E104)는 불변.
- 추가 케이스(.aip + .expect): actor_declared_twice(E107), money_without_currency(E108), in_list_empty(E210), call_wrong_arg_count(E211), count_with_body(E212), webhook_unknown_option(E214), snapshot_without_dynamic_schema(E302), call_not_an_effect(E104+E502, 미선언 함수라 E104도 같이 나옴), webhook_handled_twice(E101).
- 기존/추가로 덮임: E208은 upload_on_query. 
- 생략(케이스 없음): actor 엔티티 미선언(E102 이동분)은 prelude가 항상 actor를 선언해 conformance 형태로 못 만든다(E107만 나옴). `this` 엔티티 밖(E208), enum 값 없음/approval 0(E210), 알 수 없는 webhook source(E104), 확장 타입 미선언(E501 이동분)은 전용 케이스 없음. 확인 못함: 이 지점들을 단위 테스트로 덮지 않았다.

## 4. 런타임 UNAVAILABLE
- 레지스트리: http_status 409 -> 503, retryable true 유지, title "temporarily unavailable", explain을 "외부 의존성/자원 일시 불가"로 바꿈. 코드 이름은 불변.
- 발생 지점 3곳: engine.rs:58, exec.rs:77, http.rs:85 (모두 DB 풀/연결 실패, `.retryable()` 호출 그대로). HTTP 응답은 `error_response`가 `e.status()`를 쓰므로 레지스트리만 바꿔 503이 된다.
- 테스트: crates/aip-ir/tests/codes.rs의 http_status_and_retry_follow_the_registry에 503/retryable 단언 추가. 신규 crates/aip-cli/tests/unavailable.rs: (1) 닫힌 포트로 만든 풀에서 Engine.call이 UNAVAILABLE, status 503, retryable (2) 실제 서버(serve)에 raw HTTP 요청, 응답이 `HTTP/1.1 503`이고 본문에 AIP.UNAVAILABLE.
- 관찰(변경 안 함): idempotency 코드가 `AIP.IDEMPOTENCY.KEY_REUSED`와 `AIP.INPUT.IDEMPOTENCY_KEY_REQUIRED/UNSUPPORTED` 두 네임스페이스로 나뉘어 있다. 클라이언트 호환 때문에 유지.

## 5. 골든
- spec/diagnostics.md 재생성(`AIP_UPDATE_GOLDEN=1 cargo test -p aip-cli --test codes`), 이후 `aip diagnostics --markdown | diff` 동일. 계약 골든·플랜 골든은 갱신 없이 통과(런타임 코드 이름 불변, 컴파일 진단은 골든에 실리지 않음).

## 6. 회귀 방지 장치
- crates/aip-ir/tests/codes.rs `emission_sites_per_code_are_pinned`: crates/*/src의 `codes::NAME` 사용을 상수·파일별로 세어 crates/aip-ir/tests/code_sites.snap에 고정. 지점이 늘거나 줄면 실패하고 메시지가 "새 지점이 레지스트리 항목과 같은 의미·같은 수정인지 확인, 아니면 새 코드, 그다음 AIP_UPDATE_GOLDEN=1로 재생성"을 안내한다. negative control: 스냅숏의 E101 check.rs 개수를 1로 고치자 `emission_sites_per_code_are_pinned --- FAILED`, 복원 후 통과.
- 한계: 같은 파일에서 한 지점을 다른 코드로 바꾸고 다른 한 곳을 반대로 바꾸는 것은 못 잡는다. 의미 동일성 자체는 기계로 검증하지 못한다(확인 못함: 메시지 의미 비교는 비현실적).

## 7. 요약
### 분리 전후 코드 대응표 (옛 코드 -> 새 코드, 의미)
| 옛 | 새 | 의미 |
|---|---|---|
| E101 | E101 | 이름 충돌/중복/섀도잉/내장 타입명/이중 대입/이중 선택 (+ webhook 이벤트 중복 핸들러) |
| E101 | E107 | actor 선언 둘 |
| E101 | E210 | 값 없는 enum |
| E102 | E102 | 알 수 없는 타입/엔티티 (+ actor 엔티티 미선언) |
| E102 | E108 | Money 통화 누락, 옵션 없는 타입에 옵션 |
| E102 | E101 | webhook 이벤트 중복 핸들러 |
| E102 | E501 | 확장 타입의 extension 미선언 |
| E104 | E104 | 알 수 없는 이름 (+ 알 수 없는 webhook source) |
| E104 | E208 | this 엔티티 밖 |
| E110 | E110 | actor 미선언 |
| E110 | E102 | actor 엔티티 미선언 |
| E201 | E201 | 타입 불일치 |
| E201 | E210 | 'in' 빈 목록, approval 최소 1 |
| E201 | E211 | 인자 개수 |
| E201 | E212 | count body, same binder, 이벤트 spread, insert from as, approvers alias |
| E201 | E214 | webhook 옵션 문자열 리터럴 아님, http.webhook secret 누락 |
| E204 | E204 | touch는 counter, +=는 숫자 |
| E204 | E208 | Upload 파라미터는 command 전용 |
| E213 | E213 | 다중값 |
| E213 | E214 | webhook 옵션 알 수 없음/이름 없음 |
| E312 | E312 | validated by가 Snapshot 필드 아님 |
| E312 | E302 | Snapshot 대상에 dynamic schema 없음 |
| E501 | E501 | extension 미선언 |
| E501 | E104 | 알 수 없는 webhook source |
| E501 | E502 | 네임스페이스 없는 호출 문장 |
| AIP.UNAVAILABLE | 동일 | HTTP 409 -> 503 |

### 변경 파일
- crates/aip-ir/src/codes.rs, crates/aip-ir/tests/codes.rs, crates/aip-ir/tests/code_sites.snap(신규)
- crates/aip-sema/src/check.rs, crates/aip-sema/src/model.rs
- crates/aip-cli/tests/unavailable.rs(신규)
- conformance/sema: upload_on_query.expect 수정, 신규 9쌍(actor_declared_twice, money_without_currency, in_list_empty, call_wrong_arg_count, count_with_body, webhook_unknown_option, snapshot_without_dynamic_schema, call_not_an_effect, webhook_handled_twice)
- spec/diagnostics.md(재생성)

### 남은 TODO
- docs/design/09-verification-log.md:392의 "분리 작업 진행 중" 문장과 docs/design/10:34는 낡았다(수정 안 함, 범위 밖).
- 전용 케이스 없는 이동 지점(E102 actor 엔티티, E208 this, E210 enum/approval, E104 webhook source, E501 확장 타입).

### 최종 명령
- cargo fmt --all && cargo clippy -q --workspace --all-targets -> 출력 줄 수:        0
- cargo test -q --workspace -> passed 106 failed 0
- aip check-docs -> 62 blocks: 55 parsed, 0 failed, 7 skipped (fragments with '...')
- aip diagnostics --markdown | diff - spec/diagnostics.md -> 동일
