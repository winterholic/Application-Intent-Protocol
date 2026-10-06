# V1 의미 fixture 비판 검토 (r1)

기준 우선순위: 창시자 통합 지침 → E §4 V1 → C B.1~B.3·B.8·E. 검토 범위는 `spikes/spike-v1-fixture/`의 src·fixture·tests/v1.rs이며 본 crates는 읽지 않았다. git·비밀 저장소는 읽거나 실행하지 않았다. 변형은 임시 복사본에서만 수행한다.

## 직접 실행한 기준 검증

명령: `cd spikes/spike-v1-fixture && PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q`
```text
running 7 tests
.......
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
```

명령: `cd spikes/spike-v1-fixture && ./target/debug/spike-v1-fixture facts fixture/recruitment.aip`
```text
CLI exit: 0
executionDigest: 8b5fa9fb5b2688df47a8eb3322e345a5dd7496a47408baff92f8d76e788f963d
metadataDigest: c765763dff76324c85ef5b62e388fa70e2906136eee93f9522b5f8038f124f9f
```

위 두 digest 줄은 CLI JSON에서 추출한 값이다. 7개 테스트의 성공은 기존 대조의 성공만 입증하며 아래 확장 실험을 대신하지 않는다.

## 발견 기록 (확인 즉시 추가)

### F01 · P1 · Q3/Q6 — E가 실제 선언이 아닌 주석·문자열을 정의로 채택한다

영향: 모듈 비실행은 맞지만, “지정된 정적 리터럴”의 소속을 식별하지 못한다. 정의가 전혀 없는 파일도 기준 모델로 승인된다. V2 전 호스트 토큰/구문 경계와 허용된 import 바인딩을 검증해야 한다.

근거: `src/extract.rs:30-56`은 줄 단위 단어 검색이며 TS `//`·Python `#`만 처리하고 문자열·TS 블록 주석을 구분하지 않는다. `src/extract.rs:60-90`, `:93-134`는 그 위치에서 즉시 블록을 만든다. `src/lib.rs:31-44`는 외부 모듈 실행 없이 이 텍스트만 파싱한다. 기준 C `B.2:132`의 “지정된 리터럴” 추출 요구를 만족하지 못한다. 경로의 `src/`는 모두 `spikes/spike-v1-fixture/src/`이다.

실험: TS는 기준 A 전체를 `aip` tagged template로 감싼 뒤 그 전체를 `/* ... */` 블록 주석 안에 넣었다. Python은 기준 A 전체를 `aip("""...""")`로 감싼 뒤 그 전체를 `TEXT`의 삼중 작은따옴표 문자열 안에 넣었다. 둘 다 실행 가능한 AIP 선언은 없고 주석/문자열이다. 명령은 `spikes/spike-v1-fixture/target/debug/spike-v1-fixture facts <임시복사본>/fixture/{comment-only.e.ts,string-only.e.py}`.
```text
comment-only.e.ts exit=0 exec=8b5fa9fb5b2688df meta=c765763dff76324c
string-only.e.py exit=0 exec=8b5fa9fb5b2688df meta=c765763dff76324c
```
추가로 실제 E-TS 앞 `const note = "aip"`는 `NON_LITERAL`로 오거부된다. 정상 E를 넣고 이름만 주석/문자열에 추가해도 안전한 거부라는 결론을 낼 수 없다.

### F02 · P1 · Q3/Q6 — 다른 import와 숨은 두 번째 블록을 승인한다

영향: 소스 파일의 정의 식별과 중복 거부가 신뢰할 수 없다. V2 입력 경계에 사용하기 전 import 전체 줄 무시를 없애고 바인딩을 검사해야 한다.

근거: `src/extract.rs:35-36`은 `import `/`from `으로 시작하는 줄 전체를 건너뛴다. `:138-141`의 정확히 한 블록 검사는 검색에서 빠진 블록을 볼 수 없다. C `B.2:119-132`는 공식 바인딩의 지정된 리터럴 후보다.

실험: 기준 E-TS의 `@aip/define`을 `evil-package`로 변경, E-Py import를 `from evil_package import other as aip`로 변경, E-TS 끝에 `import "x"; const second = aip` 뒤 `enum E { A }` template를 추가했다. 명령은 각각 `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/{wrong-import.e.ts,wrong-import.e.py,hidden-second.e.ts}`.
```text
wrong-import.e.ts exit=0 exec=8b5fa9fb5b2688df meta=c765763dff76324c
wrong-import.e.py exit=0 exec=8b5fa9fb5b2688df meta=c765763dff76324c
hidden-second.e.ts exit=0 exec=8b5fa9fb5b2688df meta=c765763dff76324c
```
별칭 공식 import는 TS/Py 모두 `NO_BLOCK`, re-export 한 줄 추가는 `NON_LITERAL`이었다. 별칭/re-export를 지원하지 않는 것은 명시 부분집합이면 허용할 수 있지만, 임의 패키지를 허용하면서 공식 별칭만 거부하는 현재 경계는 일관되지 않다. 외부 모듈 실행은 발생하지 않았다.

### F03 · P2 · Q3 — TS 블록 뒤 줄바꿈 가공을 놓친다

영향: 동적 결합 거부가 같은 줄 여부에 따라 달라진다. 외부 코드를 실행하지는 않지만, 선언 소스와 추출 facts의 동등성 주장을 약화한다.

근거: `src/extract.rs:75-88`은 첫 백틱을 종료로 삼으며 닫는 template 뒤 공백/탭만 제거한다. `:124-131`의 Python도 닫는 호출 뒤 가공 검사에는 같은 제한이 있다. C `B.2:132`는 동적 문자열 결합을 금지한다.

실험 명령: `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/newline-transform.e.ts`. 기준 E-TS의 닫는 백틱 다음 줄에 `+ extra`를 추가했다.
```text
newline-transform.e.ts exit=0 exec=8b5fa9fb5b2688df meta=c765763dff76324c
```
기존 `tests/v1.rs:183`의 같은 줄 `+ extra`는 거부하지만 줄바꿈 변형은 승인한다. docs에 escape 없는 백틱을 넣으면 `LEX_BAD_STRING`이었다. 이는 실제 TS template에서도 종료를 뜻하므로 그 사례만으로 버그라 하지 않는다. 백틱 escape는 `:83-84`에서 명시 거부하는 부분집합이다. 백틱을 정상 표현하는 호스트 대안의 지원은 확인 못함: 현재 parser가 escape 전체를 지원하지 않는다.

### F04 · P1 · Q1/Q2/Q6 — 확장 출력·입력/인자 타입의 범위 제약이 facts에서 사라진다

영향: 서로 다른 확장 출력 계약을 같은 execution digest로 판정한다. EQ-10의 타입·출력 검사에 필요한 제약이 유실된다. V2에 재사용할 typed facts에서는 범위를 보존하고, 지원하지 않는 위치라면 문법을 거부해야 한다.

근거: `src/parser.rs:243-261`은 모든 TypeRef에 range를 허용한다. `src/sema.rs:212-221`의 params는 `tt.show()`만 출력하고 `:819-820`이 extension 입출력에 사용한다. `src/sema.rs:20-23`의 TT와 `:29-42`의 show는 range를 보존하지 않는다. 필드만 `:518-519`가 range를 별도로 저장한다. 기준 C `:29`(EQ-10), 창시자 지침 `:310`, `:328`은 타입·제약·출력 계약을 중시한다.

실험: 기준 A의 `output { approvedApplicants: Int }`를 `Int(0..10)`과 `Int(0..999)`로 각각 치환했다. 명령: `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/{range-1.aip,range-2.aip}`.
```text
range-1.aip exit=0 exec=8b5fa9fb5b2688df
range-2.aip exit=0 exec=8b5fa9fb5b2688df
```
두 execution facts는 전체가 일치하며 범위 없는 기준 digest와도 같다. 필드 범위 검증만으로 이 손실을 탐지할 수 없다.

### F05 · P2 · Q1/Q2 — digest는 일반 의미 동등성보다 정규화한 구조 동등성이다

영향: 의미가 같은 식도 다른 digest를 내므로 digest 차이를 의미 차이의 증명으로 쓰면 안 된다. V1의 다섯 형식 대조 자체는 유효하지만, “동등성”의 범위를 명시해야 한다.

근거: `src/sema.rs:353-363`은 `in` 원소 순서를 배열에 보존한다. `:443-444`는 enum 선언 순서를 그대로 저장한다. `:45-50`은 같은 대상의 Ref/Id 비교·전달을 호환으로 선언하지만 `:232-282`는 참조와 `.id` 경로를 서로 다른 facts로 보존한다. C `:22`(EQ-03)는 select/filter/sort를 **허용 목록**으로 정의하므로 `:719-763`의 select map·filter/sort 집합 정렬에는 동의한다. 이 서버 목록은 호출자의 실제 정렬 우선순위가 아니다. V2의 요청 sort 배열에는 이 정규화를 복사하지 말아야 한다.

실험 명령: `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/{in-order.aip,enum-order.aip,ref-id.aip}`. 기준 managerOf의 `role in (ADMIN, MANAGER)`를 역순으로, ClubRole enum을 역순으로, `club = c and member = m`을 `club.id = c.id and member.id = m.id`로 각각 변경했다.
```text
in-order.aip exit=0 exec=b26494812d6be9ff
enum-order.aip exit=0 exec=ebe025a48cd5748a
ref-id.aip exit=0 exec=555b589d20488d92
```
기준 exec `8b5fa9fb5b2688df`와 모두 다르다. `in` 두 원소의 순서 교환은 같은 회원 조건의 확실한 반례다. enum은 이름 값의 집합으로만 사용할 경우 같은 의미이나, 선언 순서의 런타임 의미는 확인 못함: 기준 문서가 ordinal/정렬 의미를 정하지 않았다. Ref와 Id는 spike가 같은 행으로 취급한다고 선언했을 때만 동등한 반례다. 인자 순서는 `:390-405`에서 zip으로 검사하고 순서를 그대로 보존하므로 집합으로 잘못 정규화하는 결함은 없음(현재 predicate call 범위).

### F06 · P1 · Q4/Q6 — H totalOfVisible 매핑은 params 값을 조용히 버린다

영향: 모르는 키는 일반적으로 거부하지만, 알려진 키를 잘못된 조합에 넣으면 타입/기호 검사까지 건너뛴다. V1의 “잘못된 기호 거부”를 깨뜨리므로 V2 전 수정한다.

근거: `src/hmap.rs:84-96`은 `params`를 허용한 후 totalOfVisible 분기에서 무조건 빈 params로 바꾼다. A에서는 `src/sema.rs:461-465`가 totalOfVisible 매개변수를 거부하지만 H에서는 검사할 정보 자체가 사라진다. C `:351`, `:356`(SY-1/SY-6), E `:53`은 잘못된 기호·설정의 조용한 무시를 허용하지 않는다.

실험 명령: `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/{ignored-params.h.ts,ignored-type.h.ts}`. 기준 `fixedTotalOfVisibleRecruitment`에 `params: 123`, `params: { bad: "MissingType" }`를 각각 추가했다.
```text
ignored-params.h.ts exit=0 exec=8b5fa9fb5b2688df
ignored-type.h.ts exit=0 exec=8b5fa9fb5b2688df
```
둘 다 기준 execution facts와 전체 일치한다. 해당 분기에서 params 존재를 거부하거나 A와 동일하게 매핑해야 한다.

### F07 · P2 · Q4 — H 호출 전체의 리터럴성은 검사하지 않는다; 값 내부 검사는 동의

영향: 리터럴 인자는 안전하게 읽지만, 호출 결과를 실행 가능한 식으로 가공하는 소스도 같은 정의로 승인한다. “호스트 모듈 비실행”과 “선언 식 전체가 고정 리터럴”을 구분하고 E와 같은 소스 경계 검사를 적용해야 한다.

근거: `src/hlit.rs:179-202`는 `define({...})`의 닫는 괄호에서 반환하고 뒤의 가공을 검사하지 않는다. `:180`은 E와 같은 word_positions를 사용하므로 F01/F02의 문자열·import 경계 문제가 공유된다. 반면 `:88-174`는 변수·함수·spread·계산 키·중복 키를 거부하며 `src/hmap.rs:35-41`의 keys를 resource/read/budget/aggregate/extension 등에서 호출한다. C `B.3:138`의 고정 생성자·리터럴 부분집합 요구를 기준으로 판정했다.

실험 명령: `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/{h-post-call.h.ts,h-string-only.h.py,h-executable-value.h.ts}`. 기준 H-TS 끝에 `(execute())` 추가, H-Py 전체를 삼중 문자열에 감싸기, budget의 rows를 `execute()`로 교체.
```text
h-post-call.h.ts exit=0 exec=8b5fa9fb5b2688df
h-string-only.h.py exit=0 exec=8b5fa9fb5b2688df
h-executable-value.h.ts exit=1 NON_LITERAL (함수 값 거부)
```
모르는 root/budget 키도 각각 `UNKNOWN_KEY`로 거부했다. **실행 가능한 값이 객체 내부에서 통과한 발견은 없음**: 기존 음성 대조와 rows 함수 값 변형을 확인했다. 외부 가공식은 읽지도 실행하지 않는 점이 현재 한계다.

### F08 · P2 · Q4 — H predicate params 객체의 키 순서가 위치 인자 ABI다

영향: 일반 객체의 키 정렬을 의미 보존 작업으로 생각하면 predicate를 깨뜨린다. 형식으로 계속 비교하려면 params를 순서가 명시된 배열로 바꾸거나 이름 인자 계약을 채택하고 양쪽에 적용해야 한다.

근거: `src/hlit.rs:12-13`에 이 의존성이 의도적으로 명시되어 있고 `src/hmap.rs:49-53`이 객체 순서를 Params로 옮긴다. `src/sema.rs:390-405`가 zip으로 호출 인자를 바인딩한다. C `:138`의 정형 데이터 후보와 창시자 지침 `:326-331`의 명시적·일관된 계약 기준으로 수정 권장이다. 이것을 뜻 없이 생긴 버그라 하지 않는다.

실험 명령: `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/h-params-reorder.h.ts`. `managerOf.params`의 `{ m: "Member", c: "Club" }`를 `{ c: "Club", m: "Member" }`로만 바꾸었다.
```text
h-params-reorder.h.ts exit=1 TYPE_MISMATCH (첫 c는 Club, 받은 a는 Member)
```
현재 fixture는 두 타입이 달라 검사가 잡는다. 같은 타입 매개변수일 때 바인딩이 조용히 바뀌는 경로는 위 zip으로 확인했으나 런타임 결과는 확인 못함: 실행기가 없다. 일반 객체 키 순서 불변 테스트 `tests/v1.rs:116-117`은 aggregate 옵션만 바꾸므로 이 ABI를 검증하지 않는다.

### F09 · P1 · Q1/Q2/Q6 — A/E 중복 정책·계약 항목이 조용히 덮어써진다

영향: 앞에 작성한 필드 정책, 전이 조건, 비용 계약이 digest에서 사라진다. 병합/교체 규칙이 명시되지 않은 중복 선언을 정상 계약으로 승인하면 잘못된 입력 탐지를 입증할 수 없다. V2 전 중복 거부를 A/E/H에 맞춰야 한다.

근거: `src/parser.rs:296-300`은 필드 정책을 중복 허용하고 `src/sema.rs:536-547`은 같은 map 키에 마지막 값을 넣는다. `src/parser.rs:325-335`의 from/allow, `:412-420`의 budget 항목도 대입으로 덮어쓴다. transition/aggregate/extension 이름은 `src/sema.rs:549-552`, `:571-596`, `:624-628`에서 중복 검사 없이 map에 삽입한다. H 객체는 `src/hlit.rs:161-162`에서 중복을 거부하므로 작성 형식별 수용 범위도 다르다. C `:325`의 중복/구조 검사 후보와 E `:53`의 잘못된 기호 거부 기준이다.

실험 명령: `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/{duplicate-policy.aip,duplicate-from.aip,duplicate-budget.aip}`. 기준 필드 정책 앞에 `field internalNote read when views = 0`, 기준 close.from 앞에 `from status = CLOSED`, rows 앞에 `rows 1;`를 각각 추가했다.
```text
duplicate-policy.aip exit=0 exec=8b5fa9fb5b2688df
duplicate-from.aip exit=0 exec=8b5fa9fb5b2688df
duplicate-budget.aip exit=0 exec=8b5fa9fb5b2688df
```
세 경우 execution facts가 기준과 전체 일치한다. “두 정책은 AND로 합성” 또는 “첫 선언이 우선”이라는 계약은 확인 못함: 기준에 없다. 그러므로 이것을 이미 정해진 AND 의미 위반으로 꾸미지 않고, 입력 손실/모호성 승인으로 판정한다.

### F10 · P1 · Q1/Q6 — 필수 타입·제약 검사에 실제 누락이 있다

영향: nullable 값을 필수 필드에 대입하는 전이와 역전된 범위가 승인된다. 필수 검증의 완전성을 보장하지 못하므로 V2 전 typed facts 입력 검사를 보강한다.

근거: `src/sema.rs:583-587`은 전이 대입에서 같은 base 타입이면 nullable 여부를 보지 않는다. 인자 검사는 `:398`에서 nullable을 확인하므로 일관되지 않다. `:184-187`은 범위 적용 타입만 확인하며 하한/상한을 검증하지 않는다. `:788-802`는 budget 항목 존재와 관계 depth만 확인하고 값의 유효 영역은 정하지 않는다. 기준 C `:351`, `:356`, 창시자 지침 `:328`, `:472`의 타입·필수 안전성 요구에 연결된다.

실험 명령: `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/{nullable-assignment.aip,reverse-range.aip,zero-budget.aip}`. close의 `to status = CLOSED`를 `to title = internalNote`로, title을 `Text(100..1)`로, rows/deadline/cost를 0으로 각각 변경했다.
```text
nullable-assignment.aip exit=0 exec=adeefa1db3069cfb
reverse-range.aip exit=0 exec=61129d97dc4034c1
zero-budget.aip exit=0 exec=1d55821e2fd2dc53
```
첫 facts는 `title`(Text) 대입 RHS를 `Text?`로 기록했다. 이는 타입 모순이다. 역전 범위는 만족 가능한 값이 없어 오류로 진단해야 한다. 0 budget은 의도적인 무결과/즉시 timeout일 수도 있어 그 자체를 보안 우회라 하지 않는다. 0/음수 허용 규칙과 실행 시 강제는 확인 못함: 실행기가 없다.

### F11 · P1 · Q1/Q5/Q6 — 자기 재귀 정책을 거부하지 않는다

영향: V2의 행 정책으로 옮기면 SQL lowering이 끝나지 않거나 실행 비용이 계약 밖으로 나갈 수 있다. 현재 실행 실패를 관찰했다는 뜻은 아니다. V2 전 호출 그래프의 cycle을 거부하거나 지원 가능한 재귀·비용 규칙을 정해야 한다.

근거: `src/sema.rs:370-375`는 호출 대상/인자 타입만 검사하고 Bool을 반환한다. `:447-457`은 각 predicate의 본문을 한 번씩 검사할 뿐 cycle 검사를 하지 않는다. E `:53`의 최소 실행 가능성, C `:29`의 자원 계약, 창시자 지침 `:310`의 실행 비용 제약을 기준으로 판정한다.

실험 명령: `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/recursive-policy.aip`. 기준 active 선언의 RHS 전체를 `active(r)`로 변경했다.
```text
recursive-policy.aip exit=0 exec=52c689a5f96b1681
```
생성된 active.body가 자기 자신을 호출하고 Recruitment.rowRead가 이를 사용한다. 실제 SQL/런타임의 실패 모양은 확인 못함: spike는 의미 분석 출력만 제공한다.

### F12 · P2 · Q1/Q5/Q6 — partialUniqueIndex는 집행 검증이 아니라 종류 표시다

영향: 현재 EQ-09의 “DB 집행” 테스트는 index 종류 문자열이 나오는지만 확인한다. 모든 허용 limit 식을 그 index로 구현할 수 있다는 검증은 없다. V1 결과를 DB 집행 성공으로 보고하면 안 된다.

근거: `src/sema.rs:493-496`은 limit 조건을 일반 Bool로 검사하고 `:615-619`는 atMost가 1이면 조건 형태와 상관없이 partialUniqueIndex로 표시한다. `tests/v1.rs:76-77`도 이 문자열만 assert한다. C `:28`(EQ-09), `:112`는 실행식·잠금 키를 빠뜨린 채 구현됐다고 주장하지 말라고 한다.

실험 명령: `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/index-now.aip`. limit 조건에 `and periodEnd >= now`를 추가했다.
```text
index-now.aip exit=0 exec=7ffa44b7c5d21d9a
```
시간에 따라 조건에 속하는 행이 달라지는 식에도 partialUniqueIndex를 생성했다. 조건 값이 시간만으로 달라질 때 어떤 DB 유지/검증 방식을 쓸지 facts에 없다. 확인 못함: DB DDL/플래너/DB 실행이 없으므로 실제 index 생성·동시성·NULL 그룹 동작을 검증할 수 없다. 기준 fixture의 비nullable club + status=PUBLISHED에 대한 index 아이디어 자체에는 반대하지 않는다. V2 읽기 진행에 DB 쓰기 실험 전체를 요구하지는 않는다.

### A01 · P3 · Q1/Q2 — 다섯 기준 fixture의 동일 facts 주장은 직접 대조 결과에 동의

`tests/v1.rs:38-46`의 digest 비교를 재실행했고, 각 형식 CLI의 execution/metadata **전체 JSON 값**도 별도로 대조하여 일치했다. 명령: 각 `fixture/recruitment.{aip,e.ts,e.py,h.ts,h.py}`에 `.../target/debug/spike-v1-fixture facts <파일>` 실행 후 Python에서 전체 값 equality 비교.
```text
5 forms: execution JSON equal=True; metadata JSON equal=True
executionDigest: 8b5fa9fb5b2688df47a8eb3322e345a5dd7496a47408baff92f8d76e788f963d
metadataDigest: c765763dff76324c85ef5b62e388fa70e2906136eee93f9522b5f8038f124f9f
```
또 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline negative_controls_reject_with_expected_code -- --nocapture`를 실행했다.
```text
negative controls: 49 cases
test negative_controls_reject_with_expected_code ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.01s
```
49개 변형은 `tests/v1.rs:25-29`의 치환 개수 assert와 `:206-216`의 거부 코드 검사까지 있으므로 공허한 테스트라고 보지 않는다. 다만 서로 다른 parser가 같은 sema를 공유하므로 다섯 결과의 일치가 sema의 정확성까지 독립 검증하는 것은 아니다(`src/lib.rs:31-44`). F04/F06/F09가 공유 오류/입력 유실의 실제 반례다.

### A02 · P3 · Q1/Q6 — docs 분리에 동의하되, 귀속/진단 검증 범위는 resource 하나다

`tests/v1.rs:121-147`의 다섯 형식 docs 내용 변경/삭제 대조는 execution 불변과 metadata 변화에 실제로 성공했다. 추가로 임시 `docs-moved.aip`에서 Recruitment.docs를 Club 선언 안으로 옮겨 CLI facts를 실행했다.
```text
docs-moved.aip exit=0 execEqual=True metadataAnchor=resource:Club
```
근거: `src/sema.rs:99-109`는 metadata를 `resource:<name>`에 귀속하고 span을 별도 저장한다. 창시자 지침 `:162-169`, C `:318-321`, `:355`의 설명 선택성과 실행 분리에는 부합한다. 귀속이 무조건 소실되는 발견은 없음: 기준 resource docs, 다섯 형식 변경/삭제, A의 resource 이동을 확인했다. 다만 field/predicate/aggregate/extension docs와 rationale는 AST가 지원하지 않는다(`src/ast.rs:132-151`, `src/hmap.rs:258-268`). 이 후보 전체 지원을 V1 완료의 필수로 확대하지 않는다. round-trip 출력/재파싱 기능은 확인 못함: CLI에 facts/check만 있고 serializer가 없다(`src/main.rs:23-32`).

### F13 · P2 · Q6 — source map이 전체 원본 위치를 보존하지 않는다

영향: H 의미 오류가 올바른 줄의 잘못된 열을 가리킨다. E의 첫 줄 블록 offset도 누락되어 에디터 연결에 제한이 있다. V1 최소 의미 가능성을 전면 부정하는 결함은 아니지만 RK-06의 source map 검증 결과로는 부족하다.

근거: `src/parser.rs:18-19`, `:25-26`은 H 문자열의 span.line만 lex에 넘기고 `src/lexer.rs:27`은 col을 1로 시작한다. E도 `src/extract.rs:10-13`의 Block에 line_base만 있어 첫 줄 열 offset을 넘길 수 없다. `tests/v1.rs:220-225`는 E-TS 한 오류의 줄만 검증한다. C `:134`, E `:17`의 source map 요구와 비교했다.

실험 명령: `.../target/debug/spike-v1-fixture check <임시복사본>/fixture/h-column.h.ts`. H의 Apply.rows에서 `recruitment.club`를 `recruitment.clubb`로 바꾸었다.
```text
h-column.h.ts:81:18 UNKNOWN_FIELD `Recruitment`에 필드 `clubb` 없음
원본 recruitment.clubb 경로 시작: 81:31
```
spans 출력도 resource docs의 시작만 포함한다(`src/sema.rs:99-109`). 나머지 기호/필드/정책의 전체 source map과 E-Py/H 의미 오류의 열 정확성 검증은 없음. byte 열과 문자 열의 일관성은 확인 못함: Unicode 오류 위치 대조는 별도로 실행하지 않았다.

### F14 · P2 · Q6 — SY-6의 optional off/없는 옵션 거부를 검증할 표면이 없다

영향: 정의 내 unknown key 거부를 CLI/검사 설정의 unknown option 거부와 동일시할 수 없다. V2의 필수 타입·권한·비용 검사가 optional 검사 설정에 의존하지 않는다는 독립 검증이 필요하다.

근거: `src/lib.rs:31-44`는 load_str 뒤 sema를 항상 실행한다. 이는 현재 analyzer가 선택적으로 꺼지지 않는다는 뜻이며 서버 검증 분리 실험은 아니다. `src/main.rs:6-8`은 첫 두 인수만 읽고 `:23-32`는 facts 이외 어떤 command도 check처럼 처리한다. C `:356`(SY-6), 창시자 지침 `:449-472`를 기준으로 미검증을 표시한다.

실험 명령: `.../target/debug/spike-v1-fixture check spikes/spike-v1-fixture/fixture/recruitment.aip --optional-checks=off --unknown=1`.
```text
exit=0 ok A exec=8b5fa9fb5b2688df meta=c765763dff76324c
```
위 옵션은 구현된 옵션이 아니라 무시되는 여분 인수다. 확인 못함: optional 검사 설정·서버 실행 경로가 존재하지 않아 off 상태의 필수 안전성 검증을 실행할 수 없다. unknown H key 음성 대조의 성공으로 SY-6 전체 완료를 표시하면 안 된다. CLI 인수 엄격 검사와 V2 mandatory 검증 경로의 테스트를 구분해서 보강한다.

### F15 · P2 · Q5 — 다섯 spike 규칙은 기술 후보이며 창시자 승인 사실이 아니다

영향: 현 규칙을 확정 의미로 승격하면 작성자가 의도하지 않은 루트 조회 차단, 정책 내부 권한 범위, ID 처리 방식이 제품 계약이 된다. V2에서 사용할 규칙은 먼저 범위와 실행 의미를 명시하고 반례를 테스트해야 한다. 창시자에게 모든 구현 선택을 다시 묻는 것은 필요하지 않다.

기준: 창시자 지침 `:13-23`, `:335-355`, `:877`은 기술 후보와 승인 사실을 구분한다. 질문 기준은 `:836-849`이다. C `:34`, `:112`, `:367`과 E `:23`도 구체 규칙의 실험/제안 상태를 명시한다. `fixture/recruitment.aip:1-2`가 추가 기호를 spike 가정으로 밝힌 점에는 동의한다.

| 규칙 | 코드/기준 근거 | 판정·필요 결정 |
|---|---|---|
| 행 정책 없으면 denyAll | `src/sema.rs:526-534`; 창시자 `:520-526`, E `:23`은 신규 **필드** 기본 비공개 방향 | P3. 안전 기본값으로 합리적이며 직접 충돌 없음. 필드 비공개 방향이 곧 모든 **행** deny 승인이라는 근거는 없음. 기술 계약으로 명시·검증하면 되고 현재 창시자 재질문 불필요. |
| budget 없는 expose는 traverse 전용 | `src/sema.rs:788-802`; C `:37-40`의 Club에는 budget 없지만 root/traverse 구분 문구 없음 | P2. 기준이 비워 둔 의미를 코드가 채웠다. 비용 안전성과 호출자 DX를 분리해야 한다. budget 누락을 오류로 할지, traverse 전용 선언을 명시할지 기술 대안을 실험한다. 호출자 표현 범위를 제품적으로 제한할 필요가 생기면 창시자 판단 대상이다. |
| 정책 안 exists는 대상 행 정책 없이 평가 | `src/sema.rs:377-385`; C `:23`의 관계 **조회** 정책 재평가, `:108`의 별도 집계 scope | P2. trusted 서버 정책의 하위조회와 caller traverse는 다른 경로이므로 EQ-04와 즉시 충돌한다고 할 수 없다. 자동 집계 권한 파생으로 확대해서는 안 된다. 어떤 선언에서 raw 정책 조회를 허용하며 tenant·인증·비용은 어떻게 강제하는지 V2 전에 기술 계약으로 명시한다. 행 존재 노출/교차 tenant 허용으로 제품 보장 자체를 바꾸게 되면 창시자 판단이 필요하다. |
| atMost 1 → partial unique index | `src/sema.rs:598-620`; C `:28`, `:112` | P2. 비교용 불변식은 C에 있으나 집행 방식 채택은 미정이다. 비nullable club/status 고정 fixture의 후보로 합리적. F12처럼 허용 조건 전체의 DB 가능성은 검증 못했다. 구현 방식은 기술 결정이며 창시자에게 index 종류를 선택하게 할 이유 없음. |
| Ref/Id 호환 | `src/sema.rs:45-50`, `:390-405`; `fixture/recruitment.aip:21`, `:25` | P2. `Club.Id` 입력을 Ref<Club> predicate에 전달하기 위해 도입한 가정이다. C `:97-99`는 ID를 쓰지만 호환/자동 dereference를 정하지 않았다. identity 비교로 lowering할지 명시 변환할지 정하고, caller ID는 actor/tenant 권한을 대체하지 않게 한다. 기본적으로 기술 결정이며 편의 때문에 승인 scope가 바뀌면 창시자 기준과 다시 대조해야 한다. |

추가 실험 명령: `.../target/debug/spike-v1-fixture facts <임시복사본>/fixture/no-root-budget.aip`. 기준 Recruitment의 budget 블록만 삭제했다.
```text
no-root-budget.aip exit=0 Recruitment.rootQueryable=False
```
비용 제한 누락을 오류로 잡은 것이 아니라 resource를 루트 조회 불가로 재해석했다. 이것은 보안 우회 증명이 아니라 개발자 의도/DX 변경의 확인이다. 기준 fixture의 School/Member는 denyAll이고 managerOf.exists는 serverPolicy로 기록된다. 실제 deny·정책 하위조회·Ref/Id 변환의 런타임 효과는 확인 못함: 실행기가 없다.

### A03 · P3 · Q1 — EQ-01~EQ-11별 표현 범위와 보장 한계

현재 facts는 **기준 fixture의 선언 구조 대부분**을 담는다. 같은 digest가 곧 의미 완전성이나 실행 안전성이라는 해석에는 반대한다. 다음은 실제 CLI JSON과 코드의 대조 범위다. C의 EQ 정의는 `plan-docs/alignment/C-syntax-proposal.md:20-30`이다.

| EQ | 현재 표현·동의 근거 | 빠진 검증/제한 |
|---|---|---|
| 01 | `src/sema.rs:514-522`의 필드 타입/nullable/range, fixture `:41-48`의 7개 필드 보존 | 범위 역전·nullable 대입 F10. enum 값 순서는 F05. |
| 02 | `src/sema.rs:526-534`, `:447-455`의 rowRead와 active/managerOf 본문, fixture `:21-22`, `:50-51` | 관계/actor의 null 처리와 시간 기준의 런타임 의미는 미검증. 익명/학교 인증은 C가 별도 검증으로 둬 V1 누락으로 과장하지 않음. |
| 03 | `src/sema.rs:719-764`의 독립 select/filter/sort 목록 | select/sort 집합 정렬은 허용 목록 의미에 적합. 실제 caller sort 우선순위·권한 검증은 V2. |
| 04 | `src/sema.rs:765-786`의 target·target expose select 확인과 reapply 표기, Club.rowRead 별도 보존 | `reapply` 문자열은 재평가 실행 증명이 아님. 실제 target 행/필드 권한의 합성은 V2. |
| 05 | `src/sema.rs:536-547`, `:721-724`의 managerOf 필드 정책과 fieldWithPolicy | 역할별 출력/필드 존재 비노출은 미검증. 중복 필드 정책 유실 F09. |
| 06 | `src/sema.rs:634-716`의 source·groupKey·sourceAccess·callerFilter/rowOutput/release, Bookmark.rowRead 분리 | 집계 count의 승인 scope/추론 공격 방어는 facts가 아닌 V2 실행으로 검증해야 함. |
| 07 | `src/sema.rs:788-802`의 rows/depth/deadlineMs/cost 위치 보존 | 실제 rows/fan-out/plan 시간/실행 시간/출력 비용 강제는 미검증. budget 없으면 루트 차단하는 가정 F15. |
| 08 | `src/sema.rs:571-596`의 close.from/to/allow 보존 | 중복 항목/nullable 대입 F09/F10. 전이 실행·원자성은 V3. |
| 09 | `src/sema.rs:484-499`, `:598-620`의 limit 정의 참조·per·집행 종류, 정의 누락 거부 | DB 집행은 F12의 종류 표시 수준. 실제 DDL/잠금/동시성은 V3. |
| 10 | `src/sema.rs:805-842`의 read kind·입출력·named aggregate 입력 binding·effect·deadline·implementation | 입출력 range 유실 F04. 실제 worker/출력 검증·취소/권한 범위 강제는 V4. |
| 11 | `src/sema.rs:99-109`의 resource anchor·summary/visibility와 별도 spans | A02 범위에서 실행 digest 분리 확인. round-trip/전체 선언 docs·전체 source map은 미지원/미검증. |

이 표의 fixture 줄은 `spikes/spike-v1-fixture/fixture/recruitment.aip`를 뜻한다. 안전성 결과가 없는 항목은 **확인 못함: spike에 요청 플래너·DB·실행기·worker가 없어 실행 보장을 대조할 수 없음**이다. 해당 기능의 전부를 V1에 구현하라는 뜻은 아니다.

### F16 · P1 · Q6 — V1 검증은 부분 성립이며 V2 진입 관문을 분리해야 한다

판정: 후보 문법의 최소 파싱·의미 출력 가능성, 다섯 기준 fixture 일치, 기존 49개 음성 대조, resource docs 분리는 성립한다. E `:53`의 **잘못된 기호/동적 평가 거부를 일반적으로 달성했다는 V1 완료 주장에는 동의하지 않는다.** F01/F02/F04/F06/F09/F10/F11이 재현되는 상태이기 때문이다.

근거: E `:17`(RK-06), `:53-54`(V1/V2), C `:351`, `:355-356`(SY-1/5/6), `tests/v1.rs:49-86`을 대조했다. EQ coverage 테스트는 각 EQ의 일부 값만 확인한다. 특히 EQ-04는 reapply 표기, EQ-09는 집행 종류 문자열을 검사한다. 이는 표기의 존재 검증이고 실행 결과 검증은 아니다.

V2 전 필수 수정/고정:

1. F01/F02의 주석·문자열·import/두 번째 블록 오인식을 차단하고 정상 호스트 문자열도 유지되는 양성 대조를 추가한다. H도 같은 탐색기를 사용한다.
2. F04의 타입 제약을 facts에 보존하거나 지원하지 않는 위치를 거부한다. F06의 버려지는 params도 오류로 처리한다. 서로 다른 계약을 같은 facts로 지우는 경로를 막는다.
3. F09의 중복 정책/계약을 거부하고, F10의 nullable 대입·역전 범위, F11의 정책 cycle을 진단한다. 이 수정이 필수 검증 경로에서 실행되도록 한다.
4. F15의 trusted policy exists, Ref/Id 변환, budget 없는 루트 조회의 의미를 **실험 규칙**으로 명시한다. V2에서 caller traverse/집계 scope와 섞이지 않게 테스트한다. enum 순서와 digest 동등성 범위도 문서화한다.
5. 기대 facts를 parser/sema 결과와 별도로 EQ별 명세 값으로 대조한다. 기존 공유 sema 결과끼리의 equality만으로 누락을 검출하지 못한다. F04/F06/F09의 변형은 재발 대조로 남긴다.

V2 자체에서 입증할 것(현재 미완을 V1 필수 구현으로 확대하지 않음): E `:54`의 정책 포함 resource 1개+관계 1개+고정 집계에 대해 열린/닫힌 capability, 정확한 출력 타입, SQL 값 바인딩, 비용 거부를 실행한다. target 행/필드 정책 재평가, 개인 북마크와 고정 총수 분리, 필수 오류의 optional 설정 독립성도 정상/실패 입력으로 대조한다(E `:12-17`, C `:353`, `:356`). DB 불변식 동시성 전체는 V3, worker 전체는 V4 관문이다(E `:55-56`).

P2 보강: F03/F07의 호출 후 가공 경계, F05의 동등성 주장 범위, F08의 순서 ABI, F12의 index 가능 조건, F13의 source map, F14의 CLI/설정 검증. 이 항목들을 모두 출시 수준으로 해결해야 V2 실험을 시작할 수 있다는 요구는 하지 않는다. 선택한 V2 경로가 사용하는 항목부터 해결한다.

확인 못함: runtime/DB/worker·권한 누출의 실제 결과, 성능/개발 비용, 다른 프로젝트/본 crates와의 통합. 사용자 지정 spike만 검사했으며 본 crates로 범위를 넓히지 않았다. 독립 subagent 검토는 실행하지 않았다(사용자 및 적용 지침에 위임 요청 없음).

## 인용·실험 경로

본문의 C/E/창시자 지침은 각각 다음 파일의 줄 번호다. `src/`, `fixture/`, `tests/`는 모두 `spikes/spike-v1-fixture/` 아래의 경로다.

- C: `plan-docs/alignment/C-syntax-proposal.md`
- E: `plan-docs/alignment/E-technical-risks.md`
- 창시자 지침: `plan-docs/sources/founder-integrated-directive-2026-10-03.md`

변형 입력은 아래 임시 디렉터리에 spike를 복사한 뒤 fixture 안에 만들었다. 원본 spike 소스/fixture/tests는 수정하지 않았다. 본문의 `.../target/debug/spike-v1-fixture`는 아래 원본 binary의 약기다. 중괄호로 여러 입력을 표시한 명령은 각 입력마다 CLI를 한 번씩 실행했다는 뜻이다.

```sh
v1_r1_bin=/Users/winterholic/development/projects/aip/spikes/spike-v1-fixture/target/debug/spike-v1-fixture
v1_r1_tmp=/var/folders/7_/5w7y5vq9329g81pk_fv8_mhh0000gn/T/aip-v1-r1-mgqlmo07/spike-v1-fixture
"$v1_r1_bin" facts "$v1_r1_tmp/fixture/range-1.aip"
```

실험은 Python subprocess로 CLI만 호출했고, 변형 TS/Python 모듈이나 문자열 속 코드는 실행하지 않았다. 명령과 결과의 `exec`/`meta`는 각각 전체 digest의 앞 16자리다. 전체 baseline digest는 위 직접 검증 절에 기록했다.
