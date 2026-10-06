# V7 마이그레이션 사전 검사 정확성 검토 (r8)

검토 기준: `plan-docs/sources/founder-integrated-directive-2026-10-03.md:642-658`(승인·롤백·호환), `plan-docs/alignment/E-technical-risks.md:21,61`(RK-10). 문서·코드의 실험 후보를 제품 정책 승인으로 해석하지 않는다. 원본 코드는 수정하지 않으며 변형·복사본은 `target/` 아래에만 둔다. `.env`·비밀 저장소·git 명령을 사용하지 않는다.

## 발견 (확인 즉시 누적)

### F1 · P1 · predicate 본문 변경이 정책 변경 검사에 도달하지 않는다 (Q1)

- 문제: `diff`는 `rowRead` AST 자체와 `fieldRead`만 비교한다. 호출하는 predicate의 본문 변경은 비교하지 않는다. 변경 목록이 비면 `check`는 적용 가능 `true`를 반환한다.
- 근거: `spikes/spike-v7-migrate/src/lib.rs:29-99,84-94,189`; `spikes/spike-v2-read/src/sqlgen.rs:262-272`는 실제 SQL 생성 시 새 facts의 predicate 본문을 읽는다. fixture의 `active(this)`·`managerOf` 호출은 그대로 두고 본문만 바꾸는 정상 정의 변경이 해당한다(`spikes/spike-v1-fixture/fixture/recruitment.aip:21-22,50-52,87`).
- 영향: 읽기 정책·필드 정책·쓰기 권한을 공유하는 predicate를 확대해도 SecurityReview 항목이 생성되지 않는다.
- 다음 단계 전 수정: predicate 의존성을 따라 영향받는 정책을 다시 비교하거나, 지원하지 않는 의미 변경은 적용 불가로 처리한다. 실행 반례는 뒤에 누적한다.

- F1 실행 근거: V1 `load_str(Form::A)` 성공 후 `active` 본문을 `true`로 변경. `changes=[]; applicable=true`. 같은 actor=2의 `{"read":"Recruitment","select":["id"]}`는 이전 `[]` / 이후 `[{"id":103}]`. 동일한 행·회원에서도 SecurityReview를 거치지 않고 CLOSED 행이 새로 보인다.

### F2 · P1 · traverse 추가·관계 대상의 공개 select 확대가 검출되지 않는다 (Q1)

- 문제: `exposeRead`에서는 루트 select/filter/sort의 이름만 비교하고 `traverse` 전체를 보지 않는다(`spikes/spike-v7-migrate/src/lib.rs:69-83`). V1은 관계별 허용 select를 별도 facts로 생성한다(`spikes/spike-v1-fixture/src/sema.rs:1117-1137`). V2는 그 허용 목록으로 실제 요청을 거부하거나 받아들인다(`spikes/spike-v2-read/src/plan.rs:194-217`).
- 실행: 원래 Club 자체의 공개 필드는 유지하고 Recruitment의 `traverse club { select id }`를 `select id, name, logo`로 확대. V1 의미 검사 성공, `changes=[]; applicable=true`. actor=1, `{"read":"Recruitment","select":["id",{"club":{"select":["name"]}}]}`는 옛 정의에서 `FIELD_NOT_EXPOSED`, 새 정의에서 `[{"club":{"name":"A"},"id":100}]`. traverse 선언 자체를 없음→원본으로 추가해도 동일한 빈 diff/적용 가능 true.
- 영향·수정: 이미 관계 대상에서 공개 중인 필드라도 해당 호출 경로를 새로 열면 권한 범위가 늘어난다. 관계 경로·대상·허용 select를 비교하여 확대를 SecurityReview로 분류해야 한다. 대상 resource의 루트 select 이름 추가는 기존 검사에서 탐지되므로 그 경우까지 미탐지라고 주장하지 않는다.

### F3 · P1 · 집계 sourceAccess 변경을 검사하지 않아 집계 값의 공개 범위가 넓어진다 (Q1)

- 근거: `spikes/spike-v7-migrate/src/lib.rs:29-99`에 aggregates/accesses/exposeAggregates 비교가 없다. V1 sourceAccess facts 생성은 `spikes/spike-v1-fixture/src/sema.rs:1014-1052,1064-1067`, V2 실제 집행은 `spikes/spike-v2-read/src/plan.rs:155-174,367-384`.
- 실행: 양쪽 정의에 `access noBookmarks(a: Member) = false`를 선언하고 Recruitment.bookmarkCount의 sourceAccess만 `noBookmarks(actor)`→`fixedTotalOfVisibleRecruitment`로 변경. V1 의미 검사 성공, `changes=[]; applicable=true`. actor=1의 `{"read":"Recruitment","select":["id","bookmarkCount"]}` 결과는 이전 `[{"id":100,"bookmarkCount":null}]`, 이후 `[{"id":100,"bookmarkCount":1}]`.
- 영향·수정: 행 정책이 그대로여도 집계 공개가 넓어진다. 집계 정의와 access 본문·의존 predicate까지 비교해야 한다. sourceAccess 변경은 기본 SecurityReview로 보내고 축소 판단은 별도 근거로 해야 한다.

### F4 · P1 · 전이 allow·expose apply/create/compose 변경이 모두 빈 diff다 (Q1)

- 실행: 각각 원본 fixture에서 (1) close의 `allow managerOf(actor, club)`→`allow true`, (2) `expose apply close { target id; bulk maxRows 10 }` 추가, (3) ClubMember에 `expose create { allow true; fields club, member, role }` 추가, (4) Recruitment에 `expose compose { bulk maxRows 10; transitions close }` 추가. 네 정의 모두 V1 의미 검사 성공, 각각 `changes=[]; applicable=true`.
- 근거: V7 `src/lib.rs:29-99`는 transitions/exposeApply/exposeCreate/exposeCompose를 읽지 않는다. V1 `src/sema.rs:707-746,749-777,779-836`는 이 변경을 실행 facts에 보존한다. 따라서 문서 `V7-migration-results.md:24`의 '누군가 새로 볼 수 있게 됨'만으로는 쓰기 권한 확대를 포괄하지 못한다.
- 영향·수정: 쓰기 기능을 공개하거나 allow를 확대해도 차단하지 않는다. 검사 범위에 쓰기 계약·효과·권한을 포함하거나, 미지원 변경을 명시적으로 적용 불가 처리해야 한다.
- 확인 못함: V3 실제 쓰기 호출 및 해당 변경의 저장 결과. 요청 범위인 V1/V2/V7에서 정상 facts 수용·diff 누락까지 확인했다. 이것을 실제 데이터 손실 실행 증거로 주장하지 않는다.

### F5 · P1 · budget 추가로 새 루트 조회가 공개되어도 적용 가능이다 (Q1)

- 실행: Club의 `expose read { select id, name, logo }`에 `budget { rows 10; depth 1; deadline 2s; cost 100 }`만 추가. V1 의미 검사 성공, `changes=[]; applicable=true`. actor=1의 `{"read":"Club","select":["id","name"]}`는 이전 `NOT_ROOT_QUERYABLE`, 이후 `[{"id":10,"name":"A"},{"id":12,"name":"U"}]`.
- 근거: V1 `src/sema.rs:1139-1158`는 budget 유무로 rootQueryable을 생성한다. V2 `src/plan.rs:112-119`는 루트 조회를 집행한다. V7 `src/lib.rs:70-83`에는 budget/rootQueryable 비교가 없다.
- 별도 호환 반례: Recruitment budget rows 50→1 역시 빈 diff/적용 가능 true. 동일한 `{"read":"Recruitment","select":["id"],"limit":2}`는 이전 성공, 이후 `ROWS_EXCEEDED`. depth/deadline/cost도 비교하지 않는 것은 정적 확인. 해당 세 항목의 개별 요청 영향은 확인 못함: 개별 실행을 하지 않았다.
- 다음 단계 전 수정: 조회 경로의 신규 공개와 기존 요청 제한 변경을 구분해 SecurityReview/Breaking을 기록한다.

### F6 · P1 · 기존 불변식 변경·unique 추가를 놓쳐 기존 데이터 위반을 허용한다 (Q1·Q2)

- 실행: `limit atMostOnePublished ... where status = PUBLISHED`의 조건을 `status = DRAFT`로 변경. V1 의미 검사 성공, `changes=[]; applicable=true`. 직접 SQL `SELECT count(*) FROM (SELECT club_id FROM <schema>.recruitment WHERE status='DRAFT' GROUP BY club_id HAVING count(*)>1) x` 결과 `1`. DRAFT 101·102가 같은 club=10에 있다.
- 실행: Recruitment에 `unique club` 추가. V1 의미 검사 성공, 빈 diff/적용 가능 true. 같은 기존 DB에 `CREATE UNIQUE INDEX ... ON <schema>.recruitment(club_id)` 적용 시 PostgreSQL `23505`(중복 키) 발생.
- 근거: V7 `src/lib.rs:95-97`는 invariant 이름 추가만 비교한다. 동일 이름의 enforcement 조건·max·per·deferred 변경, invariant 제거, unique 및 checks는 비교하지 않는다. V1 `src/sema.rs:839-890`는 이 facts를 생성하며 V2 `src/sqlgen.rs:349-375`는 unique/불변식 DDL을 생성한다.
- 다음 단계 전 수정: 추가뿐 아니라 기존 제약 변경을 검사하고 실제 집행 방식에 맞는 데이터 위반 측정을 해야 한다. checks의 개별 위반 실행은 확인 못함: 이번 변형에는 포함하지 않았다.

### F7 · P1 · nullable 필드의 range 강화가 Safe로 잘못 분류된다 (Q1·Q2)

- 실행: `internalNote: Text?`→`internalNote: Text(2..100)?`. V1 의미 검사 성공. 결과 `Recruitment.internalNote 타입 변경 Text? → Text?[Safe]`, `applicable=true`. 직접 SQL `... WHERE internal_note IS NOT NULL AND char_length(internal_note)<2` 결과 `1`(id=103, 값 `x`).
- 근거: V1 `src/sema.rs:609`는 range를 ty와 별도로 facts에 저장한다. V7 `src/lib.rs:57-67`는 fields 객체 변경을 알아채지만 ty만 읽고, 새 ty가 nullable인 동일 타입이면 무조건 Safe다. V2 `src/sqlgen.rs:324-326`는 Text range를 실제 CHECK로 만든다.
- 영향·수정: 기존 데이터가 새 필드 제약을 위반해도 적용 가능 판정이다. nullable 전환과 range 변경을 분리하고 새 제약의 기존 데이터 위반을 세어야 한다. 원본 범위의 실제 DDL 마이그레이션은 구현되지 않았으므로 데이터 삭제가 일어났다고 주장하지 않는다.

### F8 · P1 · 현재 데이터의 gained=0을 안전 판정으로 확정하며 시각도 하드코딩한다 (Q1·Q2)

- 문서 `V7-migration-results.md:48`은 미래 데이터 누락을 이미 인정한다. 추가로 V7 `src/lib.rs:111`은 평가 시각을 `2026-10-04T00:00:00Z`로 고정하고, `src/lib.rs:137-142`는 gained가 없으면 SecurityReview를 Breaking으로 낮춘다. '정적 판정과 측정을 함께'라는 문구와 달리 확대 부재를 증명하는 정적 분석은 없다.
- 실행 입력: 양쪽 predicate는 `active(r) = r.status = PUBLISHED`로 동일하게 둔다. rowRead만 `active(this) and ...`→`(active(this) or periodEnd < now) and ...`로 변경. 검사 시각에는 `gained=[]; lost=[]`, `Breaking`, 적용 가능 true. 동일 DB·actor=1로 실제 요청 시각을 `2026-10-11T00:00:00Z`로 바꾸면 이전 `[100]`, 이후 `[100,101,102]`(출력은 id 객체 배열). 새 데이터 없이도 시간이 지나면 DRAFT가 새로 보인다.
- 추가 실행: 논리적으로 같은 Club 정책을 AST만 다르게 만들면 gained/lost가 모두 비어도 Breaking이다. 이 경우 문서 `:23`의 '결과가 줄어듦'은 실제 측정과 일치하지 않는다.
- 다음 단계 전 수정: 측정 0을 확대 없음의 증명과 구분한다. 평가 시각·회원 집합·데이터 snapshot을 명시하고, 확대 부재를 입증하지 못한 정책 변경은 자동 적용 근거로 사용하지 않는다. 어떤 증명 또는 검토를 허용할지는 제품 정책 결정이다.

### F9 · P2 · 불변식의 NULL 그룹 측정과 PostgreSQL unique 집행이 다르다 (Q2)

- 실행: Club.school은 nullable. 학교가 NULL인 Club 12·13에 대해 새 `limit twoUnions on Club = atMost 1 where school = null` 및 `invariant twoUnions per school`을 추가. V7 결과 `violatingGroups=1; Blocked; applicable=false`. 그러나 `CREATE UNIQUE INDEX ... ON <schema>.club(school_id) WHERE school_id IS NULL`은 같은 두 행에서 성공했다.
- 근거: V7 `src/lib.rs:150`은 NULL을 하나의 GROUP BY 그룹으로 세지만, V1 `src/sema.rs:882-883`가 선택하고 V2 `src/sqlgen.rs:370-371`이 만드는 UNIQUE INDEX는 기본 NULL들을 서로 다른 값으로 취급한다.
- 판단: 선언한 atMost 의미를 기준으로 보면 측정 1은 타당하며 DB 집행이 약하다. 현재 생성된 DB 제약을 기준으로 보면 사전 검사만 적용을 막는다. 기준을 혼동하여 위반 수가 실제 DDL 실패 수와 같다고 주장하면 안 된다.
- 다음 단계 전 결정: nullable per를 금지할지, NULL 그룹도 제한할지, 미소속을 제한 대상에서 제외할지 정하고 사전 검사와 집행을 일치시킨다.

### F10 · P2 · rowsUsingRemoved는 고유 행 수가 아니라 필드별 사용 건수다 (Q2)

- 실행: Recruitment에 같은 enum을 쓰는 `previousStatus: RecruitmentStatus?`를 추가한 옛 정의 및 대응 DB 열을 준비. 행 101·102의 status와 previous_status를 모두 DRAFT로 둔 뒤 DRAFT 제거. 결과 `rowsUsingRemoved=4; Blocked`; 실제 영향받는 Recruitment 행은 2개다.
- 근거: V7 `src/lib.rs:161-168`은 resource·필드·제거 값별 count를 모두 더한다. 하나의 행에 같은 enum 필드가 두 개면 중복 집계한다.
- 영향: 사용 유무에 따른 Blocked/Breaking 판정은 이 반례에서 맞지만 데이터 정리 대상 '행 4개'로 읽으면 틀린다. 필드별 사용 건수로 명명·상세 제공하거나 resource별 distinct id를 별도로 센다.

- F3 추가 실행 근거(access 본문만 변경): `clubManagerOnly(a: Member, c: Club.Id)`의 본문을 `managerOf(a,c)`→`a = a and c = c`로 변경. V1 의미 검사 성공, 빈 diff/적용 가능 true. actor=2, `{"aggregate":"Apply.approvedCount","input":{"clubId":10}}`는 이전 `ACCESS_DENIED`, 이후 `[1]`. 참조 이름과 sourceAccess 자체가 같아도 access 본문 변경으로 공개가 넓어진다. 이 추가 근거는 F3에 속한다.

### F11 · P2 · 숫자 조건을 쓰는 정상 불변식 추가는 위반 수 대신 panic을 낸다 (Q2)

- 실행: 원본에 `limit twoNonnegative on Recruitment = atMost 2 where views >= 0` 및 `invariant twoNonnegative per club` 추가. V1 의미 검사 성공. 원본 seed의 club=10에는 해당 행이 3개 있으므로 위반 그룹은 1개여야 한다. `check`는 `src/lib.rs:151`에서 `Error { kind: Parameters(0, 1), cause: None }`로 panic했다.
- 근거: V7 `src/lib.rs:146-151`은 `Ctx::for_ddl`로 만든 조건을 실행하면서 params를 빈 배열로 전달한다. V2 `src/sqlgen.rs:203-204`의 숫자 리터럴은 inline 모드에서도 매개변수를 바인딩한다. 따라서 조건은 `$1`을 포함하고 호출자는 0개를 전달한다. V1 `src/sema.rs:882-886`는 이 정의를 정상 lockedCountCheck로 생성한다.
- 영향·수정: 보안상 자동 적용으로 넘어가지는 않지만 정상 입력의 데이터 측정이 중단된다. CHECK 측정용 SQL의 params를 실제로 전달하거나 지원 범위를 구조화된 오류로 제한해야 한다. 문서 `V7-migration-results.md:50`의 불변식 위반 계산은 원본의 enum 조건 사례까지로 좁혀 읽어야 한다.

### F12 · P2 · 새 필드를 참조하는 정책과 필드 추가의 동시 변경은 기존 DB에서 panic한다 (Q1·Q2)

- 실행: Club에 `canRead: Bool?` 추가와 함께 rowRead에 `or canRead = true` 추가. V1 의미 검사 성공. `check`는 새 정책을 기존 테이블에 그대로 실행하여 `column t.can_read does not exist`(SQLSTATE `42703`), V7 `src/lib.rs:117` panic. nullable 열을 먼저 NULL로 backfill한 가상 상태를 검사하거나 미지원 조합으로 반환하는 동작은 없다.
- 근거: `src/lib.rs:47-51,111-117,132-136`은 새 fields용 DB 구조를 준비하지 않은 채 새 facts로 SQL을 만든다. V2 `src/sqlgen.rs:43-45`는 새 canRead 경로를 can_read 열로 변환한다.
- 영향·수정: 단일 변경 9개 테스트만으로 복합 마이그레이션의 사전 검사가 된다고 볼 수 없다. 실제 적용 순서·backfill 이후의 검사 대상을 정하고 오류를 반환해야 한다. 자동 적용 허용으로 이어지는 반례는 아니므로 P2다.

### F13 · P2 · 테스트는 분류 일부만 검증하며 문서의 측정 수치를 회귀 검증하지 않는다 (Q2·Q3)

- 근거: `spikes/spike-v7-migrate/tests/v7.rs:51-58`은 기대 class가 목록에 **포함되는지**와 applicable만 비교한다. delta의 gained/lost actor·row 집합, violatingGroups=1, rowsUsingRemoved=2/0, existingRows=3을 assert하지 않는다. nullRows 분기에는 원본 테스트 입력 자체가 없다. 옛 요청 거부는 `:60-65`에서 별도 검증한다.
- 판단: 원본 실행 숫자는 이번 재실행에서도 맞았다. 그러나 고장 주입 1개(`V7-migration-results.md:16`)로 검사기의 다른 숫자·미탐지·복합 변경까지 검증했다고 해석할 근거는 없다. 예상 목록과 값의 독립 검증, 정상 변경/위험 변경의 양방향 회귀 입력을 추가할 것을 권장한다.

## Q2 측정 결과와 유효 범위

| 항목 | 이번 실행에서 확인한 정확한 결과 | 한계 |
|---|---|---|
| 정책 확대 | 원본 Club 확대의 gained는 6개 actor/row 쌍. 축소는 actor 1·2·3·익명이 row 12를 잃는 4쌍. 문서 출력과 일치 | 모든 actor가 아니라 Member 테이블의 id+익명(`src/lib.rs:104-106`). 고정 시각, 현재 행만 검사. predicate/access 간접 변경은 검사 자체 미실행(F1/F3). F8의 시각 반례 존재 |
| 불변식 | oneDraft는 위반 그룹 1개(Club 10의 DRAFT 2행). 원본 결과는 정확 | 위반 **행 수**가 아니다. 동일 이름 변경 미검사(F6), NULL 의미 불일치(F9), 숫자 조건 panic(F11). enforcement.max가 없는 partialUniqueIndex에는 기본 1 사용(`src/lib.rs:149`) |
| enum | 원본 DRAFT 사용 2, REJECT 사용 0. 사용 유무 분류는 맞음 | 두 enum 필드가 같은 행에 있으면 4로 중복 합산(F10). 측정값은 snapshot의 사용 건수. 새 enum 목록을 순회하므로 enum 선언 전체 제거는 이 분기에서 다루지 않음(`src/lib.rs:31-35`). 사용 중 enum 전체 삭제는 V1 정의 유효성의 영향을 먼저 받으므로 '데이터 손실을 허용한다'는 반례로 삼지 않음 |
| null | internalNote nullable→필수 변경은 nullRows=3, Blocked/false. 기존 NULL들을 'ok'로 채운 뒤 같은 변경은 nullRows=0, Safe/true | 기존 NULL의 개수 판정은 확인 범위에서 정확. 필드의 다른 제약 강화까지 검사하지 않음(F7). NULL 0은 이후 쓰기 호출의 필수 입력 호환성을 증명하지 않음 |
| 필수 필드 추가 | Club.code는 existingRows=3, Blocked/false. 빈 RecruitmentBookmark에 tag: Text 추가는 existingRows=0, Safe/true | needsBackfill은 ty가 ?로 끝나는지로만 결정. 실제 backfill/기본값/DDL 적용 성공은 검사 범위 밖 |

동시 쓰기에 대한 한계는 정적 확인이다: `check`에는 트랜잭션·snapshot·테이블 잠금이 없고, 정책은 actor별/old·new별로 쿼리한다(`src/lib.rs:104-117,131-188`). 실제 적용 시점의 데이터가 검사 때와 같다는 보장은 없다. 확인 못함: 동시 트랜잭션으로 숫자가 흔들리는 실행, 격리 수준별 영향. 이 한계를 실제 발생한 경쟁 조건이라고 단정하지 않는다.

## Q3 문서 주장과 실행 근거의 차이

1. `V7-migration-results.md:20-26`의 '자동 적용 가능'은 이 spike의 bool 반환이다. 실제 DDL 적용·승인 기록은 `:54`에서 명시적으로 제외했다. 제품의 자동 적용 허가나 안전한 데이터 이전 성공을 뜻하지 않는다. F1-F8 때문에 현재 bool을 다음 단계 적용 gate로 사용할 수 없다.
2. `:22`의 'Safe = 데이터·호출자 영향 없음'은 관측보다 강하다. F7은 기존 데이터를 새 제약 위반 상태로 만들며, 필수화의 nullRows=0은 옛 생성 요청/SDK 호환성까지 입증하지 않는다. `:23`의 Breaking 설명도 gained/lost가 모두 0인 실측과 다를 수 있다(F8).
3. `:28`의 '모든 회원과 익명'은 이 fixture의 Member id 집합에 대해서 맞다. 임의 actor resource·미래 회원·다른 시간·다른 DB snapshot 전체에 대한 판정이 아니다. `:48`에 미래 데이터 한계가 이미 있으므로 새 발견처럼 중복 주장하지 않고 고정 시각 및 자동 강등 문제를 추가로 지적했다.
4. `:50`의 '불변식 추가·enum 제거·필수 필드 추가는 위반 개수를 셀 수 있었다'는 원본 9사례의 일부에서 사실이다. '정상 V1 facts에 일반적으로 가능'으로 확대하면 F9-F12와 맞지 않는다. 원본 `tests/v7.rs`는 수치 자체의 회귀 검증도 하지 않는다(F13).
5. 문서 `:3,54`와 RK-10 `E-technical-risks.md:61`은 정책 승인·DDL·rollback·기존 PoC 비교를 미검증/범위 밖으로 분명하게 남겨 두었다. **이 범위 제외 자체의 발견은 없음.** 검토 범위는 문서·V1/V2/V7 및 임시 PG 데이터이며 실제 운영 마이그레이션은 확인 못함: 구현·실행 대상이 이번 범위에 없다.

### 다음 단계 전 기술적으로 정할 것

- 비교 대상 facts의 전체 목록과 미지원 의미 변경의 처리. 빈 diff가 자동 적용 허가가 되지 않도록 정책/접근/집계/관계/쓰기/제약의 의존성을 포함한다(F1-F7).
- 데이터 측정 단위(행·필드 사용 건수·위반 그룹), 정책 평가 시각, snapshot과 적용 순서, backfill 이후 검사 방식. 0건을 증명으로 승격할 조건을 명시한다(F8-F12).
- 정상 입력에 대한 오류 반환, 분류 및 측정 원자료를 검증하는 회귀 입력(F11-F13). SecurityReview/Destructive/Blocked가 남는 조건을 명시한다.

### 창시자 결정이 필요한 것

- `founder-integrated-directive-2026-10-03.md:644-646`은 데이터 손실·권한 확대의 명시적 승인과 영향 확인을 **DIRECTION**으로 두었다. 누가 어떤 증거를 보고 승인하는지, 읽기뿐 아니라 쓰기·집계·공개 경로 확대를 어떤 gate에 넣을지 제품 정책으로 확정해야 한다. 이 리뷰가 승인 정책을 대신 정하지 않는다.
- `:654-658`의 구버전 필드 호환/폐기 정책과 지원 기간은 OPEN이다. Breaking 자동 적용, SDK 계약 기록, 보안 축소의 즉시 적용과 구버전 요청 처리 관계를 정해야 한다. RK-10 `:21`의 '보안 축소를 호환 유예로 늦추지 않음'을 기준으로 한다.
- `:648-652`의 rollback 방향과 RK-10 `:21`의 데이터·정책·앱 복원 한계에 맞춰 roll-forward/back 보장 범위를 정한다. 삭제 데이터 자동 복구를 기본 가정하지 않는다.
- nullable per의 NULL 그룹 의미(F9)는 기술적 선택에 앞서 도메인 의미를 확정할 필요가 있다. 모든 앱에 공통 규칙을 둘지 정의 작성자가 선언할지도 결정 대상이다.

## 실행 기록·확인 못한 범위

- 작업 원본: `spikes/spike-v7-migrate/src/lib.rs`, `tests/v7.rs`; V1 fixture/parser/sema; V2 sqlgen/plan/execute. 지정 문서·지시·RK-10을 대조했다. 외부 검색은 하지 않았다(판정 기준이 저장소 자체 문서·실행에 있으므로 불필요).
- 원본 코드에 해당하는 테스트는 임시 복사본에서 schema 문자열만 바꾸어 먼저 실행: `cd target/codex-v7-r8-1791062765580952000/spike-v7-migrate && PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture` → 원본 v7 테스트 `1 passed; 0 failed`(9개 사례). 원본에서 직접 실행하면 V2 ddl이 기본 schema를 DROP/CREATE하므로 사용자의 임시 schema 요구를 우선했다.
- 임시 V2 sqlgen의 기본 schema 이름도 `aip_r8_1791062766157976000`으로 바꿨다. 추가 실험 프로세스는 `_probe`, `_edges` schema만 사용했다. 원본 V1/V2/V7 코드는 수정하지 않았다.
- 같은 명령으로 원본 및 두 추가 관찰 harness를 함께 재실행: 각 integration test `1 passed; 0 failed`. 관찰 harness의 통과는 취약점 부재가 아니라 **오분류·DB 결과·포획한 panic의 기록과 schema 정리 성공**을 뜻한다. F11/F12의 panic은 tokio task에서 포획하고 schema를 별도 정리했다.
- 실제 데이터 손실 실행 반례는 없음. 확인 범위에서는 권한 확대·기존 데이터 위반·요청 거부·panic을 재현했다. 확인 못함: 실제 migration DDL 전체 적용, 삭제/변환 데이터 손실, rollback, 구버전 SDK 및 승인 기록. 결과 문서가 범위 밖이라고 밝힌 항목을 검사했다고 주장하지 않는다.
- 테스트 재현용 변경 파일은 모두 허용된 `target/` 내부에만 생성했다. 최종 산출물은 이 파일 하나다. `.env`·비밀 저장소 읽기 및 git 명령은 실행하지 않았다.

## 최종 보강 실행·판정

- F4 보강: 이미 `expose apply close { target id; bulk maxRows 1 }`가 있는 정상 옛 정의에서 close allow만 true로 확대해도 빈 diff/적용 가능 true. 기존 apply의 target을 id→id,where 및 bulk 1→10으로 바꾼 경우도 동일하다. 기존 ClubMember expose create의 allow를 managerOf→true로 변경, 기존 Recruitment expose compose의 bulk 1→10 변경도 각각 V1 의미 검사 성공 및 빈 diff/적용 가능 true. 단순 신규 선언뿐 아니라 기존 쓰기 계약 변경의 미탐지를 확인했다.
- F6 보강: 새 불변식 조건에 해당하는 `CREATE UNIQUE INDEX ... ON <schema>.recruitment(club_id) WHERE status='DRAFT'` 실제 실행은 `23505`. 따라서 위반 그룹 1의 직접 계산과 새 제약 적용 실패가 일치한다.
- F7 보강: `ALTER TABLE <schema>.recruitment ADD CONSTRAINT ... CHECK(char_length(internal_note) BETWEEN 2 AND 100)` 실제 실행은 `23514`(CHECK 위반). Safe 판정과 새 제약 적용 성공이 일치하지 않는다.
- 최종 관찰 로그: `target/codex-v7-r8-1791062765580952000/spike-v7-migrate/r8-run.log`. 최종 명령은 해당 디렉터리에서 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture > r8-run.log 2>&1`. exit 0, 원본/관찰 2개 integration test 각각 `1 passed; 0 failed`. 의도적으로 포획한 두 panic은 F11/F12의 재현이며 원본 코드의 정확성 통과를 뜻하지 않는다.
- DB 정리 검증: `psql -h localhost -d postgres -Atc "SELECT nspname FROM pg_namespace WHERE nspname IN ('aip_r8_1791062766157976000','aip_r8_1791062766157976000_probe','aip_r8_1791062766157976000_edges');"` → exit 0, 출력 0행. 세 임시 schema 모두 남지 않았다.
- 판정: P1 8건, P2 5건. 현재 applicable 값을 실제 자동 적용 허가로 사용할 수 없다. 원본 9사례는 재현됐지만 권한 확대 누락·기존 데이터 위반 누락이 확인됐다. 코드 비교·측정·테스트 정비와 제품 승인/호환 정책 결정을 구분하여 다음 단계에 반영해야 한다.
