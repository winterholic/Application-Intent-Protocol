# C. 서버 정의와 프론트 표현의 문법 고도화 제안

> 상태: 검증 필요. 아래 신규 문법·패키지명·CLI 옵션은 전부 **개념 예시·미구현**이다. 현재 PoC에서 실행 가능한 코드로 취급하지 않는다.
> 기준: [통합 지침](../sources/founder-integrated-directive-2026-10-03.md) §2~7·9·13. 목적·서버/SDK 구조는 창시자 기준, 정확한 키워드·공통 의미 모델은 제안이다.

## A. 지금 설계할 문제

서버는 화면별 응답을 미리 구현하는 대신 데이터·정책·공개 연산을 정의한다. 프론트는 그 안에서 원하는 결과와 동작을 표현한다. 문법 비교는 파일 확장자가 아니라 **같은 계약을 빠짐없이 표현하고 수정할 수 있는가**로 한다.

선언형 A형을 우선 평가한다. 서버 정의 형식과 프론트 SDK 인터페이스는 별도 축이다. 서버를 TS로 정의한다고 프론트가 서버 정의 코드를 받거나 실행하지 않는다.

## B. 실제 개발 시나리오와 동등성 기준

아리아리에서 ‘마감 가까운 모집 카드에 동아리 이름·로고·북마크 총수를 표시’, ‘내 알림 읽음’, ‘지원자 승인’을 고른다. 기존 코드 근거와 검토 계층은 [D](D-development-experience.md) §1·2에 있다.

모집 읽기 정의의 비교 기준:

| ID | 세 작성 대안이 모두 담아야 할 의미 |
|---|---|
| EQ-01 | 제목·마감시각·상태·조회수·동아리 관계·내부 메모의 타입 |
| EQ-02 | 활성 모집이고 교외 또는 행위자 학교의 모집이라는 행 정책. 익명/학교 인증 규칙은 별도 검증 |
| EQ-03 | 선택 가능 필드와 filter·sort 연산의 독립 허용 목록 |
| EQ-04 | club 관계 접근 허용 + Club의 행/필드 정책 재평가 |
| EQ-05 | 내부 메모의 관리자 전용 가시성 |
| EQ-06 | 북마크 개인 행은 본인만; 총수는 별도 비식별 집계 capability |
| EQ-07 | 결과 행·관계 확장·시간·비용 제한의 위치 |
| EQ-08 | 공개 close 전이의 이전 상태·허용자·목표 상태 |
| EQ-09 | 동일 동아리의 게시 모집 개수 불변식과 DB 집행. 이 규칙은 비교용 가정이며 아리아리 원본 사실 아님 |
| EQ-10 | 공식 확장 읽기의 입력·출력·데이터 scope·효과·자원 계약 |
| EQ-11 | 선언에 귀속되는 선택적 설명, 공개 여부. 설명 삭제가 실행 의미를 바꾸지 않음 |

### B.1 A: 독립 선언 파일

`recruitment.aip` — **개념 예시**. `active`, `managerOf` 같은 기호는 서버 정의에서 타입과 의미를 선언하는 전제다. 예시의 시간/비용 값은 비교용 입력이며 기본값 제안이나 측정 결과가 아니다.

```text
resource Club {
  fields { id: Id; name: Text; logo: Url?; school: School? }
  rows read when school = null or school = actor.school
  expose read { select id, name, logo }
}

resource RecruitmentBookmark {
  fields { recruitment: Recruitment; member: Member }
  rows read when member = actor
}

resource Recruitment {
  fields {
    id: Id
    title: Text(1..100)
    periodEnd: Time
    status: RecruitmentStatus
    views: Int
    club: Club
    internalNote: Text?
  }
  rows read when active(this) and
    (club.school = null or club.school = actor.school)
  field internalNote read when managerOf(actor, club)
  expose read {
    select id, title, periodEnd, views, bookmarkCount, internalNote
    filter periodEnd.gte, periodEnd.lte
    sort periodEnd, views, id
    traverse club { select id, name, logo }
    budget { rows 50; depth 2; deadline 2s; cost 1000 }
  }
  aggregate bookmarkCount: Int {
    source RecruitmentBookmark
    sourceAccess fixedTotalOfVisibleRecruitment
    groupKey recruitment
    callerFilter none
    rowOutput none
    release count
  }
  transition close {
    from status = PUBLISHED
    to status = CLOSED
    allow managerOf(actor, club)
  }
  invariant atMostOnePublished per club
  extension read stats {
    input { clubId: Club.Id }
    output { approvedApplicants: Int }
    access Apply.approvedCount
    effect none
    deadline 2s
    implementation "recruitment.stats"
  }
  docs { summary "모집 정보"; visibility internal }
}

resource Apply {
  fields { id: Id; recruitment: Recruitment; status: ApplyStatus }
  rows read when managerOf(actor, recruitment.club)
  aggregate approvedCount: Int {
    input { clubId: Club.Id }
    sourceAccess clubManagerOnly(actor, input.clubId)
    where recruitment.club.id = input.clubId and status = APPROVE
    callerFilter none
    rowOutput none
    release count
  }
  expose aggregate approvedCount
}
```

집계는 개인 행 read와 다른 서버 승인 접근 scope를 명시한다. `fixedTotalOfVisibleRecruitment`는 호출자가 볼 수 있는 모집별로 북마크 전체의 count만 산출하는 고정 scope 후보이며, 개인 행 read 정책을 그대로 쓰면 ‘내 북마크 수’가 되는 것과 구분한다. 임의 회원 조건·회원 식별자·분해 집계·원본 행 반환은 열지 않는다. 이런 별도 접근은 원본 read 정책의 자동 파생 권한이 아니다. 소스 접근과 결과 공개의 안전성은 SY-3에서 검증한다.

`Apply.approvedCount`는 지정 동아리의 관리자인 서버 actor만 접근하는 집계다. `stats`의 scope는 이 계약과 교차하며 동아리 ID만 바꿔 다른 동아리 집계를 읽지 못해야 한다. 내부 메모는 select 목록에 있지만 추가 필드 정책을 만족하는 관리자만 사용할 수 있다. 공개 타입에 항상 실린다는 뜻은 아니며 역할별 계약 투영과 존재 비노출은 별도 검증한다.

불변식의 실행식과 잠금 키는 생략한 채 구현됐다고 주장하지 않는다. EQ-09는 세 대안에서 같은 정의를 참조하고, 정의 누락이면 검증이 실패해야 한다. 행 정책의 `active`도 자유로운 자연어가 아니라 타입 검사 가능한 서버 기호다.

### B.2 E: 호스트 프로젝트 안 AIP 선언 블록

**개념 예시.** B.1의 블록 전체를 그대로 사용한다. 다른 문법을 만들지 않고 포장 위치만 바꾼다.

```ts
import { aip } from "@aip/define"
export default aip`
  // B.1의 resource Club, RecruitmentBookmark, Recruitment, Apply 전체 블록
`
```

```py
from aip.define import aip
SPEC = aip("""B.1의 네 resource 전체 블록""")
```

위 두 코드는 파일 배치 설명용이다. 줄임 문구를 파싱 가능한 예제로 계산하지 않는다. 동등성 실험에서는 생략 없는 B.1 블록을 넣어 EQ-01~11을 비교한다.

추출기는 TS 모듈/Python 모듈을 실행하지 않고 지정된 리터럴만 읽는 후보다. 보간·동적 문자열 결합·임의 import 실행을 정의 경로에 허용하지 않는다. 패키지 내부 확장 파일의 실제 실행은 별도 worker 계약으로 다룬다.

비용: npm/pip 프로젝트와 함께 배포할 수 있지만 블록 내부 자동완성·source map·진단 연결을 AIP가 제공해야 한다. ‘TS 파일 안’이라는 사실만으로 TS가 블록의 타입을 검사하지 않는다.

### B.3 H: 호스트 언어의 정형 데이터 선언

**개념 예시.** 고정된 생성자와 리터럴만 받으며 policy 식은 A와 같은 정형 식 문법으로 제한하는 후보다. B-run의 임의 callback 실행과 다르다.

```ts
export const spec = define({
  resources: {
    Club: {
      fields: { id: "Id", name: "Text", logo: "Url?", school: "School?" },
      rows: "school = null or school = actor.school",
      read: { select: ["id", "name", "logo"] },
    },
    RecruitmentBookmark: {
      fields: { recruitment: "Recruitment", member: "Member" },
      rows: "member = actor",
    },
    Recruitment: {
      fields: {
        id: "Id", title: "Text(1..100)", periodEnd: "Time",
        status: "RecruitmentStatus", views: "Int", club: "Club",
        internalNote: "Text?",
      },
      rows: "active(this) and (club.school = null or club.school = actor.school)",
      fieldRead: { internalNote: "managerOf(actor, club)" },
      read: {
        select: ["id", "title", "periodEnd", "views", "bookmarkCount", "internalNote"],
        filter: ["periodEnd.gte", "periodEnd.lte"],
        sort: ["periodEnd", "views", "id"],
        traverse: { club: { select: ["id", "name", "logo"] } },
        budget: { rows: 50, depth: 2, deadline: "2s", cost: 1000 },
      },
      aggregates: {
        bookmarkCount: {
          type: "Int", source: "RecruitmentBookmark",
          sourceAccess: "fixedTotalOfVisibleRecruitment", groupKey: "recruitment",
          callerFilter: "none", rowOutput: "none", release: "count",
        },
      },
      transitions: {
        close: { from: "status = PUBLISHED", to: "status = CLOSED", allow: "managerOf(actor, club)" },
      },
      invariants: ["atMostOnePublished per club"],
      extensions: {
        stats: {
          kind: "read", input: { clubId: "Club.Id" }, output: { approvedApplicants: "Int" },
          access: ["Apply.approvedCount"], effect: "none",
          deadline: "2s", implementation: "recruitment.stats",
        },
      },
      docs: { summary: "모집 정보", visibility: "internal" },
    },
    Apply: {
      fields: { id: "Id", recruitment: "Recruitment", status: "ApplyStatus" },
      rows: "managerOf(actor, recruitment.club)",
      aggregates: {
        approvedCount: {
          type: "Int", input: { clubId: "Club.Id" },
          sourceAccess: "clubManagerOnly(actor, input.clubId)",
          where: "recruitment.club.id = input.clubId and status = APPROVE",
          callerFilter: "none", rowOutput: "none", release: "count",
        },
      },
      exposeAggregate: ["approvedCount"],
    },
  },
})
```

Python은 같은 필드 구조의 딕셔너리로 비교할 수 있다. 표현·의미의 완전 동등 지원 비용은 미측정이다. 일반 객체처럼 보이지만 리터럴 부분집합을 정적으로 추출하면 지원 문법·오류가 별도로 필요하다. 반대로 일반 호스트 코드를 실행해 정의를 얻으면 빌드 시 임의 코드 실행 경계가 생긴다.

### B.4 프론트 SDK: 화면이 원하는 읽기

세 서버 작성 대안 모두 아래 같은 Presentation 후보를 사용한다. **개념 예시·미구현**. 공개 계약에서 생성한 리소스 참조를 쓰며 내부 정책 코드·DB 구조를 배포하지 않는다.

```ts
import { createClient } from "@aip/client"
import { Recruitment, MemberAlarm, Apply } from "./generated/aip"

const aip = createClient({ endpoint: "/aip" })
const cards = await aip.read(Recruitment, {
  where: { periodEnd: { gte: cutoff } },
  orderBy: [{ periodEnd: "asc" }, { id: "asc" }],
  page: { first: 5 },
  select: {
    title: true,
    periodEnd: true,
    club: { select: { name: true, logo: true } },
    bookmarkCount: true,
  },
})
```

SDK 역할: select 결과 타입 추론·요청 트리 작성·별도 값 바인딩·계약 버전·응답/오류 처리. 서버 역할: actor 결정·타입 검증·연산별 허용·모든 관계 정책·집계 계약·비용/시간/결과 크기·실행. 클라이언트의 actor/테넌트 주장으로 신원을 정하지 않는다.

타입 생성은 공개 상한을 알려 주는 개발 지원이다. 권한 변경·탈퇴·토큰 만료 후에도 예전 생성 타입으로 호출할 수 있으므로 매 요청 서버 검증을 유지한다. 비용 오류 발생 시 SDK가 몰래 ‘이름 호출’로 돌아가거나 결과를 잘라 성공 처리하지 않는다.

### B.5 표준 쓰기: 내 알림 읽음

**개념 예시·미구현**. 실제 아리아리 서비스의 소유자 확인과 미읽음→읽음 의미를 정형화한다.

```text
resource MemberAlarm {
  fields { id: Id; member: Member; isChecked: Bool }
  rows read when member = actor
  transition read {
    allow member = actor
    from isChecked = false
    to isChecked = true
    repeat unchanged
  }
  expose apply read { target id; bulk maxRows 500 }
}
```

```ts
await aip.apply(MemberAlarm, "read", { target: { id: alarmId } })
await aip.apply(MemberAlarm, "read", {
  target: { where: { isChecked: false } },
  mode: "atomic",
})
```

target는 서버 write 정책과 교차한다. read 가시성이 곧 write 권한이 아니다. ‘모두’가 상한을 넘으면 명시 오류 또는 별도 batch 절차로 처리한다. 일부만 바꾸고 전체 성공으로 응답하는 처리는 금지한다. 두 화면용 서버 command를 따로 만들지 않는 후보이며 transition 계약은 한 번 필요하다.

### B.6 업무 동작·조건·컬렉션 처리의 경계

아리아리 일괄 승인은 상태 변경·회원 생성·같은 동아리 검사·운영진 권한·중복 회원 금지·알림을 함께 요구한다. 단순 UPDATE로 대체하지 않는다.

| 후보 | 프론트 표현 | 서버에 필요한 것 | 제거할 반복과 새 비용 |
|---|---|---|---|
| W0 독립 연산 묶음 | 이미 단독 공개된 동작들을 atomic 묶음 | 각 연산 정책 + 동일 트랜잭션·충돌 처리 | 단순 묶음 래퍼 감소. 값 전달/조건/최종 상태만 유효한 변경은 못 덮음 |
| W1 범위 제한 표준 조합 | typed 단계·입력 참조·조건·컬렉션 매핑 | 조합 capability, 범위/단계/잠금 계산, commit 불변식, 정책·효과 검증 | 관계 생성/상태 전이의 반복 구현 감소 후보. 표현기·플래너 비용 증가 |
| W2 의미 있는 표준 동작/공식 확장 | 승인이라는 결과 + 대상 | 표준 approval 형식 또는 JS/TS·Python 확장 계약 | 업무 불변식 한 곳에서 관리. 모든 화면이 별도 확장을 쓰면 존재 목적 약화 |

W1의 개념 형태:

```text
atomic within Club {
  targets Apply(ids) bounded 100
  require same club
  map each item {
    require eligible(item)
    create ClubMember from item.member
    transition item approve
  }
  afterCommit notify results
}
```

이 표현을 받은 서버는 각 연산 공개 여부·중간 정보 노출·모든 대상 존재·최종 불변식·잠금 범위·부작용 시점을 판단해야 한다. 위 문법은 ‘승인에 필요한 정책을 호출자가 보내면 신뢰한다’는 뜻이 아니다. `eligible`은 서버 정의 기호이며 불변식은 호출자가 빼도 적용된다.

평가 방향: W0만으로 영구 제한하지 않고 W1을 실제 승인/관리자 위임/순환 전이로 검증한다. 순서·값 전달을 포함해도 서버가 안전하게 의미를 정할 수 있는지 확인한다. W1이 어렵다는 이유로 표준 기능 탐색을 포기하지 않고, 안전 근거를 갖추기 전 W1을 운영 가능하다고도 쓰지 않는다.

### B.7 공식 JS/TS·Python 확장

**개념 예시·미구현**. 복잡한 적격성 판단/외부 API 연동을 공식 확장으로 표현한다. 공통 조합을 복사한 서버 래퍼를 표준으로 권장하지 않는다.

```ts
export async function stats(input: StatsInput, ctx: ReadContext): Promise<StatsOutput> {
  const result = await ctx.data.aggregate("Apply", "approvedCount", {
    clubId: input.clubId,
  })
  return { approvedApplicants: result.value }
}
```

```py
async def stats(input: StatsInput, ctx: ReadContext) -> StatsOutput:
    result = await ctx.data.aggregate(
        "Apply", "approvedCount", {"clubId": input.clubId}
    )
    return StatsOutput(approvedApplicants=result.value)
```

후보 실행 구조: Rust 서버 + 제한된 API로 연결된 Node/Python worker. ctx는 서버가 묶은 actor·tenant·허용 access·기한을 참조한다. worker에게 전체 DB 자격증명을 주면 ctx 사용을 강제할 수 없으므로 ‘우회 불가’ 보장을 할 수 없다. 프로세스 분리만으로 네트워크·파일 접근 격리가 완성되는 것도 아니다.

read 확장은 출력 검증과 승인된 결과 범위가 필요하다. write 확장은 tx handle 수명·동일 커밋·취소·재시도·외부 효과 계약이 별도 필요하다. 두 확장 예시는 호출 인터페이스만 보여 주며 보안 격리 구현을 입증하지 않는다.

이 호출의 세 번째 인수는 승인된 named aggregate의 입력이다. Apply의 임의 필드 where를 전달하는 API가 아니다. 선언된 입력 clubId와 관리자 scope가 같은 방식으로 검증되는 후보이며, generic aggregate 접근으로 바꿔서 범위를 넓히지 않는다.

### B.8 선택적 설명과 별도 개발 검사

`docs.summary/rationale/visibility`는 후보 키다. 선언 경로·안정 식별자·source span을 붙여 소속을 보존하되 작성 자체는 선택적이다.

- docs가 없거나 바뀌어도 권한·SQL·업무 규칙은 같아야 한다.
- 제공된 docs 값의 타입/소속 검사와 자연어 내용의 참·거짓 판정은 구분한다.
- 공개 계약 출력은 내부 설명을 자동 포함하지 않는 후보다.
- 원문은 설명의 실행 계약화를 요구하지 않는다. ‘스텝 이상’ 문장의 코드 생성/모순 검사 기능을 기본 기능으로 승격하지 않는다.

개발 검사 후보: 정의 참조·타입·중복·미사용·권장 표준 대신 반복 확장 작성·지원하지 않는 설정 진단. 의무 보안 검사와 같은 알고리즘을 재사용할 수 있으나 optional 검사 실행 여부와 서버 안전성은 독립이다.

## C. 대안과 트레이드오프

| 축 | A 독립 선언 | E 프로젝트 안 선언 블록 | H 호스트 정형 데이터 |
|---|---|---|---|
| 정형성·선택 수 | 한 문법 | A와 같은 한 문법, 추출 규칙 추가 | 호스트+표현식 문법; 자유 host 코드 허용하면 다양성 증가 |
| JS/TS·Python 연결 | 패키지/worker/타입 생성 연결 필요 | 같은 프로젝트·패키지에서 관리 | 언어별 타입 도구 활용 후보. 부분집합 제한 비용 |
| 에디터·오류 | AIP 도구 필요 | 블록 내부 지원·source map 필요 | 호스트 오류와 AIP 의미 오류 위치 연결 필요 |
| 빌드 신뢰 | 파서 입력은 데이터 | 정적 추출이면 데이터. 동적 보간 위험 | 정적 추출이면 데이터. 모듈 실행 허용 시 별도 신뢰 경계 |
| 제거되는 반복 | 화면별 조회/단순 쓰기 구현 | A와 같음 | A와 같음. 형식만으로 더 많이 줄어든다고 주장 못함 |
| 운영 비용 | 공통 서버 비용 | A와 같고 추출/도구 비용 추가 | 정규화/언어별 유지보수 비용 |
| 확장·업무 규칙 | 공식 host 확장 필요 | 공식 host 확장 필요 | 정의가 host 파일이라는 것만으로 확장 계약 충족 아님 |

실험 추천: A와 E는 동일 의미 문법 후보로 먼저 비교하고 H를 대조군으로 둔다. 이는 독립 파일을 확정하거나 H를 기각한 결정이 아니다. 여러 방식을 동시에 공식 지원하는 것은 정확도·수정 비용 개선이 확인될 때만 검토한다.

공통 의미 모델의 최소 후보는 서버 계약 + typed 요청 트리 + 안정 기호 참조다. 현재 Core IR 재사용은 [A](A-founder-intent-matrix.md)와 [E](E-technical-risks.md)의 비교 대상이다. 모든 소스 언어를 포괄하는 거대한 IR을 먼저 만들 이유는 아직 입증되지 않았다.

## D. 최초 철학과의 관계

원칙 1·2: 열린 계약 안 화면 요구를 프론트에서 작성해 백엔드 반복을 줄인다. 원칙 3·7: Rust 서버와 JS/TS·Python worker를 연결한다. 원칙 4: 표현을 표준화하지만 W1 후보와 host 확장을 유지한다. 원칙 5: 설명은 선택적이고 실행과 독립이다. 원칙 6: 정의·화면 요구·실행을 구분하며 패키지를 SPR 이름만으로 고정하지 않는다.

복잡성은 표준 엔진·진단·SDK로 이동한다. 공통 기능으로 반복 비용을 상쇄하는지 [D](D-development-experience.md)의 측정으로 판정한다.

## E. 검증 방법

- SY-1: EQ-01~11의 생략 없는 A/E/H 정의를 같은 typed facts로 변환. 누락·동적 보간·임의 실행을 실패 사례로 사용.
- SY-2: 허용된 logo 선택/정렬 추가는 화면 요청만 바꾸고 서버 정의 digest 불변. 닫힌 필드이면 계약 변경을 명확히 기록.
- SY-3: filter/sort/traverse/aggregate를 select 권한으로 자동 허용하는 공격을 각각 거부.
- SY-4: W0/W1/W2를 같은 업무 시나리오로 비교. 동시 승인/자기 위임/누락 ID/최종 불변식/알림 실패 확인.
- SY-5: docs 누락·내용 변경으로 execution facts 불변, docs 소속은 round-trip/진단 출력에서 보존.
- SY-6: optional 검사 꺼도 서버 타입·권한·비용 오류 거부. 알 수 없는 옵션은 조용히 무시하지 않음.

SY-5·SY-6의 좁은 실행 근거는 [V11](V11-optional-checks-results.md)에 있다. resource docs의 소속과 선택적 조언 1종을 검증했으며 field/command 설명·전체 개발 검사·인간 의도 판정은 남는다.
- SY-7: source map·자동완성·Python 동등 표현·worker 기동 실패의 실제 DX 기록.

이 문서의 비교는 설계 분석이다. SY-1~7 실행 결과는 아직 없다. [E](E-technical-risks.md)에서 작은 구현 실험의 선후 관계를 정한다.

## F. 창시자에게 필요한 결정

현재 재질문하지 않는다. 실험 뒤 작성 위치의 개발 경험이 실질적으로 갈리는 경우와 출시 생태계 범위가 남으면 [B](B-decision-reclassification.md)의 08~10·24에 근거를 연결한다.

## G. 결정 상태

**검증 필요.** 원문에서 확인된 정형 선언·서버/SDK·확장·설명/검증 분리와 이 문서의 구체 문법/worker/조합 후보를 구분한다.

## 실험이 드러낸 문법 요구 (2026-10-04)

V1~V5 spike를 만들며 B.1 예시만으로는 실행 의미를 정할 수 없어 아래 선언을 더했다. 모두 실험 가정이며 문법 확정이 아니다. 근거는 각 결과 문서다.

| 더한 것 | 왜 필요했나 | 근거 |
|---|---|---|
| `enum`, `actor`, `predicate`, `access`, `limit` 선언 | `PUBLISHED`·`active`·`managerOf`·집계 scope·불변식 실행식을 타입 있는 기호로 해석 | [V1](V1-semantic-fixture-results.md) §3 |
| 행 정책 없는 resource는 전부 거부, budget 없는 expose는 관계 대상 전용 | 정책·비용 누락이 곧 노출이 되지 않게 | V1-R1·R2 |
| `expose apply <전이> { target id, where; bulk maxRows N }`, `repeat unchanged` | 단건·조건 대상, 상한, 재시도 의미 | [V3](V3-standard-write-results.md) §1 |
| 전이 효과 `create`·`update`·`notify`, `sameScope` | 승인(회원 생성·알림)과 위임(자기 행 갱신)을 서버 정의 하나로 | V3 §6·§7 |
| `expose create`, `check … when`(쓰기 행 사후조건), `unique`, `invariant … deferred` | W0 묶음의 안전성, 교대형 불변식(관리자 1명) | V3 §6·§7 |
| `expose compose`(허용 단계·값 출처) | W1을 안전하게 하려면 서버가 단계와 값 출처를 선언해야 했음. 현재 형태로는 위임을 표현하지 못함 | V3 §6.4·§7 |
| 확장 입출력의 정확한 키·타입, access의 입력 고정 | worker가 계약 밖 값을 읽거나 내보내지 않게 | [V4](V4-official-extension-results.md) |

드러난 의미 규칙 가운데 표현이 아니라 실행 계약인 것: 경로끼리 비교의 NULL은 허용으로 보지 않음(V2-R3), 정책이 읽는 다른 행은 판정·커밋 사이 공유 잠금(V3-R5·R8), 커밋 결과 미확정 응답(V3-R6), 권한 변경도 캐시 무효화 대상([V5](V5-sdk-results.md) §4).

## 이탈 방지 점검

화면별 서버 중복 제거는 이미 열린 capability라는 전제가 있다. 새 정책·불변식·업무 의미는 서버에서 정의한다. 프론트 요청은 실행 권한이 아니며 필수 검증은 Rust 서버 책임이다. 표준 제어 흐름 후보를 실제 업무로 시험하고 확장을 공식 지원한다. 추가 parser/추출기/SDK 비용은 실험에서 함께 센다.

## 변경 이력

- 2026-10-03 통합 지침 기준 서버 A/E/H, 프론트 read/apply, bounded composition, host 확장·optional metadata 제안.
- 2026-10-03 Luna high 독립 대조 3건 반영: 내부 메모 select 허용, 북마크 집계 전용 sourceAccess, 확장 참조 대상 Apply.approvedCount 선언. 실행 검증은 별도.
- 2026-10-04 V1~V5 실험이 드러낸 문법 요구 절 추가.
