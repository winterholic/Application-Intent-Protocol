# 01. Reality Test: 실제 백엔드 케이스 15개

> 상태: Proposed · 문법은 `02-language.md` v0.1 스케치
> 출처: 케이스 1-12는 사용자의 실제 운영 서비스 **아리아리** 백엔드(`projects/ariari/ariari-backend`, Spring Boot, 2024년 운영) 코드에서 가져왔다. 13-15는 결제·캐시·메시지 브로커·외부 API를 강제로 포함시키기 위한 커머스 케이스다.
> 규칙: 단순 CRUD는 넣지 않는다. 각 케이스는 "사람이 기억해야 했던 규칙"을 최소 하나 포함한다.

각 케이스 항목: **요구사항 / 기존 구현과 실제 결함 / AIP 표현 / 필요한 구성요소 / 정적 보장 / 런타임 보장 / 실패 시나리오 / 추상화 누수 가능성**

결함 표기: `[결함]`은 코드를 읽어 확인한 것, `[확인 필요]`는 코드상 의심이지만 운영 영향(엔드포인트 노출, 외부 보정 코드)은 확인하지 못한 것.

---

## Case 1. 지원서 제출

**요구사항**: 학교 인증이 동아리 학교와 일치하는 회원만, 공개 중인 모집에, 해당 동아리 회원이 아닐 때, 한 번만 지원. 첨부 파일 선택. 동아리 관리진에게 알림.

**기존 구현** (`ApplyService.saveApply`, 약 20줄 + Validator + Repository 3개 + FileManager + AlarmManager)
- `[결함]` 중복 지원 검사가 "조회 후 저장"이고 `Apply` 엔티티에 unique 제약이 없다. 동시 요청 두 개가 모두 통과할 수 있다.
- `[결함]` 파일을 S3에 먼저 올리고 DB에 저장한다. 이후 트랜잭션이 실패하면 고아 파일이 남는다.
- `[확인 필요]` 알림 전송이 트랜잭션 안에서 호출된다. 롤백 시 알림 행도 롤백되는지는 AlarmManager 구현에 달렸다.

**AIP 표현**
```aip
relation schoolVerified(m: Member, s: School?) = s = null or m.school = s

command SubmitApply(recruitment: Recruitment, answers: List<AnswerInput> max 50,
                    file: Upload(max 10MB, types [pdf, png, jpg])?) {
  allow schoolVerified(actor, recruitment.club.school)
    and not exists membership(actor, recruitment.club)
  require recruitment is open else RECRUITMENT_CLOSED
  do {
    insert Apply { member: actor, recruitment, answers } as apply
    when file: s3.put(file, bucket: "apply") into apply.file
  }
  emit ApplySubmitted { apply }
}
```
`recruitment is open`은 Recruitment에 선언된 술어다: `predicate open = status = PUBLISHED and now in period and not earlyClosed`.

**구성요소**: relation, command, require, unique(엔티티 선언), staged 효과(s3), emit + notify 구독

**정적 보장**: 정책 존재. unique 위반 코드가 계약에 포함. `s3.put`은 staged라 커밋 전 스테이징/커밋 후 승격으로 배치됨. 알림은 deferred라 커밋 후에만.
**런타임 보장**: 중복 지원은 DB unique로 경쟁 없이 거절(`ALREADY_APPLIED`). 트랜잭션 실패 시 스테이징 객체는 GC.
**실패 시나리오**: 스테이징 업로드 성공 → DB 실패 → 스테이징 GC. 커밋 성공 → 승격 실패 → outbox 재시도(승격은 멱등).
**누수 가능성**: 파일 크기/타입 검증과 바이러스 검사 같은 파일 파이프라인 요구가 커지면 s3 extension 설정이 비대해진다.

---

## Case 2. 지원서 일괄 합격

**요구사항**: 관리진이 여러 지원서를 한 번에 합격 처리. 같은 동아리 지원서만. 합격자는 일반 멤버로 가입. 거절된 지원서는 합격 불가. 각자에게 알림.

**기존 구현** (`ApplyService.approveApplies`)
- `[결함]` `applies.get(0)`: 빈 목록이면 IndexOutOfBounds(500).
- `[결함]` `findAllByIdsWithClub(applyIds)`는 없는 id를 조용히 빼고 진행한다.
- `[결함]` 루프 안에서 지원자마다 `findByClubAndMember` 조회(N+1).
- `[확인 필요]` 상태 전이가 "REFUSAL만 금지"라 PENDING에서 면접 없이 합격 가능. 의도일 수 있음.

**AIP 표현**: `02-language.md` 3.1의 `ApproveApplies`. 상태 전이는 Apply의 `lifecycle`이 강제.

**구성요소**: `Set<T> max N` 입력, lifecycle, 집합 update/insert, unique, notify

**정적 보장**: 입력 집합 상한. 전이 규칙은 lifecycle과 일치해야 컴파일. 집합 연산이므로 쿼리 수가 입력 크기와 무관.
**런타임 보장**: 모든 id가 존재하고 볼 수 있어야 실행(아니면 `NOT_FOUND` + 어떤 id인지 path). 이미 멤버면 unique 위반이 `ALREADY_CLUB_MEMBER`로. 한 건이라도 실패하면 전체 롤백.
**실패 시나리오**: 부분 성공을 원하는 요구("되는 것만 처리")가 나오면 → `do partial` 같은 명시 형식이 필요해진다. 현재는 전부 또는 전무.
**누수 가능성**: 부분 성공 semantics. open question으로 기록.

---

## Case 3. 최고 관리자 위임

**요구사항**: 동아리 최고 관리자(ADMIN)는 정확히 1명. ADMIN만 위임 가능. 위임하면 기존 ADMIN은 일반 멤버.

**기존 구현** (`ClubMemberService.entrustAdmin` + 컨트롤러의 ADMIN 변경 차단)
- `[결함]` 자기 자신의 ClubMember id로 위임을 호출하면, 같은 엔티티에 ADMIN을 넣은 뒤 GENERAL로 덮어써 **ADMIN 0명**이 된다. 자기 위임 검사가 없다.
- `[결함]` 위임 대상이 활동 종료 상태여도 위임된다(상태 검사 없음). 의도 확인 필요.
- 규칙 "ADMIN은 1명"이 README, 컨트롤러(`modifyRoleType`에서 ADMIN 지정 차단), 서비스에 흩어져 있다. `modifyRoleType`으로 ADMIN이 자기 역할을 MANAGER로 바꾸는 것도 막혀 있지 않다(대상 역할 검사만 있음, 자기 자신 제외 검사 없음).

**AIP 표현**
```aip
entity ClubMember {
  ...
  exactly one where role = ADMIN per club
}

command EntrustAdmin(target: ClubMember) {
  allow membership(actor, target.club).role = ADMIN
  require target.status = ACTIVE else ENTRUST_TARGET_INACTIVE
  do {
    set membership(actor, target.club).role = GENERAL
    set target.role = ADMIN
  }
}
```

**구성요소**: 기수 불변식, relation을 대입 대상으로 쓰기

**정적 보장**: `exactly one`을 깰 수 있는 명령 목록이 `aip verify`에 나온다. `EntrustAdmin`은 같은 트랜잭션에서 한 명 강등, 한 명 승격이라 "보존 가능"으로 판정되지만, target과 actor가 같은 행일 수 있음을 컴파일러가 발견하고 경고한다(별칭 분석, alias analysis).
**런타임 보장**: at-most-one은 부분 unique 인덱스, at-least-one은 커밋 시 지연 검사. 자기 위임은 커밋 시 "ADMIN 0명"으로 거절(`INVARIANT_VIOLATED: exactly_one_admin`).
**실패 시나리오**: 동시 위임 두 건 → 둘 다 같은 club의 ADMIN 행을 잠금 순서대로 잡으므로 직렬화, 두 번째는 권한 재평가에서 거절.
**누수 가능성**: 별칭 분석이 모든 경우를 증명하지 못하면 런타임 지연 검사가 최후 방어선이 된다. 정적 증명과 런타임 검사의 경계를 `04-safety-matrix.md`에 명시.

---

## Case 4. 멤버 상태 변경 (단건과 일괄)

**요구사항**: MANAGER 이상이 멤버의 활동 상태를 변경. 자기보다 낮은 역할만 변경 가능.

**기존 구현**
- `[결함]` 일괄 버전(`modifyStatusTypes`)은 `isHigherRoleTypeThan`과 `belongsToClub`을 검사하지만 단건 버전(`modifyStatusType`)은 역할 우위 검사가 없다. MANAGER가 ADMIN을 활동 종료로 바꿀 수 있다.

**AIP 표현**
```aip
relation outranks(m: Member, target: ClubMember)
  = membership(m, target.club).role > target.role

command ChangeMemberStatus(targets: Set<ClubMember> max 200, newStatus: MemberStatus) {
  let club = same(targets t: t.club) else MEMBERS_FROM_DIFFERENT_CLUBS
  allow managerOf(actor, club) and all(targets t: outranks(actor, t))
  do { update targets set status = newStatus }
}
```
단건은 별도 명령이 아니라 원소 1개인 집합이다. 같은 규칙을 두 번 쓸 수 없다.

**구성요소**: relation, 집합 입력, `all` 한정자

**정적 보장**: 권한식이 한 곳. "같은 규칙의 두 구현"이라는 결함 유형 자체가 사라진다.
**런타임 보장**: 권한식은 문맥 조회 SQL 안에서 집합 전체에 대해 평가.
**누수 가능성**: 거의 없음.

---

## Case 5. 회원 탈퇴

**요구사항**: 개인정보 즉시 삭제, 후기·댓글은 "탈퇴한 회원"으로 익명화, ADMIN인 동아리는 MANAGER → GENERAL 순으로 승계, 남은 멤버가 없으면 동아리 삭제, 카카오 연결 해제.

**기존 구현** (`UnregisterService`, 약 100줄)
- 8개 저장소에 `updateMemberNull` 수동 호출, `em.flush(); em.clear();`로 영속성 컨텍스트 재동기화, 재조회.
- `[결함]` 카카오 unlink(외부, 비가역)가 DB 트랜잭션 중간에 호출된다. 이후 DB 삭제가 실패하면 카카오는 해제됐는데 회원은 남는다.
- `[결함 가능]` 새 엔티티가 Member를 참조하면서 `updateMemberNull` 추가를 빼먹으면 FK 오류나 개인정보 잔존. 이 누락을 막는 장치가 없다.
- 승계 로직에서 동아리마다 조회(N+1).

**AIP 표현**: `02-language.md` 4.3의 `personal`, `on erase`, `repair on erase` + 3.3의 `Unregister`.

**구성요소**: erase(L3), 불변식 복구 규칙, deferred 외부 효과

**정적 보장**: `personal` 참조의 `on erase` 정책 완결성(E-ERASE-INCOMPLETE). 외부 효과가 커밋 후로 배치됨. 승계 규칙이 `exactly one` 불변식을 보존하는지 검사.
**런타임 보장**: 모든 익명화/삭제가 한 트랜잭션. 카카오 unlink는 outbox에서 멱등 재시도.
**실패 시나리오**: 카카오 API 장기 장애 → outbox에 남고 재시도, 일정 횟수 후 운영 알림(dead letter). 회원 데이터는 이미 삭제됐으므로 "해제 대기" 상태가 관찰 가능해야 한다.
**누수 가능성**: 승계 규칙 문법(`repair`)이 복잡해질 수 있다. open question.

---

## Case 6. 학교 인증 해제

**요구사항**: 학교 인증을 해제하면 그 학교 소속 동아리에서 자동 탈퇴. 단 그 중 한 곳이라도 ADMIN이면 해제 불가.

**기존 구현** (`SchoolAuthService.cancelMySchoolAuth`)
- 멤버십을 순회하며 `clubMember.getClub().getSchool()` 지연 로딩(N+1).
- `[결함 가능]` 여기서는 `clubMemberRepository.delete`만 호출하고, 다른 탈퇴 경로(`quitClubMember`)가 수행하는 `deleteClubMember` 정리(댓글·활동·공지·출석의 참조 처리)는 하지 않는다. 탈퇴 경로마다 정리가 다르다.

**AIP 표현**
```aip
command CancelSchoolAuth() {
  allow authenticated
  require not exists ClubMember cm where cm.member = actor
          and cm.club.school = actor.school and cm.role = ADMIN
    else ADMIN_OF_SCHOOL_CLUB
  do {
    delete ClubMember cm where cm.member = actor and cm.club.school = actor.school
    set actor.school = null
  }
}
```
ClubMember 삭제 시 참조 정리(`on delete` 정책)는 엔티티에 선언되므로 **모든 삭제 경로에 동일하게** 적용된다.

**구성요소**: 집합 delete, 참조 정책(`on delete`)

**정적 보장**: 삭제 경로가 몇 개든 참조 정리 규칙은 하나. N+1 없음(집합 연산).
**누수 가능성**: 낮음.

---

## Case 7. 모집공고 등록

**요구사항**: MANAGER 이상. 시작 < 종료. 같은 동아리에 기간이 겹치는 공고 불가. 최신 지원서 양식을 스냅샷으로 연결. 동아리를 북마크한 회원에게 알림.

**기존 구현** (`RecruitmentService.saveRecruitment`)
- `[결함]` 기간 중복 검사가 "조회 후 저장"이라 동시 등록 시 겹치는 공고가 생길 수 있다.
- 알림 대상 조회와 발송이 트랜잭션 안.

**AIP 표현**
```aip
entity Recruitment {
  club: Club
  period: Range<Time>                                // start < end는 Range 타입이 보장
  form: ApplyFormSnapshot
  no overlap (period) per club else RECRUITMENT_PERIOD_OVERLAP
}

command CreateRecruitment(club: Club, input: RecruitmentInput, poster: Upload?) idempotent {
  allow managerOf(actor, club)
  require exists ApplyForm f where f.club = club else NO_APPLY_FORM
  do {
    insert Recruitment { club, ...input, form: snapshot(latest ApplyForm f where f.club = club by f.createdAt) } as r
    when poster: s3.put(poster, bucket: "recruitment") into r.poster
  }
  emit RecruitmentOpened { recruitment: r }
}
```

**정적 보장**: `no overlap`이 EXCLUDE 제약으로 하강 가능한지 capability 확인(postgres ✓). 불가능한 DB면 E-CAPABILITY.
**런타임 보장**: 겹침은 DB 제약으로 경쟁 없이 거절.
**누수 가능성**: `snapshot(...)`의 의미(깊은 복사인지 버전 참조인지)를 정해야 한다.

---

## Case 8. 모집공고 상세 + 조회수

**요구사항**: 비로그인도 조회 가능하지만 "교내" 공고는 같은 학교 인증 회원만. 조회수는 같은 클라이언트 1일 1회. 응답에 북마크 수, 내 동아리 여부, 내 지원 여부, 최근 임시저장 id.

**기존 구현** (`findRecruitmentDetail` + `ViewsManager` + `ViewsScheduler`)
- 읽기 요청이 `@Transactional`로 쓰기(조회수)를 수행.
- 응답 구성에 조회 쿼리 5개 이상(북마크 수, 멤버 여부, 지원 여부, 임시저장).
- `[결함]` 조회수 Redis 키가 `content.getViews().toString() + id + date`로, 콘텐츠 타입이 아닌 **현재 조회수 값**을 쓴다. 조회수가 바뀌면 키가 달라져 14일 창 차감(`subtractViews`)이 해당 키를 찾지 못한다.
- `[결함]` 조회수 증가가 get → +1 → set으로 원자적이지 않다. 중복 검사도 exists → set으로 원자적이지 않다.
- `[결함]` 클라이언트 식별에 `x-forwarded-for` 헤더를 그대로 사용해 위조 가능.
- 스케줄러가 전체 동아리와 공고를 `findAll()` 후 개별 갱신.

**AIP 표현**
```aip
entity Recruitment {
  visible to actor when club.school = null or schoolVerified(actor, club.school)
  views: Int counter via redis dedupe by client within 1d window 14d
}

query RecruitmentDetail(r: Recruitment) cached 30s per actor-class {
  allow public
  from r
  select {
    id title body period poster views
    club { id name }
    bookmarks: count(r.bookmarks)
    isMyClub: exists membership(actor, r.club)
    isMyApply: exists Apply a where a.recruitment = r and a.member = actor
    myLatestDraft: (latest ApplyDraft d where d.recruitment = r and d.member = actor by d.createdAt).id
  }
  touch r.views
}
```

**정적 보장**: 모든 파생 필드가 한 번의 계획된 조회(lateral 서브쿼리)로. `actor`가 없으면 `exists ... actor`는 false로 정의됨(null 전파 규칙). 조회는 관찰형 효과만 가진다.
**런타임 보장**: counter는 redis extension의 원자 연산. `client`는 신뢰 프록시 설정 기반.
**누수 가능성**: 캐시와 사용자별 필드의 충돌. `per actor-class`처럼 캐시 분할 기준을 명시해야 한다. 사용자별 필드가 있는 조회를 공용 캐시하려 하면 컴파일 오류가 맞다(E-CACHE-PERSONALIZED).

---

## Case 9. 모집공고 목록 필터·정렬

**요구사항**: 분야 8, 지역 10, 대상 3, 소속 2 필터. 정렬 5종(최신, 조회수, 스크랩, 오래된, 마감일). 학교 미인증 사용자가 "교내" 선택 시 안내 메시지.

**AIP 표현**
```aip
query RecruitmentList(filter: RecruitmentFilter, sort: RecruitmentSort = LATEST) {
  allow public
  from Recruitment r
  where r is open and r.field in filter.fields and r.region in filter.regions
    and r.target in filter.targets and r.affiliation in filter.affiliations
  sort by sort of {
    LATEST:   r.createdAt desc
    VIEWS:    r.views desc
    SCRAPS:   count(r.bookmarks) desc
    OLDEST:   r.createdAt asc
    DEADLINE: r.period.end asc
  }
  page 20 by keyset
  select { id title poster period club { name } bookmarks: count(r.bookmarks) }
}
```
"교내" + 미인증 안내는 `visible to` 정책으로 결과에서 제외되고, 클라이언트 안내 메시지는 UI의 책임이다. 서버가 거절 사유를 줘야 한다면 `require filter.affiliations has CAMPUS implies actor.school != null else SCHOOL_AUTH_REQUIRED`.

**정적 보장**: 정렬 키마다 필요한 인덱스를 `aip plan`이 제안(또는 마이그레이션에 포함). `count(r.bookmarks)` 정렬은 비용 경고(W-SORT-AGG)와 함께 파생 counter 사용 제안.
**누수 가능성**: 집계 정렬의 성능. 명시적 hint(`materialize bookmarkCount`)가 필요할 수 있다.

---

## Case 10. 활동 댓글 가시성

**요구사항**: 활동은 전체공개/동아리원 공개. 차단한 사용자와 나를 차단한 사용자의 댓글은 서로 보이지 않는다. 표시 이름은 동아리 내 이름.

**AIP 표현**
```aip
entity ClubActivity {
  club: Club
  visibility: Visibility
  visible to actor when visibility = PUBLIC or exists membership(actor, club)
}

entity ActivityComment {
  activity: ClubActivity
  author: Member?        on erase anonymize
  visible to actor unless blocked(actor, author) or blocked(author, actor)
}
```
`visible to`는 이 엔티티를 읽는 **모든 경로**(목록, 상세, 중첩 선택, 명령의 로딩)에 적용된다.

**정적 보장**: 새 조회를 추가해도 가시성 규칙을 빼먹을 수 없다.
**런타임 보장**: 규칙은 SQL로 하강(NOT EXISTS). 중첩 선택에서도 배치 조회에 포함.
**누수 가능성**: 가시성 규칙이 복잡해지면 모든 조회가 무거워진다. `aip plan`이 규칙별 비용을 보여줘야 한다.

---

## Case 11. 회계 기록과 잔액

**요구사항**: 동아리 회원은 회계 내역과 잔액을 볼 수 있다. MANAGER 이상만 기록/수정/삭제. 시스템 관리자는 모든 동아리 조회 가능. 목록은 페이지마다 "이전 잔액"을 이어서 표시.

**기존 구현** (`FinancialRecordService`)
- `[결함]` 수정/삭제에서 기록이 요청 경로의 동아리 소속인지 확인하지 않는다. 다른 동아리 관리자가 기록 id만 알면 수정/삭제 가능(IDOR). 코드상 "비활성화 기능" 구간이며 운영 노출 여부는 확인 필요.
- `[결함]` 잔액 조회(`findBalance`)는 시스템 관리자를 허용하지만 목록 조회(`findFinancialRecords`)는 허용하지 않는다. 같은 자원에 정책이 둘.

**AIP 표현**
```aip
relation canReadFinance(m: Member, c: Club) = exists membership(m, c) or m.isSuperAdmin

entity FinancialRecord {
  club: Club
  at: Time
  amount: Money                        // 부호 있는 금액
  visible to actor when canReadFinance(actor, club)
}

command EditFinancialRecord(record: FinancialRecord, input: FinancialInput) {
  allow managerOf(actor, record.club)
  do { set record.at = input.at, record.amount = input.amount, record.memo = input.memo }
}

query FinancialLedger(club: Club) {
  allow canReadFinance(actor, club)
  from FinancialRecord f where f.club = club
  sort by f.at desc
  page 30 by keyset
  select { id at amount memo balance: running_sum(f.amount) over club order by f.at }
}
```
IDOR가 표현 불가능한 이유: 명령이 받는 것은 "기록"이고 권한은 **그 기록의 동아리**로 계산된다. 경로의 clubId와 기록의 club이 다를 수 있다는 개념 자체가 없다.

**정적 보장**: 자원별 정책 일관성(같은 엔티티의 읽기 정책은 `visible to` 하나).
**런타임 보장**: `running_sum`은 윈도 함수로 하강. 페이지 경계의 이전 잔액은 keyset 기준 선행 합계로 계산.
**누수 가능성**: 누적합 성능. 기록이 많아지면 스냅샷 잔액 테이블이 필요해지고, 그건 `derived ... maintained` 형식으로.

---

## Case 12. 시간 기반 작업 (마감 알림, 미처리 지원서, 보존 기한)

**요구사항**: 매일 0시 관심 모집 D-1/D-7 알림, 매주 월요일 미처리 지원서가 있는 동아리에 알림, 지원서는 마감 6개월 후 영구 삭제(삭제 7일 전 메일), 신고 처리된 글은 3개월 후 물리 삭제.

**기존 구현**: `@Scheduled` 메서드 여러 개. 다중 인스턴스 배포 시 중복 실행 방지 장치가 보이지 않는다(`[확인 필요]`: ShedLock 등 외부 설정 여부). 보존 기한 삭제는 코드에서 찾지 못함(`[확인 필요]`).

**AIP 표현**
```aip
schedule RecruitmentDeadlineReminder every day at 00:00 tz "Asia/Seoul" {
  for Recruitment r where r.period.end in [today + 1d, today + 2d) or r.period.end in [today + 7d, today + 8d)
  notify bookmarkers(r) via alarm.template("recruitment-deadline") { days: days_until(r.period.end) }
}

retain Apply for 6mo after recruitment.period.end
  then purge
  notify member 7d before via mail.template("apply-expiring")

retain ReportedPost for 3mo after moderatedAt then purge
```

**정적 보장**: 스케줄 식별자와 타임존 명시. 보존 규칙과 `on erase`/참조 정책 충돌 검사.
**런타임 보장**: 스케줄은 Postgres 기반 내구성 타이머와 리더 잠금(advisory lock)으로 **클러스터에서 한 번만** 실행. 놓친 실행은 정책(`catch up once | skip`)에 따라 처리.
**누수 가능성**: 대량 삭제의 부하. 하강 결과가 배치 크기로 나눠 삭제하도록.

---

## Case 13. 주문 생성 + 재고 예약 + 쿠폰

**요구사항**: 주문 시 재고를 예약(15분 유지), 쿠폰은 1인 1회. 결제 안 하면 예약 해제.

**AIP 표현**
```aip
entity Coupon {
  owner: Member
  status: CouponStatus = ISSUED
  lifecycle status {
    ISSUED -> USED
    USED -> ISSUED
  }
}

command CreateOrder(lines: List<OrderLineInput> max 50, coupon: Coupon?) idempotent {
  allow authenticated
  require when coupon: coupon.owner = actor and coupon.status = ISSUED else COUPON_NOT_USABLE
  do {
    insert Order { customer: actor, status: PENDING } as order
    insert OrderItem from lines l { order, product: l.product, quantity: l.quantity, price: l.product.price }
    reserve stock of order.items for 15m else OUT_OF_STOCK          // L3 형식 (inventory)
    when coupon: set coupon.status = USED, order.coupon = coupon
    set order.total = sum(order.items i: i.price * i.quantity) - discount(coupon)
  }
  emit OrderCreated { order }
}

on StockReservationExpired e when e.order.status = PENDING do {
  release stock of e.order.items
  set e.order.status = EXPIRED
  when e.order.coupon: set e.order.coupon.status = ISSUED
}
```
`reserve stock ... for 15m`은 inventory L3 형식이다. 하강: 재고 행을 정렬 잠금 → 차감 → 예약 행 insert → 만료 타이머 등록.

**정적 보장**: 쿠폰 상태 전이 검사. 예약에는 반드시 해제 경로(만료 또는 확정)가 있어야 함(E-RESERVE-LEAK).
**런타임 보장**: 재고는 잠금 순서 고정으로 교착 없이 차감, `stock >= 0` 불변식은 CHECK. 만료는 내구성 타이머.
**누수 가능성**: 재고 모델이 도메인마다 다르다(옵션, 창고). inventory 형식은 표준 라이브러리 중 가장 추상화가 새기 쉬운 곳.

---

## Case 14. 결제

**요구사항**: 외부 PG로 결제. 네트워크 타임아웃이 나도 이중 결제는 없어야 한다. 결제 승인 후 서버가 죽어도 주문 상태가 결국 맞아야 한다. PG 웹훅으로도 확정될 수 있다.

**AIP 표현**
```aip
command PayOrder(order: Order, method: PaymentMethodToken) idempotent by order {
  allow order.customer = actor
  require order.status = PENDING else ORDER_NOT_PAYABLE
  do {
    payments.authorize(order.total, method, key: order.id) as auth      // reservable
    set order.status = PAID, order.paymentRef = auth.ref
    confirm stock of order.items
  }
  emit OrderPaid { order, amount: order.total }
}

webhook PaymentCaptured via payments.webhook {
  on CAPTURED(e) do { update Order o where o.paymentRef = e.ref and o.status = PAID set settledAt = now }
}
```

**컴파일러의 배치** (`aip explain PayOrder`)
1. 멱등 키(order.id) 선점
2. `payments.authorize` 실행 (커밋 전, reservable). 타임아웃이면 결과는 "모름" → 같은 키로 `payments.lookup` 조회 후 판정
3. DB 트랜잭션: 주문 잠금, 상태 재확인, 갱신, 재고 확정, outbox(capture 요청, OrderPaid)
4. 커밋 실패 → `payments.void(auth)` 보상 (saga 기록에서)
5. 커밋 성공 → outbox가 `payments.capture` 실행(멱등 재시도)
6. 프로세스가 3과 4 사이에 죽으면 → 복구 시 saga 기록을 보고 lookup 후 void 또는 확정

**정적 보장**: reservable 효과에 보상(void)이 서명에 존재, 재시도 가능한 효과는 멱등 키 또는 lookup capability 필수(E-EFFECT-RETRY-UNSAFE). 이 명령은 커밋 전 외부 효과가 있으므로 직렬화 실패 시 **자동 재시도 불가**로 판정되고, 대신 멱등 키 재요청으로 안전하게 재시도하라고 계약에 적힌다.
**런타임 보장**: saga 상태가 Postgres에 기록되어 크래시 후 복구.
**누수 가능성**: PG마다 authorize/capture 모델이 다르다(즉시 매입만 지원하는 PG). 그 경우 extension이 `reservable` 대신 `irreversible + refundable`로 서명을 낮추고, 컴파일러는 배치를 바꾼다(커밋 후 결제 + 실패 시 환불). **서로 다른 PG를 같다고 가장하지 않는다.**

---

## Case 15. 주문 취소

**요구사항**: 배송 전까지 취소 가능. 결제된 주문이면 환불. 재고 복원, 쿠폰 복원, 주문 목록 캐시 무효화, 이벤트를 Kafka로 발행.

**AIP 표현**
```aip
command CancelOrder(order: Order) idempotent by order {
  allow order.customer = actor or actor.role = ADMIN
  require order.status in [PENDING, PAID, PREPARING] else ORDER_NOT_CANCELLABLE
  do {
    when order.status = PAID or order.status = PREPARING:
      payments.refund(order.paymentRef, order.total, key: order.id)   // deferred, idempotent
    update Product p via order.items i set p.stock += sum(i.quantity)
    when order.coupon: set order.coupon.status = ISSUED
    set order.status = CANCELLED
  }
  emit OrderCancelled { order } to kafka topic "orders" key order.id
}

query MyOrders cached 60s per actor { ... }   // 무효화 대상은 컴파일러가 도출
```

**정적 보장**: `MyOrders`의 읽기 집합(Order.status 등)과 `CancelOrder`의 쓰기 집합이 겹치므로 무효화가 자동 생성됨. Kafka 발행은 outbox 경유(at-least-once), 소비자 계약에 "중복 가능, key 단위 순서"가 명시됨.
**런타임 보장**: 환불은 커밋 후 멱등 재시도. 환불이 영구 실패하면 → 주문은 이미 CANCELLED. 이것이 도메인이 답해야 할 질문이다("환불 실패한 취소"를 어떤 상태로 둘 것인가). 컴파일러는 **이 질문이 존재한다는 것**을 알려준다: deferred 효과에 실패 처리 선언이 없으면 W-EFFECT-NO-FAILURE-PATH.
**누수 가능성**: 부분 환불, 부분 취소. 금액 계산 규칙이 커지면 순수 함수(`fn`) escape hatch.

---

## 요약: 케이스별 구성요소 사용

| # | 케이스 | relation | 집합 연산 | lifecycle | 기수 불변식 | erase | 효과 등급 | 시간 | 캐시/카운터 | 이벤트 |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 지원서 제출 | ○ | | ○ | unique | | staged | | | ○ |
| 2 | 일괄 합격 | ○ | ○ | ○ | unique | | deferred | | | ○ |
| 3 | 관리자 위임 | ○ | | | exactly one | | | | | |
| 4 | 상태 변경 | ○ | ○ | | | | | | | |
| 5 | 탈퇴 | | ○ | | exactly one + repair | ○ | deferred | | | |
| 6 | 학교 인증 해제 | | ○ | | | | | | | |
| 7 | 공고 등록 | ○ | | | no overlap | | staged | | | ○ |
| 8 | 공고 상세 | ○ | | | | | observational | | cached, counter | |
| 9 | 공고 목록 | | | | | | | | counter | |
| 10 | 댓글 가시성 | ○ | | | | ○ | | | | |
| 11 | 회계 | ○ | | | | | | | | |
| 12 | 스케줄 | | ○ | | | | deferred | ○ | | |
| 13 | 주문 생성 | | ○ | ○ | CHECK | | reservable | ○ (만료) | | ○ |
| 14 | 결제 | | | ○ | | | reservable, deferred | | | ○ |
| 15 | 주문 취소 | | ○ | ○ | CHECK | | deferred | | cached | ○ (kafka) |

## 결론

1. 15개 케이스 모두 L2 Core + L3 형식으로 표현 가설이 선다. 범용 제어 흐름(루프, 재귀, 예외)은 한 번도 필요하지 않았다.
2. 아리아리 코드에서 확인한 결함 10건 이상이 **표현 불가능**(IDOR, 규칙 이중 구현, 트랜잭션 중 외부 호출) 또는 **정적/제약 검출**(중복 지원 경쟁, 기간 중복 경쟁, 관리자 0명, 조회수 키) 범주에 들어간다.
3. 새로 드러난 open question: 부분 성공 semantics(Case 2), 불변식 복구 규칙(Case 5), 스냅샷 의미(Case 7), 캐시 분할 기준(Case 8), deferred 효과 영구 실패 시 도메인 상태(Case 15), inventory 형식의 일반성(Case 13).
4. 이 문서의 모든 케이스는 M0에서 **conformance 테스트**(기대 진단 + 기대 런타임 동작)로 변환된다. 언어가 바뀌어도 이 케이스들이 통과해야 한다.
