# root PoC 문법 → 제품 정의 문법 이식 지도 (읽기 전용 조사, 관찰 수준)

> 상태: 초안(2026-10-07). 서브에이전트의 읽기 전용 조사이며 판정은 관찰 수준이다. 2.2 OPEN 의존 항목은 [창시자 결정 질문](../90-open-questions.md)과 묶여 있어 이식 대상으로 확정하지 않는다. 오늘 제품 문법 변경은 [점검 기록](2026-10-07-pattern-security-audit.md)을 따른다.

조사일 2026-10-07. 저장소 루트 기준(조사 중 수정 없음).

## 0. 기준과 방법

- 원칙: docs/PRINCIPLES.md §2(원칙 1~7), §4 설계 관문 8개. 이식 주의점은 관문 2(호출자 표현 범위), 4(서버 최종 권한), 6(같은 의도 여러 표기), 8(창시자 OPEN)로 본다.
- 경계: crates/aip-service/README.md 관문 6번. `aip service`는 caller 계약 정본만 쓰고 root PoC Core IR(crates/aip-ir)은 제품 계약과 섞지 않는다. 따라서 "이식"은 PoC IR을 가져오는 것이 아니라 제품 문법(spikes/spike-v1-fixture)에 같은 의도를 새로 표현하는 일이다.
- 제품 문법 범위(관찰): spikes/spike-v1-fixture/src/ast.rs, parser.rs, sema.rs. 선언은 enum / actor / predicate / access / limit / resource 뿐이다(parser.rs:172-214). resource 안은 fields, rows read, field read, expose read|apply|create|compose|aggregate, aggregate, transition(from/to/allow/repeat/create/update/notify), unique, check, invariant [deferred], extension(read|write), docs(parser.rs:286-535). 타입은 Id, Id<R>, Text, Url, Time, Int, Bool, enum, Ref, nullable, `(lo..hi)` 범위뿐이다(sema.rs:31-39, 178-182, parser.rs:259-284).
- 제품 쪽 부재 확인은 소스 읽기에 더해 `./target/debug/aip service check <파일>`로 PoC 구문을 넣은 최소 정의를 직접 돌린 결과다(probe 파일은 scratchpad/probe/*.aip). 결과는 아래 "프로브" 절.
- OPEN 기준: plan-docs/alignment/B-decision-reclassification.md(05 단순 변경 FOUNDER 확인, 06 복잡한 동작 조합 FOUNDER+OPEN, 14 일괄 부분 실패 DIRECTION+OPEN, 17 확장 작성 주체 FOUNDER+OPEN, 18 열람 통계 DIRECTION, 19·20 승인·롤백 DIRECTION), plan-docs/STATUS.md:19(쓰기 W0 공개 동작 묶음, W1 호출자 조합, W2 서버 정의 전이 중 표준은 미정), plan-docs/decisions/DD-02-write-scope.md(A / B+ capability 분리 create·update·delete·link·transition·bulk / C').
  "OPEN 의존" = 그 기능을 넣으려면 W0/W1/W2 선택이나 최종 WRITE API, 일괄 실패 정책 같은 미결정 사항을 사실상 정하게 되는 것.

## 0.1 테스트 실행 기록 (오늘 실제 실행)

실행 환경: PATH에 $HOME/.cargo/bin, 로컬 PG postgres://localhost/postgres (pg_isready: accepting connections). 각 파일을 `cargo test -q -p aip-cli --test <파일>`로 하나씩 돌렸고 전부 통과했다. (macOS에 `timeout` 명령이 없어 처음 한 번 헛돌았고, 제거 후 재실행했다.)

| 테스트 파일 | 결과 | 시간 |
|---|---|---|
| shop_e2e | `test result: ok. 1 passed; 0 failed` | 7s(빌드 포함) |
| ariari_e2e | `ok. 4 passed; 0 failed` | 2s |
| saas_e2e | `ok. 2 passed; 0 failed` | 3s |
| cms_e2e | `ok. 8 passed; 0 failed` | 3s |
| search_e2e | `ok. 2 passed; 0 failed` | 3s |
| encrypt_e2e | `ok. 9 passed; 0 failed` | 2s |
| outbound_e2e | `ok. 5 passed; 0 failed` | 4s |
| subscribe_e2e | `ok. 5 passed; 0 failed` | 7s |
| evolve_e2e | `ok. 14 passed; 0 failed` | 4s |
| migration_e2e | `ok. 4 passed; 0 failed` | 3s |

미실행: adoption_flow, service_flow, service_boundaries, codes, diff, golden, contract_golden, unavailable (제품 서비스·CLI 쪽이라 이번 조사 대상 기능과 직접 관계 약함. "확인 못함: 시간 배분").
주의: 위 e2e는 한 테스트 함수가 여러 기능을 순차 검증하는 구조(shop_end_to_end 등)다. 아래 "테스트" 칸의 줄 번호는 그 함수 안에서 해당 기능을 단언하는 위치다.

## 0.2 프로브: 제품 `aip service check`에 PoC 구문을 넣은 결과 (실제 실행)

| 넣은 구문 | 결과 (code @line:col) |
|---|---|
| `n: Int = 0` (기본값) | INVALID_DEFINITION PARSE_EXPECTED "이름 필요, `=` 발견" |
| `e: Email` | UNKNOWN_TYPE "알 수 없는 타입 `Email`" |
| `e: Date` | UNKNOWN_TYPE "알 수 없는 타입 `Date`" |
| `e: Decimal(10,2)` | PARSE_EXPECTED "`..` 필요, `,` 발견" (Decimal 타입 자체가 없음) |
| `expose delete { ... }` | PARSE_EXPECTED "`read` 필요, `delete` 발견" |
| `expose update { fields n }` | PARSE_EXPECTED "`read` 필요, `update` 발견" |
| `search s on n` (resource 안) | PARSE_UNKNOWN_KEY "resource 안 알 수 없는 항목 `search`" |
| `limit 5 per 1m` (resource 안) | PARSE_UNKNOWN_KEY "알 수 없는 항목 `limit`" |
| `expose create { ... idempotent }` | PARSE_UNKNOWN_KEY "expose create 안 알 수 없는 키 `idempotent`" |
| `webhook X via a.b { }` | PARSE_UNKNOWN_DECL "알 수 없는 선언 `webhook`" |
| `schedule X every day at 00:00 { }` | PARSE_UNKNOWN_DECL "알 수 없는 선언 `schedule`" |
| `n: Text personal encrypted` | PARSE_EXPECTED "`:` 필요, `encrypted` 발견" |
| 전이 `to n = n + 1` (상수 증가) | ok:true (오늘 추가된 증감) |
| 전이 `inc(by: Int)` `to n = n + by` (호출자 값 증가) | PARSE_EXPECTED "`{` 필요, `(` 발견" (전이는 호출자 인자를 받지 않음) |
| 대조군: 평범한 resource | ok:true |

## 1. 기능별 조사
(아래 절은 기능 하나씩 추가했다. 번호는 요약표와 같다.)

### 약어
PoC 소스 = crates/aip-syntax/src/parser.rs(P), ast.rs, IR = crates/aip-ir/src/lib.rs(IR) 또는 forms.rs. 제품 = spikes/spike-v1-fixture/src(parser.rs=VP, sema.rs=VS, ast.rs=VA), spike-v2-read, spike-v3-write, spike-v6-transport. 예제 = examples/<앱>/app.aip.

---
## A. 쓰기 계열 (대부분 OPEN 의존)

### A1. 서버 값으로 insert (`insert E { f: actor, g: <계산> } as x`)
- 문법: P:1576-1583 `insert Entity [from <set>] { f: expr, ... } [as bind]`. 예 shop/app.aip:89-90 `insert Order { customer: actor, product, quantity, amount: product.price * quantity } as order`, ariari/app.aip:157-162.
- IR: Stmt::Insert (IR:759). 필드 값은 일반 Expr(actor, 다른 행 필드, 산술).
- 테스트: shop_e2e.rs:49-60(shop_end_to_end, PlaceOrder 주문 생성+동시성), ariari_e2e.rs:57-61(ariari_end_to_end, CreateClub). shop_e2e 1 passed, ariari_e2e 4 passed.
- 제품: 부분. (a) `expose create { allow; fields }` VP:343-365, 호출자가 모든 필수 필드를 직접 보낸다(VS:1309 `required_fields` = nullable 아니고 id 아닌 필드 전부, 빠지면 MISSING_ITEM VS:779-786). allow가 값 검사. (b) 전이 효과 `create`(VA:126-132)는 값을 대상 행에서 서버가 계산. (c) compose `create X from [expr...]`(VP:396-408)는 항목 행 값 출처.
- 주의: 제품 create는 "서버가 채우는 값"이 없다. `customer: actor` 같은 필드도 호출자가 보내고 allow(`member = actor`)로 검증해야 한다. 서버 값 주입을 넣으면 호출자 표현은 좁아지고(보낼 값 감소) 서버 쓰기 선언은 는다. 값 계산식(`amount: price*quantity`)은 임의 식이라 W2 쪽(서버 정의 동작) 성격이다. W0/W1/W2 선택과 직결.
- OPEN 의존: 예 (DD-02 F2, STATUS.md:19의 W0/W1/W2 표준 미정, "최종 WRITE API").
- 난이도: M. 식 평가·타입 검사(VS)에 `actor`/행 경로는 이미 있다. 새로 필요한 건 create의 "서버 값 절"뿐이나 그 모양이 OPEN.

### A2. 호출자 값으로 patch/update (`set f = param`, `update E x where ... set ...`)
- 문법: command 본문 `set product.name = name, product.price = price`(shop/app.aip:95-98, P:1545-1567 assigns, P:1598 update). 낙관적 락 파라미터 `productVersion`이 `versioned`에서 자동 파생.
- IR: Stmt::Set(IR:796), Stmt::Update(IR:772), Assign/AssignOp(IR:844-856).
- 테스트: shop_e2e.rs:65-76(UpdateProduct 권한 거부 AIP.AUTH.FORBIDDEN, 갱신 성공, 동시 편집 AIP.CONFLICT.STALE_VERSION). ariari_e2e.rs:204-218(ClubNotice versioned). 통과.
- 제품: 없음. `expose update`는 프로브에서 PARSE_EXPECTED(`read` 필요) 거부. 값을 바꾸는 유일한 길은 전이(from/to 고정값, 호출자 값 없음; 프로브 `inc(by: Int)` 거부)와 확장 write(VS:1244).
- 주의: 가장 큰 호출자 표현 확장. 임의 필드 patch는 "무제한 요청"에 가장 가깝다(PRINCIPLES §3). 필드별 capability(허용 필드 목록 + allow + 값 범위)로 좁힐 때만 관문 2·4 통과. B 문서 05(단순 변경 FOUNDER 확인: 가능한 한 표준 요청 지원)와 DD-02 F1 B+(create·update·delete·link·transition·bulk 분리)에 해당하나 최종 형태는 OPEN.
- OPEN 의존: 예 (05는 방향 확인, 06·최종 WRITE API 미결).
- 난이도: L. 필드별 mutation capability, 변경 전·후 정책(W-I3 계열), 낙관적 락, 동시성 잠금(V3의 공유 잠금 재사용)이 한꺼번에 필요.

### A3. 호출자 값 증감 (`set f += param`, `-= param`)
- 문법: shop/app.aip:56-59 `command Restock(product, quantity: Int(1..10000)) { allow actor.role = STAFF; do { set product.stock += quantity } }`, :89 `set product.stock -= quantity`. P:1545 assigns, AssignOp::Add/Sub(IR:853-856). DB CHECK(`invariant out_of_stock: stock >= 0`)가 초과 판매 방지.
- 테스트: shop_e2e.rs:49-60(동시 주문 둘 중 하나만 성공, `AIP.INVARIANT.VIOLATED`/OUT_OF_STOCK, 재고 0), :110(Restock). 통과.
- 제품: 부분. 오늘 추가된 전이 `to f = f ± n`은 상수 n만(VS:363 ARITH_NOT_ALLOWED, VS:674). 호출자 인자 증감은 프로브에서 거부.
- 주의: 호출자 수량을 받으면 값 범위(`Int(1..10000)`)를 서버가 타입에서 검증해야 하고 invariant(`stock >= 0`)가 최종 방어선이어야 한다. 제품에는 invariant per/check는 있으나 `stock >= 0` 같은 행 불변식을 전이 결과에 거는 경로는 VS의 `check`(VP:506-510)로 부분 대응. 전이에 인자를 허용하면 W2 전이의 표현 범위가 넓어진다.
- OPEN 의존: 예(전이 인자 = 최종 WRITE API 모양).
- 난이도: M. 전이 인자 선언, 타입 범위 검사, SQL 증감식은 v3 `update`에 이미 `±` 정수 경로가 있다.

### A4. delete / purge / erase / soft delete / on delete 정책
- 문법: `delete Article a where a = article`(cms/app.aip:86, P:1611), `purge`(P:1615), `erase actor`(cms:143, P:1619), `retain ... then purge`(ariari:505), 필드 수식 `on delete cascade|restrict|set null`, `on erase cascade|anonymize|reassign to`(P:857-878 ref_policy), 엔티티 `soft delete [retain 3mo]`(cms:47, ariari:907).
- IR: Stmt::Delete/Purge/Erase(IR:779-787), RefPolicy(IR:320), Traits.soft_delete(IR:448-).
- 테스트: cms_e2e.rs:189-221(publishable_end_to_end: DeleteArticle이 soft delete로 `deleted_at IS NOT NULL`), :223(publishable_hard_delete), :406(impersonation의 DeleteMyAccount erase), ariari_e2e.rs:173-178(erase ADMIN → on erase cascade, repair on erase로 승계). 통과. retain/purge 자동 실행은 e2e 못 찾음(확인 못함).
- 제품: 없음. `expose delete` 프로브 거부. 참조 정책(`on delete`) 선언도 없음(VS 필드에 수식자 없음).
- 주의: 삭제는 비가역 + 권한 확대 쪽(원칙 §4-4)이라 B 문서 19(데이터 손실 승인)와 연결. 참조 정책은 쓰기 API와 독립으로 "스키마 의미"라 분리 가능. 삭제 API는 DD-02 B+ 후보에 포함이라 OPEN.
- OPEN 의존: delete/purge/erase API는 예. `on delete`/`on erase` 참조 정책 선언 자체는 아니오(DB FK 의미만).
- 난이도: 참조 정책 S~M(마이그레이션 plan의 데이터 영향 분류와 맞물림, spike-v7), 삭제 API L.

### A5. 일괄 `each ... partial`, `insert from <set>`, `upsert`, `toggle`
- 문법: P:1645 each partial(ariari:686,702,808), P:1576 insert from, P:1584 upsert(예제 사용 없음), P:1623 toggle(ariari:239,456,716,773). `toggle ClubBookmark { club, member: actor }`는 있으면 지우고 없으면 만든다.
- IR: Stmt::Each/Insert(from)/Upsert/Toggle(IR:759-804).
- 테스트: toggle ariari_e2e.rs:83-89(ToggleClubBookmark 켜고 끄기). each partial은 ariari_e2e.rs:527-546(여러 이미지, 항목별 결과)에서 실행. upsert는 테스트·예제 없음(확인 못함).
- 제품: 일괄은 있으나 의미가 다름. `expose apply ... bulk maxRows N`(VP:334-337)은 전체 성공/실패 원자적. 부분 성공(partial)은 없음.
- 주의: partial은 B 문서 14(일괄 부분 실패 DIRECTION+OPEN, 원자적 후보 우선). 임의로 넣으면 창시자 결정 선점.
- OPEN 의존: each partial 예. toggle은 W2형 전이로 풀 수 있어 약함(전이 2개 + unique).
- 난이도: partial L(결정 필요), toggle M.

### A6. command 본문 전체 (`let`, `require ... else CODE`, `when`, `do { }`, `returns`, `emit`)
- 문법: P:1328-1400 command, P:1568 stmt. 서버가 정의한 임의 절차. 예 ariari/app.aip:153-165.
- 테스트: 위 e2e 전체.
- 제품: 대응 개념이 다르다. 호출자는 `read`/`apply`/`create`/`compose` 표준 요청만 보내고 서버 정의 절차는 전이(+ create/update/notify 효과, VA:114-132)나 확장 write(VS:1244, spike-v4)로 표현.
- 주의: PoC의 "이름 있는 command + 입력 DTO" 모델을 되살리면 호출자 표현 범위는 좁아지나 원칙 1(앱별 백엔드 반복 최소화)에 반한다. 두 방식이 공존하면 관문 6(같은 의도 여러 표기)이 걸린다.
- OPEN 의존: 예(최종 WRITE API 그 자체).
- 난이도: L (그리고 하지 않는 편이 원칙과 맞을 수 있음. 판정 아님, 관찰).

---
## B. 스키마·타입·제약 (대부분 OPEN 비의존)

### B1. 필드 기본값 (`status: OrderStatus = PENDING`, `featured: Bool = false`)
- 문법: P:727 field_decl `= expr`. 예 shop/app.aip:30,41,67(Customer.role = USER, Product.featured = false, Order.status = PENDING), saas/app.aip:37,57. IR: Field.default(IR:256).
- 테스트: shop_e2e.rs:50(PlaceOrder 반환 status = PENDING), :120(featured = false). 통과.
- 제품: 없음. 프로브 PARSE_EXPECTED. create는 비null 필드를 전부 호출자가 보내야 한다(VS:1309).
- 주의: 기본값은 서버가 채우는 값이라 호출자 표현을 좁히고 서버 선언은 줄인다(관문 2·3 모두 유리). 기본값 자리에 식(`now`, `actor`)을 허용하면 "서버 값 주입"(A1)과 겹친다. 상수 리터럴/enum 값만 허용하면 OPEN과 독립.
- OPEN 의존: 아니오(상수 기본값). 식 기본값은 A1과 같은 OPEN.
- 난이도: S. 파서 한 줄, VS 타입 검사에 리터럴 비교가 있고 v3 INSERT에 DEFAULT 열만 더하면 된다. 마이그레이션 plan(`ADD COLUMN ... DEFAULT`)과의 일관 확인 필요(확인 못함: 제품 v7 plan 코드는 읽지 않음).

### B2. 도메인·확장 타입 (Email, Phone, Decimal, Money, Date, Duration, Range, Json, RichText, Upload, Localized, Credential, Uuid, Size)
- 문법: P:878-980 type_expr/refine_opt, IR Type(IR:75-195). 예 ariari `email: Email personal encrypted`(:70), `amount: Money(KRW)`(:961), `period: Range<Time>`(:46), `answers: Json validated by form`(:477), `body: RichText(policy: basic)`(:45), `Upload(max 5MB, types [png, jpg, webp])?`(:153).
- 테스트: Money 정확 소수 왕복 ariari_e2e.rs:248-276(17자리 정수도 보존, 정확한 소수가 아닌 입력 거부), Upload ariari_e2e.rs:460-546(ariari_uploads), Json 동적 스키마 :368-388, Range/no overlap :90-105, Email는 encrypt_e2e.rs:75. 통과. Duration/Localized/Credential/Recurrence는 예제·테스트 없음(확인 못함).
- 제품: Id, Text(범위), Url, Time, Int(범위), Bool, enum, Ref만(VS:178-182). 프로브: Email·Date UNKNOWN_TYPE, Decimal 파싱 실패. 단 spike-v4 write.rs와 aip-service에 "Decimal" 문자열은 Id wire(decimal/safe) 이야기로 값 타입과 무관(관찰).
- 주의: 타입은 호출자 입력 검증의 정본(IR 주석: "validators in every language derive from the same data"). 타입이 늘면 입력 표현 형식이 늘어 SDK(spike-v5)·Id wire·JSON 직렬화(Money를 문자열로 내보내는 규칙, ariari_e2e.rs:255) 결정이 따라온다. 타입 자체는 호출자 권한을 넓히지 않는다.
- OPEN 의존: Email/Decimal/Date/Money 같은 스칼라는 아니오. Json/Upload/RichText는 쓰기 입력 모양과 엮여 부분 의존(Upload는 object store 확장, B 문서 17 확장 작성 주체와 인접).
- 난이도: 스칼라(Email, Decimal(p,s), Date, Duration) S~M(파서·VS 타입표·v2 SQL 캐스트·v3 literal(spike-v3-write/src/lib.rs:60-70 filter_value)·SDK 직렬화 각각 한 곳씩). Money/Range/Json/Upload는 L.

### B3. 제약: unique where/else CODE, cardinality, capacity, no overlap, invariant, record check
- 문법: P:555-621(entity_member 안) `unique (a, b) where cond else CODE`, `exactly|at most|at least one where cond per f [repair on erase do {}]`, `capacity count(set) <= N else CODE`(ariari:655,794), `no overlap (period) per club else CODE`(ariari:348), `invariant name: expr`(shop:43).
- IR: Constraint(IR:357-420).
- 테스트: ariari_e2e.rs:90-105(겹치는 기간 DB가 거부), :180-203(capacity 동시성), :173-178(repair), shop_e2e.rs:56-60(invariant OUT_OF_STOCK). 통과.
- 제품: 부분. `unique (cols)`(VP:505), `check name when expr`(VP:506-510), `invariant name per field [deferred]`(VP:511-519), `limit name on R = atMost N where cond`(VP:203-213), 확장 가능. `else CODE` 오류 코드, `no overlap`, `exactly one`, 부분 unique(`where`)는 없음(관찰: VP에서 unique는 ident_list만).
- 주의: 제약은 서버 최종 권한을 강화하는 쪽(관문 4). 호출자 표현 범위 불변. 같은 의도를 limit/invariant/capacity 여러 문법으로 쓰게 되는 관문 6 문제는 이식 시 먼저 정리 필요(제품에 이미 `limit atMost`와 `invariant per`가 겹침).
- OPEN 의존: 아니오.
- 난이도: M. no overlap(PG exclusion)과 `else CODE`가 새 DDL, 나머지는 기존 limit/invariant 확장.

### B4. lifecycle (`lifecycle status { PENDING -> PAID, CANCELLED }`)
- 문법: P:622 entity_member "lifecycle"; IR Lifecycle(IR:428).
- 테스트: shop_e2e.rs:78-105(PAID 후 늦은 failed 이벤트가 상태를 바꾸지 않음 = 허용 전이만), ariari_e2e.rs:145-156(일괄 승인 시 lifecycle 적용). 통과.
- 제품: 개념은 다르나 목적 일부 대응. `transition name { from; to; allow }`가 이름 있는 공개 동작이고 상태 기계 제약(임의 쓰기가 불법 전이를 못 하게 DB 트리거)은 없다. 제품은 expose update가 없어 임의 상태 쓰기 경로 자체가 없음.
- 주의: update/patch(A2)를 넣는 순간 필요해지는 보호장치. 단독 이식 의미는 약함.
- OPEN 의존: 부분(A2에 종속).
- 난이도: M.

### B5. 엔티티 특성: track created/updated, versioned(낙관적 락), history, soft delete, publishable/drafts, tree, position, sequence, slug
- 문법: P:662-720 entity trait, P:788-819 field mods(tree 788, sequence 798, slug 806, position 814). 예 ariari:69(track), shop:37(versioned), ariari:325,964(history), cms:47-49(soft delete, publishable by), cms:106(`drafts query`), ariari:668(tree max depth 2), ariari:888(position within club).
- IR: Traits(IR:448-475), Generated(IR:339).
- 테스트: versioned shop_e2e.rs:65-76, ariari_e2e.rs:204-218. publishable/drafts cms_e2e.rs:189-250. soft delete cms_e2e.rs:210,221. history는 encrypt_e2e.rs:291(published_and_history_copies)에서 복사본 암호화만 간접 확인, history 자체 조회 e2e는 확인 못함. tree: ariari_e2e.rs:220(댓글 답글이 같은 activity). position: ariari_e2e.rs:353-361(expose 생성 position 순서). sequence, slug: 예제·테스트 없음(확인 못함, 문법과 IR만). track: 단독 단언 확인 못함.
- 제품: 없음(VS에 해당 키 없음). 호출자가 볼 `version` 필드를 select하고 stale 거부하는 경로도 없음.
- 주의: versioned는 update(A2) 없이는 의미 없음. publishable은 읽기 계약(게시본만 읽기)에 영향을 줘 expose read의 기본 행 집합을 바꾸므로 호출자가 보는 데이터 범위와 직결(관문 2). track은 서버가 채우는 열이라 호출자 범위 불변.
- OPEN 의존: versioned/soft delete는 A2·A4에 종속(부분). track/position/tree는 아니오. publishable은 초안·게시 워크플로 = 쓰기 조합과 인접(부분).
- 난이도: track S, position·tree M, versioned M, soft delete M(읽기 필터 + 삭제 API), history L, publishable L.

### B6. 카운터 (`views: Int counter via redis dedupe by client within 1d window 14d` + 쿼리 `touch club.views`)
- 문법: P:819 field mod counter(dedupe by/within, window, sharded), 쿼리 `touch` 절(P:1251, ariari:210,422). IR FieldKind::Counter(IR:299)+CounterDedupe(IR:313), Query.touches(IR:605). 주의: VS:327의 `counter` 함수는 전이 증감 검사 이름일 뿐 이 기능과 무관(이름 충돌).
- 테스트: ariari_e2e.rs:74-79(같은 client 하루 한 번만 증가: `views` = 1). 통과.
- 제품: 없음.
- 주의: B 문서 18(열람 통계, DIRECTION): "명시 선언 시 허용 후보, 일반 select에 숨은 증가를 붙이지 않음". PoC `touch`는 쿼리가 읽기 요청에서 쓰기를 하는 구조라 읽기=무부작용 가정(캐시·프리패치)과 충돌 검증이 남은 항목. 호출자는 증가 대상·값을 못 정하고 서버만 정하므로 관문 4는 안전.
- OPEN 의존: 부분(#18 방향만 확인, 상세 미정).
- 난이도: M. redis 의존 부분(`via redis`)은 별도 확장 경계(관문 5).

### B7. 계산·파생: relation / predicate / fn, 선택식 안 집계, group by, running_sum, avg
- 문법: `relation managerOf(m, c) = membership(m, c).role >= MANAGER`(ariari:88,139-143, P:1006), `fn`(P:1016, FnBody::Wasm 포함), 선택 `balance: running_sum(f.amount) over club order by f.at`(ariari:999), `count(p.orders o where ...)`(shop:52), `group by`(P:1208), AggFn avg(IR:1109). IR: Relation(IR:485), FnDef(IR:493), Node::Agg/RunningSum(IR:1000-1010).
- 테스트: ariari_e2e.rs:248-276(running balance), shop_e2e.rs:119-126(count 기반 rule). 통과. fn(Wasm)·group by·avg는 예제·테스트 없음(확인 못함).
- 제품: 부분. `predicate name(params) = expr`(VP:180)은 접근 판정용, `aggregate name: T { source; groupKey; where; release count|sum(f)|min(f)|max(f) }`(VP:409-421, 638-660)로 집계 필드를 select에 노출. 임의 계산 select, avg, running_sum, fn 없음.
- 주의: 제품은 "호출자가 고르는 것은 이름뿐이고 식은 서버 정의"라는 모양을 유지해 호출자 범위가 좁다. 계산 필드는 서버 선언 식이라 호출자 범위 불변. 단 select 안 임의식을 호출자가 쓰게 하면 안 됨(PoC에서도 서버 정의).
- OPEN 의존: 아니오.
- 난이도: avg S, 계산 필드(`computed name: T = expr`) M, running_sum M~L, fn(Wasm) L(원칙 7 확장과 겹침 → 확장 경계).

---
## C. 읽기 계열

### C1. search (전문/부분 검색) `search TaskSearch on Task fields [title weight A, notes weight B] language english` + `from TaskSearch.match(q) r` + `rank: r.rank`
- 문법: P:1861 search decl, 쿼리 소스 `X.match(q)`; saas/app.aip:187,197,190-194. `language korean`은 형태소 분석 없이 접두 일치(컴파일러 경고). IR: Form::Search(forms.rs:82), QuerySource::Search(IR:634), Node::SearchRank(IR:918).
- 테스트: search_e2e.rs:189(search_end_to_end), :349(search_negative_controls, 각 보호층을 꺼서 공격이 성공하는지 대조). 한국어 접두/AND/타 테넌트/offset 페이지 :256-275. `cargo test -q -p aip-cli --test search_e2e` → `ok. 2 passed`.
- 제품: 없음. 프로브 PARSE_UNKNOWN_KEY. v2-read 소스에 search 문자열 없음.
- 주의: 호출자가 자유 문자열 `q`를 보내는 새 읽기 입력이라 호출자 표현 범위가 넓어진다(관문 2). 단 대상 필드·가중치·언어는 서버 선언, 가시성은 기존 `rows read when`이 최종 결정(PoC도 `visible to`가 행을 거름, saas:184-185 주석). 비용(PG tsvector 점수 계산)은 budget cost에 포함해야 함.
- OPEN 의존: 아니오(읽기 쪽. 05·07 방향과도 충돌 없음).
- 난이도: M. DDL(GIN 인덱스)·SQL 생성·budget 비용 산식·마이그레이션 plan(spike-v7)에 인덱스 추가가 같이 필요.

### C2. keyset 페이지네이션 / 정렬 파라미터 (`page 20 by keyset`, `sort by param ... cases`)
- 문법: P:1223-1237 page, `sort by` P:1282-1315. IR: Page(IR:661), Sort::ByParam(IR:648).
- 테스트: ariari_e2e.rs:278-293(25개 동아리 keyset 두 페이지, 겹침 없음, has_more false). 통과.
- 제품: offset은 있음(opt-in `budget { offset N }`, VP:599 / VA:76 / v2 plan.rs:170-174, 비용에 offset 가산 plan.rs:72-74). keyset/cursor는 없음(v2-read 소스에서 cursor·keyset 문자열 없음). sort는 `sort f1, f2`에서 호출자가 선택(VP:571).
- 주의: keyset 커서는 서버가 서명/검증해야 하는 호출자 입력(변조 시 임의 행 열람 방지). 정렬 키가 가시성에 영향이 없어야 함.
- OPEN 의존: 아니오.
- 난이도: M. 커서 봉투(정렬 키 + id 타이브레이크), 필터와 정렬 키 일관성, v2 SQL 생성 확장.

### C3. 행 가시성·필드 마스킹 (`visible to actor when ...`, `field ... visible to self`, `masked unless ... as ...`)
- 문법: P:642(entity visibility), P:763,768 field mods. IR: Visibility(IR:441), Field.visible_to/masked(IR:264-266).
- 테스트: ariari_e2e.rs:140-144(지원자 본인만 보고 타인은 존재도 모름 NOT_FOUND), shop_e2e.rs:62-63(MyOrder 타인 NOT_FOUND), saas_e2e.rs 전체.
- 제품: 있음(다른 표기). `rows read when expr`(VP:303-309), `field f read when expr`(VP:311-316). masked는 없음.
- 주의: 같은 의도를 두 표기로 두지 말 것(관문 6). 이식 대상 아님, masked만 후보.
- OPEN 의존: 아니오. 난이도: masked S~M.

### C4. 쿼리 속도·비용 절: `limit N per 1m per actor|client`(rate limit), `cached 30s per actor-class`, `plan`, `consistency`, `fetch ext() as x`
- 문법: P:1059 rates, P:1129-1145 cached, P:1238-1245 plan/consistency. 예 ariari:229,446(rate limit). cached는 예제 사용 없음. IR: RateLimit(IR:564), Cache(IR:609).
- 구현: runtime engine.rs:159-186 (`_aip_rate` 테이블, 창 단위 카운터, 초과 시 RATE_LIMITED).
- 테스트: rate limit을 실제로 실행하는 e2e를 찾지 못했다(grep RATE_LIMITED 없음, ClubActiveRecruitment·MyAdminSchoolClubs를 호출하는 테스트 없음). "확인 못함: 실행 테스트 없음, 문법·IR·golden 계획만". cached도 runtime에서 사용 코드 확인 못함.
- 제품: 없음(프로브 `limit` PARSE_UNKNOWN_KEY). 제품의 `budget { rows depth deadline cost }`와 `limit ... atMost`는 다른 개념(요청 1건 비용 상한 vs 데이터 불변식).
- 주의: rate limit은 서버가 호출자를 제한하는 쪽이라 원칙 2에 순기능. 호출자 범위 불변, 서버 선언 추가. cached는 권한이 다른 호출자 간 캐시 오염 위험이 있어 `per actor`가 필수 키여야 한다.
- OPEN 의존: 아니오. 난이도: rate limit S~M(테이블 하나, 전송층 호출 전 검사. 제품 전송층 spike-v6가 호출 단위를 이미 가짐), cached L(권한 안전 키 설계).

---
## D. 멱등·비동기·외부 연동

### D1. 멱등 `command X(...) idempotent [by expr]`
- 문법: P:1349-1350. 예 shop/app.aip:85, ariari:153,365,522,539. IR: Idempotency::{None, CallerKey, Derived}(IR:701-710).
- 테스트: shop_e2e.rs:49-52(같은 키 o1 재전송이 같은 id 반환), ariari_e2e.rs:57-61, :475,537(업로드 키). 통과. 제품 쪽 멱등 이관은 adoption_flow.rs(미실행, 확인 못함).
- 제품: 전송층에는 있고 문법 선언은 없다. spike-v6-transport/src/lib.rs:93-132: (principal, key) 기준 `aip_idem` 표, 같은 키 + 다른 요청이면 IDEMPOTENCY_MISMATCH, 같은 요청이면 저장된 결과 + `replayed`. write extension도 body `{key, request}`(write_extension.rs:18-22,48-51). 프로브: `expose create`에 `idempotent` 키 거부. 어떤 apply가 키를 요구하는지는 문법으로 선언되지 않는다(관찰). aip-service 경로의 키 사용은 소스에서 직접 확인 못함(lib.rs:52 주석만 있음).
- 주의: 키는 호출자가 고르지만 principal로 격리되고 요청 본문 불일치를 서버가 거부하므로 신뢰 경계는 건전. 선언을 문법에 넣을지 전송층 규약(모든 쓰기에 선택 키)으로 둘지는 같은 의도를 두 곳에 쓰지 않게(관문 6) 정해야 한다.
- OPEN 의존: 아니오(W-I6 멱등은 DD-02의 쓰기 불변식 목록이지 미결정 항목 아님). 쓰기 API 모양에는 종속.
- 난이도: S(전송층 기존) ~ M(문법에 요구 여부 선언 + 컴파일 검사).

### D2. 인바운드 webhook `webhook StripePayments via payments.stripe.webhook { on "payment_intent.succeeded"(pi: StripePaymentIntent) do { update ... } }`
- 문법: P:1458-1483. shop/app.aip:106-114. IR: Reaction::Webhook(IR:1251). 서명 검증·5분 창·중복 제거·outbox 경유 dispatcher.
- 테스트: shop_e2e.rs:78-105. 위조(`AIP.AUTH.UNAUTHENTICATED`), 5분 창 밖, ack 후 dispatcher 적용, 재전송 `duplicate: true`, 핸들러 없는 이벤트 `handled: false`. 통과.
- 제품: 없음(프로브 PARSE_UNKNOWN_DECL).
- 주의: 호출자 표현 범위와 무관(외부 시스템이 서명으로 신원 증명). 핸들러 본문이 서버 정의 쓰기 → 값은 서버가 정함. 서버 최종 권한 유지. 제품 spike-v4 outbox(최소 한 번 전달)를 재사용 가능한 경계. 쓰기 문법(update)이 필요하므로 A2의 서버 정의 부분과 연결.
- OPEN 의존: 부분(핸들러 쓰기 모양). 난이도: L.

### D3. 아웃바운드 webhook `outbound webhooks for Endpoint e { events [TaskCompleted] where ... sign hmac_sha256 retry 8 over 24h disable after 3d failing }`
- 문법: P:2118-2145, saas/app.aip:295-298. IR: forms.rs:162 OutboundWebhooks.
- 테스트: outbound_e2e.rs:210(outbound_webhooks_end_to_end), :337(실패만 하는 endpoint 자동 비활성), :398(사설 주소 호출 금지), :439(이벤트가 tenant 명시), :468(negative controls). `ok. 5 passed`.
- 제품: 없음. 가장 가까운 것은 전이 효과 `notify`(VP:491) + v4 outbox(spike-v4-worker/src/outbox.rs, 커밋된 효과만 최소 한 번 전달).
- 주의: SSRF(사설 주소) 방어가 본질. 호출자 표현 범위 불변. endpoint URL을 사용자 데이터로 받으므로 위험은 서버 쪽에 있다.
- OPEN 의존: 아니오(확장 경계는 B 17과 인접). 난이도: L.

### D4. subscribe (실시간 구독) `subscribe LiveProjectTasks(project: Project) { allow ...; from Task t where ...; select {...} }`
- 문법: P:1420-1435, saas/app.aip:159-176. IR: forms.rs:32 Subscribe. 변경이 읽은 테이블이면 모든 구독을 가시성 재검사 후 다시 보냄.
- 테스트: subscribe_e2e.rs:154(subscriptions_end_to_end), :253(인증 필수), :312(클라이언트별 한도), :388(impersonation 토큰 수명), :419(읽은 것만 깨움). `ok. 5 passed`.
- 제품: 없음. B 문서 22(전파 속도: 다시 열 때가 기본)와 DD-11에서 "즉시"는 후순위 선택지.
- 주의: 장기 연결로 호출자가 서버 자원을 점유하므로 한도 필요(PoC에 있음). 갱신마다 권한 재검사(PoC 구현). 읽기 계약 확장이므로 DD-11 결정 전 도입은 임의 결정.
- OPEN 의존: 부분(DD-11 F1 동기화 속도 미정). 난이도: L.

### D5. 시간·백그라운드: schedule / job / retain / at-run
- 문법: `schedule Name every day|week on mon|<duration> at 00:00 tz "Asia/Seoul" { }`(P:1749-1801, ariari:459,627,633, saas:345), `job Name(params) { allow; progress over ...; do {...}; notify ... }`(P:1882-1916, ariari:490,498, saas:319,326), `retain E for 6mo after path then purge notify ... 7d before`(P:1802-1827, ariari:505), `at <time> run Intent(args)`(P:1663, 예제 사용 없음). IR: Schedule(IR:1221), forms.rs:92 Job, IR Reaction::Retain(IR:1187).
- 테스트: schedule은 saas_e2e.rs:335-337(`aip_runtime::schedule::run_now`로 ArchiveSweep·RestoreArchived 실행, saas_e2e 2 passed), cron 시각 계산(`last_due`, schedule.rs:23)의 e2e는 확인 못함. job은 ariari_e2e.rs:389-448(진행률, 가시성 범위 CSV, 만료, 알림, 본문 있는 job은 스냅샷 이후 변경 항목 skip)와 saas_e2e.rs:268(run_jobs). retain과 at-run의 실행 e2e는 확인 못함.
- 제품: 없음. 프로브 schedule PARSE_UNKNOWN_DECL. v4 worker는 확장 실행·outbox 전달용이고 정의 문법 선언이 아님.
- 주의: 호출자 없는 실행 맥락(actor 없음)이 생겨 "서버 최종 권한"이 정의의 `allow` 대신 tenant 고정 같은 구조 보호에 의존(saas_e2e 모듈 주석 1-13). 시간대 필수 규칙은 golden(sema_schedule_without_timezone). job은 `visible to` 범위만 export하는 호출자 맥락 재사용(ariari_e2e.rs:389). schedule 본문의 쓰기 문법이 A2와 얽힌다.
- OPEN 의존: 부분(쓰기 본문). 난이도: schedule M, job L, retain L, at-run M.

### D6. 규칙/이벤트: `rule Name on Entity x when cond do {...}`(edge-triggered), `event`, `emit E {...} [to broker topic "..."]`, `on E e [when ...] do { notify ... }`, `consume`
- 문법: rule P:1828, on_event P:1733, emit P:1402, consume P:1484. shop/app.aip:47-54, ariari:194,197,385. IR Reaction::{Event, Rule, Consume}(IR:1174-1215).
- 테스트: shop_e2e.rs:107-126(SoldOut: 재고 0 한 번만 알림, 재고 보충 뒤 다시 0이면 다시 발송, 다른 표의 count 조건 rule도 dispatcher에서 발화, `_aip_rule_pending` 0). ariari_e2e.rs:106(RecruitmentOpened 이벤트로 북마크한 사용자에게 알림), :294-310(NoticePosted가 회원 전원에게 알림). consume(외부 broker 소비)은 예제·테스트 없음(확인 못함).
- 제품: 부분. 전이 효과 `notify to topic`(VP:491-499, VS:994)와 outbox. 규칙(조건 엣지 트리거), 이벤트 버스는 없음.
- 주의: 호출자 영향 없음. 이벤트 핸들러가 호출자 없이 쓰기하는 문제(D5와 동일).
- OPEN 의존: 아니오(notify는 이미 방향 있음). 난이도: rule L(edge 상태 보관 `_aip_rule_pending`), notify 템플릿 M.

### D7. 알림 `notify <Entity> x where cond via notify.template("name") {vars}`
- 문법: P:1705-1732. ariari:198,386; shop:48. IR Notify(IR:860).
- 테스트: shop_e2e.rs:109(`_aip_notification`에 sold-out 2건), ariari_e2e.rs:106,298. 통과.
- 제품: 있음(범위 좁음): 전이 효과 `notify to expr topic "..."`(VP:491-499, VS:994-). v4 outbox로 전달 보장(최소 한 번 + 공급자 멱등 키).
- OPEN 의존: 아니오. 난이도: S~M(수신자 집합 조건식 `where` 확장).

### D8. 확장/외부 효과 `use payments`, 문장 `ext.call(...) as r into ... on failure do {}`, `fetch ext() as x`, Upload 객체 저장소
- 문법: P:1683-1703(Effect 문, `_` 분기), P:1194(fetch), use P:367. 예 ariari:1074 `export personal data of actor to s3 notify actor`.
- 테스트: Upload ariari_e2e.rs:460-546(스테이징→커밋 뒤 활성, 실패 트랜잭션은 객체 안 남김, 교체 시 커밋 뒤 이전 것 해제, 다중 파트).
- 제품: 확장은 있음 (resource 안 `extension read|write name { input; output; access; effect; deadline; implementation "..." }`, VP:666-702, VS:1244-1268; 구현은 spike-v4 worker). PoC의 first-party 확장(payments, s3, mail)과 객체 저장소 의미는 없음.
- 주의: B 문서 17(확장 작성 주체)이 OPEN. 제품 확장은 계약·권한·비용 통제를 명시 선언하는 모양(원칙 7)이라 PoC `use` 모델과 철학이 다르다. 그대로 이식하면 관문 6(같은 의도 두 방식).
- OPEN 의존: 예(확장 범위·first-party 목록은 17). 난이도: L.

---
## E. 권한·신원·개인정보 구조

### E1. 테넌트 격리 `tenant via path`, `internal cross tenant`
- 문법: P:703(trait tenant), P:373-398(`internal`/`cross tenant`), spec/grammar.md:134-161. 예 saas/app.aip:49,62, cms:48. 다섯 규칙: 참조는 테넌트를 넘지 않음(DB 트리거 `AIP.TENANT.MISMATCH`), intent 하나 = 테넌트 하나, 읽기는 앵커 테넌트로 자동 필터, 한 트랜잭션 쓰기는 한 테넌트(`aip.tenant` 고정), 테넌트 간 작업은 `internal cross tenant`만. IR: Traits.tenant(IR:448-), crates/aip-ir/src/tenant.rs.
- 테스트: saas_e2e.rs:424(saas_end_to_end: 다른 workspace 행은 존재하지 않는 것처럼 NOT_FOUND :441-464, 두 테넌트 소속자의 교차 공격 :465-492, 호출자 없는 맥락의 테넌트 고정 :493-511, 두 테넌트 파라미터 :512-540), :582(saas_negative_controls: 각 층을 끄면 공격이 성공하는지). search_e2e.rs·outbound_e2e.rs에도 타 테넌트 대조. `cargo test -q -p aip-cli --test saas_e2e` → `ok. 2 passed`.
- 제품: 없음. 행 접근은 `rows read when ...`을 앱이 직접 써야 하고 참조 일관성(교차 테넌트 참조 금지)은 선언이 없다.
- 주의: 서버 최종 권한을 구조적으로 강화(원칙 2·4, 호출자 범위 불변, 앱 개발자 서버 선언은 줄음). 다만 DB 트리거·`aip.tenant` 세션 변수는 v3 쓰기 트랜잭션과 v2 읽기 SQL 모두를 건드린다. 쓰기 조합(W0/W1)에서 한 번에 여러 행을 건드릴 때 "한 트랜잭션 = 한 테넌트" 규칙이 조합 설계를 제약한다(OPEN 쪽에 영향을 주는 방향).
- OPEN 의존: 아니오. 단 쓰기 조합 설계와 상호작용.
- 난이도: L. 선언 + 읽기 필터 파생 + 쓰기 트리거 + 마이그레이션 + 음성 대조 테스트까지 5층.

### E2. actor `superuser when ... audited`, 쿼리/명령 `audited`
- 문법: P:450-474 actor(superuser audited 470), P:1351 audited. ariari:13, cms:10, ariari:379. 모든 allow/visible에 일관 적용되는 전역 우회.
- 테스트: ariari_e2e.rs:90-105(ApproveRecruitment 슈퍼 관리자, 감사), cms_e2e.rs:690-756(negative controls에서 privileged check 제거 대조).
- 제품: 없음(소스에 audit/superuser 문자열 없음). actor는 `actor Member` 한 줄 + 외부 JWT 매핑(aip-service README).
- 주의: 가장 큰 권한 확대 장치. "요청의 자유와 실행 권한 분리"에서 호출자 쪽 권한이 아니라 서버가 정한 조건이라 허용 가능하나, 제품의 B 문서 19(권한 확대 변경 승인)·SecurityReview 분류와 맞물려야 하며 `audited` 기록 저장소가 필요.
- OPEN 의존: 아니오(19는 배포 승인이지 문법 아님). 난이도: M.

### E3. 개인정보·암호화 `personal`, `encrypted`, `personal ... access audited`, `erase`, `export personal data`
- 문법: P:525(entity personal), P:755-760(field personal/encrypted), erase P:1619, export P:1672. ariari:68-72,1074; cms:16,138,143. 키는 환경변수(`crypto::ENV_KEYS`) 주입, 키 회전/rekey.
- 테스트: encrypt_e2e.rs 9개 전부(`ok. 9 passed`): 75(DB에는 암호문, API는 평문), 133(다른 행·필드로 옮긴 암호문 거부), 163(키 없는 서버는 기동 안 함), 218(회전과 rekey가 모든 복사본에 도달), 291(게시본·history 복사본 동일 암호문), 327(job export는 요청자가 볼 수 있는 것만 복호화), 346(계약이 클라이언트에 암호화를 알리지 않음), 356(암호화 on/off·이름 변경은 거부), 378(키 잃은 서버는 평문 저장 대신 쓰기 실패). ariari_e2e.rs:25 이메일 암호화. cms_e2e.rs:406(erase).
- 제품: 없음(프로브 `personal encrypted` 거부).
- 주의: 호출자 범위 불변(서버 내부 보호). 원칙 4와 부합. 키 관리는 운영 설정이 늘고(관문 3 운영 쪽) 제품 README 운영 경계(토큰·DB URL·JWKS를 로그에 안 남김)와 같은 위생 요구. 마이그레이션에서 암호화 on/off 거부 규칙(encrypt_e2e.rs:356)을 v7 분류에 맞춰야 함.
- OPEN 의존: 아니오. 난이도: L(키 스토어, 행 바인딩 암호문, 회전, v2 필터/정렬 불가 처리).

### E4. 도메인 형식 L3: verification / grant link / approval / consent / impersonate
- 문법: verification P:1917(ariari:90-98: 6자리 코드 ttl 10m 시도 5회), grant link P:1976(ariari:302-311, saas 없음: 1회용·대상 지정 초대), approval P:2039(ariari:969-975, saas:308-313: 승인자 집합·본인 승인 금지·만료), consent P:2146(cms:125), impersonate P:2188(cms:129: 이유 필수 ttl 30m, 연쇄 불가, 본인만 가능한 일 불가). IR는 crates/aip-ir/src/forms.rs:114-195, 생성 intent는 form_intents.rs.
- 테스트: ariari_e2e.rs:303(ariari_forms)에서 verification :312-331, grant links :332-352; ariari_approval :548-654(요청자 제한, 투표 규칙, 한 번의 거절이 결정, 철회, 만료, 동시 최종 투표 결정 한 번). cms_e2e.rs:279(consent_end_to_end), :481(impersonation_end_to_end), :550(ttl), :603(HTTP 토큰), :807(운영자 권한을 매 호출 재검사). cms 8 passed, ariari 4 passed.
- 제품: 없음. impersonation 토큰과 principal 관리는 제품 서비스(`aip service principal bind|revoke`)가 JWT 쪽으로 따로 다룸(README 실행 절차).
- 주의: 이 형식들은 모두 "내장 intent를 컴파일러가 생성"하는 구조라 호출자가 보내는 이름 있는 동작이 늘어난다(원칙 1 vs 관문 6). 그대로 이식하면 제품의 표준 요청(read/apply/create/compose) 밖에 새 호출 종류가 생김. impersonate는 호출자 쪽 신원을 대리하게 하므로 관문 4에서 가장 민감.
- OPEN 의존: approval·grant는 쓰기 조합과 인접(부분), impersonate·consent는 아니오. 난이도: verification M, consent M, grant link L, approval L, impersonate L.

### E5. expose (생성 CRUD) `expose ClubFaq { read: visible; create: adminOf(actor, club) fields [club, question, answer]; update: ...; delete: ... }`
- 문법: P:2091-2117, ariari:891-896, :1027(ClubEvent). 자동으로 CreateX/UpdateX/DeleteX/ListX intent 생성, `position` 순서.
- 테스트: ariari_e2e.rs:353-366(admin만 쓰기: bob 생성·갱신 `AIP.AUTH.FORBIDDEN`, 목록은 position 순, 삭제 후 1건). 통과.
- 제품: 일부 같은 의도가 이미 다른 형태로 존재한다. `expose read`(VP:317, 546-611), `expose create {allow, fields}`(VP:343-365), `expose apply T { target; bulk; sameScope }`(VP:318-342), `expose compose`(VP:366-409). update/delete 해당 없음(프로브 거부).
- 주의: PoC expose는 DD-02 B+(capability 분리 create·update·delete·link·transition·bulk)와 구조가 같다. 제품 expose create/apply/compose는 W0/W1/W2 실험 산물이다. PoC expose를 가져오면 "최종 WRITE API"를 정하는 일이 된다. 정확히 관문 6·8.
- OPEN 의존: 예. 난이도: L(결정이 먼저).

---
## F. 정의 진화·운영

### F1. 이름 변경·삭제 의도 `entity New was Old`, `field was old`, `removed field x`, `removed entity X`, `upcast`, 정의 안 데이터 마이그레이션 `migration name { update ... }`
- 문법: was P:522(entity)/751(field), removed P:426(entity)/550(field), migration P:432, upcast P:1046. 예 saas/app.aip:367 `migration backfill_done_notes`. IR: Program.removed_entities(IR:68-72), Entity.was/removed_fields(IR:228-235), forms.rs:194,213.
- 테스트: evolve_e2e.rs 14개(`ok. 14 passed`): 122(추가는 데이터 보존), 148(필드 삭제는 선언 필요, 거부되면 아무것도 안 바뀜), 174(`was`로 이름 변경해도 값 이동), 199·221(깨지는 데이터는 건수와 함께 거부), 280(plan은 아무것도 안 바꿈), 318(동시 기동 한 번만 적용), 332(`removed entity`만 삭제), 431(제약·인덱스·참조까지 이동). migration_e2e.rs 4개(`ok. 4 passed`): 64(한 번만, 순서대로, 수정 불가), 107(실패 시 롤백·기동 중단), 128(동시 기동 한 번), 145(모든 tenant 한 트랜잭션).
- 제품: 별도 방식이 있다. spike-v7-migrate가 facts 두 버전을 비교해 Safe/Breaking/Destructive/SecurityReview/Blocked로 분류(lib.rs:1-5), `aip service migrate`가 plan digest를 인정한 것만 적용(README 운영 경계). 정의 안에 `was`/`removed` 의도를 쓰는 문법은 없음(VP에서 `was` 문자열 없음).
- 주의: 이름 변경을 문법으로 선언하는 것은 "AI·사람이 의도를 선언" 쪽(원칙 4)이지만 B 문서 19·20·21(승인, 롤백, 구필드 호환 기간)이 OPEN. 제품의 v7 분류는 "의도 없이 diff로 추측"하는 구조라 rename을 delete+add로 볼 위험이 있고 `was`는 그 간극을 메운다(관찰: 실제로 v7이 rename을 어떻게 분류하는지는 확인 못함).
- OPEN 의존: 부분(19·20·21). 난이도: `was` M, `removed` M, 정의 안 migration 블록 L(쓰기 문법 의존).

### F2. 설정·플래그·projection·event upcast: `config`, `flag`, `projection`, `upcast`
- 문법: config P:2157, flag P:2166, projection P:1841, upcast P:1046. IR ConfigDef(IR:520), FlagDef(IR:527), forms.rs:73,213.
- 테스트·예제: 네 예제에 쓰인 곳이 없고 e2e도 없음. 문법·IR·golden 정도만 존재(확인 못함: 별도 단위 테스트 유무).
- 제품: 없음. 우선순위 낮음.
- OPEN 의존: 아니오. 난이도: config S, flag S~M, projection L, upcast M.

---
## 2. 요약표 (OPEN 비의존이면서 영향 큰 순 → OPEN 의존)

"통과" = 오늘 `cargo test -q -p aip-cli --test <파일>` 실제 실행 결과. 영향 크기는 예제 4개(ariari, shop, saas, cms)에서 쓰이는 정도와 이 기능 없이 제품 정의로 표현할 수 없는 앱 범위에 대한 관찰 기반 추정이며 측정값이 아니다.

### 2.1 OPEN 비의존

| # | 기능 | PoC 근거·테스트 통과 | 제품 문법 현황 | OPEN 의존 | 난이도 |
|---|---|---|---|---|---|
| B1 | 필드 기본값 `= 0`, `= USER` | P:727, IR:256, shop:30,41,67. shop_e2e:50,120 통과(1 passed) | 없음. 프로브 PARSE_EXPECTED. create는 비null 전부 호출자 입력(VS:1309) | 아니오(상수만) | S |
| B2a | 스칼라 타입 Email, Phone, Decimal, Date, Duration, Money | P:878-980, IR:75-195. ariari_e2e:248-276 Money 통과(4 passed), encrypt_e2e:75 Email 통과(9 passed). Duration은 확인 못함 | Id/Text/Url/Time/Int/Bool/enum/Ref만. 프로브 Email·Date UNKNOWN_TYPE, Decimal 파싱 실패 | 아니오 | S~M(Money·Range는 L) |
| E1 | 테넌트 격리 `tenant via`, `cross tenant` | P:703, grammar.md:134-161, tenant.rs. saas_e2e:424,582 통과(2 passed). search·outbound에서도 대조 | 없음. 행 접근은 앱이 `rows read when`으로 직접 | 아니오(쓰기 조합과는 상호작용) | L |
| D1 | 멱등 선언 `idempotent [by]` | P:1349, IR:701. shop_e2e:51, ariari_e2e:57-61 통과 | 전송층에만 있음(spike-v6 lib.rs:93-132 `aip_idem`, `IDEMPOTENCY_MISMATCH`). 문법 선언 없음. 프로브 `idempotent` 거부 | 아니오 | S~M |
| C1 | search 전문/접두 검색 | P:1861, forms.rs:82, IR:634. search_e2e:189,349 통과(2 passed) | 없음. 프로브 PARSE_UNKNOWN_KEY | 아니오 | M |
| C2 | keyset 페이지네이션, sort by param | P:1223-1237, IR:661. ariari_e2e:278-293 통과 | offset만 있음(opt-in `budget offset`, VP:599). keyset 없음 | 아니오 | M |
| B3 | 제약 확장: no overlap, exactly/at least one, 부분 unique, `else CODE`, capacity | P:555-621, IR:357-420. ariari_e2e:90-105,180-203, shop_e2e:56-60 통과 | 부분: unique(cols), check, invariant per [deferred], limit atMost where | 아니오 | M |
| C4a | rate limit `limit N per 1m per actor` | P:1059, runtime engine.rs:159-186. 실행 테스트 확인 못함(e2e 없음) | 없음. 프로브 `limit` 거부. budget은 다른 개념 | 아니오 | S~M |
| E3 | personal / encrypted / erase / export personal data | P:525,755-760, encrypt_e2e 9개 전부, cms_e2e:406. 통과(9 passed, 8 passed) | 없음. 프로브 `personal encrypted` 거부 | 아니오 | L |
| B7 | 계산·파생(computed select, avg, group by, running_sum, fn) | ariari:999, shop:52. ariari_e2e:248-276 통과. avg·group by·fn 확인 못함 | 부분: predicate, aggregate release count/sum/min/max | 아니오 | S~L(avg S, computed M, running_sum M~L, fn L) |
| B5a | track created/updated, position, tree | P:662,788,814. ariari_e2e:220,353-361 통과. track 단독 단언 확인 못함 | 없음 | 아니오 | S(track)/M(position, tree) |
| E2 | actor `superuser when ... audited` | P:470, ariari:13, cms:10. ariari_e2e:90-105, cms_e2e:690-756 통과 | 없음(소스에 audit/superuser 없음) | 아니오(권한 확대 승인 19와 인접) | M |
| D7 | notify 수신자 집합·템플릿 | P:1705, shop:48, ariari:198. shop_e2e:109, ariari_e2e:106 통과 | 부분: 전이 효과 notify + v4 outbox(VP:491) | 아니오 | S~M |
| D3 | 아웃바운드 webhook(서명·재시도·SSRF 방어) | P:2118, forms.rs:162. outbound_e2e 5개 통과(5 passed) | 없음(outbox만) | 아니오 | L |
| D5 | schedule, job, retain, at-run | P:1749,1882,1802,1663. saas_e2e:335(run_now), ariari_e2e:389-448(job) 통과. cron 시각·retain·at-run e2e 확인 못함 | 없음. 프로브 schedule 거부 | 부분(본문 쓰기) | M(schedule), L(job, retain) |
| D2 | 인바운드 webhook | P:1458, shop:106. shop_e2e:78-105 통과 | 없음. 프로브 PARSE_UNKNOWN_DECL | 부분(핸들러 쓰기) | L |
| D6 | rule(엣지 트리거), event/emit/on, consume | P:1828,1733,1402. shop_e2e:107-126, ariari_e2e:106,294 통과. consume 확인 못함 | 없음(notify만) | 아니오 | L |
| A4b | 참조 정책 `on delete cascade/restrict/set null`, `on erase` | P:857, IR:320. ariari_e2e:173-178 통과 | 없음 | 아니오(삭제 API와 분리 가능) | S~M |
| B6 | 카운터 `counter via ... dedupe`, `touch` | P:819,1251, IR:299. ariari_e2e:74-79 통과 | 없음(VS:327 counter는 이름만 같은 다른 것) | 부분(#18 DIRECTION) | M |
| C3b | masked 필드 | P:768. 실행 테스트 확인 못함 | 없음(`field read when`은 있음) | 아니오 | S~M |
| C4b | cached 쿼리 | P:1129, IR:609. 예제·runtime 사용 확인 못함 | 없음 | 아니오 | L |
| F2 | config, flag, projection, upcast | P:2157,2166,1841,1046. 예제·e2e 없음, 확인 못함 | 없음 | 아니오 | S~L |

### 2.2 OPEN 의존 (쓰기 조합 W0/W1/W2, 최종 WRITE API, 일괄 실패 정책, 확장 범위, 동기화 속도, 승인·롤백 등)

| # | 기능 | PoC 근거·테스트 통과 | 제품 문법 현황 | OPEN 의존 | 난이도 |
|---|---|---|---|---|---|
| A2 | 호출자 값 patch/update (`set f = param`, `update ... set`) | P:1598, IR:772,796. shop_e2e:65-76, ariari_e2e:204-218 통과 | 없음. `expose update` 거부. 전이는 호출자 값 없음 | 예(DD-02 F1 B+, 05 방향만 확정) | L |
| A1 | 서버 값 insert (`customer: actor, amount: price*qty`) | P:1576, IR:759. shop_e2e:49-60, ariari_e2e:57-61 통과 | 부분: expose create(호출자가 전 필드), 전이 효과 create, compose create | 예(W0/W1/W2) | M |
| A3 | 호출자 값 증감 `+= quantity` | shop:56-59,89, IR:853. shop_e2e:49-60,110 통과 | 상수 증감만(`to f = f ± n`). 호출자 인자 거부 | 예(전이 인자 모양) | M |
| A4a | delete / purge / erase / soft delete API | P:1611-1619, IR:779-787. cms_e2e:189-221,406, ariari_e2e:173 통과. retain·purge 실행 확인 못함 | 없음. 프로브 `expose delete` 거부 | 예(DD-02 B+, 19 데이터 손실 승인) | L |
| E5 | expose 생성 CRUD (read/create/update/delete) | P:2091, ariari:891. ariari_e2e:353-366 통과 | 일부 같은 의도: expose read/create/apply/compose(W 실험 산물). update/delete 없음 | 예(최종 WRITE API 그 자체) | L |
| A6 | command 본문(let/require/when/do/returns) | P:1328, ariari:153. e2e 전체 통과 | 전이 + 확장 write로 대체. 호출자 정의 절차 없음 | 예 | L |
| A5 | each partial, insert from, upsert, toggle | P:1645,1584,1623. ariari_e2e:527,83 통과. upsert 확인 못함 | 원자적 bulk만(`maxRows`). partial 없음 | 예(14 일괄 부분 실패) | L(partial)/M(toggle) |
| B4 | lifecycle 상태 기계 제약 | P:622. shop_e2e:78-105, ariari_e2e:145 통과 | 전이로 부분 대응, 불법 전이 차단 없음(update 자체가 없음) | 부분(A2 종속) | M |
| B5b | versioned(낙관 락), soft delete, history, publishable/drafts | P:694,688,684,698. shop_e2e:65-76, cms_e2e:189-250 통과. history 조회 e2e 확인 못함 | 없음 | 부분(A2·A4 종속, publishable은 읽기 범위에도 영향) | M(versioned, soft delete)/L(history, publishable) |
| D8 | first-party 확장(payments/s3/mail), 객체 저장소 Upload | P:1683, ariari_e2e:460-546 통과 | extension read/write는 있음(계약·권한·비용 선언형). first-party 의미·객체 저장소 없음 | 예(17 확장 작성 주체) | L |
| D4 | subscribe 실시간 구독 | P:1420, forms.rs:32. subscribe_e2e 5개 통과(5 passed) | 없음 | 부분(DD-11 F1 동기화 속도) | L |
| E4 | approval, grant link, verification, consent, impersonate | P:1917-2188, forms.rs:114-195. ariari_e2e:303-352,548-654, cms_e2e:279-550 통과 | 없음 | approval·grant는 부분(쓰기 조합), consent·verification·impersonate는 아니오 | M~L |
| F1 | `was`/`removed` 의도, 정의 안 migration 블록 | P:522,751,426,550,432. evolve_e2e 14개, migration_e2e 4개 통과 | 다른 방식: v7 facts diff 분류 + `aip service migrate` plan digest. 정의 안 의도 문법 없음 | 부분(19·20·21) | M~L |

## 3. 관찰 요약

1. 제품 문법은 "읽기 계약(select/filter/sort/traverse/budget/aggregate), 서버 정의 전이, 호출자 값 create, compose, 확장"까지다. PoC의 대부분(타입 풍부화, 기본값, 제약 풍부화, 검색, 키셋, 멱등 선언, rate limit, 암호화, 테넌트, 백그라운드·연동 선언)은 프로브로 거부됨을 확인했다.
2. OPEN 비의존이면서 가장 작은 비용으로 막힘을 푸는 후보는 기본값(B1)과 스칼라 타입(B2a)이다. 둘 다 호출자 표현을 넓히지 않고 서버 선언을 줄인다. 다만 제품 create는 필수 필드를 호출자가 전부 보내야 하므로, 기본값을 넣을 때 "create `fields`에서 빠진 필드는 기본값으로 채운다"는 규칙이 expose create의 현재 MISSING_ITEM 규칙(VS:779-786)과 바뀌는 지점이라 먼저 정의해야 한다(관찰 기반 추정).
3. 쓰기 모양을 정하는 기능(A1~A6, E5, B4, B5b 일부)은 W0/W1/W2와 최종 WRITE API를 사실상 정하게 되므로 이번 이식 범위에서 분리하는 편이 관문 8에 맞다. 특히 PoC `expose X { create/update/delete }`와 `command` 모델은 제품의 compose/apply와 같은 의도를 다른 표기로 쓰는 것이라 관문 6에 걸린다.
4. 이식은 PoC IR을 가져오는 것이 아니라 제품 문법(VP/VS/v2/v3)에 새로 쓰는 일이다(README 관문 6). 대부분의 PoC 구현(예: aip-pg 계획 생성, aip-runtime)은 제품 경로에서 재사용되지 않으므로 "난이도"는 제품 쪽 파서·검사·SQL 생성·마이그레이션 분류를 같이 고치는 비용 추정이다.
5. 테스트 신뢰도 한계: e2e는 기능별 단위가 아니라 시나리오 통합이다. 통과는 "PoC에서 그 기능이 동작한다"의 근거이고 제품 이식 후의 정답 기준(오라클)으로는 입력·단언을 추려서 써야 한다. rate limit, retain, at-run, cached, slug, sequence, config, flag, projection, upcast, consume, upsert, history 조회는 실행하는 e2e를 찾지 못했다("확인 못함").


## 변경 이력

- 2026-10-07: 초안. PoC e2e 10개 실행, 제품 check 프로브.
