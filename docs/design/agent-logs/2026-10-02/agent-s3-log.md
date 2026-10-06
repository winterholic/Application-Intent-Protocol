# S3 진행 로그 (aip-pg -> Core IR)
- [시작] 구조 파악: aip-cli main.rs compile() = parse->expand->analyze->aip_pg::compile. 'aip ir' 이 Program 전체 출력.
- [Step 0] 골든 고정. crates/aip-cli/tests/golden.rs + common::compile_source(src). 입력: ariari, shop, conformance/sema 중 프런트엔드 오류 없는 3개(ok_ariari_style, self_compare, unbounded_list; 나머지는 sema 오류). 출력 crates/aip-pg/tests/golden/<name>.plan.json = {diagnostics:[severity,code,message,line,col], program}. 재생성은 AIP_UPDATE_GOLDEN=1 만. 골든 5개, 재실행 결정적(두 번째 실행 통과). ariari 에 AIP-W602 line70 col25 진단 포함.
- [Step 0 negative control] shop.plan.json 의 첫 'SELECT'를 'SELECt' 로 1글자 변경 -> `compiled_plans_match_golden` 실패 확인(아래), 원복 후 통과. (첫 시도는 BSD sed 가 변경을 못 해 무효였고 python 으로 다시 수행.)
- [check-docs 전]
    
    36 blocks: 29 parsed, 0 failed, 7 skipped (fragments with '...')
- [분석] aip-pg 전체(약 4800줄) 읽고 IR 로 옮길 때 정보 부족 항목을 먼저 식별. 골든 바이트 동일 요건 때문에 IR 추가가 필요한 항목:
- IR 추가: Literal::DurationSeconds(i64) -> Literal::Duration(Dur{n, unit: DurUnit}) — 백엔드가 `interval '6 months'`(달력 개월), `'1 days'` 처럼 쓴 단위 그대로 SQL 을 내는데 초로 정규화하면 복원 불가(7 days vs 1 week, month=30일 아님). 단위는 의미 정보(달력 단위 여부). Dur::seconds() 제공(month=30일, year=365일: 기존 dur_secs 와 동일).
- IR 추가: Reaction::Retain.keep_seconds -> keep: Dur, RetainNotify.before_seconds -> before: Dur — 같은 이유(retain SQL 이 `interval '6 months'` 를 낸다).
- IR 추가: Constraint(5개 변형 전부).ordinal: u32 — 엔티티 본문 멤버 중 위치. 기존 백엔드가 DB 객체 이름(`<table>__u7`, `__one3`, `__cap5`...)과 erase repair 스텝 이름(`__groups.<Entity>.<i>`)에 쓰는 멤버 인덱스이며 IR 의 constraints Vec 순서로는 복원 불가. SQL 개념이 아니라 선언 위치(안정 식별자). validate: AIP-I120(같은 엔티티에서 ordinal 중복) 추가 + 테스트.
- SourceMap 키 추가(정본 IR 아님): entities.<E>.traits.tenant / .traits.publishable, entities.<E>.fields.<f>.encrypted, reactions[i].name, forms[i].name — E602/W602 를 기존과 같은 줄·열로 내기 위해.
- to_core/validate/core_ir 테스트를 함께 맞춤(aip-ir, aip-sema 빌드 통과).
- [Step 1~N 전환] aip-pg 전 모듈을 Core IR 입력으로 재작성: ty.rs(신규: 백엔드 Ty + type_spec), schema.rs, writes.rs, sqlexpr.rs, select.rs, shape.rs(IR Shape -> 계약 JSON), ddl.rs, plan.rs + plan/{approval,job,rule,webhook}.rs, lib.rs(`compile(core, map)`). aip-plan 에 백엔드 진단 타입(Diagnostic{severity,code,message,path,line,col}, Severity) 추가, aip-cli 가 `analyze -> to_core -> validate -> compile` 순서 + 기존 `render` 형식으로 출력. aip-pg/Cargo.toml 에서 aip-syntax/aip-sema 제거.
  * 절차상 편차: Compiler/Schema/Planner 가 모듈 간 서로 물려 있어 모듈 하나씩 빌드+골든을 돌릴 수 없었음(한 모듈만 IR 로 바꾸면 나머지가 컴파일 안 됨). 전 모듈을 옮긴 뒤 한 번에 빌드하고 골든으로 불일치를 하나씩 확인했다.
- [골든 1차 결과] shop + conformance 3개 바이트 동일, ariari 만 불일치 2건:
  1) jobs[0].export.bucket: 기대 'exports', 실제 's3'. 원인 = 하강 버그. to_core 가 `produce csv to s3 bucket "exports"` 튜플 (format, store, bucket, exp) 을 (format, bucket, template, exp) 로 받아 store 를 bucket 에, 실제 bucket 을 template 에 넣고 있었음. 수정: ir::JobProduce { format, store, bucket, expires_seconds } (template 필드 삭제, store 추가), to_core 교정. [IR 수정/추가: JobProduce.store — 이유: 하강이 이름을 잘못 붙여 bucket 이 'exports' 대신 's3' 가 됨]
  2) intents.GetClubEvent.variants[0].main: 기존 SQL 이 `FROM "club_faq" ... 'title', NULL, 'at', NULL` 인 것을 이번엔 `FROM "club_event" ... to_jsonb(t6."title")` 로 생성. 원인 = 기존 aip-pg 버그. `from <param>` 쿼리의 파라미터 타입을 `lookup_param` 이 '이름이 같은 파라미터를 가진 첫 번째 query 선언'에서 찾았고(expose 로 생긴 GetClubFaq 의 `item`), 그래서 GetClubEvent 가 엉뚱한 엔티티(ClubFaq) 테이블에서 읽고 이벤트 컬럼을 NULL 로 내보냈음. IR 은 쿼리마다 자기 params 를 가지므로 자연히 올바른 ClubEvent 가 됨. 골든 바이트 동일 요건에서 의도적으로 벗어나는 유일한 항목: 기존 골든(버그를 박제)을 갱신했고, 갱신 diff 는 이 1줄뿐임을 확인(shop/sema_* 는 변화 없음). 
- [진단 확인] 기존 출력 형식(`severity CODE file:line:col  message`)을 aip_plan::Diagnostic::render 가 그대로 낸다. E602/W602 는 줄·열이 기존과 같음: ariari W602 70:25 골든 동일, 수동 케이스(tenant/publishable 27:3,28:3, encrypted 26:16, search/consent/migration 이름 위치 32:8,34:9,35:11)가 기존 span 과 같은 위치. E601 `fetch`·"command without allow" 는 intent 이름 위치(SourceMap `intents.<X>.name` 추가, 24:7)로 기존과 같음.
  * 위치가 바뀌는 부분: E600(식 컴파일 실패)과 그 밖의 E601 은 기존에 실패한 식/절의 span 이었으나 IR 엔 식 span 이 없어 선언 시작 위치(`intents.X`/`reactions[i]`/`forms[i]`)로 나온다. 예: `where x.name = notify.weird()` -> 기존 식 위치, 지금은 24:1(query 선언). 이유: IR 은 위치를 갖지 않는 규칙. 메시지·코드·종료코드(FAILURE)는 동일.
  * 진단 순서는 기존과 같게 유지: planner 오류는 선언 위치순(안정 정렬), 그다음 엔티티 E602/W602, 그다음 선언 E602.
- [최종 골든] 5개 모두 일치(`compiled_plans_match_golden` ok). negative control 재수행: ariari.plan.json 의 첫 'INSERT INTO'를 'INSERT INTo' 로 1글자 변경 -> 실패 확인, 원복 후 통과.

## 알려진 동작 차이 / 근사 (골든이 덮지 못하는 입력에서의 차이 포함)
1. GetClubEvent 버그 교정(위 2번): 골든 1줄 갱신.
2. 레코드 필드 타입·파라미터 타입은 `Ty::from_ir`(정밀 매핑)을 쓴다. 기존 `record_field_ty`/`ty_of` 는 Phone·Duration·Money(밑줄 없는)·Upload 등 일부를 Unknown(jsonb 캐스트)으로 취급했다. 예제에는 해당 사례가 없어 골든은 같음. 확인 못함: 그런 타입의 파라미터를 가진 프로그램에서의 SQL 비교(기존 구현이 없어 비교 불가).
3. `...input`(spread)은 IR 에서 필드별 `input.f` 로 펼쳐진다. 백엔드는 "레코드 JSON 의 멤버를 컬럼에 쓰는" 값을 기존 spread SQL(단일 캐스트, Range 는 {start,end} 해석)로 만든다. 그래서 명시적 `title: input.title` 도 이제 같은 SQL 이 된다(기존엔 이중 캐스트 `((js->>'f'))::text)::text`). 의미는 동일, Range 는 오히려 교정. 예제에서는 spread 만 쓰므로 골든 동일.
4. 필드에 `visible to` 와 `masked` 가 둘 다 있으면 IR 엔 선언 순서가 없어 visible -> masked 고정 순서로 감싼다(예제엔 masked 없음).
5. IR `Generated` 는 필드당 하나(마지막 것)라, 한 필드에 sequence/slug/position 을 겹쳐 쓰면 기존(모두 적용)과 다름. 
6. 파생 값(식)의 Snapshot/Union 선택 출력은 IR Shape 가 필드와 구분하지 않아 `{"kind":"snapshot"}`/`union_ref` 형태로 나온다(기존엔 `{"kind":"snapshot","entity":..}`/`{"kind":"union",..}`). 타입을 알 수 없는 값은 `{"kind":"unknown"}`(기존엔 일부 `other Unknown`). 예제 골든은 동일.
7. `Json<X>`(스키마 이름 있는 Json)와 `Json validated by snap`을 IR 이 구분하지 않는다. 동적 검증은 "첫 세그먼트가 같은 insert 에 대입된 Snapshot 필드"일 때만 동작해 실제 차이는 없다고 판단(확인 못함: 동명 필드가 있는 인위적 사례).
8. 단계별 빌드+골든 검증은 모듈 상호 의존 때문에 못 함(위 절차상 편차).
9. 문서 `docs/design/10-philosophy-alignment.md` 의 'S3 남은 단계' 서술은 갱신하지 않았음(요청 범위 밖).

## 변경 파일
- 신규: crates/aip-pg/src/ty.rs, crates/aip-cli/tests/golden.rs, crates/aip-pg/tests/golden/{ariari,shop,sema_ok_ariari_style,sema_self_compare,sema_unbounded_list}.plan.json
- aip-pg: Cargo.toml(aip-syntax/aip-sema 제거, aip-ir 추가), src/{lib,schema,writes,sqlexpr,select,shape,ddl,plan}.rs, src/plan/{approval,job,rule,webhook}.rs
- aip-plan: src/lib.rs(Diagnostic, Severity)
- aip-ir: src/lib.rs(Dur/DurUnit, Literal::Duration, Retain keep/before, Constraint.ordinal), src/forms.rs(JobProduce), src/validate.rs(AIP-I120 + 테스트, Constraint 테스트 인자)
- aip-sema: src/to_core.rs(하강: Dur, ordinal, JobProduce 교정, SourceMap 키)
- aip-cli: src/main.rs(compile = analyze -> to_core -> validate -> aip_pg::compile; 진단 render), tests/common/mod.rs(compile_source, setup_app). explain.rs 는 aip_plan 만 써서 변경 없음.

## IR 추가/수정 목록
- IR 추가: Dur{n, unit}/DurUnit, Literal::Duration (구 DurationSeconds 대체) — 달력 단위 보존, SQL interval 단위 복원
- IR 추가: Retain.keep: Dur, RetainNotify.before: Dur (구 *_seconds 대체) — 같은 이유
- IR 추가: Constraint.ordinal (5개 변형) + validate AIP-I120 — DB 객체/스텝 이름이 쓰는 멤버 위치
- IR 수정: JobProduce { format, store, bucket, expires_seconds } (template 삭제) — 하강 버그 교정
- SourceMap 키 추가(정본 아님): entities.<E>.traits.tenant/.publishable, entities.<E>.fields.<f>.encrypted, reactions[i].name, forms[i].name, intents.<X>.name
- CORE_IR_VERSION 은 올리지 않음(aip-core/0.2 유지). 확인 필요: 직렬화 형식이 바뀌었으므로(Literal duration, Constraint.ordinal, JobProduce) 버전 태그를 올릴지는 사용자 판단.

## 남은 근사/TODO
위 '알려진 동작 차이' 2~7번과 E600/E601 위치 정밀도(선언 단위). 그 외 없음.

## 최종 명령 출력
$ grep -rn "aip_syntax\|aip_sema" crates/aip-pg
(출력 0줄, wc -l = 0)
$ grep aip- crates/aip-pg/Cargo.toml
aip-ir = { path = "../aip-ir" }
aip-plan = { path = "../aip-plan" }
$ ./target/debug/aip check-docs   (전: 36 blocks: 29 parsed, 0 failed, 7 skipped)
36 blocks: 29 parsed, 0 failed, 7 skipped (fragments with '...')
$ cargo fmt --all && cargo clippy -q --workspace --all-targets
(경고 0, 출력 없음)
$ cargo test -q --workspace
합계 85 passed, 0 failed (기준 83 + golden 1 + aip-ir I120 1 = 85; ariari_e2e 4, shop_e2e 1 포함)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
