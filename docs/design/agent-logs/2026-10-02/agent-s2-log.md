# 진행 로그 (agent-s2)
- 단계1: IrDiagnostic에 help/related/is_warning 추가, ir::locate 추가(aip-pg locate는 위임)
- 단계2: analyze.rs 작성, check.rs/model.rs에서 S 규칙 제거, to_core 마킹(SourceMap) 추가, aip-sema pipeline.rs + CLI 통합

## 1. 분류(관찰): check.rs / model.rs의 진단 발생 지점 전체 (변경 전 줄 번호, 172곳)

분류 기준: (F) IR이 만들어지기 전에 필요한 이름 해석·타입 추론·구문 형태. (S) 해석이 끝난 IR만 보고 판단 가능. 애매하면 F.
`S(불일치)+F(필드 없음)` 는 한 지점이 둘로 갈린 것(아래 3절).

| 코드 | 위치(변경 전) | 검사 내용 | 분류 |
|---|---|---|---|
| E100 | check.rs:1320 | query {} needs 'from' (or 'fetch' for external data) | S |
| E101 | check.rs:237 | '{}' shadows another name | F |
| E101 | check.rs:859 | '{}' assigned twice | F |
| E101 | check.rs:1234 | '{}' selected twice | F |
| E101 | check.rs:2039 | event '{}' is handled twice | S |
| E101 | model.rs:196 | '{}' is a built-in type name | F |
| E101 | model.rs:198 | duplicate type name '{}' (first declared at line {}) | F |
| E101 | model.rs:215 | duplicate value '{}' in enum {} | F |
| E101 | model.rs:228 | duplicate relation '{}' | F |
| E101 | model.rs:233 | duplicate function '{}' | F |
| E101 | model.rs:265 | duplicate operation name '{}' (first declared at line {}) | F |
| E102 | check.rs:225 | unknown type for parameter '{}' | F |
| E102 | check.rs:823 | unknown entity '{}' | F |
| E102 | check.rs:1725 | actor entity '{}' is not declared | F |
| E102 | check.rs:1744 | unknown type for field '{}' | F |
| E102 | model.rs:340 | unknown generic type '{other}' | F |
| E102 | model.rs:358 | unknown entity '{}' in union reference | F |
| E102 | model.rs:398 | unknown type '{n}' | F |
| E103 | check.rs:386 | {ent} has no predicate '{}' | F |
| E103 | check.rs:401 | scope '{}' is not declared on the actor | S |
| E103 | check.rs:564 | {other} has no field '{}' | F |
| E103 | check.rs:835 | {entity} has no field '{}' | F |
| E103 | check.rs:884 | spread record {r} has field '{}' that {entity} cannot take | F |
| E103 | check.rs:932 | {ent_name} has no field '{}' | F |
| E103 | check.rs:1243 | {row} has no field '{}' | F |
| E103 | check.rs:1620 | {} has no field '{}' | F |
| E103 | check.rs:1643 | {} has no field '{}' | F |
| E103 | check.rs:1664 | {} has no field '{}' | F |
| E103 | check.rs:1678 | {} has no field '{}' | F |
| E103 | check.rs:1698 | {} has no field '{}' | F |
| E103 | check.rs:1702 | {} has no field '{}' | F |
| E103 | check.rs:1973 | {ent} has no field '{}' | F |
| E104 | check.rs:468 | unknown partition '{}' | F |
| E104 | check.rs:533 | unknown name '{}' | F |
| E104 | check.rs:735 | unknown flag '{}' | F |
| E104 | check.rs:741 | unknown function or relation '{name}' | F |
| E104 | check.rs:1116 | unknown intent '{}' | F |
| E104 | check.rs:1299 | 'from {}': no such parameter; write 'from Entity alias' | F |
| E104 | check.rs:1308 | '{}' is not a search declaration | F |
| E104 | check.rs:1798 | event '{}' is never emitted or declared | F |
| E104 | check.rs:2008 | unknown webhook source '{via}' (known: {}) | F |
| E104 | check.rs:2055 | unknown intent '{}' | F |
| E105 | model.rs:477 |  | F |
| E106 | check.rs:1534 | {}.{} is 'via {}', but {target}.{} is not a reference to {} | S(불일치)+F(필드 없음) |
| E106 | check.rs:1544 | 'via' is only valid on inverse relation fields ('Items[] via parent') | F |
| E106 | model.rs:496 | {}.{} needs 'via <backref field>' | F |
| E107 | model.rs:207 | only one actor declaration is supported | F |
| E108 | model.rs:308 | Money needs a currency: Money(KRW) | F |
| E108 | model.rs:314 | type '{other}' takes no options | F |
| E110 | check.rs:283 | 'actor' used but no actor is declared | S |
| E110 | check.rs:307 | 'authenticated' used but no actor is declared | S |
| E110 | check.rs:1625 | 'visible to actor' needs an actor declaration | S |
| E201 | check.rs:233 | default for '{}' must be {ty}, got {dt} | F |
| E201 | check.rs:253 | {what} must be {want}, got {t} | F |
| E201 | check.rs:320 | cannot negate {t} | F |
| E201 | check.rs:338 | 'in' list item is {ti}, expected {elem} | F |
| E201 | check.rs:352 | range bound is {bt}, value is {t} | F |
| E201 | check.rs:364 | right side of 'in' must be a collection or range, got {other} | F |
| E201 | check.rs:374 | {tt} cannot be in a collection of {el} | F |
| E201 | check.rs:392 | 'is' applies to entity rows, not {other} | F |
| E201 | check.rs:422 | 'latest ... by' needs a time or number, got {bt} | F |
| E201 | check.rs:452 | cannot aggregate {t} | F |
| E201 | check.rs:478 | if branches differ: {a} vs {b} | F |
| E201 | check.rs:528 | '{}' is an entity type, not a value | F |
| E201 | check.rs:592 | cannot compare {tl} with {tr} | F |
| E201 | check.rs:600 | {tl} has no order; only numbers, times, texts and 'ordered' enums compare with | F |
| E201 | check.rs:629 | cannot apply '{}' to {a} and {b} | F |
| E201 | check.rs:638 | cannot multiply/divide {tl} and {tr} | F |
| E201 | check.rs:725 | snapshot() takes an entity row, got {other} | F |
| E201 | check.rs:759 | argument '{}' of {name} must be {want}, got {got} | F |
| E201 | check.rs:777 | expected a set of rows, got {other} | F |
| E201 | check.rs:853 | {entity}.{} is {want}, got {got} | F |
| E201 | check.rs:875 | spread field '{}' is {rt}, but {entity}.{} is {} | F |
| E201 | check.rs:895 | only records can be spread, got {other} | F |
| E201 | check.rs:946 | {ent_name}.{} is {}, got {got} | F |
| E201 | check.rs:949 | {ent_name}.{} is not optional and cannot be set to null | F |
| E201 | check.rs:1010 | only entity rows can be deleted, got {t} | F |
| E201 | check.rs:1093 | 'into' target must be an s3.Object field, got {tt} | F |
| E201 | check.rs:1126 | personal data export is for the actor entity | F |
| E201 | check.rs:1295 | 'from {}' needs an entity parameter, got {other} | F |
| E201 | check.rs:1367 | 'sort by {} of {{...}}' needs an enum parameter | F |
| E201 | check.rs:1493 | 'on delete/erase' only applies to references; {} is {} | F |
| E201 | check.rs:1496 | 'set null' needs an optional field ('{}?') | S |
| E201 | check.rs:1549 | 'tree' needs a reference to {} itself | S |
| E201 | check.rs:1557 | counter fields are Int | S |
| E201 | check.rs:1561 | sequence/slug/position fields are Text | S |
| E201 | check.rs:1595 | default of {} must be {}, got {t} | F |
| E201 | check.rs:1617 | lifecycle needs an enum field; {} is {other} | F |
| E201 | check.rs:1640 | dynamic schema needs a List<Record> field, {} is {other} | S |
| E201 | check.rs:1660 | '{}' cannot be part of a unique constraint | S |
| E201 | check.rs:1695 | no overlap needs a Range field; {} is {other} | S |
| E201 | check.rs:1766 | relation {} is declared to return {}, but its body is {t} | F |
| E201 | check.rs:1775 | relation {} must be a condition (Bool) or declare its result type, got {t} | F |
| E201 | check.rs:1828 | retain ... after needs a time, got {t} | F |
| E201 | check.rs:1899 | 'grants' must name a relation that returns a membership row | F |
| E201 | check.rs:1934 | approvers must be {} rows or rows with exactly one {} field | F |
| E201 | check.rs:2044 | a webhook payload is a record or Json | F |
| E201 | model.rs:324 | Range<{inner}> is not supported; ranges are over time, date or numbers | F |
| E201 | model.rs:335 | Snapshot<{other}>: only entities can be snapshotted | F |
| E201 | model.rs:351 | '{other}[]' is only valid for entities (inverse relations) | F |
| E202 | check.rs:519 | '{}' is not a value of {en} | F |
| E202 | check.rs:523 | '{}' belongs to several enums; compare it with a typed value | F |
| E202 | check.rs:1347 | '{}' is not a value of {en} | F |
| E202 | check.rs:1611 | '{}' is not a value of {en} | F |
| E202 | check.rs:1902 | unknown role '{}' | F |
| E203 | check.rs:842 | {entity}.{} cannot be assigned | F |
| E203 | check.rs:936 | {ent_name}.{} cannot be assigned | F |
| E203 | check.rs:1970 | {ent}.{} cannot be written through expose | F |
| E204 | check.rs:941 | '+='/'-=' need a numeric field; {ent_name}.{} is {} | F |
| E204 | check.rs:1390 | 'touch' needs a counter field, '{}' is not one | S |
| E204 | check.rs:1393 | 'touch' needs a counter field path | S |
| E205 | check.rs:906 | insert {entity} is missing {} | S |
| E205 | check.rs:1991 | expose create for {ent} does not accept required field(s) {} | F |
| E206 | check.rs:1061 | 'each' needs a bounded collection input, got {t} | F |
| E206 | check.rs:1070 | 'each' needs an item name: each items x partial { ... } | F |
| E207 | check.rs:980 | upsert {ent} by ({}) needs a matching 'unique ({})' constraint | S |
| E207 | check.rs:1041 | toggle {ent} needs a unique constraint over exactly ({}) | S |
| E208 | check.rs:228 | Upload parameters are only allowed on commands | S |
| E208 | check.rs:299 | 'this' is only valid inside an entity declaration | F |
| E209 | check.rs:917 | assignment target must be a field, e.g. 'set order.status = PAID' | F |
| E209 | check.rs:927 | cannot assign a field of {owner} | F |
| E209 | check.rs:1096 | 'into' target must be a field of a row | F |
| E210 | check.rs:342 | 'in' needs at least one value | S |
| E210 | check.rs:1947 | an approval needs at least 1 approval | S |
| E210 | model.rs:219 | enum {} has no values | S |
| E211 | check.rs:687 | {name} takes {n} argument(s), got {} | F |
| E211 | check.rs:750 | {name} takes {} argument(s), got {} | F |
| E212 | check.rs:445 | count takes no ': body'; filter with 'where' | F |
| E212 | check.rs:694 | same() takes a binder: same(items x: x.field) | F |
| E212 | check.rs:969 | 'insert ... from' creates several rows and cannot be bound with 'as' | F |
| E212 | check.rs:1194 | events do not take spreads; name each field | F |
| E212 | check.rs:1944 | approvers need an alias (e.g. 'ClubMember m where ...') | F |
| E213 | check.rs:248 | {what} is multi-valued ({t}); aggregate it or bind one element | F |
| E213 | check.rs:756 | argument '{}' of {name} is multi-valued ({got}) | F |
| E213 | check.rs:944 | value for {ent_name}.{} is multi-valued; aggregate it | F |
| E213 | check.rs:1053 | 'when' condition is multi-valued | F |
| E213 | check.rs:1409 | let {} is multi-valued; use same(...), the ... or an aggregate | F |
| E214 | check.rs:2018 | '{}' must be a string literal | S |
| E214 | check.rs:2022 | unknown webhook option '{}' (known: {}) | S |
| E214 | check.rs:2025 | webhook options are named (e.g. secret: \"ENV_NAME\") | S |
| E214 | check.rs:2031 | http.webhook needs secret: \"<ENV VAR NAME>\" (the secret itself never goes in | S |
| E216 | check.rs:1220 | event {} is emitted with a different shape than at line {} | S |
| E220 | check.rs:1261 | '{}' is {t}, not a relation; it cannot have a sub-selection | F |
| E220 | check.rs:1264 | relation '{}' needs a sub-selection | F |
| E221 | check.rs:1356 | sort by {} does not cover {} | S |
| E222 | check.rs:1487 | a field can have at most one generator ({}.{}) | F |
| E301 | check.rs:1169 | {owner} has no 'allow' clause | S |
| E302 | check.rs:1575 | {t} has no 'dynamic schema from ...' and cannot validate {} | S |
| E306 | check.rs:291 | 'self' is only valid in field visibility/masking of the actor entity | S |
| E309 | check.rs:1468 | {}.{} references personal data but does not say what happens when it is erased | S |
| E309 | check.rs:1477 | {}.{} is anonymized on erase and must be optional ('{}?') | S |
| E310 | check.rs:1019 | erase applies to 'personal' entities; {e} is not personal | S |
| E311 | check.rs:1147 | cannot tell who to notify from {e}: it has {refs} references to {actor} | S |
| E312 | check.rs:1580 | '{}' validates against {t}, which can change later; pin it with Snapshot<{t}> | S |
| E312 | check.rs:1587 | 'validated by {}' must name a Snapshot field of {} | F |
| E501 | check.rs:665 | '{name}' belongs to extension '{ns}', which is not declared | F |
| E501 | check.rs:1109 | reserve/confirm/release come from extension 'inventory' | F |
| E501 | check.rs:1140 | '{}' belongs to extension '{ns}', which is not declared | F |
| E501 | check.rs:1200 | broker '{}' is not a declared extension | F |
| E501 | check.rs:1525 | mask function '{}' comes from an undeclared extension | F |
| E501 | check.rs:1554 | counter via '{}' needs 'use {}' | F |
| E501 | check.rs:1722 | actor provider '{}' needs 'use {ns}' | F |
| E501 | check.rs:1877 | '{}' needs 'use {ns}' | F |
| E501 | check.rs:2010 | '{via}' needs 'use {}' | F |
| E501 | model.rs:286 | type '{}' comes from extension '{ns}', which is not declared | F |
| E502 | check.rs:1079 | '{}' is not a statement; extension effects are written ns.effect(...) | F |
| W402 | check.rs:604 | both sides of this comparison are the same expression | S |
| W402 | check.rs:763 | {name} is called with the same expression twice | S |
| W403 | check.rs:1374 | query {} returns an unbounded list | S |
| W404 | check.rs:1817 | schedule {} has a time of day but no timezone | S |
| W501 | check.rs:1430 | command {} creates rows but is not idempotent; a client retry creates duplicat | S |
| W502 | check.rs:2077 | '{}' is not a first-party extension | S |

### 코드별 요약 (S 지점 수 / F 지점 수, check.rs+model.rs 기준)

| 코드 | S | F | F 이유 |
|---|---|---|---|
| E100 | 1 | 0 | (query에 from/fetch 없음) |
| E101 | 1 | 9 | 중복 선언·이름 충돌은 이름 해석. IR은 BTreeMap/단일 필드라 중복이 하강 때 사라짐. S는 webhook 이벤트 중복 처리 |
| E102 | 0 | 7 | 타입·엔티티 이름 해석 |
| E103 | 1 | 13 | 필드·이름 해석. S는 `has scope` 가 actor 스코프에 없음 |
| E104 | 0 | 10 | 이름 해석 (IR 쪽 구조 대응은 I108/I111/I114) |
| E105 | 0 | 1 | 암묵 필드와의 선언 충돌 |
| E106 | 1 | 2 | `via` 형태와 `via` 필드 해석은 F. 존재하는 필드가 되돌아 참조하지 않는 경우만 S |
| E107 | 0 | 1 | IR이 actor를 하나만 담아 정보 소실 |
| E108 | 0 | 2 | 타입 표현식 옵션 구문 |
| E110 | 3 | 0 | actor 선언 없음 (actor/authenticated/visible to actor) |
| E201 | 7 | 41 | 식 타입 추론·호환은 F. S는 생성기·counter·set null·unique 열·no overlap·dynamic schema 필드 모양 |
| E202 | 0 | 5 | enum 값 이름 해석. lifecycle 값도 여기(3절) |
| E203 | 0 | 3 | 값 타이핑 전에 건너뜀(하강에 타입 필요), 애매하므로 F |
| E204 | 2 | 1 | S는 touch 대상이 counter가 아님. `+=` 숫자 필드는 타입이라 F |
| E205 | 1 | 1 | S는 insert/upsert/toggle 필수 필드 누락. expose 전용 메시지는 expose가 AST에서 전개되어 IR에 없으므로 F |
| E206 | 0 | 2 | 바운드 컬렉션 입력 타입 |
| E207 | 2 | 0 | upsert/toggle의 unique |
| E208 | 1 | 1 | S는 Upload 파라미터(query 등). `this` 문맥은 F |
| E209 | 0 | 3 | 대입 대상 형태/타입 |
| E210 | 3 | 0 | 빈 enum, 빈 `in []`, approval 0건 |
| E211 | 0 | 2 | 호출 해석(인자 쌍 맞추기) |
| E212 | 0 | 5 | 구문 형태 |
| E213 | 0 | 5 | 다중값 타입 |
| E214 | 4 | 0 | webhook 옵션 |
| E216 | 1 | 0 | 이벤트 모양 |
| E220 | 0 | 2 | 타입 기반 하위 선택 |
| E221 | 1 | 0 | 정렬 case 완전성 |
| E222 | 0 | 1 | IR은 필드당 생성기 하나라 두 번째가 하강에서 사라짐 |
| E301 | 1 | 0 | allow 누락 |
| E302 | 1 | 0 | Snapshot 대상에 dynamic schema 없음 |
| E306 | 1 | 0 | `self` 문맥 |
| E309 | 2 | 0 | erase 정책 누락, anonymize는 optional 필요 |
| E310 | 1 | 0 | personal이 아닌 엔티티 erase |
| E311 | 1 | 0 | notify 수신자 모호 |
| E312 | 1 | 1 | `validated by` 머리 필드가 없으면 경로 이름 해석이라 F. 존재하는 필드의 종류는 S |
| E501 | 0 | 10 | 확장 이름 해석(`ns.fn` 이 확장 호출인지 정하는 일) |
| E502 | 0 | 1 | 문장 위치의 호출 구문 |
| W402 | 2 | 0 | 자기 비교, 같은 인자 두 번 |
| W403 | 1 | 0 | 무제한 목록 |
| W404 | 1 | 0 | 타임존 없는 스케줄 |
| W501 | 1 | 0 | 멱등 누락 |
| W502 | 1 | 0 | 비 1st-party 확장 |

합계: S 45곳(분할 2곳 포함) / F 127곳. (parser/lexer의 E100 등 check.rs/model.rs 밖은 범위 밖, 모두 F.)

## 2. 이전한 규칙 (S) 과 새 위치

모두 `crates/aip-ir/src/analyze.rs` 의 `analyze(&Program) -> Vec<IrDiagnostic>` 에서만 나옴. check.rs/model.rs에서는 제거(이중 실행 없음). 코드 번호·메시지·help 문구는 그대로.
- E301 intent allow 누락 (`A::query`/`A::command`)
- W402 자기 비교(`Binary` 비교 연산, l==r), 같은 인자 두 번(`Fn`/`Relation` 호출 인자 2개가 같음)
- W403 무제한 목록
- W501 멱등 누락
- E205 insert/upsert/toggle 필수 필드 누락 (`effect ... into binding.field` 로 나중에 채우는 경우 제외, 바깥 문장 목록의 것도 포함)
- E207 upsert/toggle의 unique
- E310 personal이 아닌 엔티티 erase
- E309 erase 정책 누락, anonymize는 optional 필요
- E221 정렬 case 완전성
- E216 이벤트 모양 (선언 또는 첫 emit과 비교)
- E311 notify 수신자 모호
- E302/E312 `Json validated by` 대상 (Snapshot 아님/dynamic schema 없음)
- E106 존재하는 via 필드가 되돌아 참조하지 않음
- E110 actor 없음 (`actor`, `authenticated`, `visible to actor`)
- E103 `has scope` 가 actor 스코프에 없음
- E306 `self` 문맥 (actor 엔티티 필드의 visible to/masked unless 안에서만 허용)
- E208 Upload 파라미터(query, relation, subscribe, job, grant link)
- E210 빈 enum, 빈 `in []`, approval 0건
- E204 touch 대상이 counter 아님
- E100 query에 from/fetch 없음
- E214 webhook 옵션 4종, E101 webhook 이벤트 중복 처리
- W404 타임존 없는 스케줄, W502 비 1st-party 확장
- E201 모양 규칙 7개: tree 자기참조, counter Int, sequence/slug/position Text, set null optional, unique 열 종류, no overlap Range, dynamic schema List<Record>

`aip_ir::analyze::CODES` 가 분석기가 낼 수 있는 코드 목록(26개). conformance가 이 코드마다 IR 전용 케이스가 있는지 검사함.

## 3. F로 남긴 것과 이유 (애매한 경계)

- lifecycle (E202/E201/E103 at lifecycle): F. 필드 이름과 enum 값 이름의 해석이고, IR에는 이미 같은 검사가 `validate` 의 I113으로 있어 언어 독립성이 확보되어 있음. S로 옮기면 하강된 IR에서 I113과 E202가 같은 노드에 둘 다 나와 코드가 겹친다. 요청서 예시에 lifecycle이 있었으나 "애매하면 F" 규칙을 따름.
- E106 필드 없음 / E312 머리 필드 없음: 이름 해석이므로 F. 없는 필드는 IR에서 I107/I121이 잡고, 존재하지만 종류가 틀린 경우만 analyze가 잡아 겹침을 없앰(검증: 코드 한 번씩만 나옴).
- E205 expose 전용: expose는 `lower::expand` 가 AST에서 명령으로 전개. IR에 expose가 없어 F. (전개된 insert는 일반 E205 규칙이 잡음)
- E222 두 생성기: IR이 필드당 생성기 하나라 하강 때 두 번째가 사라짐. IR로는 판정 불가.
- E107 actor 두 번: IR이 actor 하나.
- E501 확장 이름: `ns.fn` 이 확장 호출인지 지역 값인지 정하는 것이 해석. (counter store는 IR의 I119가 따로 있음)
- E201 식 타입 불일치·E213 다중값·E206 each 입력·E220 하위 선택: 식 타입 추론 결과가 필요.
- E203/E209: 값 타이핑을 건너뛰는 경로라 하강에 타입이 필요, 애매하므로 F.
- E211 호출 인자 수: 인자와 파라미터를 짝지어 타이핑하는 일의 일부.
- E212/E502/E108 구문 형태.
- E104/E102/E103(필드)/E101(중복): 이름 해석.

## 4. 공용 파이프라인

`crates/aip-sema/src/pipeline.rs`: `check_source(&str) -> Checked`, `check_ast(&File) -> Checked`.
순서: 파싱 -> expand -> sema(`analyze`, F 오류) -> F 오류가 있으면 중단 -> `to_core` -> `ir::validate` -> `ir::analyze` -> 진단 합침(SourceMap 줄·열, 줄/열 정렬, 같은 코드·위치·메시지는 하나로) .
`Checked { diagnostics, parsed, lowered }` : `lowered` 는 F 오류만 없으면 있음(S 오류가 있어도 있음, IR JSON 테스트가 쓰는 이유), `core()`/`into_core()` 는 오류가 하나도 없을 때만 반환.
위치 근거: aip-sema는 aip-syntax와 aip-ir에 이미 의존하고 `Diagnostic`/`SourceMap`/`to_core` 를 모두 가진 가장 낮은 크레이트. aip-cli 라이브러리는 없고(바이너리 전용) 테스트 크레이트들이 aip-cli를 참조할 수 없어서, aip-cli에 두면 sema 테스트(conformance)가 쓸 수 없음. aip-pg 이후 단계(plan)는 CLI가 이어 붙임.
사용처: `aip check`(텍스트/--json), `run/ddl/ir/explain/contract/gen-ts/core/migrate`(모두 `lower_core_with_map` 한 함수 -> `check_source`), conformance 실행기, `tests/common/mod.rs`(`compile_source`, `setup_app`), `unavailable.rs`.
`aip core` 의 별도 validate 호출과 `compile_full` 의 별도 validate 호출은 제거(파이프라인 안에 있음).
`ir::locate(map, path)` 를 aip-ir로 올리고 aip-pg의 `locate` 는 이를 호출하도록 바꿈(중복 제거).
`IrDiagnostic` 에 `help`, `related` 필드와 `is_warning()` 추가(severity 는 레지스트리에서 얻음). 직렬화는 값이 있을 때만.

## 5. 테스트

- `ir_json_alone_yields_the_semantic_codes` (aip-sema/tests/conformance.rs): 프런트엔드를 통과해 IR이 만들어진 모든 conformance 케이스(S 케이스와 깨끗한 케이스)에 대해 IR -> JSON 텍스트 -> 역직렬화 -> `validate`+`analyze` 만 실행, 코드 집합이 `.expect` 와 같아야 통과. `analyze::CODES` 의 모든 코드가 적어도 한 케이스에서 검증되는지도 검사.
- 케이스 분리 기준은 코드가 아니라 "하강되는가"(E201/E103 은 F와 S가 같은 코드를 씀). `// no prelude` 로 시작하는 케이스는 prelude(actor)를 붙이지 않음(E110용). golden.rs도 같은 규칙.
- 새 conformance 케이스 19개: query_without_source(E100), scope_not_declared(E103), no_actor_declared(E110), touch_not_counter(E204), schedule_without_timezone(W404), create_not_idempotent(W501), extension_not_first_party(W502), counter_not_int / tree_not_self / set_null_required / sequence_not_text / unique_on_inverse / no_overlap_not_range / dynamic_schema_not_list (E201 x7), via_not_a_reference(E106), upsert_without_unique(E207), same_argument_twice(W402), empty_enum / approval_needs_nobody(E210).
- CLI: `semantic_rule_stops_every_command_that_needs_a_program` (aip-cli/tests/codes.rs): missing_allow 케이스에서 `check --json` 이 E301, line 25, help를 내고 ddl/ir/core/explain/contract/gen-ts 가 모두 실패하며 E301을 stderr에 냄.
- negative control 2회 (분석기 규칙을 `false &&` 로 끄고 실행, 이후 파일을 보관본과 `cmp` 로 확인해 원복):
  1. toggle의 E207 끔: `toggle_without_unique.aip: from the IR alone expected {"AIP-E207"}, got {}` 로 `ir_json_alone_yields_the_semantic_codes` 와 `sema_conformance` 가 FAILED. 원복 후 3 passed.
  2. E309(erase 정책 누락) 끔: `erase_policy_missing.aip: from the IR alone expected {"AIP-E309"}, got {}` 로 같은 두 테스트 FAILED. 원복(`restored`) 후 3 passed.

## 6. 위치 정밀도

SourceMap에 추가한 경로(to_core): `uses[i]`, `intents.X.{allow,lets[i],requires[i](.cond,.when),filter,sort,touches[i],emits[i],returns,params.p}`, 문장 `<목록>[i]`(command body, 반응 body, webhook handlers[j].body, 폼의 body/on_verified/on_redeem/on_approved/on_rejected, 제약 repair, 중첩 body/on_failure), 문장 하위 `.filter` `.cond` `.target` `.to`, `entities.E.fields.f.{kind,kind.on_delete,kind.on_erase,generated,ty,default,visible_to,masked.unless}`, `entities.E.{constraints[i](.filter,.cond),lifecycles[i],visibility,predicates.p,traits.dynamic_schema}`, `relations.R.body`, `reactions[i].via`, `reactions[i].via.args[j]`, `reactions[i].handlers[j]`, `<decl>.params.<p>`.
조회는 `ir::locate` : 경로 자체, 없으면 가까운 상위 경로.
확인 방법: 변경 전 바이너리를 보관(`aip.old`)해 기존 conformance 34개 케이스와 예제 2개(총 36개)와 줄 번호 프로브 파일 2개의 `aip check` 출력을 변경 후와 diff.
- 기존 36개(conformance 34 + 예제 2): 줄·열·메시지·help 모두 diff 없음(마지막 코드 상태에서 재실행, `compared 36, differing 0`).
- 줄 번호가 달라지는 경우(프로브에서 확인): 여러 줄에 걸친 relation 본문 안의 W402. 예: relation 본문이 `a = b and` 다음 줄에 `both(a, a)` 인 경우 원래 34행, 이제 본문 시작인 33행. 식 단위 위치는 IR 경로가 문장·절 단위라 relation/fn 본문, select/returns 항목 안의 식은 그 절의 시작 줄을 가리킴.
- 열만 달라질 수 있는 경우: 해당 절 안의 식이 줄 중간에서 시작할 때(`allow`, `require`, `let`, `where`, 술어, 제약 필터는 식 위치로 맞춰 두어 프로브에서 동일). `check --json` 의 `span.start/end` 는 IR에 바이트 오프셋이 없어 0.
- 다른 동작 변화: (a) F 오류가 있으면 S 규칙은 보고되지 않음(예전에는 같이 나왔음). (b) 하강된 IR에서만 보이던 `validate` 의 I1xx 가 이제 `aip check` 에도 나옴. (c) `consume`/projection 본문, fn 본문처럼 sema가 방문하지 않던 곳도 analyze가 방문하므로 W402 등이 새로 나올 수 있음(예제 두 개는 여전히 0/0). (d) toggle 의 E207 메시지가 스프레드로 들어온 필드를 이름 목록에 포함(레코드 스프레드는 하강에서 필드로 펼쳐지므로). (e) E205 `effect ... into` 인식 범위가 스케줄 본문에도 적용.
- 비결정적인 help("in scope: ...")의 이름 순서는 변경 전부터 HashMap 순서라 실행마다 다름(이번 변경과 무관).

## 7. 골든과 레지스트리

- 실행 계획 골든(`crates/aip-pg/tests/golden`), 계약 골든(`crates/aip-cli/tests/golden`): 기존 파일은 `diff -rq` 로 바이트 동일. 새 경고 전용 케이스 4개의 계획 골든이 추가됨(sema_create_not_idempotent, sema_extension_not_first_party, sema_same_argument_twice, sema_schedule_without_timezone). golden 테스트가 "오류 없는 모든 케이스" 를 대상으로 하기 때문.
- `spec/diagnostics.md`: 변경 없음(`diff` 확인). 레지스트리(codes.rs) 불변.
- `code_sites.snap` 재생성. 코드별 지점 수 변화(파일=개수):

| 코드 | 변경 전 | 변경 후 |
|---|---|---|
| E100 | check.rs=1 lexer=8 parser=16 | analyze.rs=2 lexer=8 parser=16 |
| E101 | check=4 model=7 | analyze=2 check=3 model=7 |
| E103 | check=14 | analyze=2 check=13 |
| E106 | check=2 model=1 | analyze=2 check=2 model=1 |
| E110 | check=3 | analyze=4 |
| E201 | check=45 model=3 | analyze=8 check=38 model=3 |
| E204 | check=3 | analyze=3 check=1 |
| E205 | check=2 | analyze=2 check=1 |
| E207 | check=2 | analyze=3 |
| E208 | check=2 | analyze=2 check=1 |
| E210 | check=2 model=1 | analyze=4 |
| E214 | check=4 | analyze=5 |
| E216 | check=1 | analyze=2 |
| E221 | check=1 | analyze=2 |
| E301 | check=1 | analyze=3 |
| E302 | check=1 | analyze=2 |
| E306 | check=1 | analyze=2 |
| E309 | check=2 | analyze=3 |
| E310 | check=1 | analyze=2 |
| E311 | check=1 | analyze=2 |
| E312 | check=2 | analyze=3 check=1 |
| W402 | check=2 | analyze=3 |
| W403 | check=1 | analyze=2 |
| W404 | check=1 | analyze=2 |
| W501 | check=1 | analyze=2 |
| W502 | check=1 | analyze=2 |
| S (codes::Severity 부산물) | main.rs=1 | main.rs=1 validate.rs=1 |

analyze.rs 의 개수에는 `CODES` 목록 항목 1개씩이 포함됨(그래서 규칙 지점 수보다 1 큼). 마지막 행의 "S" 는 `codes::Severity` 를 스냅숏 스캐너가 코드로 읽은 기존 부산물이며, validate.rs 의 `is_warning` 이 같은 이름을 하나 더 씀.

## 8. 변경 파일

- conformance/sema/approval_needs_nobody.aip
- conformance/sema/approval_needs_nobody.expect
- conformance/sema/counter_not_int.aip
- conformance/sema/counter_not_int.expect
- conformance/sema/create_not_idempotent.aip
- conformance/sema/create_not_idempotent.expect
- conformance/sema/dynamic_schema_not_list.aip
- conformance/sema/dynamic_schema_not_list.expect
- conformance/sema/empty_enum.aip
- conformance/sema/empty_enum.expect
- conformance/sema/extension_not_first_party.aip
- conformance/sema/extension_not_first_party.expect
- conformance/sema/no_actor_declared.aip
- conformance/sema/no_actor_declared.expect
- conformance/sema/no_overlap_not_range.aip
- conformance/sema/no_overlap_not_range.expect
- conformance/sema/query_without_source.aip
- conformance/sema/query_without_source.expect
- conformance/sema/same_argument_twice.aip
- conformance/sema/same_argument_twice.expect
- conformance/sema/schedule_without_timezone.aip
- conformance/sema/schedule_without_timezone.expect
- conformance/sema/scope_not_declared.aip
- conformance/sema/scope_not_declared.expect
- conformance/sema/sequence_not_text.aip
- conformance/sema/sequence_not_text.expect
- conformance/sema/set_null_required.aip
- conformance/sema/set_null_required.expect
- conformance/sema/touch_not_counter.aip
- conformance/sema/touch_not_counter.expect
- conformance/sema/tree_not_self.aip
- conformance/sema/tree_not_self.expect
- conformance/sema/unique_on_inverse.aip
- conformance/sema/unique_on_inverse.expect
- conformance/sema/upsert_without_unique.aip
- conformance/sema/upsert_without_unique.expect
- conformance/sema/via_not_a_reference.aip
- conformance/sema/via_not_a_reference.expect
- crates/aip-cli/src/main.rs
- crates/aip-cli/tests/codes.rs
- crates/aip-cli/tests/common/mod.rs
- crates/aip-cli/tests/golden.rs
- crates/aip-cli/tests/unavailable.rs
- crates/aip-ir/src/analyze.rs
- crates/aip-ir/src/lib.rs
- crates/aip-ir/src/validate.rs
- crates/aip-ir/tests/code_sites.snap
- crates/aip-pg/src/lib.rs
- crates/aip-sema/src/check.rs
- crates/aip-sema/src/lib.rs
- crates/aip-sema/src/model.rs
- crates/aip-sema/src/pipeline.rs
- crates/aip-sema/src/to_core.rs
- crates/aip-sema/tests/conformance.rs
- crates/aip-pg/tests/golden/*.plan.json 9개: 재생성으로 수정 시각만 바뀜(내용 동일), 새로 생긴 4개는 7절

## 9. 남은 TODO

- E216 메시지의 "than at line N" 은 `IrDiagnostic.related` 경로를 파이프라인이 줄 번호로 바꿔서 만든다. SourceMap이 없는 소비자(다른 프런트엔드)는 `than at events.X` 또는 `than at intents.A.emits[0]` 처럼 IR 경로를 본다.
- 식 단위 위치(relation/fn 본문, select·returns 항목 안): 줄이 달라질 수 있음(6절). 필요하면 식 경로 체계(`...expr[n]`)를 SourceMap에 추가해야 함.
- lifecycle을 S로 옮기려면 `validate` 의 I113과 코드 역할을 정리해야 함(3절). 결정 필요.
- F 오류가 있을 때 S 규칙을 같이 보여주려면 "오류가 있어도 하강" 모드가 필요하고, Unknown 타입으로 인한 오탐 방지를 따로 설계해야 함. 현재는 요청대로 중단.
- docs/design/04-safety-matrix.md 가 규칙의 위치(check.rs)를 말하는지는 확인하지 않음 (확인 못함: 이번 범위는 코드·테스트).

## 10. 최종 명령 출력
```
$ cargo fmt --all && cargo clippy -q --workspace --all-targets
0 lines of output (경고 0)
$ cargo test -q --workspace
합계 passed 108, failed 0
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
$ ./target/debug/aip check-docs
62 blocks: 55 parsed, 0 failed, 7 skipped (fragments with '...')
$ diff spec/diagnostics.md (변경 전 사본)
identical
$ aip check examples/{ariari,shop}/app.aip
0 error(s), 0 warning(s)
0 error(s), 0 warning(s)
```
