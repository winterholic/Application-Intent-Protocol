# agent-evolve log
## 진행
- [탐색] ddl.rs(program.ddl 평면 문장 목록), runtime migrate(IF NOT EXISTS 위주), CLI main.rs 확인. 설계 초안: IR diff(aip-ir/diff.rs, 의미 변화) + aip-pg/evolve.rs(변화 -> 단계, 객체(인덱스/제약)는 old/new DDL 최종 상태 비교, 트리거는 카탈로그 기준 전면 교체) + runtime(_aip_deployment, 한 트랜잭션 적용).
- [IR/문법] was/removed 문법(파서·AST·sema E111·IR 필드), CORE_IR 0.11, aip-ir/diff.rs(Change 목록, relation) 작성. 다음: aip-pg evolve
- [aip-pg evolve.rs, runtime schema.rs, CLI migrate --plan] 구현 및 수동 스모크: v1->v2(필드/엔티티/enum/unique 추가) 적용 후 카탈로그 덤프가 fresh v2와 동일(SAME). 다음: 4개 예제 어댑트 스모크, e2e 테스트, aip diff(Part B)
- [e2e] crates/aip-cli/tests/evolve_e2e.rs 11개 통과(추가/삭제선언/이름변경/unique 위반/타이트닝 거부/트리거 교체/plan 무변경/adopt/동시 기동/엔티티 삭제/미지원). 다음: Part B aip diff
- [Part B] aip-contract/src/compat.rs + CLI diff, tests/diff.rs 8개, evolve_e2e CLI 테스트 포함 12개 통과. 다음: 전체 테스트/골든/문서/negative control
- [검증] 4개 예제 어댑트(plan(new,new))와 saas/cms 필드 추가+search 가중치 변경 진화 모두 fresh 배포와 카탈로그 덤프 동일. e2e 14개(evolve_e2e), diff 8개, 적합성 케이스 9개(E111), core_ir 단언 추가. 골든: 기존 plan/계약/클라이언트는 core_ir 문자열(0.10->0.11)만 변경, 새 골든 sema_ok_evolution.plan.json 1개 추가. 예제 client/aip.ts 4개 재생성(헤더 core 버전만).
- 확인 못함: tsc 실행(npx -p typescript 설치 불가, exit 127). 변경은 생성 TS 헤더 주석의 core 버전뿐.

## 결정 후보 (내가 정한 세부와 근거)
1. 의도 선언 문법(한 표현씩): 필드/엔티티 이름 변경 = `was 옛이름` (필드 수식어, `entity New was Old`), 삭제 = `removed field x`(엔티티 본문), `removed entity X`(최상위). 문맥 키워드라 예약어 추가 없음(`removed: Bool` 같은 필드명 유지). 선언은 배포 후에도 소스에 남겨도 무해(DB에 옛 이름이 없으면 no-op). 모순은 AIP-E111(자기 이름, 아직 선언된 이름, 중복 was, 이름변경+삭제 동시, record의 was).
2. IR: Field.was, Entity.was/removed_fields, Program.removed_entities (비어 있으면 직렬화 생략 -> 선언 없는 프로그램의 IR/digest 불변). CORE_IR 0.11.
3. 배포 기록 `_aip_deployment(id, applied_at, digest, ddl_version, ir text(canonical JSON), steps jsonb)`: 런타임이 CREATE IF NOT EXISTS(forms6의 _aip_migration과 같은 이유: 계획 골든 불변). 같은 트랜잭션에서 기록. DDL_VERSION=1(생성기 버전): digest가 같아도 버전이 다르면 트리거/함수를 교체하는 계획이 돈다.
4. 분할: aip-ir/diff.rs = 의미 변화 목록(SQL 모름, TypeRelation widen/narrow/incompatible 포함). aip-pg/evolve.rs = 변화 -> 단계. 테이블/컬럼/제약/인덱스는 "옛 IR을 현재 생성기로 다시 생성한 DDL"과 "새 DDL"의 최종 상태 비교(불변식: 옛 DB + 계획 = 새 프로그램을 빈 DB에 배포한 스키마. e2e가 덤프로 검증). 제약 이름이 ordinal을 따라 움직이거나 식이 바뀌면 drop+create. 트리거/함수는 비교하지 않고 전부 현재 텍스트로 교체 + 이름에 `__`가 든 고아 트리거와 `__*_fn` 고아 함수 삭제(이름 규칙으로 DBA가 만든 트리거는 건드리지 않음).
5. 데이터 변환과 `migration`의 연결: 스키마 변경은 한 트랜잭션, `migration`은 그 뒤 기존대로(각자 트랜잭션) 같은 advisory lock 안에서 실행. 그래서 migration 결과에 기대는 조임(필수 전환, unique, 옛 필드 삭제)은 다음 배포로 나눈다(expand/contract). 거부 코드의 fix에 안내.
6. `CREATE INDEX CONCURRENTLY`는 쓰지 않음: 한 트랜잭션 보장과 양립하지 않음. 단계 설명에 "쓰기가 기다린다"고 기록(대형 테이블 운영은 확인 못함: 측정 안 함).
7. 타입 변경: 같은 종류의 길이/범위 완화(Text min/max, Int min/max, 더 넓은 Text, 이메일류 -> Text)는 자동(DDL 없음, DB는 이 제약을 모름). 축소는 Text 길이와 Int 범위만 기존 행을 검사해 허용, 그 밖(trim/lower/pattern/목록)은 TYPE_CHANGE. 다른 기본 타입/enum은 TYPE_CHANGE. Int->Decimal은 자동 허용하지 않음(USING 변환 테이블 재작성, 보수적으로 거부).
8. enum: 끝 추가 자동, ordered 중간 삽입 자동(+알림: 순위는 SQL CASE로 컴파일되고 DB에는 이름이 저장돼 기존 행의 상대 순서는 불변), ordered 기존 값 순서 변경은 UNSUPPORTED, 값 삭제/이름 변경은 그 값을 쓰는 행이 0일 때만(CHECK 재생성 전에 개수 검사).
9. 새 코드: AIP-E111, AIP.SCHEMA.{UNDECLARED, TYPE_CHANGE, DATA_CONFLICT, UNSUPPORTED, UNRECORDED, FAILED} (모두 startup 오류, http 500).
10. 기록 없는 DB(이전 버전이 만든 DB): 스키마가 프로그램과 맞으면 adopt(plan(new,new)를 적용해 트리거까지 갱신하고 기록), 안 맞으면 UNRECORDED. 이것이 forms6의 "기존 DB에 알림 트리거가 안 생김" 해결 경로.
11. --plan: 읽기 전용 검사 질의만 실행(트랜잭션에서 실행 후 롤백하지 않음: 락을 잡지 않으려고). 이번 변경이 만드는 컬럼을 가리키는 검사는 "적용 때 실행"으로 표시. 막히면 종료코드 1.
12. Part B는 `aip-contract/src/compat.rs`(describe 문서 + 선언된 intent의 allow/requires IR 비교). 입력 삭제=깨짐(런타임이 알 수 없는 필드를 거부), 멱등성 없음->caller key=깨짐(헤더 필수화), enum 값 추가는 출력에 쓰이면 경고/입력만이면 호환. record와 event 페이로드는 입력/출력 규칙을 근사로 적용.

## 분류표 (변경 종류 -> 자동/거부/선언 필요)
- 엔티티/선택 필드/기본값 있는 필수 필드/참조 필드 추가: 자동(safe)
- 기본값 없는 필수 필드 추가: 빈 테이블이면 자동, 아니면 거부(DATA_CONFLICT)
- 일반 인덱스 추가: 자동(safe, CONCURRENTLY 아님)
- unique/check/enum 목록 변경: 기존 행 검사 후 자동, 위반 시 거부(개수+id 예시)
- enum 끝/중간 추가: 자동(중간+ordered는 알림); 삭제/이름변경: 쓰는 행 없을 때만; ordered 순서 변경: 거부(UNSUPPORTED)
- 길이/범위 완화, 필수->선택, 기본값 변경: 자동; 축소(Text 길이, Int 범위): 행이 맞을 때만; 그 밖 축소/다른 타입: 거부(TYPE_CHANGE)
- 선택->필수: NULL 행이 없을 때만
- 필드/엔티티 삭제: 선언 없으면 거부(UNDECLARED), `removed`로 선언하면 자동(declared 표시)
- 이름 변경: 선언 없으면 거부(UNDECLARED), `was`로 선언하면 자동(RENAME, 값 보존)
- 필드 종류 변경, history/publishable/tenant/dynamic schema 추가·삭제: 거부(UNSUPPORTED)
- 제약/인덱스 삭제: 자동(relaxing, 알림); 트리거/함수: 항상 현재 텍스트로 교체(refresh)
- 가상 필드(역참조) 삭제: 컬럼이 없어 선언 없이도 자동

## negative control 결과 (전부 원복+touch 후 재통과 확인)
- 선언 없는 삭제 거부를 끔(`declared || true`) -> removing_a_field_needs_a_declaration... 실패: 배포가 성공하고 note 컬럼의 값('keep me')이 사라짐(데이터 손실이 실제로 일어남).
- RENAME COLUMN 단계를 끔 -> a_rename_declared_with_was_moves_the_values 실패(값이 새 이름 컬럼에 없음).
- 데이터 검사를 끔 -> a_unique_rule... 와 rules_the_rows_already_break... 실패(DB가 자체 오류로 막지만 AIP.SCHEMA.FAILED로 개수/예시 없이 보고됨. 즉 검사가 개수와 예시를 담당).
- 고아 트리거 정리를 끔 -> generated_triggers_are_replaced... 실패(task__stale 남음).
- 트리거/함수 교체를 끔 -> 같은 테스트 실패(task__notify가 --reset 없이 안 생김).
- advisory lock을 끔 -> processes_starting_together... 3회 모두 실패(second: relation "label" already exists).

## 변경 파일
- 문법/파서/AST: crates/aip-syntax/src/{ast,parser}.rs
- sema: crates/aip-sema/src/{check,to_core}.rs, crates/aip-sema/tests/core_ir.rs, conformance/sema/{ok_evolution,evolution_*}.{aip,expect}(9쌍)
- IR: crates/aip-ir/src/{lib,validate,codes,diff}.rs(diff.rs 신규), crates/aip-ir/tests/code_sites.snap
- 계약: crates/aip-contract/src/{lib,compat}.rs(compat.rs 신규)
- 백엔드/플랜: crates/aip-pg/src/{lib,evolve}.rs(evolve.rs 신규), crates/aip-plan/src/lib.rs(EvolvePlan 등), crates/aip-pg/tests/golden/sema_ok_evolution.plan.json(신규)
- 런타임: crates/aip-runtime/src/{lib,migration,schema}.rs(schema.rs 신규)
- CLI: crates/aip-cli/src/main.rs(migrate --plan/--json, diff), tests/{evolve_e2e,diff}.rs(신규), tests/golden/*(core_ir 문자열)
- 문서/생성물: spec/grammar.md(스키마 진화 절), spec/diagnostics.md(재생성), docs/design/03-ir.md(§10 구현 위치 한 줄), examples/*/client/aip.ts 4개(헤더 core 버전)
- 골든 diff 요약: 기존 plan 골든 바이트 불변(새 파일 1개 추가), 계약/클라이언트 골든은 core_ir 0.10->0.11 문자열만.

## 남은 TODO
- 엔티티 trait(history/publishable/tenant) 추가·삭제와 필드 종류 변경 자동화(지금은 UNSUPPORTED).
- Int->Decimal 같은 USING 변환 타입 변경, enum 이름 변경의 데이터 이동(지금은 새 필드+migration 2단계).
- 대형 테이블에서 인덱스/ADD COLUMN GENERATED의 잠금 시간(확인 못함: 측정 안 함), CONCURRENTLY 옵션.
- `aip diff`: 폼이 만든 intent의 allow/requires는 비교하지 않음(IR intent가 없어서), record/event는 근사 규칙.
- TS 클라이언트 tsc 재검증(확인 못함: 실행 환경에 typescript 없음).
- `aip run`의 --plan 사전 점검 옵션과 `_aip_deployment` 이력 조회 명령 없음.

## 최종 명령 출력
- `cargo fmt --all && cargo clippy -q --workspace --all-targets` -> 출력 0줄(경고 0)
- `cargo test -q --workspace --no-fail-fast` -> passed=167 failed=0 (기존 142 + 25)
- `./target/debug/aip check-docs docs spec README.md` -> 71 blocks: 64 parsed, 0 failed, 7 skipped (fragments with '...')
- `pkill -f "aip run"` 후 `pgrep -f "aip run" | wc -l` -> 0
