# 프레임워크 실험 정확성 검토 r4b

기준일: 2026-10-04. 범위: Q1–Q6. 프로세스 격리는 제외한다. 저장소 코드와 결과 문서는 수정하지 않는다.

검증 방식: 명시된 기준 문서 → 코드 → 기존 테스트 → 임시 복사본의 정상 변형·경계 사례. 발견은 확인 즉시 아래에 append한다.

## 실행 절차 이탈 기록

V2 기존 테스트의 첫 임시 실행에서 테스트 SQL의 schema만 치환하고 `sqlgen.rs:17` 기본 schema를 놓쳤다. DDL(`sqlgen.rs:298`)이 `aip_v2_spike`를 DROP/CREATE하고 seed는 `r4b_2_spike.school`을 찾아 `42P01`로 실패했다. 실행 전 기존 schema 존재·내용은 확인 못함: 사전 조회를 하지 않았다. 따라서 이 실행은 사용자 지정 DB 범위를 위반했다. 새로 생성한 schema는 비어 있는 fixture 테이블 상태에서 제거하고, 이후 임시 코드 기본값도 `r4b_2_spike`로 치환한다. 이 사건은 프레임워크 발견이나 기존 테스트 실패로 집계하지 않는다.

## F01 · P1 · Q4: 동적 select의 결과 타입이 실제 선택보다 넓어진다

잘못된 점: `select: [cond ? "title" : "views"]`에서 SDK는 `title: string`과 `views: number`가 모두 있는 행으로 추론한다. 화면 분기에 따른 정상 select가 선택하지 않은 필드를 필수 필드로 보장한다. SDK 캐시 런타임 전에 수정해야 한다.

근거: `spikes/spike-v5-sdk/sdk/aip.ts:30`의 union→intersection과 `:32`·`:42`의 `S[number]` 분배. 임시 `r4b_sdk`에서 `rows[0].title`을 string, `rows[0].views`를 number 변수에 동시에 대입해도 `SDK dynamic: tsc=true`. `any`, 타입 단언, SDK 변경 없이 재현했다. 실제 V2 응답과의 실행 대조는 뒤에 추가한다.

권고: 고정 tuple과 선택 항목 자체의 union을 구별한다. 동적 선택은 가능한 결과의 union 또는 조건부/optional 필드로 보수적으로 추론하고, 고정 tuple 정밀 추론은 유지한다.

## F02 · P2 · Q4: select 형식 오류가 타입 검사를 통과한다

잘못된 점: 관계 select 객체에 계약 밖 키를 더하거나 빈 select를 보내도 컴파일된다. 서버의 보호는 유지되지만 SDK가 약속한 조기 오류 탐지를 일부 놓친다.

근거: `spikes/spike-v5-sdk/sdk/aip.ts:12`·`:48`은 구조적 generic 제약으로 여분 키와 빈 배열을 허용한다. 입력 `select: [{club:{select:["name"]}, school:{select:["id"]}}]` → `SDK extra: tsc=true`; 같은 JSON을 V2 `plan_read`에 보내면 `BAD_REQUEST`(select 항목 형식 오류). `select: []` → `SDK empty: tsc=true`, 서버 → `BAD_REQUEST`(select가 비었음). 서버 검사: `spikes/spike-v2-read/src/plan.rs:123`·`:190`·`:242`. V5 결과 §2의 8개 음성 대조는 해당 형식을 다루지 않는다.

권고: 정확히 한 관계 키와 비어 있지 않은 select를 검사한다. 일반 TS 구조적 타입의 한계와 서버 검증 유지도 계약에 명시한다.

## F03 · P1 · Q4: select에서 숨긴 정상 filter 필드의 값 타입이 never다

잘못된 점: 필터 계약은 열려 있고 select에서는 닫힌 필드에 대한 정상 요청을 SDK가 거부한다. 공개 출력과 허용 필터 입력을 하나의 타입 목록으로 합쳤기 때문이다. 계약 생성 범위를 넓히기 전에 분리해야 한다.

근거: V1 fixture의 `expose read` select에서 `periodEnd`를 빼고 `filter periodEnd.gte, periodEnd.lte`는 유지한 선언을 V1이 정상 facts로 만든다. V2 요청 `{read:"Recruitment",select:["id"],filter:[{field:"periodEnd",op:"gte",value:"2026-10-09T00:00:00Z"}]}`의 계획은 정상(`SERVER filteronly None`). 생성 SDK는 `GEN filteronly: tsc=false ... Type 'string' is not assignable to type 'never'`. `spikes/spike-v5-sdk/src/lib.rs:30`은 select 필드만 생성하고 `sdk/aip.ts:16`은 filter 값 타입을 그 fields에서 찾는다. V1 허용 기준은 `spikes/spike-v1-fixture/src/sema.rs:1070`, V2는 `src/plan.rs:262`에서 원본 facts 필드 타입을 쓴다.

권고: 공개 select 출력 타입과 filter 전용 입력 타입을 별도로 생성한다. filter 허용이 곧 필드 select 허용이 되지 않도록 한다.

## F04 · P2 · Q4: Id filter에서 SDK 숫자와 서버 문자열 계약이 다르다

잘못된 점: V1이 허용하는 `id.eq` 계약을 추가하면 SDK는 number를 받지만 V2는 문자열 Id만 받는다. 정상 필터 요청에 transport가 별도 변환을 하지 않는 현재 `client(send)` 구현에서는 런타임 거부다.

근거: `spikes/spike-v5-sdk/src/lib.rs:51`의 `Id<R> = number & { readonly __resource?: R }`, `sdk/aip.ts:16`·`:55`의 값 타입과 그대로 전달. 임시 facts에서 `id.eq`를 열어 `{field:"id",op:"eq",value:100}` → `SDK numberid: tsc=true`, 같은 JSON의 V2 계획 → `BAD_VALUE`. V2 문자열 조건: `spikes/spike-v2-read/src/plan.rs:86`. 이 variant의 사실은 V1 의미 검사를 별도 대조한 후 추가 기록한다.

권고: 응답 Id와 요청 Id의 표현을 정하고 명시적 codec 또는 서로 다른 타입을 사용한다. 런타임 없는 mock transport 타입 테스트만으로 양방향 계약 일치를 주장하지 않는다.

## F05 · P1 · Q1/Q6: 집계 실행 중에는 확장 기한을 넘겨도 성공 응답을 반환한다

잘못된 점: 정상 `ctx.data.aggregate`가 DB 잠금을 기다리면 선언 기한이 전체 호출에 적용되지 않는다. 기한 뒤 성공 응답과 집계 결과가 worker·호출자에게 전달된다. 쓰기 확장 전에 호출 전체와 ctx 작업의 기한·취소·토큰 정리를 통합해야 한다.

실행: 임시 schema `r4b_4_variants.apply`에 다른 연결이 `ACCESS EXCLUSIVE` 잠금을 잡고 450ms 후 해제했다. 확장 계약 deadline을 100ms로 둔 정상 stats 입력 `{clubId:"10"}`, actor 1의 결과:

```text
fanout Node OK {"approvedApplicants":10}
deadline Node declared100 elapsed453 OK {"approvedApplicants":1}
deadline Python declared100 elapsed454 OK {"approvedApplicants":1}
```

기존 기한 실험은 확장 자체가 sleep하는 경로였다. 이번에는 정상 집계 10개 병렬 호출도 Node·Python 모두 정확히 10을 돌려줬고, 기한 사례 뒤 actor 3·clubId 11은 정확히 2였다. 호출 id 혼동 없이 기한만 깨진다.

근거: `spikes/spike-v4-worker/src/lib.rs:176`은 stdout 대기에만 timeout을 건다. `:189`의 `serve_call(...).await`와 `:241`의 V2 execute는 확장 남은 시간을 받지 않는다. V2 단독 집계 기한은 `spikes/spike-v2-read/src/plan.rs:387`의 고정 2000ms. `lib.rs:198`·`:203`은 done 처리 직전 expires를 다시 확인하지 않는다. V4 결과 `V4-official-extension-results.md:55`의 “2s 초과는 기한 오류”, `:75`·`:78`의 기한·수명 규칙은 sleep 경로보다 넓게 해석하면 실행 근거보다 강하다.

권고: 전체 invoke와 send/serve/execute를 하나의 deadline 아래 두고 DB statement_timeout을 남은 시간 이하로 제한한다. 기한 후 집계 값·done을 성공으로 전달하지 않도록 검사하고, 모든 종료 경로에서 grant를 회수한다.

## F06 · P2 · Q2/Q6: outbox의 무조건 최소 한 번 전달·최대 3회 시도 주장은 구현보다 강하다

잘못된 점: 3회 실패 후 dead로 격리하므로 공급자가 회복돼도 전달 성공·효과 발생은 보장하지 않는다. 또 attempts는 소비 트랜잭션이 커밋한 횟수여서 중단된 시도는 한도에 포함되지 않는다. 재처리 정책과 측정 단위를 다음 단계 전에 명시할 것을 권한다.

실행(같은 공급자 대역, 매 사례 초기화):

```text
four_crashes attempts=0 dead=false delivered=false calls=4
one_commit attempts=1 dead=false delivered=false calls=5
transient3_then_healthy attempts=3 dead=true delivered=false provider=3 effects=0
```

첫 3회만 실패하는 공급자는 4번째 호출부터 성공할 수 있지만 dead 행은 선택되지 않아 호출 3·효과 0으로 남는다. 영구 실패 공급자에 `crash_after_send=true`를 4회 실행하면 공급자는 4번 호출되고 DB attempts는 0이다. 이후 정상 소비에서도 attempts는 1이다. 이것은 코드가 정한 유한 재시도·격리 자체의 결함으로 단정하지 않는다. 문서의 보장 표현이 이 조건을 생략한 것이 문제다.

근거: `spikes/spike-v4-worker/src/outbox.rs:57`의 NOT dead, `:72`·`:78`의 트랜잭션 내부 attempts 증가, `:83`·`:84`의 rollback. `plan-docs/alignment/V4-official-extension-results.md:100`의 “최소 한 번 전달…한 번 효과”, `:106`의 “최대 3회 시도 뒤 격리”. 보장은 “커밋된 outbox만 전달 시도, 커밋된 실패 3회까지 자동 재시도, 이후 격리; 성공 시 동일 키 중복 효과 억제”로 한정해야 한다. 중단 반복의 공급자 호출 횟수는 별도로 관측한다.

추가 대조: 미커밋 쓰기는 소비자 호출 0. 소비자 중단을 (1) 선택 뒤 send 전, (2) send 뒤 기록 전, (3) 기록 뒤 commit 전으로 나누면 모두 DB delivered=false·attempts=0이며, 각각 공급자 효과는 0·1·1이다. 재개 후 delivered=true·효과 1. (2)·(3)의 중복 호출 억제는 대역이 유지한 동일 멱등 키 덕분이다. 기존 `crash_after_send` 테스트(`outbox.rs:69`·`:83`)는 실제로 모든 행의 전달 기록까지 실행하고 rollback하므로 (3)을 모사한다. 두 지점의 DB 최종 결과는 같지만 실제 프로세스 강제 종료·공급자 timeout·commit 결과 미확정은 확인 못함: 대역과 rollback 주입으로만 검증했다.

### F01/F03/F04 실행 대조 추가

임시 `r4b_5_variants` DB에서 V2 계획·실행을 직접 호출했다. F01의 동적 선택 두 분기는 각각 `[{"title":"A 모집"}]`, `[{"views":5}]`를 반환했다. 한 분기에서는 views가, 다른 분기에서는 title이 실제로 없다. F03의 select에서 periodEnd를 뺀 facts로도 정상 필터가 `[{"id":100}]`를 반환했다. F04는 V1 원문에 `filter id.eq`를 추가해 의미 검사를 통과한 facts로 재검증했다. number 100 → `BAD_VALUE`; string "100" → `[{"id":100}]`. 응답 Id는 숫자, 입력 Id는 문자열이라는 양방향 차이가 확인됐다.

## F07 · P3 · Q5: deps_of가 SQL 구문을 인식하지 않아 literal·다른 schema를 오탐하고 인용 identifier를 놓친다

현재 생성 SQL에서 결과를 바꾸는 쓰기가 deps에서 빠지는 사례는 없음. 아래는 `deps_of` 함수 자체의 한계이며 현 planner에서 도달하는 누락으로 승격하지 않는다.

근거: `spikes/spike-v2-read/src/plan.rs:37`·`:44`·`:45`는 테이블 문자열을 찾고 오른쪽 문자 경계만 본다. 임시 schema 이름으로 실행한 반환값:

```text
SELECT 1 FROM r4b_5_variants.club_member           → ["ClubMember"]
SELECT 'r4b_5_variants.member'                    → ["Member"] (literal 오탐)
SELECT 1 FROM other_r4b_5_variants.member          → ["Member"] (다른 schema 오탐)
SELECT 1 FROM "r4b_5_variants"."member"            → [] (인용 identifier 누락)
```

현 `sqlgen.rs:38`·`:39`는 고정 schema의 비인용 테이블명을 만들고 값은 바인딩하므로 마지막 세 SQL은 현 planner 생성 경로가 아니다. Club/ClubMember처럼 테이블명이 접두어 관계인 경우 오른쪽 경계 검사가 정확히 구분했다. SDK 캐시·다른 SQL 생성 경로를 붙이기 전에 지원 SQL 형태를 제한하거나, SQL 생성 시 resource 의존 집합을 직접 모으는 방식을 권한다.

Q5 확대 실행: 기존 V5 select/관계/필드 정책 8개 resource 변경 대조에 더해, filter 행 제외·sort/limit 선두 변경·행별 집계 guard false→true·단독 집계 원본 status 변경·원본의 recruitment.club 경로 변경·단독 집계 guard 권한 회수까지 6건을 실제 실행했다. 모두 결과가 바뀌고 해당 resource가 deps에 있었다. filter/sort의 deps는 `[Club, Member, Recruitment]`; 집계 guard는 `[Club, ClubMember, Member, Recruitment, RecruitmentBookmark]`; 단독 집계는 `[Apply, ClubMember, Recruitment]`. 단독 guard 권한 회수는 `[1]`→`ACCESS_DENIED`로 확인했다. SQL 문자열 방식의 건전성을 모든 미래 query 형태에 일반화하지 않는다.

## F08 · P2 · Q1: nullable 출력의 선언된 키가 없어도 호출자에게 그대로 전달된다

잘못된 점: nullable과 키 생략을 같은 것으로 처리해 출력 키 집합이 선언과 정확히 일치하지 않는다. 계약은 `approvedApplicants: Int; note: Text?`인데 `{approvedApplicants:1}`만 반환한 정상 확장을 Node·Python 모두 `OK {"approvedApplicants":1}`로 전달했다. 명시적 `{note:null}`과 키 부재는 JS/TS 호출자에게 다른 구조다.

근거: `spikes/spike-v4-worker/src/lib.rs:138`은 없는 키를 `Value::Null`로 대체한다. `:117`은 nullable이면 null을 허용하고, 실제 out 값은 `:202`·`:203`에서 그대로 반환한다. V1 원문 output 선언을 변경해 load_str를 통과시킨 실험이다. 문서 `V4-official-extension-results.md:77`의 “출력은 선언된 키·타입과 정확히 일치” 및 검사 주석 `lib.rs:126`을 기준으로 계약과 실행 불일치다. 선언 밖 추가 출력 키는 기존 사례에서 정상 거부됐다.

권고: nullable과 optional을 분리한다. 정확한 키 계약을 유지한다면 키 존재를 검사하거나 누락 nullable 키를 null로 정규화한다. 생략을 공식 지원하려면 출력 타입·문서에 optional을 표현한다.

## F09 · P2 · Q1: V1이 허용한 enum 출력 계약을 V4가 무조건 거부한다

잘못된 점: 정상 enum 값도 확장 출력 검사를 통과하지 못한다. 허용 계약 타입과 실행 검증 타입의 범위를 맞추거나 지원 범위 밖 계약을 정의 단계에서 거부해야 한다.

실행: fixture output을 `approvedApplicants: ClubRole`로 바꾸고 정상 함수가 `{approvedApplicants:"ADMIN"}`을 반환하도록 했다. V1 의미 검사는 정상이며 Node·Python 둘 다 `enum_output ... OUTPUT_INVALID`. enum 값 ADMIN은 fixture의 ClubRole에 선언돼 있다.

근거: `spikes/spike-v1-fixture/src/sema.rs:201`·`:1160`의 계약 타입 허용과 `spikes/spike-v4-worker/src/lib.rs:114`·`:122`의 검사 목록. V4는 Int/Text/Bool/Id만 비-null 값으로 받으며 enum membership을 검사할 facts도 받지 않는다. Time·Url도 정적으로 같은 지원 공백이 보이지만 실제 실행은 확인 못함: 이번 추가 실험은 enum 출력만 실행했다. nullable enum은 별개로 V5 생성기에서 정상 union 타입으로 확인했다.

권고: 공통 값 검증기 또는 실행 capability 목록을 사용한다. V1이 성공한 선언이 정상 값에서 무조건 실패하는 상태를 공식 확장 계약으로 확대하지 않는다.

### F05 계약 출처 보강

deadline 100ms는 facts 수동 변조 대신 원문 `deadline 100ms`를 V1 parser·sema에 통과시킨 계약으로 다시 실행했다. `deadline Node declared100 elapsed452 OK {"approvedApplicants":1}`, Python은 454ms·동일 성공 값이었다. 따라서 지원되지 않는 facts를 넣어서 생긴 현상이 아니다.

## F10 · P3 · Q1/Q6: extension access와 caller 집계 공개가 현재 같은 실행 경로를 탄다

기록할 점: extension에 선언된 `access Apply.approvedCount`와 입력 binding·guard가 그대로 있어도, `expose aggregate approvedCount`만 제거하면 정상 stats가 실패한다. V1 원문은 의미 검사를 통과하지만 Node·Python 모두 `EXTENSION_ERROR(NOT_EXPOSED)`다.

근거: `spikes/spike-v1-fixture/src/sema.rs:1162`는 집계 선언의 존재·입력 타입을 검증한다. `spikes/spike-v4-worker/src/lib.rs:240`은 caller용 `plan_read`를 그대로 재사용하고 `spikes/spike-v2-read/src/plan.rs:340`의 exposeAggregates 검사를 추가로 받는다. 공개 집계만 extension access에 쓸 수 있다는 정의 단계 진단은 없다.

이 의미가 창시자가 결정한 계약인지 확인 못함: C B.7(`C-syntax-proposal.md:295`·`:314`)은 승인 named aggregate의 확장 접근을 말하지만 caller 직접 공개까지 필수라고 확정하지 않는다. 따라서 허용 범위를 넓히는 수정부터 하지 않고, “확장 전용 access”와 “caller 직접 공개”를 구별할지 기술 계약으로 먼저 정한다. 현 동작은 집계를 더 넓게 읽게 하는 우회가 아니라 정상 요청의 추가 거부다.

## 질문별 판정과 확인 범위

| 질문 | 판정 | 실행·정적 확인 범위 |
|---|---|---|
| Q1 확장 계약 | 선언되지 않은 집계·다른 입력 값·추가 출력 키가 ctx 경로를 통과한 사례는 없음. 기한·누락 nullable 키·정상 enum 출력에서 불일치(F05·08·09). 비공개 집계는 F10 | Node/Python 정상·회원·익명·잘못된 입력·binding·access·actor 위장·출력 검사·토큰 재사용·늦은 done·worker 종료/재기동. 한 호출 안 집계 10개 동시 요청. 같은 worker의 invoke는 `lib.rs:146`의 `&mut Worker`라 직접 호출은 직렬이다. 이전 만료 작업의 늦은 메시지가 다음 invoke와 겹치는 경로는 기존 테스트로 확인 |
| Q2 outbox | 커밋된 행만 전달 시도, 실패 재시도, 동일 키 중복 효과 억제, 동시 소비자, dead 격리는 대역 범위에서 맞음. 보장 표현은 F06 조건 필요 | 기존 6사례 및 미커밋 행·send 전/후·기록 후 중단·반복 중단·일시 실패 3회 후 회복 |
| Q3 위임 | 없음(아래 확인 범위). 현재 V3-3 선언에서는 허용된 최종 상태·전체 rollback·오류 분류가 의도와 일치 | W2/W0 정상·자기/권한 없는/타 동아리 위임·잘못된 순서·관리자 2/0명·기존 관리자 재위임·동시 위임, 새 관리자 재위임, 서로 다른 동아리 동시 위임, 교대 뒤 추가 작업, update 대상 0/2행 |
| Q4 SDK | F01–04. nullable enum·집계 guard·관계 없는 resource의 생성 오류는 없음(시험한 선언 범위) | 기존 tsc 양성/8개 음성·type-neg 및 변형 요청. V2 DB와 동적 select·filter 전용 필드·Id 입력/응답 대조. nullable enum 출력 union, guard number\|null, MemberAlarm 루트·Bool filter 정상 생성 |
| Q5 deps | 결과를 바꾸는 쓰기의 누락 없음(현재 생성 경로와 실험 변형). 문자열 분석기 한계는 F07 | 기존 8개 resource 대조 + filter·sort/limit·행별 guard·단독 집계 원본/경로/guard 6개 변경. lexical 오탐/누락 3개를 planner 생성 SQL과 구별 |
| Q6 주장·다음 단계 | 아래 조치·결정 표 참조 | 결과 문서의 조건부 관찰을 제품 보장이나 창시자 승인으로 승격하지 않음 |

### Q3 실행 근거

`spikes/spike-v1-fixture/src/parser.rs:509`의 deferred 표식이 `sema.rs:865`·`:867`에 보존되고, `spikes/spike-v2-read/src/sqlgen.rs:363`·`:367`에서 아래 DDL로 내려간다.

```sql
ALTER TABLE r4b_3_variants.club_member
ADD CONSTRAINT club_member_one_admin
EXCLUDE USING btree (club_id WITH =)
WHERE ((role = 'ADMIN')) DEFERRABLE INITIALLY DEFERRED
```

이 제약은 최대 1명이다. 최소 1명은 `check keepsAdmin when hasAdmin(club)` 사후조건이 맡는다. V3 `lib.rs:101`→`:298`의 write_err가 커밋 시 23P01을 `INVARIANT_VIOLATED`로, `:41`이 40P01/40001을 `CONFLICT`로 분류한다. 이번 기존 v3_3 실행에서 임명만 → `INVARIANT_VIOLATED`, 내려놓기만 → `CHECK_FAILED`, 같은 관리자 동시 위임은 W2·W0 모두 T1 OK/T2 CONFLICT였고 실패 효과는 남지 않았다. 재시도가 새 요청이라면 권한 재평가 때문에 원래 관리자에게 `MISSING_TARGET`가 될 수 있다. 자동 재시도가 업무 성공을 보장한다는 뜻은 아니다.

정상 변형 출력:

```text
new_admin_again OK roles=1:MEMBER,2:ADMIN,3:MEMBER,4:ADMIN,5:MEMBER
other_club W2 T1=OK T2=OK roles=1:MEMBER,2:MANAGER,3:ADMIN,4:MEMBER,5:ADMIN
other_club W0 T1=OK T2=OK roles=1:MEMBER,2:MANAGER,3:ADMIN,4:MEMBER,5:ADMIN
post_swap_touch (Ok BundleResult) roles=1:MEMBER,2:MANAGER,3:ADMIN,4:ADMIN,5:MEMBER
w2_post_swap_touch OK roles=1:MEMBER,2:MANAGER,3:ADMIN,4:ADMIN,5:MEMBER
```

`post_swap_touch`는 V1을 통과한 자기 행 전이 `from true; to role = MEMBER; allow member = actor`를 공개하고, 같은 W0 트랜잭션에서 makeAdmin(3)→resignAdmin(1)→touch(1)을 실행했다. 이것은 W0 전이 뒤 추가 작업의 확인이다. 별도로 같은 bundle에서 entrust(3)→touch(1)을 실행했다. W2 entrust의 update 효과가 자신의 행을 이미 MEMBER로 바꾼 뒤 추가 작업을 해도 OK이며 같은 최종 역할이 유지됐다. 역할 중복행을 추가해 update match가 2행이거나, match에 `role = MANAGER`를 추가해 0행이 되면 각각 `EFFECT_TARGET_MISSING`이고 승격까지 전체 rollback됐다(`spikes/spike-v3-write/src/lib.rs:315`·`:336`). `run_checks`(`:411`·`:420`)는 이 트랜잭션이 쓴 행의 사후조건이라는 기존 문서 한정과 일치한다.

확인 못함: 서로 다른 동아리 사이의 모든 복합 잠금 순서, savepoint·삭제를 포함한 전역 불변식, 모든 deferred invariant 형태. 이번 결론은 실제 V3-3 선언과 실행한 정상 변형에 한정한다.

## Q6: 문서 주장의 강도와 다음 단계

| 구분 | 실행 근거와 맞출 것 | 다음 단계 전 조치 |
|---|---|---|
| V4 기한 강제 | `V4-official-extension-results.md:55`는 sleep 사례 관찰이다. 정상 ctx DB 대기에는 전체 기한이 깨짐(F05) | 쓰기 확장 전 invoke 전체 기한, tx handle 회수, 외부 취소, 남은 DB 시간, 모든 종료 경로 토큰 정리 기준을 정하고 실행 |
| V4 출력 계약 | `:77`의 정확한 키·타입 계약은 누락 nullable(F08)·enum(F09)을 덮지 못함 | optional/null 구분과 지원 타입 집합을 정의·실행·SDK에 동일하게 적용 |
| outbox 보장 | `:100`의 무조건 최소 한 번 전달·한 번 효과는 dead·공급자 회복·키 보존 조건이 빠짐(F06). 최대 3은 커밋된 시도 수 | dead 관측·재처리·공급자 timeout/결과 미확정, 멱등 키의 보존 기간·namespace·payload 일치, consumer 중단/commit 결과 미확정을 검증. 현 대역은 실공급자 보장의 증거가 아님 |
| SDK 결과 타입 | `V5-sdk-results.md:43`의 정확한 일치는 고정 리터럴 select·기존 계약에 한정. 동적 select와 filter 입력 계약은 깨짐 | 캐시 런타임 전 F01·F03 해결, 요청/응답 Id codec과 정확한 select 검사, V2 실행을 붙인 타입 계약 대조 |
| deps 건전성 | `V5-sdk-results.md:62`·`:64`의 관찰은 생성 SQL 형태·선택된 actor에 한정. 현재 실행에서는 확대 사례도 맞음 | SQL 형태 변화 시 dependency 추적 회귀 검사. write 변경 태그는 직접 대상뿐 아니라 update/create 효과·권한 변경도 포함하고 outbox 후속 쓰기를 별도 반영 |
| 캐시 운영 의미 | `V5-sdk-results.md:87`·`:88`은 actor 분리와 결과 유실 때 전체 무효화를 미구현 후보로 정확히 기록함 | actor/tenant·계약/정책 버전·요청 값의 키, 로그아웃·재연결·COMMIT_UNKNOWN 복구, 늦은 응답이 최신 캐시를 덮는 순서, now 의존 조회의 만료를 정하고 실행. deps는 시간 경과를 표현하지 않음 |
| 위임·W1 | `V3-standard-write-results.md:226`의 교착 결과는 이번 재현과 일치. `:235`의 W1 표현 불가는 현재 compose 구조에 한정 | 단계별 다른 대상·actor 행 참조를 추가할 때 중간 권한·노출·잠금 순서와 update 효과 범위·사후조건·deferred 커밋을 다시 검증. “최종 관리자 1명”만으로 중간 권한 안전성을 일반화하지 않음 |
| 문서 상태 | `E-technical-risks.md:61`의 “캐시·무효화(RK-09)는 미착수”는 deps 실험 착수를 반영하지 않음. 실제 캐시 런타임 미착수는 맞음 | deps 실험 착수와 캐시 런타임 미착수를 구분해 이후 문서 갱신 때 상태를 맞춘다. 범위 제한 때문에 해당 문서는 수정하지 않음 |

마지막 상태 문구는 deps 실험과 캐시 런타임을 구별하자는 설명 권고다. 별도 정확성 결함으로 집계하지 않는다.

### 기술 계약과 창시자 결정의 구분

기한·출력 검사·Id codec·deps 추적·교착 rollback 분류는 기술적으로 검증·정할 항목이다. extension 전용 access와 caller 공개의 관계(F10), check의 사후조건 범위, 중단된 시도 계수도 우선 기술 계약으로 제안한다. 창시자에게 이미 정한 Rust 서버·공식 JS/TS/Python 확장·프론트 라이브러리 방향을 다시 묻지 않는다.

창시자 판단이 필요한 것은 기술 자료 이후의 제품 경험 선택이다. (1) 첫 출시 TS/Python 확장의 타입·기능 동등성 범위, (2) outbox dead 이후 개발자가 수동 복구하는 경험과 프레임워크 자동 재처리의 초기 지원 범위, (3) W1 단계별 대상 조합을 공식 호출 경험으로 노출할지와 그 복잡성 비용, (4) 필드 존재·역할별 계약 공개 수준 및 계약 호환 기간. 이 검토로 어느 후보도 승인으로 간주하지 않는다. 기준: 통합 지침 `founder-integrated-directive-2026-10-03.md:15`의 수준 구분, `:584`의 초기 동등성 미결정, `:838`의 기술 세부/제품 가치 판단 구분 및 E RK-04·05·08·09(`E-technical-risks.md:15`·`:16`·`:19`·`:20`), C B.4·B.6·B.7(`C-syntax-proposal.md:228`·`:285`·`:312`).

## 실행 기록·한계

임시 루트: `/private/tmp/aip-r4b-08kftt2o`. 다섯 spike의 src/tests/fixture/workers/extensions/sdk와 Cargo manifest/lock만 복사했다. Python bytecode·V5 생성 계약·추가 probe 파일은 이 복사본에만 생성됐다. tsc는 원본 `spikes/spike-0-ts/node_modules/.bin/tsc`를 사용했다. target은 사용자 예외 범위인 원본 spike별 target에 뒀다. git·.env·비밀 저장소는 읽거나 실행하지 않았다. 앞의 V2 schema 절차 이탈은 이 제한 준수 주장과 분리해 명시했다.

기본 실행 명령은 각 임시 spike에서 다음과 같았다.

```sh
PATH="$HOME/.cargo/bin:$PATH" \
CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/<spike>/target" \
cargo test --offline -q -- --nocapture
```

| 실행 | 기존 integration test 결과 | 한정 |
|---|---|---|
| V1 | 16 passed (7+5+4) | parser/sema/facts 전체 기존 테스트 |
| V2 | 5 passed (4+1) | 앞의 준비 오류 수정 후 재실행. default schema도 임시 이름으로 치환 |
| V3 | 5 passed (각 integration 1개) | v3_1, v3_2, v3_3, r2b, r3 |
| V4 | 2 passed | sandbox 및 DB 직접접속 probe 제외. 계약·수명·outbox는 유지 |
| V5 | 2 passed | 타입·deps. 계약 생성은 복사본에서만 |

추가 실행은 각 해당 임시 spike에서 같은 PATH/CARGO_TARGET_DIR 환경으로 `cargo test --offline -q --test <probe> -- --nocapture`를 사용했다. probe 이름은 `r4b_sdk`, `r4b_deps`, `r4b_delegation`, `r4b_worker`, `r4b_outbox`이며 각각 최종 실행 1 passed다. probe의 “passed”만으로 발견을 판정하지 않고, 본문에 적은 tsc 오류·성공과 실제 DB/worker 반환값을 판정 근거로 삼았다. 준비 중 발생한 guard 경로·Rust lifetime/문자열 표기·제약에 걸리는 seed 변형·대역 Mutex 중복 잠금 오류는 임시 실험 코드에서 바로잡았고 프레임워크 발견으로 세지 않았다.

실행 환경 실측: node v26.9.0, Python 3.14.7, tsc 7.0.2, cargo 1.98.1. 프로세스 격리는 요청에 따라 미실행이다. 실제 공급자·HTTP 전송·SDK 캐시 런타임·쓰기 확장 tx handle·W1 확장은 확인 못함: 현 spike에 구현되지 않았거나 이번 범위에서 제외됐다. 문서의 과거 고장 주입 이력은 현재 정상 코드·추가 반례와 구별하며 모두 다시 수행한 것으로 표시하지 않는다.

최종 판정: P1 3건(F01·F03·F05), P2 5건(F02·F04·F06·F08·F09), P3 2건(F07·F10). P1은 다음 단계 전 수정 대상, P2는 계약·검증 보강 권장, P3는 현재 지원 범위·의미 결정 기록이다. Q3 위임과 Q5 현 생성 SQL deps에는 실행한 범위에서 추가 P1/P2 발견이 없다.

최종 정리 검증: 구조·명시적 파일:줄 인용·최종 로그 10개를 대조했다. 저장소에서 검토 파일 생성 시각 이후 바뀐 다른 파일은 target 제외 0개였다(시각 기준 검사이며 내용 snapshot 비교는 아님). `psql -d postgres -At -c "SELECT count(*) FROM pg_namespace WHERE nspname LIKE 'r4b_%'"`의 출력은 `0`이다. 임시 schema가 남지 않았으며 앞의 기본 schema 재생성 사고는 별도 이탈 기록으로 유지한다.
