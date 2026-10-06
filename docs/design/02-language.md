# 02. 언어 설계: 반복 코드를 문법으로 승격하는 탑(tower)

> 2026-10-02: 이 문서는 `.aip`를 독자 작성 언어로 전제한다. 창시자 정정으로 AIP는 새 프로그래밍 언어가 아니며 JS/Python 생태계에서 정의를 쓴다(`docs/PRINCIPLES.md`, D-R6). 의미 하강 탑과 형식 목록은 의미 설계 자료로만 참고한다.

> 상태: Proposed · 문법은 v0.1 스케치이며 IR(`03-ir.md`)이 우선한다.

## 1. 핵심 발상: 의미 하강 탑 (Semantic Lowering Tower)

사용자의 비유를 그대로 설계 원리로 쓴다. 이진수 위에 어셈블리가, 그 위에 C가 올라간 이유는 **아래 층의 반복 패턴을 위 층의 한 구문이 대신하면서도, 위 층의 모든 구문이 아래 층으로 정확히 번역되기 때문**이다. AIP도 같은 구조를 갖는다.

```
L3  도메인 형식 (Domain Forms)      lifecycle, exactly one, erase, retain, counter, cached, notify ...
     │  extension과 표준 라이브러리가 제공. 반드시 L2로 하강한다.
L2  AIP Core Language               entity, relation, query, command, allow, require, do, emit, schedule
     │  컴파일러가 이해하는 최소 의미 집합.
L1  Core IR (검증 대상)              읽기 집합, 쓰기 집합, 잠금 집합, 관계 연산, 효과 호출(서명 포함), 의무(obligation)
     │  모든 정적 분석은 이 층에서 한다. 여기서 증명된 것만 실행된다.
L0  Effect Kernel (실행 대상)         SQL 문, Redis 명령, Kafka 레코드, HTTP 요청, S3 연산
        extension의 runtime adapter가 담당. 사람이 직접 쓰지 않는다.
```

규칙:

1. **모든 상위 구문은 하강 규칙을 갖는다.** `lifecycle`은 L2의 `require`와 enum 제약으로, `exactly one`은 L1의 부분 unique index 의무와 지연(deferred) 검사 의무로 내려간다. 하강 결과는 `aip lower <file>`로 볼 수 있다.
2. **검증은 L1에서만 한다.** 새 문법을 추가해도 분석기를 다시 짜지 않는다. extension이 L3 구문을 추가해도 Core의 안전 규칙을 우회할 수 없는 이유가 이것이다. 우회하려면 L1에서 증명을 통과해야 하는데 L1은 Core가 소유한다.
3. **L3는 닫혀 있지 않다.** 백엔드에서 반복되는 패턴이 발견될 때마다 L3 형식이 늘어난다. 이것이 사용자가 말한 "전형적인 코드를 모듈화해서 문법화"의 공식 경로다. 단 새 형식은 하강 규칙과 conformance 테스트를 함께 가져와야 채택된다.
4. **L2는 거의 닫혀 있다.** L2에 구문을 더하는 것은 언어 개정이며, Reality Test 케이스로 필요성이 증명될 때만 한다.

이 구조가 "DSL 팽창" 위험에 대한 답이다. 팽창은 L3에서 일어나도 괜찮다. 검증 부담은 L1 크기에 비례하고, L1은 작게 유지된다. 범용 언어가 되는 것은 L2에 루프, 재귀, 가변 변수가 들어가는 순간이며, 그것은 금지한다.

## 2. L2 Core: 의미 단위

| 구성요소 | 의미 | 왜 필요한가 (다른 것으로 대체 불가 이유) |
|---|---|---|
| `entity` | 저장되는 사실과 관계, 필드 수준 제약 | 모든 것의 기반 |
| `relation` | 이름 붙은 관계 술어. 권한과 조회에서 재사용 | 권한 규칙의 90%가 "이 사람과 이 자원의 관계"다. 관계를 1급으로 두지 않으면 권한이 명령마다 복붙된다 (아리아리의 `findByClubAndMember` + `isClubManagerOrHigher` 반복) |
| `query` | 상태를 바꾸지 않는 Intent. 관찰형 효과(조회수)만 허용 | 읽기/쓰기 분리 |
| `command` | 상태를 바꾸는 Intent | 원자성, 권한, 효과 계획의 단위 |
| `allow` | 권한. Intent마다 필수 | 누락을 구조적으로 불가능하게 |
| `require` | 사전조건과 그 실패 코드 | 비즈니스 거절 사유를 계약에 싣기 위해 |
| `do` | 관계 연산(집합 단위 insert/update/delete)과 효과 호출 | 변경의 유일한 통로 |
| `emit` | 도메인 이벤트(과거형 사실) | 부작용을 소비자에게 위임 |
| `invariant` | 항상 참이어야 하는 조건 | 명령과 독립된 보장 |
| `schedule` | 시간 트리거 Intent | 아리아리 배치 4종이 전부 스케줄 |
| `fn` | 순수 함수 (L2 식으로 작성) 또는 샌드박스 함수(escape hatch) | 계산 로직의 재사용 |

L2에 **없는** 것: 루프, 재귀, 가변 변수, 예외 처리, 임의 조건 분기(단 입력 존재 여부나 불리언에 대한 `when`은 허용), 사용자 정의 제어 흐름.

## 3. 사용자가 짚은 한계 5개와 해법

iteration 1(spike-0)에서 "한계"로 적었던 것들은 언어가 풀 수 있는 문제였다. spike-0이 하지 않은 설계를 여기서 한다.

### 3.1 반복이 필요하다 → 반복이 아니라 집합 연산으로

주문 취소의 "각 항목마다 재고 복원"은 루프가 아니라 **관계 대수**다. SQL이 50년간 증명한 사실이다.

```aip
do {
  update Product p via order.items i
    set p.stock += sum(i.quantity)
}
```

- `via`는 조인이고 `sum`은 같은 대상 행으로 모이는 값의 집계다. 같은 상품이 두 줄이어도 정확하다.
- 종료가 보장되고 항상 단일 SQL(집합 UPDATE)로 내려간다. N+1이 구조적으로 없다.
- 집계 없이 여러 행이 한 대상에 대입되면(`set p.name = i.label`) "대입값이 여럿" 오류다(E-SET-AMBIG). spike-0의 E207을 일반화한 것.

집합 입력도 1급이다. 아리아리 `approveApplies`는 id 목록을 받아 `get(0)`로 동아리를 추정하고, 없는 id는 조용히 무시했다.

```aip
command ApproveApplies(applies: Set<Apply> max 100) {
  let club = same(applies a: a.recruitment.club) else APPLIES_FROM_DIFFERENT_CLUBS
  allow managerOf(actor, club)
  do {
    update applies set status = APPROVED
    insert ClubMember from applies a {
      club: a.recruitment.club, member: a.member, name: a.member.nickname,
      role: GENERAL, status: ACTIVE
    }
  }
}
```

- `Set<T> max N`은 상한이 필수다. 빈 집합, 중복 id, 존재하지 않거나 볼 수 없는 id는 입력 검증에서 거절된다.
- 원본의 루프 안 `findByClubAndMember`(N+1)는 `unique(club, member)` 제약 위반으로 한 번에 처리되고, 위반은 선언된 오류 코드로 매핑된다.

### 3.2 관계를 거친 권한 → 관계 술어를 SQL로 컴파일

spike-0은 "암묵적 로딩 금지" 때문에 `owner(part.box.owner)`를 쓸 수 없었다. 잘못된 결론이었다. 금지해야 할 것은 **계획되지 않은** 로딩이지 깊은 경로가 아니다. 컴파일러는 명령이 쓰는 모든 경로(읽기 집합)를 미리 알기 때문에 한 번의 계획된 조회로 가져올 수 있다.

```aip
enum ClubRole ordered { GENERAL < MANAGER < ADMIN }

relation membership(m: Member, c: Club): ClubMember
  = the ClubMember cm where cm.club = c and cm.member = m and cm.status = ACTIVE

relation managerOf(m: Member, c: Club)
  = membership(m, c).role >= MANAGER

command ChangeMemberStatus(target: ClubMember, newStatus: MemberStatus) {
  allow managerOf(actor, target.club)
    and membership(actor, target.club).role > target.role
  do { set target.status = newStatus }
}
```

- `relation`은 권한 전용 언어가 아니라 조회에서도 쓰는 관계 술어다(Zanzibar류 ReBAC와 같은 계열).
- 권한식은 명령의 문맥 조회(context fetch) SQL 안에 `EXISTS`/`JOIN`으로 들어간다. 아리아리에서 단건 `modifyStatusType`은 대량 버전과 달리 `isHigherRoleTypeThan` 검사가 빠져 있었다. 관계 술어가 1급이면 같은 규칙을 두 번 쓸 일이 없다.

### 3.3 `transaction` 블록의 의미가 약하다 → 트랜잭션은 암묵, 일관성 등급이 명시

단일 DB 안의 원자성은 **안전한 것이므로 암묵적**이다(명령 = DB 트랜잭션 1개). 명시해야 할 위험은 "DB 밖으로 나가는 효과가 어떤 일관성으로 실행되는가"다. 그래서 `transaction` 키워드를 없애고, 효과마다 **일관성 등급**을 extension이 서명으로 선언하게 한다.

| 등급 | 의미 | 예 | 컴파일러가 배치하는 위치 |
|---|---|---|---|
| `local` | DB 트랜잭션 안 | insert/update, outbox 기록 | 트랜잭션 내부 |
| `staged` | 커밋 전에 임시 기록, 커밋 후 확정, 실패 시 GC | S3 업로드 | 커밋 전 스테이징, 커밋 후 승격 |
| `reservable` | 예약-확정-취소 3단계 | 결제 승인/매입/취소, 재고 예약 | 커밋 전 예약, 커밋 후 확정, 롤백 시 취소 |
| `deferred` | 커밋 후 at-least-once, 멱등 키 필수 | 메일, 알림, 카카오 연결 해제, 캐시 무효화, 이벤트 발행 | outbox 경유 |
| `observational` | 실패해도 비즈니스에 무해, 조회에서도 허용 | 조회수, 분석 이벤트 | 최선 노력 |

사용자는 효과 호출만 쓴다. 어디에 놓을지는 컴파일러가 서명을 보고 정한다.

```aip
command Unregister() {
  allow authenticated
  do {
    erase actor
    kakao.unlink(actor.kakaoId)
  }
}
```

`kakao.unlink`의 서명이 `deferred, idempotent`이므로 커밋 이후 outbox로 배치된다. 아리아리 원본처럼 DB 트랜잭션 중간에 외부 호출이 실행되는 구조는 표현 자체가 불가능하다. 서명이 `irreversible, non-idempotent`인 효과를 커밋 전에 두려고 하면 E-EFFECT-ORDER 오류다.

### 3.4 존재 여부 노출 → 정책을 로딩 조건에 넣는다

엔티티 타입 파라미터(`target: ClubMember`)는 "id를 받아 로딩"을 뜻한다. 로딩 SQL의 WHERE에 읽기 가시성(`visible to`)과 `allow` 중 행 의존 부분이 들어간다. 볼 수 없는 행은 없는 행과 구분되지 않아 응답이 `NOT_FOUND`로 통일된다. 구분이 필요한 도메인은 `disclose existence`를 명시한다(위험한 쪽이 명시).

### 3.5 교착(deadlock) → 컴파일러가 잠금 순서를 정한다

명령의 잠금 집합(쓰기 대상, 불변식 검사 대상)은 정적으로 알려진다. 런타임은 잠금을 **(테이블 순서, 기본키 순서)라는 전역 정렬**로 획득한다(`SELECT ... FOR UPDATE ORDER BY`). 한 명령 안에서 모든 잠금이 한 순서로 잡히면 명령 간 순환 대기가 생기지 않는다. 사람은 이 규칙을 매번 지키기 어렵지만 컴파일러에게는 기본 동작이다.

집합 UPDATE가 행을 잠그는 순서는 PostgreSQL이 보장하지 않으므로, 컴파일러는 집합 변경 전에 대상 행을 정렬 잠금하는 선행 문장을 생성한다. 그래도 발생하는 직렬화 실패는 명령이 **재시도 안전**(커밋 전 외부 효과가 없거나 전부 멱등 키를 가짐)할 때만 런타임이 자동 재시도한다. 재시도 안전성은 컴파일러가 판정한다.

## 4. L3 도메인 형식: 반복 코드의 문법화

아래는 아리아리와 커머스 도메인에서 반복되던 코드가 어떤 형식이 되는지다. 각 형식은 L2/L1로의 하강 규칙을 갖는다.

### 4.1 `lifecycle` (상태 기계)

```aip
entity Apply {
  member: Member      on erase cascade
  recruitment: Recruitment
  status: ApplyStatus = PENDING
  unique (member, recruitment) else ALREADY_APPLIED

  lifecycle status {
    PENDING            -> INTERVIEW
    PENDING, INTERVIEW -> APPROVED, REFUSED
  }
}
```

- 하강: `set x.status = S`마다 `require x.status in 전이원(S) else APPLY_STATUS_INVALID_TRANSITION`이 자동 삽입된다.
- 정적 검사: 도달 불가 상태, 어떤 명령도 일으키지 않는 전이(dead transition), 선언에 없는 전이를 일으키는 명령.
- 아리아리 원본은 전이 규칙이 세 메서드의 if 문에 흩어져 있었다(`approve`는 REFUSAL만 막아 PENDING에서 면접 없이 합격 가능, `refuse`는 APPROVE만 막음). 의도였는지는 확인 필요지만, lifecycle로 쓰면 의도가 선언으로 드러난다.

### 4.2 기수 불변식 (`unique`, `exactly one`, `no overlap`)

```aip
entity ClubMember {
  club: Club
  member: Member?     on erase anonymize
  role: ClubRole
  unique (club, member) else ALREADY_CLUB_MEMBER
  exactly one where role = ADMIN per club
}

entity Recruitment {
  club: Club
  period: Range<Time>
  no overlap (period) per club else RECRUITMENT_PERIOD_OVERLAP
}
```

컴파일러는 불변식마다 **강제 전략**을 고른다. 우선순위는 DB 제약 > 잠금 후 검사 > 직렬화 격리다.

| 불변식 | 하강 결과 (PostgreSQL) |
|---|---|
| `unique (club, member)` | UNIQUE 인덱스. 위반은 선언된 코드로 매핑 |
| `at most one where role = ADMIN per club` | 부분 UNIQUE 인덱스 `ON (club_id) WHERE role = 'ADMIN'` |
| `at least one where role = ADMIN per club` | 커밋 시점 지연 검사(deferred constraint trigger). 변경된 club에 대해서만 |
| `no overlap (period) per club` | `EXCLUDE USING gist (club_id WITH =, period WITH &&)` (btree_gist 확장 필요, 확인 필요) |

`exactly one` = at most + at least. 그러면 아리아리의 자기 위임 버그는 커밋 시점에 "관리자 0명" 위반으로 거절된다. 더 나아가 컴파일러는 **이 불변식을 깰 수 있는 모든 명령**(role을 쓰거나 ClubMember를 지우는 명령)을 나열하고, 각 명령이 불변식을 보존하는지 정적으로 판정하거나(위임 명령은 승격과 강등을 같은 트랜잭션에서 하므로 보존), 판정 불가면 경고한다.

### 4.3 `erase` (개인정보 삭제 완결성)

```aip
entity Member personal {
  email: Email        personal
  nickname: Text
  kakaoId: Text       personal
}

entity ClubReview { author: Member?  on erase anonymize }
entity Apply      { member: Member   on erase cascade }
```

- `personal` 엔티티를 참조하는 모든 필드는 `on erase` 정책(`cascade`, `anonymize`, `restrict`, `reassign <rule>`)을 **반드시** 가져야 한다. 하나라도 없으면 컴파일 오류(E-ERASE-INCOMPLETE).
- `erase actor`는 참조 그래프 전체의 처리 계획으로 하강한다. 아리아리 `UnregisterService`가 8개 저장소에 수동으로 `updateMemberNull`을 호출하고 `em.flush/clear`로 영속성 컨텍스트를 맞추던 코드가 선언 몇 줄이 된다. 새 엔티티가 Member를 참조하면서 정책을 빼먹는 실수는 컴파일되지 않는다.
- 불변식과 결합: `erase`가 `exactly one ADMIN`을 깨면 복구 규칙이 필요하다.

```aip
entity ClubMember {
  club: Club
  member: Member?  on erase anonymize
  role: ClubRole
  status: MemberStatus
  joinedAt: Time
  exactly one where role = ADMIN per club
    repair on erase do {
      let heir = first ClubMember m where m.club = club and m.status = ACTIVE and m.member != null
                 order by m.role desc, m.joinedAt asc
      when heir != null: set heir.role = ADMIN
      when heir = null: delete Club c where c = club
    }
}
```

  이것은 아리아리 `entrustClubAdmin`의 "MANAGER 우선, 없으면 GENERAL, 아무도 없으면 동아리 삭제"를 그대로 옮긴 것이다. 복구 블록 안에서 `per` 필드 이름(`club`)은 불변식이 깨진 그 그룹의 값에 바인딩된다. `first ... order by`와 블록 안의 `let`은 이 요구 때문에 추가됐다(09 Pass 3, OI-04).

### 4.4 `retain` (보존 기한)

```aip
retain Apply for 6mo after recruitment.endsAt
  then purge
  notify member 7d before via mail.template("apply-expiring")
```

하강: 내구성 타이머를 가진 `schedule` + 집합 delete + deferred 메일 효과. 아리아리 기획의 "지원서 6개월 보관 후 영구삭제, 삭제 전 이메일"과 "신고 처리 3개월 후 물리 삭제"가 이 형식이다.

### 4.5 파생 값 (`counter`, `aggregate`)

```aip
entity Recruitment {
  views: Int counter via redis dedupe by client within 1d window 14d
  bookmarkCount: count(bookmarks)            // 조회 시 계산, 저장 안 함
}
```

- `counter`는 redis extension이 제공하는 L3 형식이다. 원자적 INCR, `SET NX EX` 중복 방지, 창(window) 만료, DB 동기화 주기를 extension이 구현한다.
- 아리아리 `ViewsManager`는 이 기능을 손으로 구현하면서 키 생성에 콘텐츠 타입 대신 조회수 값을 넣는 버그가 있었고, 읽기 후 쓰기(get→set)로 원자성이 없었고, `x-forwarded-for` 헤더를 그대로 믿었다. 형식으로 쓰면 이 세 가지를 개별 서비스가 다시 구현할 일이 없다. `client`의 정의(신뢰할 프록시 목록 기반 IP, 또는 세션)는 런타임 설정이다.

### 4.6 `cached` (읽기 캐시와 자동 무효화)

```aip
query RecruitmentDetail(r: Recruitment) cached 60s {
  ...
}
```

컴파일러는 이 조회의 읽기 집합(엔티티, 필드, 관계)을 알고, 모든 명령의 쓰기 집합을 안다. 그 교집합으로 **무효화 대상 명령과 키를 자동 도출**한다. 무효화는 커밋 후 deferred 효과로 실행되고, 무효화 직후 오래된 값이 다시 채워지는 경쟁을 막기 위해 버전 스탬프를 쓴다. 사람이 "어떤 명령이 이 캐시를 깨야 하는가"를 기억할 필요가 없다. 사람이 직접 짠 캐시에서 가장 흔한 버그(무효화 누락)가 구조적으로 사라진다.

### 4.7 `notify` (알림 팬아웃)

```aip
// CreateRecruitment 안에서: emit RecruitmentOpened { recruitment: r }

on RecruitmentOpened e
  notify bookmarkers(e.recruitment.club)
    via alarm.template("club-recruitment-opened")
```

아리아리 서비스 곳곳의 `memberAlarmManger.sendXxx(...)` 호출이 이벤트 구독으로 바뀐다. 알림은 deferred이므로 커밋 이후에만 나가고, 트랜잭션이 롤백되면 알림도 없다.

## 5. Intent 문법

정본은 `spec/grammar.md`다. 요지:

- command 절 순서: `let* → allow → require* → do → emit* → returns`
- query 절 순서: `let* → allow → fetch* → from → where → group by → sort → page → plan → consistency → select → touch*`
- `let`은 한 번 정해지면 바뀌지 않는 이름 붙은 값이다(가변 변수 아님). `same(...)` 같은 확인과 함께 쓰면 실패 코드를 붙일 수 있다.
- 파라미터 타입: 스칼라, enum, 레코드, 엔티티(=로딩되는 참조), `Set<Entity> max N`, `List<Record> max N`, `Upload(...)`.
- `page` 절이 있으면 cursor 입력이 transport 수준에서 자동으로 붙는다. 파라미터로 선언하지 않는다.
- `allow`는 필수. 공개는 `allow public`, 로그인 필요는 `allow authenticated`.
- 명령 안의 모든 경로는 한 번의 문맥 조회로 계획된다. 문맥 조회가 너무 커지면(설정 가능한 한도) 경고한다.

## 6. Escape hatch

L2 식으로 쓸 수 없는 계산(가격 규칙 엔진, 텍스트 정규화, 외부 포맷 파싱)은 `fn`으로 쓴다.

- **순수 함수**: 입력만 보고 출력을 낸다. WASM 컴포넌트(Rust, TS, Python 등에서 컴파일)로 실행한다. 호스트가 I/O capability를 주지 않으므로 순수성이 샌드박스로 강제된다. 분석기는 순수 함수를 믿어도 된다.
- **효과 함수**: 새 외부 연동이 필요하면 함수가 아니라 extension을 만든다. extension은 효과 서명을 선언해야 하고, 서명은 런타임이 host capability로 강제한다(`06-extensions.md`).

escape hatch가 커지는 것은 실패 신호로 추적한다. `aip stats`가 정의 대비 escape hatch 비율을 보고한다.

## 7. 미결정 (Open Questions)

1. 불변식 복구 규칙(`repair`)의 문법과 표현 범위.
2. `relation`의 재귀 허용 여부(조직도, 폴더 트리). 허용하면 재귀 CTE로 하강. 종료 조건 증명 필요.
3. 조건부 효과(`when`)의 범위. 현재는 입력 존재 여부와 불리언만.
4. 문자열 템플릿, 날짜 연산, 타임존의 표준 라이브러리 경계.
5. 여러 Intent가 공유하는 선택(selection) 조각의 재사용 문법.
