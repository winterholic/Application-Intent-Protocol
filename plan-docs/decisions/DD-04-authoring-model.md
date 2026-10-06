# DD-04. 서버 쪽 정의(리소스·정책·전이·불변식)를 무엇으로 쓰는가

> 결정 상태: **검증 필요** (새 기준: [B §5](../alignment/B-decision-reclassification.md#5-dd-문서의-현재-처리-상태))
> 기존 에이전트 리뷰·추천은 이전 지침 아래 작성된 검토 이력이다. 제안 본문은 보존하되 새 원문과 B의 확정 기준에 충돌하면 그 기준을 우선한다.
> 범위: 애플리케이션 개발자(사람과 AI)가 **서버 쪽 정의**를 쓰는 수단과 그 신뢰 경계. 호출자 요청을 쓰는 방식(T06)과 엔진 실행 형태(DD-06 예정)는 다루지 않는다.
> 기준 원문: 창시자 인계 PART I §3·§4·§5, PART II §9.1·§9.4, PART VII §14, 의제 1. `docs/PRINCIPLES.md` §3. `../00-rules.md` §4
> 이전 지침 토론 기록: `../reviews/DD-04-codex-r1.md`, `../reviews/DD-04-codex-r2.md`, `../reviews/DD-04-codex-r3.md` (이전 지침 아래 에이전트 검토 이력; 새 기준은 위 B §5)
> 관련: `../topics/T02-authoring-model.md`, `DD-01`·`DD-02`(무엇을 써야 하는지에 대한 에이전트 합의안)

---

## A. 지금 결정할 문제

확인된 호출자 읽기 조합과 표준 쓰기 목표를 실현하려면 서버 쪽에 리소스의 필드·관계, 정책, 전이·불변식, 공개 집계, 확장 동작 참조를 정형화할 필요가 있다. 구체 선언 목록과 문법은 기술 설계 대상이다. 이 문서는 **그것을 개발자와 AI가 무엇으로 쓰는가**를 정한다.

창시자 원문이 요구하는 것은 두 가지다.

1. 개발자는 JS/Python 프로젝트·패키지·확장 생태계 안에서 자연스럽게 작업한다(10-02 "새로운 프로그래밍 언어가 아니다, JS/Python 생태계", 10-03 "JS/Python 표준 인터페이스").
2. 서버 정의는 AIP 고유의 선언적이고 기계 검증 가능한 한 구조로 쓴다(10-02 "풍부하고 정형화된 문법", 10-03 "선언 블록 중심 DSL 선호", "고유 문법을 제거하지 마라").

둘 중 하나를 버리는 문제가 아니라, **둘을 함께 만족하는 개발 경험이 무엇인가**를 고르는 문제다. 10-03 원문은 고유 문법이 "독립 소스 파일인지, JS/Python 안의 인터페이스인지, 둘 다인지" 재검토하라고 했고, 조건문·반복문·함수·표현식 지원도 논의했다고 적는다. 현재 PoC 문법에도 함수(`fn`)와 `if` 식이 있다(`spec/grammar.md` 47·257행).

---

## B. 실제 개발 시나리오: 같은 내용을 네 가지로 쓰기

### B.0 비교용 내용 (모든 예시가 이 항목을 빠짐없이 담는다)

아리아리 모집(Recruitment)을 DD-01·DD-02 합의안 모델로 쓴다.

| # | 항목 |
|---|---|
| 1 | 필드: 동아리(삭제 시 함께 삭제), 제목(1~100자), 기간, 상태(기본 PENDING_APPROVAL), 조기 마감 여부, 조회수(기본 0), 운영진 메모, 생성 시각 기록 |
| 2 | 제약: 같은 동아리 안에서 기간 겹침 금지(오류 `RECRUITMENT_PERIOD_OVERLAP`) |
| 3 | 판정식: `open` = 게시 상태 && 조기 마감 아님 && 지금이 기간 안 |
| 4 | 행 정책(읽기): 교외 모집이거나 내 학교 모집 |
| 5 | 읽기 preset `publicList`: select 제목·기간·조회수, filter 기간, sort 기간·생성 시각 |
| 6 | 공개 집계: 모집별 북마크 총수 |
| 7 | 쓰기 전이 `close`: PUBLISHED → CLOSED, 동아리 MANAGER 이상만 |
| 8 | 불변식: 동아리마다 PUBLISHED 상태 모집은 최대 1개 (예시용 가정 규칙. 아리아리 실제 규칙 아님) |
| 9 | 확장 읽기 참조: `recruitmentStats` (JS/Python으로 구현한 통계, 타입 있는 기호 참조) |
| 10 | 필드 정책: 운영진 메모는 그 동아리 MANAGER 이상만 읽을 수 있다 |
| 11 | 간선 정책: 북마크 행(누가 북마크했나)은 본인 것만 보인다(공개 집계 6과 별개) |

### B.1 A: 독립 `.aip` 파일 (`개념 예시`. 지난 PoC 문법을 DD-01·02 모델로 확장했다고 가정)

```text
// recruitment.aip
resource Recruitment {
  track created
  club: Club on delete cascade
  title: Text(1..100)
  period: Range<Time>
  status: RecruitmentStatus = PENDING_APPROVAL
  earlyClosed: Bool = false
  views: Int = 0
  internalNote: Text?  visible when actor is MANAGER+ of club
  bookmarks: RecruitmentBookmark[] via recruitment

  no overlap (period) per club else RECRUITMENT_PERIOD_OVERLAP
  predicate open = status = PUBLISHED and not earlyClosed and now in period
  rows when club.school = null or actor.school = club.school

  preset publicList { select title, period, views  filter period  sort period, created }
  public aggregate bookmarkCount = count(bookmarks)

  transition close: status PUBLISHED -> CLOSED
    allow when actor is MANAGER+ of club
  invariant at most one where status = PUBLISHED per club

  extension read recruitmentStats from "./ext/recruitmentStats.ts"
}
resource RecruitmentBookmark {
  recruitment: Recruitment on delete cascade
  member: Member
  rows when member = actor
}
```

### B.2 B-run: TS 정형 API, 모듈을 실행해 정의를 얻음 (`개념 예시`)

```ts
// recruitment.def.ts
import { resource, f, p } from "@aip/define"
import { recruitmentStats } from "./ext/recruitmentStats"

export const Recruitment = resource("Recruitment", {
  track: ["created"],
  fields: {
    club: f.ref("Club", { onDelete: "cascade" }),
    title: f.text({ min: 1, max: 100 }),
    period: f.timeRange(),
    status: f.enum(RecruitmentStatus, { default: "PENDING_APPROVAL" }),
    earlyClosed: f.bool({ default: false }),
    views: f.int({ default: 0 }),
    internalNote: f.text({ optional: true, visible: r => p.actor.roleIn(r.club).atLeast("MANAGER") }),
    bookmarks: f.many("RecruitmentBookmark", { via: "recruitment" }),
  },
  constraints: [f.noOverlap("period", { per: "club", error: "RECRUITMENT_PERIOD_OVERLAP" })],
  predicates: { open: r => r.status.eq("PUBLISHED").and(r.earlyClosed.not()).and(p.now.in(r.period)) },
  rows: r => r.club.school.isNull().or(p.actor.school.eq(r.club.school)),
  preset: { publicList: { select: ["title", "period", "views"], filter: ["period"], sort: ["period", "created"] } },
  publicAggregate: { bookmarkCount: r => r.bookmarks.count() },
  transitions: { close: { field: "status", from: "PUBLISHED", to: "CLOSED", allow: r => p.actor.roleIn(r.club).atLeast("MANAGER") } },
  invariants: [f.atMostOne(r => r.status.eq("PUBLISHED"), { per: "club" })],
  extensions: { read: { recruitmentStats } },          // 함수 import. 빌드가 기호 참조로 바꿔 IR에는 함수가 들어가지 않는다(C.3-5)
})
export const RecruitmentBookmark = resource("RecruitmentBookmark", {
  fields: { recruitment: f.ref("Recruitment", { onDelete: "cascade" }), member: f.ref("Member") },
  rows: b => b.member.eq(p.actor),
})
```

### B.3 B-data: TS/Python 정형 API, 모듈을 실행하지 않고 정적으로 읽음 (`개념 예시`)

```ts
// recruitment.def.ts  — 리터럴과 AIP 생성자만 허용. callback·임의 import 금지
import { resource } from "@aip/define"

export const Recruitment = resource("Recruitment", {
  track: ["created"],
  fields: {
    club: { ref: "Club", onDelete: "cascade" },
    title: { text: [1, 100] },
    period: "Range<Time>",
    status: { enum: "RecruitmentStatus", default: "PENDING_APPROVAL" },
    earlyClosed: { bool: true, default: false },
    views: { int: true, default: 0 },
    internalNote: { text: true, optional: true, visible: "actor is MANAGER+ of club" },
    bookmarks: { many: "RecruitmentBookmark", via: "recruitment" },
  },
  constraints: [{ noOverlap: "period", per: "club", error: "RECRUITMENT_PERIOD_OVERLAP" }],
  predicates: { open: "status = PUBLISHED and not earlyClosed and now in period" },   // 식은 AIP 식 문자열
  rows: "club.school = null or actor.school = club.school",
  preset: { publicList: { select: ["title", "period", "views"], filter: ["period"], sort: ["period", "created"] } },
  publicAggregate: { bookmarkCount: "count(bookmarks)" },
  transitions: { close: { field: "status", from: "PUBLISHED", to: "CLOSED", allow: "actor is MANAGER+ of club" } },
  invariants: [{ atMostOne: "status = PUBLISHED", per: "club" }],
  extensions: { read: { recruitmentStats: { module: "./ext/recruitmentStats", export: "recruitmentStats" } } },  // 기호 참조
})
export const RecruitmentBookmark = resource("RecruitmentBookmark", {
  fields: { recruitment: { ref: "Recruitment", onDelete: "cascade" }, member: { ref: "Member" } },
  rows: "member = actor",
})
```

Python도 같은 모양(딕셔너리 리터럴)으로 쓴다. 식(조건)은 결국 AIP 식 문법 문자열이 된다는 점에 주목: B-data에서도 **조건식 부분은 고유 문법**이다.

### B.4 E: TS/Python 프로젝트 안에 쓰는 AIP 선언 블록 (`개념 예시`)

```ts
// recruitment.def.ts  — 빌드 도구가 모듈을 실행하지 않고 블록만 추출해 파서로 처리
import { aip } from "@aip/define"
import type { recruitmentStats } from "./ext/recruitmentStats"   // 타입 확인용

export default aip`
resource Recruitment {
  track created
  club: Club on delete cascade
  title: Text(1..100)
  period: Range<Time>
  status: RecruitmentStatus = PENDING_APPROVAL
  earlyClosed: Bool = false
  views: Int = 0
  internalNote: Text?  visible when actor is MANAGER+ of club
  bookmarks: RecruitmentBookmark[] via recruitment

  no overlap (period) per club else RECRUITMENT_PERIOD_OVERLAP
  predicate open = status = PUBLISHED and not earlyClosed and now in period
  rows when club.school = null or actor.school = club.school

  preset publicList { select title, period, views  filter period  sort period, created }
  public aggregate bookmarkCount = count(bookmarks)

  transition close: status PUBLISHED -> CLOSED
    allow when actor is MANAGER+ of club
  invariant at most one where status = PUBLISHED per club

  extension read recruitmentStats
}
resource RecruitmentBookmark {
  recruitment: Recruitment on delete cascade
  member: Member
  rows when member = actor
}
`
```

Python 프로젝트라면 같은 블록을 이렇게 둔다(`개념 예시`):

```py
# recruitment_def.py — 빌드 도구가 모듈을 실행하지 않고 문자열 블록만 추출
from aip.define import aip

RECRUITMENT = aip("""
resource Recruitment {
  ...  (위 TS 블록과 같은 내용)
}
""")
```

블록 안은 A와 같은 문법이다. 파일은 TS/Python 프로젝트 안에 있고 각 패키지 생태계(npm, pip)로 배포되며, 확장 코드는 같은 프로젝트의 확장 파일이다. 블록 내부는 호스트 언어의 타입 검사가 보지 못하므로 AIP 진단과 에디터 지원이 따로 필요하다.

### B.5 관찰

- B-run은 조건식을 빌더(`r.status.eq(...)`)로 쓰는 대신 TS 타입 검사를 받는다. B-data와 E는 조건식이 AIP 식 문법이다.
- A와 E는 **같은 문법**이다. 차이는 파일이 독립 `.aip`인지, TS/Python 프로젝트 안의 블록인지다.
- 모든 예시가 B.0의 11개 항목을 담았다(B.3 Python 예시는 같은 모양이라 생략). 줄 수만 보면 A·E가 가장 짧다. 그러나 줄 수는 보조 지표다(DD-01 E2와 같은 원칙).

---

## C. 대안과 트레이드오프

### C.1 대안

| | A 독립 `.aip` | B-run TS/Python API(실행) | B-data TS/Python API(실행 안 함) | E 프로젝트 안 AIP 선언 블록 | C2 여러 공식 작성 방식(평가 보류) |
|---|---|---|---|---|---|
| 쓰는 문법 | AIP 문법 | 호스트 언어 + 빌더 | 호스트 리터럴 + AIP 식 문자열 | AIP 문법 | 위 중 여럿 |
| 파일 위치 | 별도 `.aip` | 호스트 프로젝트 | 호스트 프로젝트 | 호스트 프로젝트 | 언어별 |
| 원문 PART VII §14 | A | B | B의 안전한 하위안 | 원문에 없는 조합(창시자 인계 §5 "JS/Python 내부 API 형태" 쪽 후보) | C |

C2(여러 방식 동시 공식 지원)는 공통 IR의 이점 대비 프런트엔드·문서·진단·테스트가 방식 수만큼 늘어나는 비용이 커서, 하나를 먼저 정한 뒤 후속 단계에서 다시 평가한다. 아래 C.2 표의 C2 열은 참고용이다.

이 문서의 A·B-run·B-data·E·C2는 비교용 약칭이다.

**정규 리뷰 표기는 작성 방식과 다른 축이다.** 리뷰 표기는 정본으로 되돌아가지 않는(왕복하지 않는) 파생 출력으로 둔다 `[추천]`. 어떤 작성 방식이든 Core IR에서 사람이 읽기 좋은 표기를 출력해 리뷰·diff에 쓸 수 있다. 이것을 "고유 문법을 남긴 것"으로 셀지는 F2에서 묻는다. 초안 v1의 C1(TS로 쓰고 `.aip`는 읽기 표기)은 **B-run + 리뷰 표기**이고 원문의 C가 아니다.

### C.2 비교 (원문 PART VII §13의 8개 축)

| 축 | A | B-run | B-data | E | C2 |
|---|---|---|---|---|---|
| 철학 정합성 | 고유 선언 문법 충족. JS/Python 생태계와는 파일 체계가 분리 | 생태계 충족. 고유 문법을 쓰는 형식은 없음 | 생태계 충족. 조건식만 고유 문법 | 두 요구를 가장 직접 함께 시도 | 둘 다 충족하나 쓰는 방법이 여럿 |
| 정의 평가 보안 | 실행 코드 아님 | 모듈 실행 시 파일·환경·네트워크 접근 가능. 신뢰 모델 필수(C.3) | 실행 안 함 | 실행 안 함(블록만 추출) | 방식별 |
| 비즈니스 규칙 처리 | 문법이 허용하는 범위. 표준 밖은 확장 | 호스트 언어 표현력, 그러나 정적 분석 어려움 | 리터럴 범위로 제한 | A와 같음 | 방식별 |
| 구현 난이도 | 기존 파서 재사용 가능하나 DD-01·02 capability·전이·불변식 문법 확장 필요. 포매터(`aip fmt`는 명세에만 있고 CLI에 없음)·언어 서버 신규 | TS·Python 각각 프런트엔드(이름·타입 해석, 소스 위치) + 평가 격리 | TS·Python 정적 추출기 각각 + 식 문자열 파서(기존 재사용) | 블록 추출기(TS·Python) + 기존 파서 + 에디터 지원(블록 안 진단) | 가장 큼 |
| 기존 자산 | `aip-syntax` 파서, 문법 | Core IR 검증·분석·백엔드는 재사용 경계가 코드에 있음(`aip-ir/src/lib.rs` 1~22행, `validate.rs` 1~4행). 이름·타입 해석은 새로 | 같음 + 식 파서 | 파서·문법 + Core IR 이하 | 전부 |
| 확장 연결 | 경로 문자열로 참조. 존재·서명 검사는 빌드 | 함수를 import하며 빌드가 기호 참조로 변환. 모듈 평가 신뢰 경계 필요 | 기호 참조 | 기호 참조 + TS 타입 import | 방식별 |
| 디버깅 | 소스 위치 정확 | 빌더 호출 위치 | 리터럴 위치 | 블록 안 위치를 호스트 파일 위치로 매핑 필요 | 방식별 |
| AI 개발 경험 | 처음 보는 문법. 대신 쓸 수 있는 모양이 적음 | 익숙한 언어. 쓸 수 있는 모양이 많아 린트로 좁혀야 | 리터럴이라 모양이 좁음. 식 문자열은 처음 보는 문법 | A와 같은 문법을 익숙한 프로젝트 안에서 | 언어 선택이 하나 더 |
| 사람 검토 | 짧음 | 길고 빌더 섞임 | 중간 | 짧음 | 방식별 |

"AI가 TS를 이미 잘 쓴다", "낯선 문법은 AI가 틀린다"는 둘 다 **확인 필요**다. 새 `@aip/define` API의 정책·불변식 의미는 일반 TS 숙련과 다르다. E2로 잰다.

### C.3 정의를 읽는 시점의 신뢰 경계 `[추천]` (창시자 질문 아님)

"정의 파일의 결과가 선언 데이터뿐이면 안전하다"는 틀렸다. TS/Python 모듈은 불러오는 순간 파일·환경 변수·네트워크에 접근하고 프로세스를 실행할 수 있다. 결과가 데이터인 것과 평가 중 부작용이 없는 것은 별개다. 후보:

| 신뢰 모델 | 내용 | 맞는 대안 |
|---|---|---|
| 정적 추출 | 허용된 구문만 읽고 모듈은 실행하지 않는다 | A, B-data, E |
| 격리 평가 | 별도 프로세스에서 네트워크·파일·환경·자식 프로세스를 막고 시간·메모리 제한(현실성과 탈출 위험은 확인 필요) | B-run |
| 신뢰된 빌드 코드 | 정의를 빌드 스크립트와 같은 신뢰 코드로 보고 CI 권한·공급망 위험을 비용으로 기록 | B-run |

어느 모델이든 공통 조건:
1. 운영 서버는 정의 모듈을 실행하지 않고, 검증되고 버전이 고정된 Core IR 산출물만 받는다.
2. 같은 입력은 시간·환경·난수와 무관하게 같은 IR을 만든다(결정성).
3. 정의 변경의 보안 영향(capability·정책·불변식 확대)은 소스 diff가 아니라 **Core IR 의미 diff**로 사람에게 보인다.
4. 오류는 작성한 파일의 위치를 가리킨다.
5. 확장 동작은 타입 있는 기호 참조로 연결되고 없거나 서명이 맞지 않으면 빌드 오류. 정의에서 함수를 직접 import하더라도(B-run) 빌드가 기호 참조로 바꾸고 구현은 별도 확장 묶음(bundle)으로 결합한다. **Core IR에는 함수 객체가 들어가지 않는다.**
6. 자연어의 문법화(DD-05)가 들어갈 자리를 막지 않는다.

---

## D. 최초 철학과의 관계

| 원칙 | A | B-run | B-data | E |
|---|---|---|---|---|
| JS/Python 생태계 | 확장만 | 충족 | 충족 | 프로젝트·배포·확장은 충족, 정의 문법은 AIP |
| 고유 선언 문법(쓰는 형식) | 충족 | 없음 | 조건식만 | 충족 |
| 한 의도 한 구조 | 문법이 강제 | 린트로 좁혀야 | 리터럴이라 좁음 | 문법이 강제 |
| 수단과 목적(PART V) | 언어 도구가 수단으로 커질 위험 | 두 언어 프런트엔드·격리 | 두 언어 추출기 | 추출기 + 에디터 지원 |

E와 A는 별도 파서·블록 안 진단·에디터 지원과 AIP 문법 학습 비용을 요구한다. 이것이 "새 프로그래밍 언어"에 해당하는지라는 이름 문제보다, 그 비용을 감수하고 이 작성 경험을 원하는지를 창시자가 판단한다(F1).

---

## E. 검증 방법 (코드 변경 없음, 에이전트가 수행)

| # | 실험 | 확인하는 것 |
|---|---|---|
| E1 | 아리아리 리소스 5개(Club, ClubMember, Recruitment, RecruitmentBookmark, MemberAlarm)를 B.0 체크리스트에 **이후 문서의 선언 종류**(쓰기 capability 6종·bulk·`audited`(DD-02), 열람 지점(DD-09), 소유 컬럼·viewer 값(DD-10), `was`·`removed`(DD-12))를 더해 A·B-run·B-data·E 각각 작성 | 작성·검토 부담. 줄 수는 보조 |
| E2 | 같은 요구 문서·같은 참고 자료로 AI 세션 여러 개(모델·버전·프롬프트·샘플 수 기록)에 각 형식으로 쓰게 하고 측정: 첫 시도 검증 통과율, 체크리스트 대비 누락·과잉(특히 과다 공개 capability), DD-01 I1~I7·DD-02 W-I1~W-I11 위반 수, 진단을 받고 고치는 반복 횟수, 정규화 결과 동일성, 사람이 오류를 찾는 시간 | 형식별 AI 작성 정확도(지표 S3) |
| E3 | 재사용 경계를 코드로 나눈다: `aip-syntax` AST·이름·타입 해석(새 프런트엔드가 채워야 할 부분) / `aip-ir` 검증·분석·백엔드(재사용) | 새 프런트엔드가 메워야 할 간극 |
| E4 | Core IR → 사람이 읽는 표기 출력의 의미 보존·안정된 diff·소스 위치 연결. 설계 검토만으로는 확인할 수 없어, 작은 출력기 시제품(승인 후 `spikes/`)이 필요하다. 그 전까지 리뷰 표기 비용은 검증 못함 | 리뷰 표기 비용 |

이전 비교 이력: 창시자의 과거 A형·B형 예시는 저장소에서 확인되지 않았다. Q3 자료 대기는 해제됐으며, 기술 비교는 확인된 선언형 선호를 기준으로 대표 사례를 구성해 진행한다.

---

## F. 확인된 답과 남은 일

- 확인됨: 정형화된 AIP 고유 선언 체계를 지향하고, JS/TS 및 Python 생태계와 자연스럽게 연결한다. 고유 문법 자체를 없앨지 묻는 기존 F1은 재질문하지 않는다.
- 기술 검증: 독립 `.aip` 파일과 호스트 프로젝트 내 선언을 비교한다. 독립 파일 여부 및 호스트 임베딩은 미정이며 양자택일로 미리 고정하지 않는다. A형 선호는 확인됐으나 과거 예시는 확정 문법이 아니다(Q3).
- 추가 가치 판단: TS·Python의 첫 출시 동등성은 비교 가능한 구현 비용·사용성 자료를 낸 뒤 판단한다. 중립적인 문법 사례와 에디터 진단 검증은 에이전트가 우선 수행한다.

---

## G. 결정 상태

**검증 필요.** 정형 선언형 방향과 양 생태계 지원은 확인됐다. 파일·호스트 형식 및 초기 지원 동등성은 비교가 남았다. 기존 E 우선 추천은 창시자 승인으로 보지 않는다.

---

## 이탈 방지 점검 (E 기준)

| 질문 | 답 |
|---|---|
| 백엔드 개발이나 이중 유지보수를 줄이는가 | 직접 관계는 적다. 쓰는 문법이 하나라 정의와 리뷰 표기가 같다. 도구(추출기, 에디터 지원) 유지비는 E1·E3로 확인 |
| 호출자 표현에 도움이 되는가 | 직접 관계 적음. 서버 정의와 호출자 요청의 타입 연결은 T06·코드 생성이 정해져야 판단 |
| 서버가 최종 권한을 통제하는가 | 정의는 실행하지 않고 추출하므로 정의 평가 단계의 부작용이 없다. 운영은 검증된 IR만 받는다(C.3) |
| AI의 불필요한 선택을 줄이는가 | 문법이 모양을 강제한다. 낯선 문법의 정확도는 E2로 확인 |
| JS/Python 생태계와 사람의 사용성 | 프로젝트·배포·확장은 생태계 안. 블록 안 에디터 지원이 없으면 사용성이 떨어진다 |
| 복잡성을 옮기기만 했나 | 언어 서버 대신 블록 추출기와 블록 안 진단. 순감소는 미입증 |

## 변경 이력

- 2026-10-03 창시자 통합 지침 및 B §2·§5 대조: 고유 문법 제거 여부를 재질문하지 않고 독립 파일·임베드 형식의 검증으로 정리.
- 2026-10-03 초안 v1(추천 C1)
- 2026-10-03 v2: Codex 1라운드 반영. "루프·변수 없는 선언 언어" 전제 삭제(현재 문법에 `fn`·`if` 식 있음, 원문도 제어 흐름 논의), 문제를 "둘 중 무엇을 버리나"에서 "둘을 함께 만족하는 경험"으로, B.0 체크리스트로 모든 예시를 같은 내용으로, 대안 B-run·B-data·E 분리, C1을 "B-run + 리뷰 표기"로 재분류하고 추천 철회, 정의 평가 신뢰 경계(C.3), 기존 자산 평가 정정(포매터 없음, 리뷰 표기 "쉬움" 근거 없음), E2를 정확도 지표로, F를 쓰는 경험·쓰는 형식 여부·Python 범위로, A형·B형 원본은 자료 요청으로
- 2026-10-03 v3: Codex 2라운드 반영(미합의 3개). B.0에 필드 정책·간선 정책·조회수 필드 추가(11항목)하고 네 예시 모두 반영, Python 선언 블록 예시 추가, 확장 참조는 직접 import해도 빌드가 기호 참조로 바꾸고 IR에 함수 없음, C2는 평가 보류, 리뷰 표기는 왕복하지 않는 파생 출력, E4는 시제품 필요로 검증 못함, "새 언어인지" 이름 질문을 비용 질문으로, F를 F1(고유 문법을 쓰는 형식으로 남기나) → F2(그에 맞는 작성 위치·모양) → F3(Python 범위) 순서로
- 2026-10-03 v3.1: Codex 3라운드 합의 확인. 선택적 다듬기 3건 반영(C.2 B-run 확장 참조 문구, B.4 설명을 TS/Python으로, F2 신뢰 모델 문구)
- 2026-10-03 교차 검토(`../reviews/CROSS-sonnet-r1.md`) 반영: E1 체크리스트에 DD-09~12 선언 종류 추가
