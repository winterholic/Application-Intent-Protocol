# AI 친화성·런타임 신뢰성(결정성·추적성) 감사 (2026-10-13)

> 기준: [창시자 심층 감사 지침](../sources/founder-deep-audit-directive-2026-10-13.md)의 "4. AI 친화성"과 "3. 런타임 신뢰성" 중 선언 해석·실행 결과의 결정성, 오류의 추적·재현 가능성.
> 범위 밖: 설계 의도·문법 완전성·내장 동작·누락 기능(백엔드 포괄성 작업자), 보안·권한·테넌트 격리(보안 작업자). 겹치는 사안은 각 항목의 "겹침"에 메모만 남겼다.
> 방법: 실제로 잘못된 선언 샘플을 만들어 출력을 관찰하고, 같은 입력을 여러 프로세스·여러 순서로 실행해 비교했다. 실행하지 않은 것은 "미실행"으로 적었다.

## 0. 요약

| # | 영역 | 분류(지침 §8) | 영향도 | 상태 |
|---|---|---|---|---|
| F1 | 제품 정의 오류가 코드·줄만 줌 | 실제 사용 시나리오 미지원(AI 자가 수정) | 높음 | 수정·테스트 |
| F2 | PoC 검사기 진단 107개 중 69개에 고치는 방법 없음 | 실제 사용 시나리오 미지원 | 중간 | 수정·테스트 |
| F3 | 제품 설정 오류가 모두 같은 문장 | 실제 사용 시나리오 미지원 | 중간 | 수정·테스트 |
| F4 | 제품 서버에 요청 ID·요청 로그 없음 | 실제 사용 시나리오 미지원(추적) | 높음 | 수정·테스트 |
| F5 | 같은 `.aip` 확장자의 두 언어가 서로의 파일을 오류로 오인시킴 | 의미론적 모순(AI 혼동) | 높음 | 안내 추가·테스트, 구조 결정은 남김 |
| F6 | `budget` 없는 `expose read`가 조용히 도달 불가 | 불필요한 복잡도(암묵적 기본값) | 낮음 | 개발 조언 추가·테스트 |
| F7 | rule 동시 발화가 선언 순서가 아닌 이름 순으로 실행 | 의미론적 모순 | 중간 | 수정·테스트 |
| F8 | 제품 언어의 정본 문법 문서 없음, `X`/`X.Id` 두 표기 | 불필요한 복잡도(토큰·중복) | 중간 | 작성 카드·조언·테스트 |
| F9 | 오류 코드 체계가 두 개 | 추상화 실패 | 낮음 | 결정 대상 |
| F10 | 계약 JSON의 크기(들여쓰기 87~92%, `errors` 24~35%) | 불필요한 복잡도(토큰) | 낮음 | 측정만, 제안 |
| F11 | 제품 읽기 거부의 허용 목록 부재 | 실제 사용 시나리오 미지원 | 낮음 | 보안 작업자와 겹침, 미적용 |
| F12 | 선언 해석의 결정성 | (검증, 결함 아님) | – | 검증·테스트 |
| F13 | 제품 wire 오류 코드에 재시도 지침 없음 | 실제 사용 시나리오 미지원 | 중간 | 레지스트리·테스트 |

문제를 만들어내지 않기 위해, 결정성(F12)은 결함을 찾지 못했고 그 사실을 그대로 적었다.

## F1. 제품 정의 오류 진단

- 분류·영향도: 실제 사용 시나리오 미지원(AI가 오류만 보고 스스로 고치는 시나리오). 높음. 제품 경로(`aip service check`)가 사용자가 쓰는 입구다.
- 재현 근거: `spikes/spike-v1-fixture/fixture/recruitment.aip`의 한 줄을 바꾼 샘플의 출력.
  - `status: RecruitmentStatuz`(필드 타입 오타): 오류 6개가 줄 순서로 나왔고, 원인(UNKNOWN_TYPE)은 세 번째였다. 나머지 다섯은 `PUBLISHED를 해석할 수 없음`처럼 원인과 무관해 보이는 연쇄 오류였다.
  - `rows fifty`: 메시지가 `정수 필요, `Ident("fifty")` 발견`이었다. 컴파일러 내부 Debug 표기가 사용자에게 나갔다.
  - `sorts`(키 오타): `expose read 안 알 수 없는 항목 `sorts``. 어떤 키가 허용되는지, 어디 선언인지 없음.
  - JSON 출력에 파일·선언 경로·고치는 방법·코드 설명 없음. 코드는 레지스트리에 없어 `aip explain-code`가 모른다.
- 근본 원인: 제품 정의 검사기(`spike-v1-fixture`)의 `Diag`가 `code, msg, span`뿐이다. PoC 검사기는 레지스트리·help·`explain-code`를 갖췄지만(D-P17) 제품 경로는 이를 이어받지 않았다.
- 수정(커밋 511f85a):
  - `Diag`에 선언 경로(`scope`)와 `help`를 더했다(상자에 담아 `Diag` 크기를 늘리지 않음, 아래 주의).
  - 제품 정의 코드 레지스트리 `spikes/spike-v1-fixture/src/codes.rs`(코드 68개: 제목·원인·고치는 방법·유효한 예제)와 생성 문서 `spec/definition-diagnostics.md`.
  - 한 번 실패한 로드의 진단을 `enrich`가 한꺼번에 다듬는다: 원인(선언 수준) 오류를 먼저 두고, 그 결과로 생긴 오류에 `cascade: 줄 N의 CODE를 먼저 고칠 것`을 붙인다. 가까운 철자(`did you mean`), 블록이 받는 키 목록, 고치는 방법은 코드마다 한 번만 쓴다.
  - 위치한 선언 경로: `resource Recruitment > expose read > budget`. `.aip`와 내장(E) 형식에서 계산하고, 호스트 리터럴(H) 형식은 데이터 구조라 경로를 만들지 않는다.
  - `aip explain-code <제품 코드>`와 `aip diagnostics --definition [--json|--markdown]`.
  - 파서의 Debug 토큰 표기를 사용자가 쓴 토큰으로 바꿨다.
- 주의(회귀에서 배움): `Diag`에 문자열 두 개를 그대로 넣자 `production_limits`의 작은 스택 중첩 테스트가 SIGABRT로 실패했다. `Diag`가 재귀 하강 파서 모든 함수의 오류 타입이라 크기가 곧 스택 사용량이기 때문이다. 부가 필드를 `Option<Box<..>>`로 옮겨 해결했다. 테스트를 약화하지 않았다.
- 검증: `spikes/spike-v1-fixture/tests/diagnostics.rs` 17개(오타 필드·키·연산자·선언 키워드·타입·술어, 연쇄 순서, 내장 형식 줄 번호, H 형식, 레지스트리 완전성과 죽은 항목 없음, 모든 예제가 유효한 정의, 블록 키와 파서 arm 일치, 생성 문서 일치). `crates/aip-cli/tests/codes.rs`의 제품 경로 테스트. 기존 spike-v1 테스트 전체 통과(115개).

## F2. PoC 검사기의 고치는 방법

- 분류·영향도: 실제 사용 시나리오 미지원. 중간. PoC는 이미 레지스트리가 성숙하다(113개 코드, 예제는 파서로 검증).
- 재현 근거: `conformance/sema`의 235개 파일(진단 107개) 중 69개(64%)에 `help`가 없었다. 레지스트리는 E104의 고치는 방법을 "the help text suggests close matches"라고 적었지만, 실제 출력(`events [Renamd]`)에는 제안이 없었다. 레지스트리 문구와 출력이 어긋났다.
- 근본 원인: 방출 지점이 `help`를 직접 쓰는 방식이라, 쓰지 않은 지점은 `explain-code`를 따로 실행해야 한다.
- 수정(커밋 2febb28): `aip-sema/src/hints.rs`. 방출 지점이 쓴 help는 건드리지 않고, 없으면 레지스트리의 고치는 방법을 코드마다 한 번 붙이고, 알 수 없는 이름(E102~E104)에는 소스에서 가까운 철자를 제안한다. 같은 모양 코드를 `codes::` 상수로 참조하면 방출 위치 스냅샷(`code_sites.snap`)이 오염되므로 문자열로 참조했다.
- 검증: `every_checker_diagnostic_carries_help`(conformance 전체: help가 없는 진단 69 → 1, 남은 1은 같은 코드의 반복), `a_misspelled_name_gets_the_close_spelling`.

## F3. 제품 설정 오류

- 분류·영향도: 실제 사용 시나리오 미지원. 중간. 운영자와 AI가 `service.json`을 고치는 입구다.
- 재현 근거: 설정 파일의 오타 키(`lissten`), 문법 오류(닫히지 않은 객체), 필수 키 누락이 모두 `{"code":"BAD_CONFIG","message":"제품 설정 형식 오류"}` 한 문장이었다. 외부 연동 검증은 열 군데가 모두 `외부 연동 설정 또는 DNS 검증 실패`였다.
- 근본 원인: 설정에 비밀이 있을 수 있어 serde 오류를 통째로 버렸다. 이유와 위치까지 버릴 필요는 없었다.
- 수정(커밋 4416432): serde 오류의 이유(unknown/missing field, 기대 타입)와 줄·열을 알려주되, 틀린 값은 지운다(`invalid type: string "..."` → `invalid type: string`, unknown variant의 값 삭제, Deserialize 구현이 만든 임의 메시지는 "a value is not accepted"). 외부 연동은 어느 integration이 어느 규칙(예: `timeout_ms는 1~30000`)을 어겼는지 이름과 한계로 알리고, URL과 비밀 환경 변수 이름은 출력하지 않는다(기존 테스트 `secret_is_loaded_only_from_explicit_environment_reference`가 변수 이름 비노출을 고정하고 있어 그 결정을 지켰다).
- 검증: `aip-service` 단위 테스트 5개(키·줄 번호, 비밀 값이 어떤 오류에도 나오지 않음, 이유 정리 함수, URL 자격 증명 비노출).
- 남은 것: `SERVER_CONFIG "listener·공식 확장 설정 실패"`, `invalid local READ extension configuration`, `AUTH_CONFIG`, `RAG_CONFIG`, `BROKER_CONFIG`는 여전히 포괄 문장이다. 서로 다른 원인이 한 코드에 묶여 있어 같은 방식의 분리가 필요하다(후속).

## F4. 요청 ID와 요청 로그

- 분류·영향도: 실제 사용 시나리오 미지원(추적). 높음. 지침 3의 "오류를 추적하고 재현할 수 있는가"에 대한 직접 답이 "아니오"였다.
- 재현 근거:
  - 제품 서버(`spike-v6-transport`)는 실패 시에만 stderr에 `accept_failed`, `request_panicked` 한 줄씩 쓴다. 요청마다의 기록, 요청 ID, 응답의 추적 정보가 없다.
  - 제품 wire는 앱 오류도 HTTP 200과 `{"ok":false,"code":...}`로 돌려준다. 그래서 프록시·접근 로그만으로는 실패한 요청을 구분할 수 없다.
  - PoC 런타임은 호출마다 `intent, actor, status, code, reason, ms`를 로그에 남기지만 요청 ID는 없다(`aip-runtime/src/http.rs`).
- 근본 원인: 요청 단위 상관 키 개념이 서버에 없다.
- 수정: `spikes/spike-v6-transport/src/trace.rs`.
  - 모든 요청이 ID를 가진다(`req_` + 난수 24자 hex). 신뢰하는 상위(프록시·서버 호출자)가 보낸 `x-request-id`는 8~64자의 안전 문자일 때만 채택한다.
  - JSON 응답에 `x-request-id`를 싣고, 브라우저가 읽도록 `access-control-expose-headers`를 더한다.
  - 요청이 끝날 때 한 줄: `{"event":"request","requestId","method","route","status","code"?,"ms"}`. 라우트는 고정 이름 집합이고 그 밖은 `other`다. 토큰·주체·쿼리·본문·객체 키·파일 이름은 기록하지 않는다. `AIP_REQUEST_LOG=off`로 줄을 끌 수 있다(헤더는 유지).
  - 응답을 `Response`가 소유한 `Trace`의 `Drop`에서 기록하므로 조기 반환(헤더 오류, 인증 실패, 시간 초과)도 빠지지 않는다.
- 한계(미적용): 객체 스트림·정적 파일·SSE·WebSocket 응답은 자체 헤더 작성 경로라 `x-request-id`를 싣지 않는다. 로그 줄은 모든 요청에 남지만 이 경로에서는 `status`가 `null`이다. 오류 본문 JSON에 `requestId`를 넣는 것은 본문 계약(DD-14)을 바꾸는 일이라 결정 대상으로 남긴다. SDK(TypeScript)가 실패 객체에 `requestId`를 싣는 것도 후속이다.
- 검증: `tests/request_id.rs` 3개(응답마다 서로 다른 생성 ID, 안전한 상위 ID만 채택, 브라우저 노출 헤더), `trace.rs` 단위 테스트 4개(ID 형태, 채택 규칙, 라우트가 경로를 담지 않음, 줄에 페이로드 없음). 전체 spike-v6 통합 테스트 실행 결과는 §검증 기록.

## F5. 같은 확장자의 두 언어

- 분류·영향도: 의미론적 모순(AI 혼동). 높음. AI가 `.aip` 파일을 만들 때 어느 언어인지 구분하는 단서가 확장자에 없다.
- 재현 근거:
  - `aip check spikes/spike-v1-fixture/fixture/recruitment.aip`(제품 정의를 PoC 검사기로) → `error AIP-E100 ...:9:18 unexpected character ';'` + help `Follow the message... see spec/grammar.md`. 정의가 틀린 것이 아니라 다른 언어인데, 문법 문서(`spec/grammar.md`)를 가리켜 오히려 잘못된 방향으로 이끈다.
  - `aip service check examples/shop/app.aip`(PoC 프로그램을 제품 검사기로) → `LEX_UNEXPECTED_CHAR ...` 줄 42 `]`.
  - PoC: `entity`, `command`, `query`, `use`, `actor X via`. 제품: `resource ... { fields { } }`, `expose`, `transition`, `actor X`.
- 근본 원인: 제품 언어는 PoC의 문법을 이어받지 않았고(독립 spike), 확장자와 명령 이름(`aip check`/`aip service check`)이 같은 계열처럼 보인다. 독립 `.aip` 필요성은 OPEN([PRINCIPLES §3](../../docs/PRINCIPLES.md))이라 확장자 변경은 임의로 정하지 않는다.
- 수정(비호환 없음): 두 검사기가 상대 언어의 신호를 보면 첫 오류에 `language:` 안내를 붙인다. PoC 검사기는 최상위 `resource`와 `fields {`가 있으면 `aip service check`를, 제품 검사기는 최상위 `entity/command/query/use`가 있으면 `aip check`를 안내한다.
- 검증: 아래 §검증 기록의 테스트.
- 결정 대상(`docs/DECISIONS.md`에 제안으로 남김): (a) 제품 언어에 별도 확장자(예: `.aipd`), (b) 한 파일 머리에 언어 표지, (c) 현재처럼 확장자 공유와 오류 안내. 사용자 코드·문서의 `.aip`를 바꾸는 (a)·(b)는 공개 문법 변경이라 확정하지 않았다.

## F6. `budget` 없는 `expose read`

- 분류·영향도: 불필요한 복잡도(암묵적 의미). 낮음.
- 재현 근거: 제품 언어는 무제한 기본값을 두지 않는 방침이라 `budget`이 없으면 그 resource는 "다른 resource의 traverse 대상 전용"이 되고, 어느 resource도 그것을 traverse하지 않으면 모든 읽기가 런타임에 `NOT_ROOT_QUERYABLE`로 실패한다. 정의 검사는 통과한다. 이것이 AI가 `budget`을 빠뜨렸을 때의 증상이다(런타임 메시지 자체는 `관계 대상 전용(budget 없음)`으로 명확하다).
- 수정: 개발 조언 `ADVISORY_UNREACHABLE_READ`(`devChecks`가 켜졌을 때만, 계약·digest에 영향 없음). 같은 의도의 정당한 정의(`Club`처럼 traverse 대상 전용)에는 경고하지 않는다.
- 검증: `spikes/spike-v11-dev-checks/tests/unreachable_read.rs` 3개(fixture는 조언 없음, 도달 불가를 지적, budget을 주면 사라지고 digest 불변).

## F7. rule 동시 발화 순서

- 분류·영향도: 의미론적 모순(선언 순서가 digest에는 들어가는데 실행 순서에는 안 들어감). 중간. 같은 행에서 둘 이상의 rule이 한 트랜잭션에 참이 되고 같은 필드를 쓰면 결과가 어느 rule이 마지막에 도는가에 달려 있다.
- 재현 근거(실행함): `crates/aip-cli/tests/rule_order_e2e.rs`. 세 rule을 `Zeta`, `Mid`, `Alpha` 순으로 선언하고 모두 같은 필드에 자기 이름을 쓰게 했다. 수정 전 최종 값은 `zeta`였다. 즉 실행 순서는 Alpha → Mid → Zeta(이름 순)였고 선언 순서(Zeta → Mid → Alpha)가 아니었다. 선언 순서로 도는 것이라면 최종 값은 `alpha`여야 한다.
- 근본 원인: `settle_rules`가 `DELETE ... RETURNING`으로 대기 행을 한꺼번에 받아 받은 순서대로 실행한다. PostgreSQL은 RETURNING의 순서를 보장하지 않고, 대기 행을 쌓는 트리거는 같은 테이블에서 트리거 이름 순으로 발화한다. 어느 쪽도 정의가 말할 수 있는 규칙이 아니다. `Reaction` 배열은 선언 순서로 보존되어 digest에 들어가는데(F12), 그 순서가 실행에 쓰이지 않았다.
- 수정: 대기 행을 (rule의 선언 위치, 행 id)로 정렬해 실행한다(`aip-runtime/src/exec.rs`). 행 id까지 키에 넣어 같은 rule의 여러 행도 재현 가능한 순서가 된다. 이전 동작은 문서에 없던 것이라 공개 동작의 호환을 깬다고 보지 않았다.
- 검증: 수정 전 실패(`zeta`)와 수정 후 통과(5회 반복)를 확인했다. `shop_e2e`, `saas_e2e`, `cms_e2e`의 기존 rule 테스트도 다시 돌렸다(§검증 기록).
- 남은 것: 사양(`spec/grammar.md`)에 "한 트랜잭션에서 함께 참이 되는 rule은 선언 순서대로 실행된다"를 적지 않았다(백엔드 포괄성 작업자의 문법 문서와 겹칠 수 있어 메모만 남김).

## F8. 제품 언어의 정본 문법 문서와 같은 뜻의 두 표기

- 분류·영향도: 불필요한 복잡도(토큰), 중복 표현. 중간.
- 재현 근거: `spec/grammar.md`(51KB)는 PoC 언어의 문법이고 제품 언어(`resource`/`expose`/`transition`)에는 정본이 없었다. AI가 제품 정의 하나를 쓰려면 제안 문서 `plan-docs/alignment/C-syntax-proposal.md`(24KB)와 V1~V15 결과 문서를 읽어야 했다. 같은 뜻의 두 표기도 찾았다: 필드 `club: Club`(참조, `REFERENCES` 외래 키)와 `club: Club.Id`(외래 키 없는 bigint)는 둘 다 "어느 동아리에 속한다"로 읽힌다(`spikes/spike-v2-read/tests/reference_vs_id.rs`가 DDL 차이를 고정). `;`와 줄바꿈이 목록 구분자로 모두 허용되지만 제품 언어에는 포매터가 없어 문체가 갈린다(관찰만).
- 수정: `spec/definition-syntax.md`(약 13KB, 그중 4KB는 작성 예 둘): 블록·키·형태·뜻의 표, 언어가 가정하지 않는 기본값 목록, 정의 두 개. 키 표는 파서와 같은 표에서 나오며 양방향으로 검사하고, 기본값 문장은 각각 정의를 로드해 보이는 테스트로 고정했다(`spikes/spike-v1-fixture/tests/diagnostics.rs`). `X.Id`는 `devChecks` 조언 `ADVISORY_ID_WITHOUT_REFERENCE`로 알린다(오류로 올리면 공개 문법 변경이라 결정 대상).
- 결정과 조치(2026-10-13, D-A16): 창시자가 `fields` 안의 `X.Id`를 정의 오류로 막기로 했다(별도 철자는 두지 않음: "괜히 선택지를 또 주는거 자체가 내가 지향한설계와 다른 거 같아"). 위 개발 조언은 오류 `ID_FIELD_NEEDS_REFERENCE`로 대체해 제거했고 레지스트리·생성 문서·작성 카드·`reference_vs_id.rs`를 새 의미로 바꿨다. 기본 키 `id` 필드는 기존 `TYPE_MISMATCH` 규칙이 계속 맡는다.

## F13. 제품 wire 오류 코드에 재시도 지침이 없음

- 분류·영향도: 실제 사용 시나리오 미지원. 중간. PoC 런타임은 코드마다 HTTP 상태와 `retryable`을 레지스트리에 가졌지만 제품 wire는 대부분의 실패를 HTTP 200과 `{"ok":false,"code"}`로 돌려주고, 102개 코드 어디에도 재시도 지침이 없었다. 클라이언트에는 이벤트 스트림용 재시도 코드 집합 하나(`client/jobs.ts`)뿐이다. 알 수 없는 결과(`COMMIT_UNKNOWN`)를 모르고 다시 보내면 효과가 반복될 수 있다.
- 수정: `spikes/spike-v6-transport/src/wire_codes.rs`(분류, 재시도 `never/later/check-first/reauthenticate`, 원인, 수정), 생성 문서 `spec/wire-errors.md`, `aip explain-code`. 테스트는 읽기·쓰기·worker·전송 계층 소스의 대문자 문자열을 훑어 항목 없는 코드를 막고(음성 대조 포함) 방출하지 않는 항목도 막는다. 작업 중 다른 작업자가 추가한 webhook 전달 코드 다섯 개가 이 테스트에 걸려 항목을 더했다.
- 한계: 설명은 방출 코드를 읽어 적었고, 한 코드가 여러 원인을 덮는 곳은 공통 부분만 적었다. 응답 본문에 `retryable`을 넣는 것은 본문 계약(DD-14) 변경이라 결정 대상이다.

## F9. 오류 코드 체계 둘

- 분류·영향도: 추상화 실패. 낮음.
- 사실: PoC는 `AIP-E###`/`AIP-W###`/`AIP-I###`/`AIP.*`, 제품 정의는 `UPPER_SNAKE`(`UNKNOWN_FIELD` 등), 제품 wire는 `{"ok":false,"code":"BAD_VALUE"}`(HTTP 200), PoC wire는 `AIP.*`와 HTTP 상태다. 같은 이름의 의미가 겹친다: 제품 `UNKNOWN_FIELD`는 정의 오류와 런타임 거부 둘 다 쓴다(`plan.rs`의 `UNKNOWN_KEY`, `DUPLICATE`, `BAD_VALUE`가 정의 코드와 이름이 같다).
- 이번에 한 것: 정의 코드와 PoC 코드 모두 `aip explain-code`로 조회하게 했고, 생성 문서를 둘로 나눠 서로 가리킨다. 이름 충돌을 해소하는 통합(코드 접두 도입)은 공개 wire를 바꾸므로 결정 대상이다.

## F10. 계약 크기

- 측정(`aip contract`, 4개 예제; 바이트): shop 소스 3.1KB → 출력 9.8KB(compact 5.2KB), cms 4.5KB → 35.6KB(19.1KB), saas 12.5KB → 55.3KB(29.9KB), ariari 34.8KB → 173.4KB(90.4KB). 들여쓰기가 compact 대비 87~92%를 더하고, 각 intent의 `errors` 배열이 compact 크기의 24~35%를 차지한다(shop 9종, cms 22종, saas 21종, ariari 76종의 서로 다른 항목이 intent마다 반복).
- 영향: 생성 계약을 그대로 AI 컨텍스트에 넣는 경우에만 문제다. AI가 읽는 정상 경로는 타입이 있는 클라이언트(`gen-ts`: shop 6.1KB, cms 17.2KB)이고, 제품 경로의 `service gen` 결과는 정의(2.9KB) 대비 2.4KB로 작다. 그래서 낮음으로 두었다.
- 제안(미적용, 공개 계약 형식): `aip contract --compact`, 공통 `errors`를 한 번만 두고 intent는 차이만 기록. 후자는 계약 형식을 바꾼다.

## F11. 제품 읽기 거부의 허용 목록

- 사실: `spike-v2-read/src/plan.rs`의 거부 메시지는 대부분 리소스·필드·원인을 이름으로 말한다(예: `FIELD_NOT_EXPOSED`는 "서버 정의의 expose 변경이 필요"). 반면 `UNKNOWN_KEY`, `FILTER_NOT_ALLOWED`, `SORT_NOT_ALLOWED`는 허용되는 값을 말하지 않는다. 허용 필터·정렬·select 이름은 공개 계약에 이미 있으므로 알려도 새로 드러나는 정보가 없다.
- 겹침: 보안 작업자가 같은 파일을 수정 중이라 충돌을 피하려고 적용하지 않았다. 그쪽 작업이 끝난 뒤 `rej`에 허용 목록(계약에서 계산)을 붙이는 후속으로 남긴다.

## F12. 선언 해석과 실행 결과의 결정성

- 방법과 결과(실행함):
  - 프로세스 25회 반복: `aip core/ir/ddl/contract/gen-ts/check`를 네 예제(ariari·shop·saas·cms)에, `aip service check`를 정상·오류 샘플 4개에. 출력 SHA가 모두 하나였다(24+4개 조합, 각 distinct=1). Rust `HashMap`의 프로세스별 난수 시드가 출력에 새지 않는다는 근거다.
  - 선언 순서: 제품 정의는 최상위 선언 블록을 8번 섞어도 `executionDigest`가 같았다(0/8 차이). PoC는 `shop`, `saas`, `cms`에서 섞을 때마다 Core IR digest가 달랐다. 원인을 찾아 보니 차이는 `reactions`(rule·이벤트 처리기·스케줄·webhook) 배열 하나뿐이었다(IR 헤더의 규약: 맵은 `BTreeMap`, 목록은 선언 순서).
- 판단: 선언 순서가 digest를 바꾸는 것은 `reactions`가 선언 순서를 의미로 삼기 때문이다. 이것이 의도인지는 문서에 없다. 그래서 F7로 이어졌다.
- 테스트(`spikes/spike-v2-read/tests/determinism.rs`, 4개): 한 프로세스 안에서 40번 로드·계획해도 digest, 읽기 계획 SQL·매개변수·비용, DDL이 같다(`HashMap` 인스턴스마다 시드가 달라 순회 순서가 새면 드러난다). 선언 블록을 뒤집기·회전·고정 시드 섞기로 바꿔도 digest·SQL·DDL이 같다. 필드·멤버·select·filter 순서와 주석·공백도 digest 불변이다. 결함은 찾지 못했다.
- 기록(결함 아님): 열거형 값의 순서는 제품 digest에 들어간다. DB에서는 `CHECK (col IN (...))`라 순서에 의미가 없으므로 순서만 바꿔도 계약 변경(새 digest, 이행 계획)이 된다. 의도 문서가 없어 `enum_value_order_is_part_of_the_digest`로 현재 동작만 고정했고, digest 호환 때문에 정렬 여부는 결정 대상으로 남긴다.

## 결정 대상 (확정하지 않음, 창시자 결정 필요)

공개 문법·wire를 바꾸는 사안이라 임의로 정하지 않았다. 각각 지금 적용한 비호환 없는 완화책이 있다.

| 사안 | 대안 | 지금의 완화 |
|---|---|---|
| 제품 언어와 PoC 언어의 확장자 공유(F5) | 별도 확장자, 파일 머리 언어 표지, 현행 유지 | 두 검사기의 `language:` 안내 |
| `club: Club.Id`(외래 키 없음) 허용 여부(F8) | 오류로 금지, 별도 철자 | 결정됨(2026-10-13): 오류로 금지, 별도 철자 없음 |
| 오류 본문에 `requestId`·`retryable`·`explain` 추가(F4, F13) | 본문 필드 추가(DD-14), 헤더만 유지 | 헤더 `x-request-id`, `spec/wire-errors.md` |
| 오류 코드 두 체계 통합(F9) | 접두 도입, 현행 유지 | 세 레지스트리를 `aip explain-code` 하나로 조회 |
| 열거형 값 순서의 digest 반영(F12) | 정렬해서 digest | 현재 동작을 테스트로 고정 |
| 계약 JSON의 공통 `errors` 분리와 compact 출력(F10) | 계약 형식 변경 | 측정만 |

## 검증 기록

- 개별 실행(통과): spike-v1 전체 115개, spike-v11 12개, `aip-cli --test codes` 11개, `aip-sema`+`aip-ir` 68개, `aip-service` 단위 24개 이상, spike-v2-read `determinism` 4개·`reference_vs_id` 1개, spike-v6 `request_id` 3개·`wire_codes` 5개, PoC e2e(`shop`, `saas`, `cms`, `rule_order_e2e`) 통과, spike-v6 통합 테스트 39개 파일 통과.
- spike-v6 `extension_lifetime`은 내 변경을 되돌린 상태에서도 실패한다(worker가 PID 파일을 쓰지 못함). 이 환경의 실행 제약으로 보이며 원인은 미확인이다.
- `bash tools/verify.sh` 전체는 **통과를 확인하지 못했다.** 04:48~04:53 마지막 실행에서 fmt, clippy, 워크스페이스 테스트, spike-v1 테스트까지 통과했고 spike-v2-read 단계에서 `--locked`가 Cargo.lock 불일치(다른 작업자가 `aip-execution`에 추가한 `hmac` 의존성이 spike-v2-read/Cargo.lock에 없음)로 멈췄다. 잠금 파일은 오프라인으로 갱신해 커밋했지만 새벽 5시 제한 때문에 그 뒤 단계(spike-v2/v3/v5 테스트, node 테스트)는 다시 돌리지 못했다. 그 앞선 두 번의 실행은 다른 작업자의 임시 검증 워크트리 경로가 박힌 낡은 `aip-ir` 테스트 산출물 때문에 실패했고(`cargo clean -p aip-ir` 후 해소), 코드 결함이 아니었다.
- `tools/check_plan_docs.py`: 오류 0.

(마지막에 `bash tools/verify.sh` 결과와 함께 기록)
