# 제품 정의 언어: 작성 카드

> 이 문서는 `spikes/spike-v1-fixture/src/syntax_card.rs`에서 생성된다. 손으로 고치지 않는다. 다시 만들려면 `AIP_UPDATE_GOLDEN=1 cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --test diagnostics`를 실행한다.

제품 경로(`aip service check/gen/serve`)가 읽는 정의 언어의 요약이다. PoC `.aip`(entity·command·query, [grammar.md](grammar.md))와는 다른 언어이며 확장자만 같다. 정의는 `.aip` 텍스트, TS·Python 안의 `aip` 블록(`.e.ts`, `.e.py`), TS·Python 데이터 리터럴(`.h.ts`, `.h.py`) 다섯 형식 중 하나로 쓰고 모두 같은 계약이 된다. 키 목록은 파서와 같은 표에서 나오고, 오류 코드와 고치는 방법은 [definition-diagnostics.md](definition-diagnostics.md)에 있다.

## 언어가 가정하지 않는 것

무제한 기본값과 암묵적 허용이 없다. 빠뜨리면 열리지 않거나 오류가 된다.

- `rows read`를 쓰지 않은 resource는 읽을 수 있는 행이 없다.
- `expose read`를 쓰지 않은 resource는 읽을 수 없다.
- `budget`이 없는 `expose read`는 루트로 읽을 수 없다. 다른 resource의 `traverse` 대상으로만 쓰인다.
- `budget`은 rows·depth·deadline·cost 네 값이 모두 있어야 한다.
- `expose apply`에는 `bulk maxRows`가 필요하다.
- `aggregate`에는 `sourceAccess`가 필요하다. 원본 행의 읽기 정책을 물려받지 않는다.
- 필드 읽기 정책(`field ... read when`)이 있는 필드는 filter·sort·집계에 쓸 수 없다.
- 필드 읽기 정책이 있는 필드는 전이의 `from`, `to`의 값 식, `repeat unchanged`가 걸린 `to` 대상에도 쓸 수 없다(`DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`). 결과로 가려진 값이 추론되기 때문이다. `allow`와 `repeat unchanged` 없는 `to` 쓰기는 가능하다.
- `unique`·`limit`·`invariant` 위반(`WIRE.ALREADY_EXISTS`, `WIRE.INVARIANT_VIOLATED`)은 호출자가 볼 수 없는 행과 충돌해도 같은 코드로 실패한다. 존재는 알려도 오류 본문에 그 행의 id나 값은 싣지 않는다.
- `fields` 안에서 다른 resource를 가리키는 타입은 참조 `X` 하나뿐이다(외래 키가 생긴다). `X.Id`는 `input`·`access` 인자·operation 입출력처럼 DB 열이 아닌 자리에서만 쓰며, `fields` 안에서는 정의 오류(`DEF.ID_FIELD_NEEDS_REFERENCE`)다. 기본 키도 `id: Id` 한 가지뿐이고 `id: Item.Id`는 같은 오류다.
- 다른 행이 없음에 의존하는 `allow`·`from`(`not exists`, predicate 안과 중첩 포함)이나 그런 `check`가 걸린 전이·create는 서버가 SERIALIZABLE 트랜잭션으로 실행하고, 직렬화 실패·교착은 같은 요청을 서버가 최대 3회 다시 실행한다. 유일성·정원이 목적이면 `unique`나 `limit`과 `invariant`가 더 싸다.
- 알 수 없는 키는 무시하지 않고 오류다.
- 문법 오류는 첫 번째에서 멈춘다. 의미 오류는 모아서 보고하며 원인을 먼저 둔다.

## 블록과 키

### 선언

| 키 | 형태 | 뜻 |
|---|---|---|
| `enum` | `enum Name { A, B }` | 닫힌 값 집합. 값은 따옴표 없이 쓰고 DB에는 text CHECK로 저장된다 |
| `actor` | `actor Resource` | 호출자 행을 가리키는 resource. 조건식의 actor는 그 행이다. 정확히 하나 |
| `predicate` | `predicate name(p: Type, ...) = condition` | 이름 붙인 재사용 조건. name(args)로 부르고 서로 순환 호출할 수 없다 |
| `access` | `access name = totalOfVisible(R)  \|  access name(a: Actor, c: X.Id) = condition` | 집계·확장 결과를 누구에게 줄지 정하는 guard |
| `limit` | `limit name on R = atMost N where condition` | 조건에 맞는 행 수의 상한. resource가 invariant로 per 필드마다 건다 |
| `resource` | `resource Name { ... }` | 테이블 하나와 그 테이블을 여는 방식 |
| `operation` | `operation read\|job name { input; output; allow; effect; deadline; implementation }` | resource 없는 연산. 확장 구현을 부른다 |

### resource

| 키 | 형태 | 뜻 |
|---|---|---|
| `fields` | `fields { id: Id; name: Text(1..100); owner: Member? }` | 열. ?는 null 허용. owner: Member는 외래 키가 있는 참조(조인·traverse 가능). fields 안에서 `X.Id`는 쓸 수 없고 오류다(DEF.ID_FIELD_NEEDS_REFERENCE). 참조 `X`를 쓴다. resource마다 한 번 |
| `rows` | `rows read when condition` | 호출자가 읽을 수 있는 행. 쓰지 않으면 읽을 수 있는 행이 없다 |
| `field` | `field name read when condition` | 필드별 읽기 조건. 조건이 거짓인 행에서 select하면 null이고 출력 타입은 nullable이다(열이 필수여도). 이 필드는 filter·sort·집계에 쓸 수 없다 |
| `expose` | `expose read {..} \| expose apply T {..} \| expose create {..} \| expose compose {..} \| expose aggregate name` | 호출자에게 여는 것. 쓰지 않은 것은 열리지 않는다 |
| `aggregate` | `aggregate name: Int { source R; sourceAccess a; groupKey ref; release count }` | 행별 또는 단독 집계 |
| `transition` | `transition name { from cond; to f = v; allow cond }` | 상태 전이. from·to·allow가 모두 필요하다. 숫자는 f = f + 1처럼만 바꾼다 |
| `unique` | `unique a, b` | 유일 제약 |
| `check` | `check name when condition` | 쓰기 뒤 행이 만족해야 하는 조건 |
| `invariant` | `invariant limitName per refField [deferred]` | limit을 참조 필드별로 적용한다(유일 인덱스로 집행) |
| `extension` | `extension read\|write\|job name { input; output; access; effect; deadline; implementation }` | 표준 밖 Node·Python 확장 호출 |
| `docs` | `docs { summary "..."; visibility internal }` | 사람이 읽는 설명. 실행·권한에 쓰이지 않는다 |

### expose read

| 키 | 형태 | 뜻 |
|---|---|---|
| `select` | `select a, b` | 읽을 수 있는 필드·집계 이름 |
| `refIds` | `refIds a, b` | select한 참조 필드 가운데 원시 외래 키 id를 대상 행의 가시성과 무관하게 공개할 것. 쓰지 않은 참조 select는 대상 resource의 `rows read`를 적용해 호출자가 대상 행을 볼 수 없으면 null이 된다(출력 타입도 nullable) |
| `filter` | `filter f.op, g.op` | 필터 허용 목록. 연산 eq in isNull gte lte gt lt prefix contains 중 필드 타입이 지원하는 것 |
| `sort` | `sort a, b` | 정렬 허용 목록 |
| `traverse` | `traverse rel { select a }  \|  traverse Child via field { select a; limit N }` | 참조를 따라 포함한다. 대상 resource의 expose select 안에서만 |
| `budget` | `budget { rows N; depth N; deadline 2s; cost N }` | 이 읽기를 루트로 부를 수 있게 하는 한도. 네 값이 모두 필요하다 |

### budget

| 키 | 형태 | 뜻 |
|---|---|---|
| `rows` | `rows N` | 한 번에 돌려줄 최대 행 수 |
| `depth` | `depth N` | 관계 깊이. 루트가 1이고 traverse마다 1 늘어난다 |
| `deadline` | `deadline 2s` | 시간 한도. ms, s, m 단위 |
| `cost` | `cost N` | 추정 비용 한도 |
| `offset` | `offset N` | offset 페이징을 허용하고 그 상한을 둔다(선택) |
| `cursor` | `cursor` | cursor 페이징을 허용한다(선택). id와 sort 필드가 select에 있어야 한다 |

### traverse

| 키 | 형태 | 뜻 |
|---|---|---|
| `select` | `select a, b` | 따라간 행에서 읽을 필드 |
| `sort` | `sort a` | 1:N traverse의 정렬 필드 하나 |
| `limit` | `limit N` | 1:N traverse가 돌려줄 최대 행(1~1000) |

### aggregate

| 키 | 형태 | 뜻 |
|---|---|---|
| `input` | `input { x: Type }` | 호출자가 넘기는 값 |
| `source` | `source R` | 집계할 행의 resource. 쓰지 않으면 자기 resource |
| `sourceAccess` | `sourceAccess name  \|  name(args)` | 원본 행을 읽는 근거가 되는 access. 반드시 쓴다(원본 행 정책을 물려받지 않는다) |
| `groupKey` | `groupKey ref` | 원본에서 이 resource를 가리키는 참조 필드. 있으면 행별 집계 |
| `where` | `where condition` | 집계에 넣을 원본 행 조건 |
| `callerFilter` | `callerFilter none` | 호출자가 집계 범위를 좁히는 것을 허용하는지 |
| `rowOutput` | `rowOutput none` | 집계가 행을 내보내는지 |
| `release` | `release count  \|  sum(f)  \|  min(f)  \|  max(f)` | 내보내는 값 |

### extension

| 키 | 형태 | 뜻 |
|---|---|---|
| `input` | `input { x: Type }` | 확장이 받는 값 |
| `output` | `output { y: Type }` | 확장이 돌려주는 값 |
| `access` | `access R.transition  \|  R.aggregate` | 확장이 쓸 수 있는 전이·집계 |
| `effect` | `effect none \| db \| managed` | read는 none, write는 db |
| `deadline` | `deadline 2s` | 실행 시간 한도 |
| `implementation` | `implementation "module.function"` | 확장 모듈과 함수 이름 |

### transition

| 키 | 형태 | 뜻 |
|---|---|---|
| `from` | `from condition` | 전이를 시작할 수 있는 행의 조건 |
| `to` | `to f = v; g = g + 1` | 바뀌는 값 |
| `allow` | `allow condition` | 누가 부를 수 있는지 |
| `repeat` | `repeat unchanged \| reject` | 이미 to 상태인 행에 다시 부를 때의 처리 |
| `create` | `create R { f = v }` | 같은 트랜잭션에서 행을 만든다 |
| `update` | `update R where f = v { g = w }  \|  update many R maxRows N where f = v { g = w }` | 같은 트랜잭션에서 다른 행을 바꾼다. many는 maxRows(1~1000)와 부모 참조 조건이 필요하다 |
| `notify` | `notify actorRef "topic"` | 알림. 대상은 actor 타입 참조여야 한다 |

### expose apply

| 키 | 형태 | 뜻 |
|---|---|---|
| `target` | `target id, where` | 전이를 적용할 행을 id로 또는 허용한 필터로 고른다 |
| `bulk` | `bulk maxRows N` | 한 번에 바꿀 최대 행 수. 무제한 기본값이 없다 |
| `sameScope` | `sameScope field` | 대상 행이 모두 같은 값(예: 같은 동아리)이어야 한다 |

### expose create

| 키 | 형태 | 뜻 |
|---|---|---|
| `allow` | `allow condition` | 새 행이 만족해야 하는 조건 |
| `fields` | `fields a, b` | 호출자가 값을 줄 수 있는 필드 |

### expose compose

| 키 | 형태 | 뜻 |
|---|---|---|
| `bulk` | `bulk maxRows N` | 최대 대상 수 |
| `sameScope` | `sameScope field` | 대상이 같은 범위 |
| `transitions` | `transitions a, b` | 허용하는 전이 단계 |
| `selfRow` | `selfRow actorRef by scopeField` | 대상과 같은 범위의 요청자 자신의 행 |
| `create` | `create R from source, ...` | 값을 서버가 읽은 행에서 가져와 만드는 행 |

### docs

| 키 | 형태 | 뜻 |
|---|---|---|
| `summary` | `summary "..."` | 한 줄 설명 |
| `visibility` | `visibility internal` | 문서 노출 범위 표시 |

## 작성 예 1: 읽기, 정책, 집계, 관계

```aip
// V1 의미 fixture. C B.1의 네 resource를 그대로 두고, 빠진 기호 선언만 추가했다.
// 추가 선언(enum, actor, predicate, access, limit, School/Member/ClubMember)은 spike 가정이다.

enum RecruitmentStatus { DRAFT, PUBLISHED, CLOSED }
enum ApplyStatus { PENDING, APPROVE, REJECT }
enum ClubRole { ADMIN, MANAGER, MEMBER }

resource School {
  fields { id: Id; name: Text }
}
resource Member {
  fields { id: Id; school: School? }
}
actor Member

resource ClubMember {
  fields { club: Club; member: Member; role: ClubRole }
  rows read when member = actor
}

predicate managerOf(m: Member, c: Club) = exists ClubMember where club = c and member = m and role in (ADMIN, MANAGER)
predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now

access fixedTotalOfVisibleRecruitment = totalOfVisible(Recruitment)
access clubManagerOnly(a: Member, c: Club.Id) = managerOf(a, c)

limit atMostOnePublished on Recruitment = atMost 1 where status = PUBLISHED

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

## 작성 예 2: 쓰기, 연산, 확장

```aip
enum Phase { READY, DONE }
resource Member { fields { id: Id } }
actor Member
operation read textLength {
  input { text: Text(1..1000) }
  output { length: Int(1..1000) }
  allow actor != null
  effect none
  deadline 2s
  implementation "event.textLength"
}
operation job textLengthJob {
  input { text: Text(1..1000) }
  output { length: Int(1..1000) }
  allow actor != null
  effect none
  deadline 10s
  implementation "event.textLength"
}
resource Event {
  fields { id: Id; member: Member; checked: Bool; title: Text; link: Url; phase: Phase; date: Time }
  rows read when member = actor
  transition mark { allow member = actor; from checked = false; to checked = true; repeat unchanged }
  expose read {
    select id, checked, title, link, phase, date
    filter id.eq, title.eq, title.prefix, link.eq, phase.eq, date.eq, date.gte
    sort id
    budget { rows 50; depth 1; deadline 1s; cost 100 }
  }
  expose apply mark { target id, where; bulk maxRows 20 }
  extension write confirm {
    input { id: Event.Id }
    output { id: Event.Id; count: Int }
    access Event.mark
    effect db
    deadline 2s
    implementation "event.confirm"
  }
}
```
