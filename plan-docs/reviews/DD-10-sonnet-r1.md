# DD-10 독립 비판 (Sonnet, 1라운드)

> 대상: `plan-docs/decisions/DD-10-viewer-derived-values.md` (초안, 133행)
> 기준: `docs/PRINCIPLES.md`, `plan-docs/sources/founder-intent-handoff-2026-10-03.md`, `design-operating-rules-2026-10-03.md`
> 일관성 대조: DD-01(I1~I7, 공개 집계, 간선 정책), DD-03(plan cache, T-I2), DD-04(정의 문법)
> 표기: 사실 주장은 `파일:행`. 아리아리 경로는 `.../ariari/` = `/Users/winterholic/development/projects/ariari/ariari-backend/src/main/java/com/ariari/ariari/` 의 약칭이다. 직접 `sed`/`grep`으로 확인했다.
> 이 문서는 지적과 제안이다. 창시자 미결정 사항을 확정하지 않는다.

---

## 1. 사실 검증 (DD-10 B.1, B.2)

### 1.1 B.1 아리아리

| # | DD-10 주장 | 판정 | 근거와 정정 |
|---|---|---|---|
| 1 | `RecruitmentData.fromEntity(recruitment, reqMember)`가 `isMyBookmark`를 붙인다 (69·93·99~126행 근처) | **맞음** | `.../recruitment/recruitment/dto/RecruitmentData.java` 69행 필드, 75~96행 단건(93행 `getMyBookmarkRecruitments(reqMember).contains(recruitment)`), 98~106행 목록(99행 집합 1회 생성), 126행 `myBookmarkRecruitments.contains(recruitment)`, 134~142행 헬퍼 |
| 2 | "요청자의 북마크 집합을 **따로 읽어**" | **부분적으로 맞음** | 쿼리를 따로 날리는 것이 아니라 `reqMember.getRecruitmentBookmarks()`(137행)로 회원 엔티티의 연관 컬렉션 전체를 읽는다(`domain/member/Member.java` 115~116행 `@OneToMany`). fetch 전략과 실제 SQL은 **확인 못함: 실행하지 않았고 전역 fetch 설정을 보지 않았다** |
| 3 | "목록 응답 DTO마다 이 처리가 들어간다" | **과장** | 모집 쪽 계산은 `RecruitmentData` 한 클래스에 있고 `RecruitmentListRes` 3개 팩토리(`dto/res/RecruitmentListRes.java` 27·41·51행), `RecruitmentRes` 17행, `RecruitmentDetailRes` 42·55행이 모두 이를 재사용한다. 반복은 모집 DTO 사이가 아니라 **리소스 종류 사이(모집·동아리·활동)와 계산 방식 사이**에 있다(2절). 오히려 예외가 두 곳이다. `RecruitmentInClubData.java` 49행은 `isMyBookmark`에 `null`을 넣고(`RecruitmentListRes.java` 33~37행 `createInClubRes`), `apply/temp/dto/res/ApplyTempDetailRes.java` 59행은 `RecruitmentData.fromEntity(recruitment, null)`로 항상 false를 만든다 |
| 4 | `findRecruitmentDetail`이 `isMyClub`을 조회해 붙인다 | **맞음** | `.../recruitment/recruitment/RecruitmentService.java` 78·81행. `clubMemberRepository.findByClubAndMember(...).isPresent()` |
| 5 | `isMyApply`(지원 여부) | **맞음** | 같은 파일 79·82행. `applyRepository.findByMemberAndRecruitment(...).isPresent()` |
| 6 | `myRecentApplyTempId`(최근 임시 저장) | **맞음** | 같은 파일 85~90행. `findFirstByMemberAndRecruitmentOrderByCreatedDateTimeDesc` |
| 7 | (DD-10 75행, C.1 비즈니스 규칙 행) "내 동아리"의 정의(활성 회원만인가)가 서버 한 곳이 되는 장점 | **예시가 현재 코드와 다름** | 원본 `isMyClub`은 상태를 보지 않는다. 같은 파일 81행과 `club/clubmember/ClubMemberRepository.java` 20행 `findByClubAndMember`에 `ClubMemberStatusType`(ACTIVE/INACTIVE/WITHDRAWN, `enums/ClubMemberStatusType.java` 6~10행) 조건이 없다. `ClubMember`는 소프트 삭제(`commons/entity/ClubMember.java` 20~21행 `@SQLRestriction`)라 탈퇴가 상태 변경인지 삭제인지는 **확인 못함: 탈퇴 서비스 경로를 읽지 않았다**. 따라서 V1로 옮길 때 "활성만"으로 정하면 동작이 바뀐다. 장점 자체는 타당하나, 전환 시 결정이 필요한 사실로 적어야 한다 |
| 8 | 비로그인 처리 (C.2-3의 근거 암시) | **DD-10에 없음, 확인함** | 비로그인이면 `isMyBookmark=false`(`RecruitmentData.java` 135~136행 빈 집합), `isMyClub/isMyApply=FALSE`, `myRecentApplyTempId=null`(`RecruitmentService.java` 78~85행). 교내 모집이면 값 이전에 `NoProperSchoolAuthException`(`commons/validator/GlobalValidator.java` 39~51행). 즉 불리언은 false, id는 null이라는 구분이 이미 있다. DD-10은 "false/null"로만 쓴다(C.2-3) |

### 1.2 B.2 지난 AIP PoC (`examples/ariari/app.aip`)

| # | DD-10 주장 | 판정 | 근거와 정정 |
|---|---|---|---|
| 1 | `isMyClub: exists membership(actor, r.club)` | **맞음** | `app.aip` 418행(`RecruitmentDetail`) |
| 2 | `isMyApply: exists Apply a where a.recruitment = r and a.member = actor` | **맞음** | 419행 |
| 3 | `myLatestDraft: (latest ApplyDraft d ... by d.createdAt).id` | **맞음** | 420행 |
| 4 | `isBookmarked: exists RecruitmentBookmark b where ...` | **맞음** | 442행(`RecruitmentList`) |
| 5 | (누락) PoC에는 이 식을 **이름 붙은 relation으로 빼는 장치가 이미 있다** | **DD-10이 빠뜨림** | `app.aip` 88행 `relation blocked(a,b) = exists Block ...`, 139행 `relation membership(m, c): ClubMember`, 142~144행 `managerOf/adminOf/outranks`, 519행 `schoolVerified`. 즉 PoC의 viewer 값은 "relation(actor, x)를 select에서 부른 것"이다. V1을 새 `viewer:` 블록으로 제안하기 전에 이 기존 구조와의 관계를 밝혀야 한다(3.3절) |
| 6 | (누락) PoC는 viewer 값을 `RecruitmentList`/`RecruitmentDetail` 둘 외에도 쓴다 | **DD-10이 빠뜨림** | `ClubDetail`의 `myMembership`(207행)·`isBookmarked`(208행), `ActivityList`의 `liked`(731행), `ActivityDetail`의 `liked`(743행). 반대로 댓글의 좋아요·본인 여부는 PoC에 없다(744~750행 `comments`에는 `likeCount`만) |
| 7 | (누락) 차단은 PoC에서 **값이 아니라 행 정책**이다 | **DD-10이 빠뜨림** | `app.aip` 672행 `visible to actor unless blocked(actor, author) or blocked(author, actor)`. Spring은 숨기지 않고 `isBlocked` 플래그를 내려준다(2절). 동작이 다르다 |

### 1.3 DD-10 본문의 참조 정확성

- 헤더의 "관련: DD-03(plan cache)": 맞다. 다만 C.1 표(77행)와 C.2-2는 "DD-03 층 2 원칙"이라 쓰는데, "결과 캐시는 행위자별"이라는 문장은 DD-03에 **없다.** DD-03 90행·DD-01 164행은 plan cache에 대해 "정책이 매개변수로 들어간 plan만 재사용, 판정 결과 공유 금지"만 말한다. DD-01 166행은 결과 캐시를 "무효화와 동기화 범위(T13)"로만 언급한다. 따라서 C.2-2는 근거 인용이 아니라 **DD-10이 새로 하는 제안**이다. 인용 표기를 바로잡거나 새 불변식으로 격상해야 한다.
- DD-10은 T-I2(DD-03 83행, 등록 식별자를 알아도 권한이 생기지 않고 actor가 바뀌면 새 판정)를 인용하지 않는다. viewer 값은 등록(`registered-only`) 요청 안에서도 매번 actor로 평가돼야 하므로 T-I2가 직접 관련된다(5절).
- DD-10은 DD-03과 달리 **전제 문장이 없다.** DD-03 4행은 "전제: DD-01 수정 대안 2 ... 다른 안으로 결정되면 다시 쓴다"를 명시한다. DD-10 5행은 일반 규칙을 DD-01에 맡기고 110행은 DD-01 F1에 영향받는다고만 쓰며 헤더에는 전제를 두지 않았다(6절).

---

## 2. E1 수행: 아리아리 응답 DTO의 보는 사람 기준 값 전체 목록

### 2.1 수집 방법과 한계

- `domain/` 아래 모든 `private (Boolean|boolean)` 필드를 훑고, `reqMember`/`reqMemberId`를 받는 `*Data`/`*Res`를 전부 열었고, `my*` 접두 필드와 스키마 설명의 "내가·본인·나의"를 grep했다. 서비스에서 불리언을 만들어 DTO로 넘기는 곳과 MyBatis 매퍼(`src/main/resources/mapper/ClubActivity.xml`)도 봤다.
- **확인 못함:** 불리언이 아닌 보는 사람 기준 값을 이 패턴 밖에서 만드는 곳이 있을 수 있다(예: 이름이 `is`/`my`로 시작하지 않는 파생 값). 아리아리 전 코드를 정독하지는 않았다. 아래 숫자는 "찾은 것"의 하한이다.
- `isChecked`(알림 3종, `ClubAlarmData.java` 27행, `MemberAlarmData.java` 27행)와 `isFixed`(`ClubNoticeData.java` 28행)는 **제외**했다. 알림 행은 본인 소유 행의 저장 필드이고, `isFixed`는 보는 사람과 무관하다.

### 2.2 전체 목록 (11개 필드)

| # | 필드 (파일:행) | 리소스 | 계산 위치 | 비로그인 값 |
|---|---|---|---|---|
| 1 | `RecruitmentData.isMyBookmark` (`recruitment/dto/RecruitmentData.java` 69) | 모집 | 같은 파일 93·126·134~142 | false |
| 2 | `ClubData.isMyBookmark` (`club/club/dto/ClubData.java` 53) | 동아리 | 같은 파일 81~98(오버로드마다), 142~149. **또 하나의 경로**: `ClubListRes.fromPage`가 사후에 `setIsMyBookmark(true)` (`dto/res/ClubListRes.java` 25~34) | false |
| 3 | `RecruitmentDetailRes.isMyClub` (`recruitment/dto/res/RecruitmentDetailRes.java` 32) | 모집 상세 | `RecruitmentService.java` 78·81 | false |
| 4 | `RecruitmentDetailRes.isMyApply` (같은 파일 34) | 모집 상세 | `RecruitmentService.java` 79·82 | false |
| 5 | `RecruitmentDetailRes.myRecentApplyTempId` (같은 파일 38) | 모집 상세 | `RecruitmentService.java` 85~90 | null |
| 6 | `ClubActivityData.isMyLiked` (`club/activity/dto/ClubActivityData.java` 57) | 활동후기 | 상세 `ClubActivityService.java` 223, 목록 SQL `resources/mapper/ClubActivity.xml` 49·71, 비로그인 `ClubActivityService.java` 167 `setMyLiked(false)` | false |
| 7 | `ClubActivityCommentData.isMyLiked` (`dto/ClubActivityCommentData.java` 49) | 활동후기 댓글 | `ClubActivityAssembler.java` 84·98 (`LikeMemberSet.contains(reqMember)`) | 비로그인용 오버로드는 호출자 인자 |
| 8 | `ClubActivityCommentData.isMine` (같은 파일 52) | 댓글 | 같은 파일 151 `reqMember.equals(comment.getMember())`. 로그인 안 한 경로는 102행에서 `isMine(false)`로 **하드코딩** | false |
| 9 | `ClubActivityCommentData.isBlocked` (같은 파일 55) | 댓글 | `ClubActivityAssembler.java` 85·99 `blockSet.contains(작성자)`. `blockSet`은 **내가 차단한 사람과 나를 차단한 사람의 합집합**(`club/activity/ClubActivityUtils.java` 37~44, `ClubActivityService.java` 226~228) | 빈 집합 |
| 10 | `ClubDetailRes.clubMemberData` (`club/dto/res/ClubDetailRes.java` 19. 설명 15행 "나의 동아리 회원 데이터, 속하지 않으면 null") | 동아리 상세 | 같은 파일 24·31 `ClubMemberData.fromEntity(reqClubMember)` (id·활동명·역할·상태·`MemberData` 중첩) | null |
| 11 | `ClubData.schoolData` (`ClubData.java` 48). 오버로드 `fromEntity(club, myBookmarkClubs, reqMember)`에서 **보는 사람의 학교**를 넣는다 (83~85) | 동아리 목록 | 같은 파일 82~85 | null |

덧붙임.
- 11번은 동아리의 학교가 아니라 `reqMember.getSchool()`이다. 동아리 목록에 교외 동아리가 섞이면 값의 의미가 달라진다. 의도인지 결함인지는 **확인 못함: 이 경로를 쓰는 엔드포인트의 필터를 끝까지 추적하지 않았다.** 어느 쪽이든 "누가 보느냐에 따라 달라지는 값"이므로 목록에는 넣었다.
- 1번과 별도로 `RecruitmentInClubData`는 `isMyBookmark` 자리에 `null`을 넣는다(`recruitment/dto/RecruitmentInClubData.java` 49). 이 목록(`/clubs/{id}/recruitments`)은 필드는 있고 값이 없다.
- 계산 방식이 **네 가지**다. (a) 회원 엔티티의 연관 컬렉션을 통째로 읽어 집합 대조(1, 2). (b) 리포지토리 `findBy...isPresent()`(3, 4, 6 상세). (c) 매퍼 SQL의 `LEFT JOIN ... CASE WHEN`(6 목록). (d) 응답 조립 뒤 setter로 덮어쓰기(2의 `fromPage`). DD-10의 "반복"은 이 네 방식이 같은 질문("내가 이 대상과 간선이 있나")에 각각 답한다는 사실이 정확한 표현이다.

### 2.3 종류별 분류

| 종류 | 설명 | 해당 필드 | 수 |
|---|---|---|---|
| K1 내 소유 간선의 존재 | `exists 간선 where 대상 = 이 행 and member = actor` | 1, 2, 4, 6, 7 | 5 |
| K2 내 소속 관계의 존재 | 간선이 내 것이지만 의미에 "상태·역할" 정의가 얹힘 | 3 (`isMyClub`) | 1 |
| K3 내 간선 중 하나를 골라 스칼라로 | `latest ... by createdAt`의 id | 5 | 1 |
| K4 내 간선 행을 통째로 (중첩 객체) | 역할·상태·활동명 | 10 | 1 |
| K5 행의 필드와 actor 비교 | `comment.author = actor` | 8 | 1 |
| K6 남이 쥔 간선을 근거로 | 나를 차단한 사람 | 9 | 1 |
| K7 actor 자신의 속성을 행에 붙임 | `actor.school` | 11 | 1 |
| 합계 | | | 11 |

### 2.4 V1 선언으로 덮이는 비율

DD-10 B.3 V1의 문법은 `exists ... where b.member = actor` 형태의 **불리언 한 종류**뿐이다(DD-10 47~52행).

| 기준 | 덮이는 필드 | 비율 |
|---|---|---|
| (가) DD-10 V1 예시 그대로 (K1 + K2) | 1, 2, 3, 4, 6, 7 | **6/11 (55%)** |
| (나) PoC가 이미 쓰는 `latest ... by` 와 `membership(actor, x) { ... }` 중첩 선택을 V1에 포함 (K3, K4 추가) | + 5, 10 | 8/11 (73%) |
| (다) actor와 행 필드의 동등 비교 허용 (K5 추가) | + 8 | 9/11 (82%) |
| (라) 남이 쥔 간선 (K6), actor 속성 부착 (K7) | 9, 11 | 나머지 2/11 (18%) |

해석.
1. **결론은 V1 방향이 맞다는 쪽이다.** 11개 중 9개가 "내 소유 간선/행에서 파생"이라는 한 가족이다. 그러나 이 가족은 불리언만이 아니다. 11개 중 2개(5, 10)는 **내 간선 행의 내용**을 원한다. DD-10 V1이 `exists`만 그리면 덮이는 폭이 55%로 줄어든다.
2. 선언 수는 "한 줄"이 아니다. 리소스마다 따로 선언하므로 위 필드 수만큼, 즉 모집·동아리·활동·댓글·모집상세에 걸쳐 11줄 안팎이다. 줄어드는 것은 **구현 방식 4종과 계산 위치 10곳 안팎(표의 계산 위치 열을 세면 11곳)**이지 "한 줄 대 11줄"이 아니다. DD-10 92행의 "선언 한 줄로"는 과장이다.
3. K6(9번)은 C.2-1과 정면으로 부딪친다(4절). K7(11번)은 `actor.school`을 행마다 붙이는 것인데, 행에 붙일 이유가 약하다(응답 최상위에 한 번 있으면 충분). viewer 값이 아니라 **세션 정보**로 분리해야 할 후보다.
4. 호출 빈도 근거(이 값이 화면에서 얼마나 자주 쓰이는지)는 이 E1로 알 수 없다. **확인 못함.**

---

## 3. 철학 정합성

### 3.1 V1은 DD-01의 "화면이 아니라 리소스 단위"와 모순되는가

**좁은 의미에서는 모순이 아니다.** DD-01은 화면별 서버 작업을 없애고 capability를 "리소스에 한 번" 여는 쪽이다(DD-01 60행 "화면이 아니라 리소스 단위", 65행 "화면과 무관하게 리소스마다 한 번"). V1은 리소스(`Recruitment`)에 값을 선언하고(DD-10 46~53행) 화면은 이름을 `select`로 고르므로 단위가 리소스다. DD-01이 이미 `publicAggregate: { bookmarkCount: ... }`(DD-01 77행)로 같은 모양(서버가 이름 붙인 파생 결과를 capability로 연다)을 인정했으므로 선례도 있다. 인용에 대해 한 가지 짚는다. 과제 지시문의 "서버가 화면별 조회를 선언하지 않는다"는 DD-01에 그 문장 그대로는 없고, 위 60·65행의 요약이다. 이 문서도 같은 의미로 쓴다.

**그러나 세 가지 조건이 지켜져야 모순이 안 생긴다. DD-10은 셋 다 쓰지 않았다.**

1. **무엇이 `viewer:` 블록에 들어갈 수 있는지의 경계가 없다.** 블록이 열려 있으면 `canApplyNow`, `showManagerBanner` 같은 **화면 모양의 값**이 리소스 선언으로 들어와 "화면별 서버 작업"이 이름만 바꿔 되살아난다(DD-03 74행이 승인 절차에 대해 한 경고와 같은 구조). E1의 11개는 모두 "내가 이 대상과 맺은 간선/행"에서 나왔다(2.3절). 경계 후보: "actor와 리소스 사이의 도메인 관계(간선, 소속, 소유)에서만 파생, 화면 이름이 아니라 관계 이름". 이 경계를 불변식으로 쓰지 않으면 V1은 DD-01이 경계한 대안 1(서버가 조회를 하나씩 선언)의 필드 수준 재현이 된다.
2. **새 viewer 값마다 서버 수정이 생긴다.** E1 값 중 `isMyClub`(3)과 `myRecentApplyTempId`(5)는 상세 한 화면에서만 쓰인다. 처음 필요가 생기는 순간은 항상 "화면 하나의 요구"다. V1은 그때마다 리소스 선언을 고치는 구조라, DD-01 E2의 지표(서버 수정 횟수, 정의 종류, 계약 동시 변경 수. DD-01 195행)로 대안 1·3 대비 실제로 적은지 재야 한다. DD-10 E절에는 이 지표가 없다(E1은 비율, E2는 공격뿐).
3. **viewer 값을 `filter`/`sort`에 쓸 수 있는지가 비어 있다.** DD-01 I1은 select, filter, sort를 별도 capability로 기본 거부한다(DD-01 141행). 아리아리에는 viewer 값으로 거르는 엔드포인트가 이미 있다. `/recruitments/my-bookmarks`(`RecruitmentController.java` 108~114행), 동아리 `my-bookmarks`(`ClubController.java` 76행, `hasActiveRecruitment` 파라미터 79행). 모집 쪽은 DD-01 E1이 "북마크 행 정책 = 본인 + 모집 traverse"로 풀었다(DD-01 211행). 즉 **viewer 값으로 거르는 일은 값이 아니라 간선 traverse로** 가는 것이 DD-01에서 이미 정해진 길인데, DD-10은 이를 언급하지 않아 V1에서 `where: { isBookmarked: true }`가 되는지 `select`만인지가 비어 있다. `select`만 열면 이 길과 일관되고, `filter`까지 열면 행마다 서브쿼리로 걸러야 해서 I4 비용 문제가 생긴다.

### 3.2 V2와 "한 의도 한 구조"

PRINCIPLES §4 6번(같은 의도를 여러 방식으로 쓸 여지를 만들지 않는가)과 handoff 208행("동일한 의도는 가능한 한 동일한 구조로")에 비추어 DD-10 안에 **내부 모순이 있다.**

- D절 94행은 "V1: 이름 하나. V2: 식의 모양이 다양"이라 평가하고, 이탈 방지 점검 127행은 "AI의 불필요한 선택을 줄이는가: 예. 이름 하나"라고 답한다.
- 그러나 G절 116행의 추천은 "V1 기본, **V2는 본인 간선 exists만 허용**, V3는 SDK 보조"다. 즉 같은 의도("내가 북마크했나")를 표현하는 길이 **셋**이다(이름 선택, 요청 안의 `exists`, 클라이언트 결합).
- 이탈 방지 점검 125행은 "V2를 본인 간선 범위로 열어 표현을 보완"이라고 인정한다. 127행의 "이름 하나"와 한 표 안에서 충돌한다. 평가는 V1 단독으로 하고 추천은 혼합으로 하는 구조다.

DD-01 C.3의 canonical 정리(158행)가 V2의 `exists(bookmarks where member=actor)`와 V1 이름을 내부에서 같은 형태로 모을 수는 있다. 그러나 canonical은 **서버 내부 정리**이고, AI가 쓸 때 고르는 표면은 여전히 둘이다. PRINCIPLES §4 6번이 막으려는 것은 "작성 시점의 선택지"다. 표면은 하나만 남겨야 한다. 대안은 4절에서 낸다.

### 3.3 새 구성요소를 하나 더 얹는가 (기존 구조와의 중복)

V1의 `viewer: { 이름: "식" }`은 기존에 있거나 제안된 이름 붙은 파생 값 구조와 겹친다.

| 구조 | 위치 | 구별 속성 |
|---|---|---|
| `predicate open = ...` | DD-04 B.0 3번(34행), B.1 60행 | 행의 판정식. 같은 문법 안에서 `actor`를 쓸 수 있음(`rows when ... actor.school`, DD-04 61행) |
| `public aggregate bookmarkCount = count(bookmarks)` | DD-01 77행, DD-04 B.1 64행 | 집계, 정책을 건너뛰는 공개 결과 |
| PoC `relation membership(m, c)`, `managerOf` 등 | `app.aip` 139~144행 | actor를 매개변수로 받는 이름 붙은 관계 |
| 제안 `viewer: {...}` | DD-10 46~53행 | actor 의존 파생 값 |

네 가지가 모두 "서버가 이름을 붙이고 호출자는 이름으로 고르는 파생 값"이고, 차이는 (집계인가, actor를 쓰는가) 두 속성이다. 그러면 작성자가 `viewer`, `predicate`, `publicAggregate` 중 어디에 둘지 **고르게 된다.** PRINCIPLES §4 6번이 경고한 선택지다. 더 위험한 것은 오분류다. 작성자가 actor를 쓰는 식을 `publicAggregate`나 `predicate`에 넣으면 결과가 공유 캐시로 샐 수 있다(5절). 따라서:

- 파생 값의 종류는 **작성자가 선언하지 않고 식에서 추론**하는 쪽이 안전하다. 식이 `actor`(직접 또는 `relation` 경유)를 참조하면 actor 의존, 아니면 공용. 추론 결과가 캐시 키 구성과 비로그인 기본값 규칙을 결정한다.
- PoC의 `relation`과 DD-04의 `predicate`를 재사용하는 길을 먼저 검토해야 한다. DD-04 B.0의 11개 항목(30~42행)에는 viewer 값이 없으므로 DD-04의 네 표기 모두 확장이 필요하다. DD-10은 그 비용을 적지 않았다.

### 3.4 handoff 핵심 문제 A와의 관계

handoff 744~751행의 핵심 문제 A는 "클라이언트가 서버에 미리 정의된 이름의 Intent만 호출할 수 있다"는 것이다. V1이 **유일한** viewer 경로가 되면, 호출자가 "내가 북마크한 모집 중 마감 5일 이내" 같은 조합을 표현하려 할 때 viewer 값 이름과 filter capability가 열려 있는지에 매번 의존한다. 반대로 V1 이름은 `select` 안의 필드 이름이지 호출 가능한 Intent 이름이 아니므로 문제 A의 정의 그대로는 아니다. 따라서 V1은 문제 A를 재현하지 않는다고 판단하나, 이 판단은 3.1의 세 조건(경계, 서버 수정 비용 측정, filter 가능 여부)이 문서에 들어갈 때 성립한다. 현재 문서만으로는 "조건부 정합"이다.

---

## 4. 대안 비판

### 4.1 빠진 대안: viewer 값을 "값"이 아니라 "관계"로 노출한다 (V4, V5)

DD-10의 세 안은 모두 "불리언 값을 만든다"를 전제한다. 그런데 E1의 11개 중 K3, K4(5번, 10번)는 불리언이 아니라 **내 간선 행의 내용**이고, K1의 북마크도 북마크 해제 때 쓰는 대상이 간선 행이다. 값이 아니라 **내 간선 관계**를 노출하면 더 적은 구조로 같은 필요가 풀린다. 아래는 `개념 예시`이며 문법을 확정하자는 것이 아니다.

**V4. 간선 정책이 걸린 기존 관계를 그대로 읽는다 (새 구성요소 0).**
DD-01은 이미 `RecruitmentBookmark`의 행 정책을 `member == actor`로 두었다(DD-01 79~81행, 같은 정책이 "내 북마크 모집"의 근거, DD-01 211행). I2 때문에 호출자가 `bookmarks` 관계를 select하면 **본인 것만** 나온다(`[]` 또는 `[내 행]`).

```ts
aip.read("Recruitment", { select: { title: true, bookmarks: { id: true } } })  // 호출자: length > 0 이면 북마크함
```

- 장점: `viewer:` 블록, `$actor` 토큰, C.2-1 규칙이 모두 필요 없다. 같은 의도의 표면이 하나다(관계 select).
- 한계 1. `bookmarkCount: count(bookmarks)`가 같은 관계 이름에 **다른 평가 방식**(정책을 건너뛰는 공개 집계, DD-01 77·95행)을 붙인다. 호출자가 쓰는 `count(bookmarks)`와 서버가 정의한 `bookmarkCount`가 같은 식으로 다른 값을 낼 수 있어 "한 의도 한 구조"를 오히려 해친다. 이름 규칙(집계는 항상 `*Count` 이름으로만)이 필요하다.
- 한계 2. **간선 정책이 본인 한정인 간선에서만 맞다.** 지원(`Apply`)은 본인과 동아리 운영진이 본다(PoC `app.aip` 486행 `visible to actor when member = actor or managerOf(...)`, Spring 운영진 지원 목록은 `ApplyRepositoryImpl`에 있음). 운영진에게 `applies` 관계를 select하면 지원자 전원이 나오고 "내가 지원했나"가 아니다. `ClubMember`(동아리 구성원끼리 보임), `ActivityLike`도 같다.
- 한계 3. 불리언을 원하는 호출자는 배열을 받아 `length`를 본다. 목록 20행이면 큰 문제는 아니나, 응답 모양이 의도(불리언)를 직접 말하지 않는다.

**V5. 간선 선언에 "소유 컬럼"을 한 번 적고, 엔진이 `mine` 관계를 자동 제공한다.**
간선 리소스가 "이 행의 주인은 `member`"라고 한 번 선언하면(정책 `rows`와 별개), 부모에서 `mine` 뷰(소유 컬럼 = actor로 고정된 관계)를 호출자가 select/exists로 쓴다.

```ts
resource("RecruitmentBookmark", { owner: "member", rows: "member == actor" })
resource("Apply",               { owner: "member", rows: "member == actor || managerOf(actor, recruitment.club)" })
// 호출자
aip.read("Recruitment", { select: { title: true, mine: { bookmarks: { id: true }, applies: { id: true } } } })
```

- 한 표면(`mine`), 서버는 간선당 `owner` 한 줄. 새 "내 ○○" 요구가 생겨도 **그 간선이 이미 선언돼 있으면 서버 수정이 0**이다(V1은 값마다 선언). `Apply`처럼 정책이 넓은 간선도 `mine`은 `owner = actor`로 고정되므로 운영진에게도 정확하다.
- 덮는 범위: K1(5개), K3(최근 임시 저장은 `mine.drafts`를 정렬·1건), K4(`mine.membership`), K2(`mine.membership != null`, 상태 정의는 `ClubMember` 간선의 `rows`/`owner` 쪽에서 한 번). 기존 `ClubDetail.myMembership`(PoC 207행)과 같은 사상이다.
- C.2-1이 **구조 규칙으로 바뀐다.** "본인 간선만"을 요청 식에서 검사하지 않고 `owner` 선언이 보장한다. 호출자가 `where member = ...`를 쓸 자리가 없다.
- 비용: `owner` 선언이 DD-04의 네 표기에 생긴다. `isMine`(K5, 행의 `author == actor`)은 `mine`으로 안 풀리고, 차단(K6)은 아래 4.3절처럼 별도다.

**V1과의 비교.** V1은 PoC 자산(`select` 안 식)을 거의 그대로 쓰고 구현이 가장 작다. V5는 한 단계 더 일반적이지만 새 개념(`owner`, `mine`)이 생긴다. 이 문서의 판단은 "V1이 틀렸다"가 아니라 **V1을 불리언 `exists` 한 종류로 시작하면 E1의 45%(K3~K7)를 못 덮는다**는 것이다. V1을 택하더라도 K3, K4, K5를 표현하는 문법을 같이 정의해야 한다. 어느 쪽이 나은지는 DD-04의 표기 결정과 DD-01 F1에 걸려 있어 이 라운드에서 확정하지 않는다.

### 4.2 V3(클라이언트에서 맞춤)의 평가 보정

DD-10 C.1은 V3의 성능을 "요청 두 번 + 클라이언트 결합. 목록이 크면 비효율"로 접는다(74행 근방). 두 가지를 바로잡는다.

- **캐시 면에서는 V3가 가장 유리하다.** 공용 모집 목록은 행위자와 무관해 공유 캐시가 가능하고, 내 북마크는 작은 별도 요청이다. V1은 목록 응답 전체가 행위자별이 된다(5절). DD-10은 V3의 장점을 표의 한 칸에만 적었다.
- 반대로 V3의 안전 근거인 "간선 정책이 본인 것만 보여 준다"는 DD-10 61행의 주석처럼 `RecruitmentBookmark`에서만 성립한다(4.1절 한계 2). 그리고 호출자가 "내 북마크 id 전체"를 페이지네이션 없이 받으면 I4(결과 크기 상한)에 걸리거나 잘린다. 잘리면 하트가 틀린다. **확인 못함:** 아리아리에서 회원당 북마크 수의 실제 분포는 코드로 알 수 없다.

### 4.3 C.2-1 "본인 간선만" 규칙의 구멍

C.2-1 원문(DD-10 82행): "행위자 자신과 관련된 간선(내 북마크, 내 지원, 내 회원 자격)만 근거로 쓸 수 있고, 남의 간선을 근거로 한 값(예: 친구 X가 북마크했나)은 별도 정책 없이는 거부."

방향은 옳다. 아래는 문장이 그대로 구현 규칙이 되기에는 비어 있는 곳이다. E2(DD-10 104행)가 "공격 요청을 만들어 막히는지" 보겠다고 했으므로, 종이 위 공격 요청으로 정리한다. 아직 실행한 것은 없다(엔진이 없다).

| # | 구멍 | 공격/문제 요청 (`개념 예시`) | C.2-1 원문대로면 | 필요한 보완 |
|---|---|---|---|---|
| H1 | **"자신과 관련된"의 정의가 세 갈래** | (a) 소유 컬럼이 actor인 행 `Bookmark.member = actor`, (b) actor가 대상인 행 `Block.blocked = actor`, (c) actor가 볼 권한이 있는 행(운영진이 보는 `Apply`) | 예시(내 북마크, 내 지원, 내 회원 자격)는 모두 (a)다. (b)는 "관련"이라 통과할 수도 있다. E1의 9번 `isBlocked`가 정확히 (b) | "본인 간선 = 소유 컬럼이 actor인 행"으로 좁게 정의. (b)(c)는 값이 아니라 행 정책(서버 내부)에서만 사용 |
| H2 | **구문 검사냐 의미 검사냐** | `exists bookmarks where member.id = $actor`, `where member in [$actor]`, `where not (member != $actor)`, `where member.email = actor.email`, `where member.school = actor.school` (마지막은 본인이 아니라 **같은 학교의 누군가**) | `member = actor` 문자열만 보면 변형이 걸러지거나, 반대로 마지막 두 개가 "actor 참조"라서 통과할 수 있다. 술어가 소유자 고정을 **함의**하는지 판정해야 하는데 일반적으로 어렵다 | 호출자가 where를 쓰지 않게 한다(V1, V5). V2를 열려면 canonical 정규형(`owner == actor` 한 모양)만 허용 |
| H3 | **안전의 출처가 C.2-1이 아니다** | `exists bookmarks`(where 없이) | `RecruitmentBookmark`에 `rows: member == actor`가 있으면 I2가 이미 본인으로 제한한다(DD-01 80행). C.2-1은 중복이다. 간선 정책이 **넓은** 간선(Apply, ClubMember)에서만 새 의미를 갖는다 | C.2-1을 "간선에 소유 한정 정책이 없으면 그 간선은 viewer 값 근거로 쓸 수 없다"로 바꿔 I2 위에 얹는다 |
| H4 | **본인 간선을 출발점으로 남의 행을 조건으로 쓰기** | `exists membership(actor, club) and exists club.members m where m.role = ADMIN and m.name = "홍길동"` (내 동아리의 회장 이름 추측) | "본인 간선을 근거로"는 출발점만 본인이면 통과한다. 두 번째 hop은 `ClubMember`의 일반 행 정책에 맡겨지는데, 동아리 구성원끼리는 서로 보이는 정책이면 막지 못한다. 구성원에게는 정당한 정보일 수 있으나 **값으로 부울 오라클화**하면 filter 없는 select보다 추측 통로가 넓다 | viewer 값은 **간선 한 단계**(actor-소유 간선과 그 대상)까지만. 이후 traverse는 일반 select/filter 규칙(I1, I2) 소관이며 viewer 식 안에 두 번째 hop을 쓰지 못하게 함 |
| H5 | **V1 작성 실수가 조용한 오류** | `isMyApply: "exists applies a"` (owner 조건 누락) | 서버가 식을 정하므로 안전하다는 DD-10 C.1 보안 칸(73행)과 달리, 일반 회원에게는 정책이 본인으로 줄여 우연히 맞고 **운영진에게는 "누군가 지원했나"**가 된다. 필드명은 `isMy…`인데 역할에 따라 의미가 변한다 | 엔진이 viewer 식의 모든 간선 참조가 `owner == actor`를 함의하는지 **선언 시점**에 정적 검사. 못 하면 선언 거부 |
| H6 | **집계 클래스 충돌 (I7)** | `exists bookmarks where member = $actor` | DD-10 73행은 `exists`를 집계로 보고 I7을 받아야 한다고 쓴다. 그런데 DD-01 95행·147행은 공개 집계에 **호출자 임의 필터를 금지**한다. V2는 호출자가 where를 쓰는 집계이므로 별도 집계 클래스("정책 통과 집계")가 DD-01에 정의돼야 한다 | DD-01에 그 클래스를 추가하든지, V2를 포기하고 V1/V5로 간다 |
| H7 | **남의 간선을 경유한 추측은 이미 현행 코드에 있다** | 아리아리 `isBlocked` 플래그 | 현행: `blockSet`이 내가 차단한 사람과 **나를 차단한 사람**을 합친다(`ClubActivityUtils.java` 37~44행). 내가 차단한 적 없는 작성자의 `isBlocked`가 true면 나는 "그가 나를 차단했다"를 추론한다. 실제 노출은 프론트가 이 플래그를 어떻게 쓰는지에 달려 **확인 필요**(프론트 미확인) | C.2-1의 의도가 옳다는 증거. 보완책: "내가 차단함"은 K1 값(`Block.blocker = actor`), "나를 차단함"은 값으로 내보내지 않고 **행 정책으로 숨김**(PoC `app.aip` 672행). 표시 방식(접기/숨기기)은 앱 개발자의 제품 결정 |
| H8 | **actor 표기 불일치** | `member: "$actor"` (DD-10 58행) vs DD-01 209행 `actor.school` | 한 요청 문법 안에 `$actor`와 `actor`가 공존한다. 요청 문법은 T06 미정이지만, 호출자가 보낸 값을 서버가 신뢰하지 않는다는 점(서버가 credential에서 해석, PRINCIPLES §4 4번)을 문서가 명시해야 한다. 호출자가 다른 사람 id를 직접 넣는 요청은 I2가 막아야 한다 | 표기를 하나로, "actor는 항상 서버가 credential에서 해석"을 불변식 문장으로 |

이 표의 H3와 H6이 가장 중요하다. **C.2-1은 호출자에게 where를 쓰게 하는 한 영원히 의미 검사 문제를 안고, 소유 컬럼을 선언하는 쪽(V1/V5)으로 가면 구조로 사라진다.**

> (이어쓰기 메모) 앞선 비판자가 4.3절 끝(H8과 마지막 요약 문장)까지 쓰고 멈췄다. 4절은 내용상 완결로 판단해 수정하지 않았다. 5절부터 이어서 쓴다. 5~8절의 파일·행은 이 세션에서 다시 열어 확인했다(DD-01 136~170행, DD-03 75~95행, DD-09, PRINCIPLES 14~46행, design-operating-rules 40~52행).

---

## 5. 보안과 캐시

### 5.1 "결과 캐시는 행위자별"은 필요조건일 뿐이다

DD-10 C.2-2는 "결과 캐시는 행위자별, plan은 공유"만 쓴다(DD-10 83행). 1.3절에서 이 문장의 DD-03 출처가 없다고 지적했다. 그 위에 부족한 점이 네 가지 더 있다.

1. **plan에 actor 값이 박히면 안 된다.** DD-03 90행·DD-01 164행(층 2 원칙)은 "정책이 매개변수로 들어간 plan만 재사용"이라고 쓴다. viewer 식의 `actor`도 같은 취급이어야 한다. `b.member = actor`가 plan에 리터럴로 굳으면 plan 공유가 곧 타인 결과 공유가 된다. DD-10은 "plan은 공유"라고만 쓰고 actor가 매개변수여야 한다는 조건을 적지 않았다. 이것이 C.2-2의 실제 안전 조건이다.
2. **캐시 키가 actor만으로 부족하다.** T-I1(DD-03 82행)이 "같은 actor·tenant·데이터 상태·정책·스키마 버전"을 동일성 조건으로 둔다. 같은 논리로 viewer 값 결과 캐시 키에는 actor, tenant, 정책 버전, 스키마 버전이 들어가야 하고, **근거 간선이 바뀌면 무효화**돼야 한다. 북마크를 누른 직후 목록의 하트가 낡은 값이면 사용자에게는 바로 결함이다. DD-01 166행은 결과 캐시 무효화를 T13으로 미뤘으므로 viewer 값 캐시는 **T13이 풀릴 때까지 쓰지 않는 것**이 안전한 기본값이다. DD-10은 이를 적지 않았다.
3. **HTTP·CDN 캐시가 더 큰 위험이다.** DD-09 97행은 "HTTP 계층 캐시·브라우저 프리페치가 선언 읽기를 대신 응답"하는 문제를 이미 열린 점으로 적었고, 읽기를 GET으로 노출할지는 DD-08 W7에 걸려 있다. viewer 값이 select에 하나라도 들어간 응답이 공유 캐시에 저장되면 **다른 사람의 하트·소속·지원 여부가 나간다.** 서버 내부 결과 캐시보다 이쪽이 노출 면이 넓다. 필요한 문장: "viewer 값을 하나라도 평가한 응답은 `private`(공유 캐시 금지, 인증 정보 기준 Vary)으로만 나간다." DD-10에 없다.
4. **요청 하나가 공용이냐 행위자별이냐를 서버가 정해야 한다.** `select`에 viewer 값이 있으면 응답 전체가 행위자별이고, 없으면 공용이다. 같은 목록이 선택 필드 하나로 캐시 클래스가 바뀐다. 3.3절의 "식이 actor를 참조하는지 서버가 추론"이 이 분류의 전제다. 작성자가 `viewer`/`predicate`/`publicAggregate`를 직접 고르게 하면 오분류가 곧 캐시 누출이 된다. 부분 캐시(행 집합은 공용, viewer 값만 행위자별로 계산)는 성능상 좋은 구조지만, 이것도 T13 이후의 층 2 주제다.

판정: C.2-2는 방향은 옳지만 **불변식으로 쓰이려면 위 1·3번이 문장이 돼야 한다.** 이것이 5절에서 가장 실질적인 보강이다.

### 5.2 비로그인 기본값(C.2-3)이 존재를 노출하는가

존재 노출이라는 좁은 의미에서는 **새로운 노출이 없다.** 근거: 값은 행이 정책(I5)을 통과해 보이게 된 뒤에 붙는다. 아리아리에서도 교내 모집이면 비로그인은 값 이전에 `NoProperSchoolAuthException`으로 막힌다(`GlobalValidator.java` 39~51행, 1.1절 8번). 행이 보이는 상황에서 `isBookmarked=false`는 새 정보가 아니다.

그러나 C.2-3에는 존재 노출과 별개로 **틀릴 수 있는 곳이 셋 있다.**

| # | 문제 | 설명 | 보완 |
|---|---|---|---|
| N1 | **만료·변조된 credential이 조용히 "비로그인"으로 강등된다** | C.2-3은 "로그인하지 않은 사람"과 "로그인했는데 credential이 무효한 사람"을 가르지 않는다. 후자에게 기본값 false를 주면, 토큰이 만료된 사용자는 자기 북마크가 모두 사라진 화면을 본다. 다시 하트를 누르면 쓰기 쪽에서 인증 오류가 나는데, 읽기는 성공했으니 원인을 알 수 없다. 클라이언트가 만료를 감지할 신호가 읽기에서 사라진다 | 기본값은 **credential 자체가 없을 때만.** 무효하면 인증 오류. 이 구분을 불변식 문장으로 |
| N2 | **false와 "알 수 없음"이 같은 값이다** | 비로그인의 `isMyClub=false`는 "내 동아리가 아님"이 아니라 "판정 불가"다. 화면이 "로그인하면 지원할 수 있어요"와 "이 동아리 소속이라 지원 불가"를 가르려면 둘이 달라야 한다. 아리아리도 같은 구분이 없다(`RecruitmentService.java` 78~85행은 비로그인에 FALSE). 다만 DD-10은 "false/null"로만 적어(C.2-3) 불리언과 id의 규칙이 다르다는 사실을 계약으로 올리지 않았다 | 선택지 둘. (가) 비로그인은 viewer 값을 **생략**(필드 없음). (나) 모양은 유지하되 세션 정보(`viewer.authenticated`)를 응답에 하나 둔다. 둘 중 무엇이 나은지는 앱 개발자·프론트 계약 문제라 확정하지 않는다 |
| N3 | **로그인했지만 아직 자격이 없는 경우** | 아리아리는 교내 모집에 학교 인증이 없으면 예외를 던진다. viewer 값 평가 전에 행 정책이 이를 걸러야 한다. 정책 평가와 viewer 평가의 **순서**("행 정책 통과 후 viewer 값 평가")가 DD-10에 없다. 순서가 뒤집히면 `exists membership(actor, club)`의 서브쿼리가 행 정책을 우회해 존재 오라클이 된다(I2가 서브쿼리에도 걸리므로 구현 결함이지 설계 결함은 아니나, 문장이 없으면 구현자가 모른다) | "viewer 값은 행·필드 정책을 통과한 행에 대해서만 평가한다"를 C.2에 추가 |

### 5.3 "본인 간선만" 규칙(C.2-1)의 구멍: 4.3절에 더해

4.3절 H1~H8이 구문 수준 구멍을 다뤘다. 여기서는 **다른 불변식과의 접점**만 보탠다. 여기서 쓰는 H 번호는 4.3절 표의 번호다.

- **T-I2와의 접점.** 등록(`registered-only`)된 요청에 viewer 값이 있어도 등록 식별자가 권한을 만들지 못하고 actor가 바뀌면 새 판정이다(DD-03 83행). DD-10은 T-I2를 인용하지 않았다. 등록된 요청의 결과를 등록 시점의 actor로 평가하는 구현은 막혀야 한다. 5.1의 1번과 같은 뿌리다.
- **저장된 조회와의 접점.** DD-01 117행은 저장된 조회를 "표준 조합 요청에 이름만 붙인 것, 새 의미가 없다"로 정의한다. viewer 값이 select에 들어간 요청도 저장할 수 있게 되면 저장된 조회의 결과가 actor별이 된다. 정의는 그대로 유지되지만(이름만 붙임) 저장된 조회 자체를 공유 캐시 대상으로 삼는 최적화는 금지돼야 한다. 5.1의 3번에 포함시켜 한 문장으로 쓰면 된다.
- **H3 재강조.** 안전의 출처를 C.2-1이 아니라 **간선 정책이 소유 한정인지**(I2)에 두어야 한다. DD-10 C.2-1은 "남의 간선을 근거로 한 값은 거부"라고 하나 거부를 **누가 어떻게 판정하는가**가 없다. V1에서 서버 선언자가 `owner == actor`를 못 적고 `exists applies a`를 적으면(H5) 거부 장치가 없다. 선언 시점 정적 검사 요구가 필수다.
- **우선순위 판단.** 현재 DD-10에서 가장 위험한 문장은 C.1 보안 칸 V1의 "서버가 식을 정하므로 노출 범위가 명확"(DD-10 73행)이다. 식을 서버 개발자가 쓰는 것과 노출이 안전한 것은 다른 문제다. H5가 그 반례다.

---

## 6. "창시자 질문 없음"(DD-10 F절)이 정당한가

운영 규칙 F의 기준은 "제품 철학이나 사용자 경험에 영향을 미치는 선택만 질문하고, 내부 구현 세부는 기술 제안"이다(`design-operating-rules-2026-10-03.md` 45~47행). 이 기준에 비추어 본다.

**판정: 결론("질문 없음")은 대체로 정당하나, 사유 서술이 부정확하고 전제 하나가 빠졌다. 조건부 정당이다.**

1. **정당한 부분.** viewer 값이 서버 선언이냐 호출자 조합이냐는 DD-01 F1(호출자 조합을 기본으로)의 하위 결정이다. 창시자가 DD-01 F1에 답하지 않은 상태에서 DD-10이 별도 질문을 올리면 같은 질문을 두 번 하게 된다. 캐시(C.2-2)·비로그인 기본값(C.2-3)도 구현 세부다.
2. **그러나 F절이 "없음(기술 제안)"이라 쓰고 바로 다음 문장에서 DD-01 F1에 영향받는다고 인정한다(DD-10 110행).** 이는 사실상 **창시자 결정에 종속된 문서**다. DD-03은 4행에 "전제: DD-01 수정 대안 2 ... 다른 안으로 결정되면 다시 쓴다"를 헤더에 두었다. DD-10도 헤더에 같은 전제 문장을 두어야 독자(그리고 창시자)가 "F1이 다르게 정해지면 이 문서의 어느 부분이 무효인가"를 알 수 있다. 지금은 G절 116행의 추천이 F1의 답을 암묵적으로 가정한다. (V1을 택하면 DD-01 F1이 어느 쪽이든 큰 영향이 없지만, V2 비중은 크게 변한다.)
3. **숨은 제품 결정이 최소 둘 있다. 창시자가 아니라 앱 개발자(AIP 사용자)의 결정이다.** DD-10은 이 구분을 하지 않아 읽는 이가 모두 기술 제안으로 오해한다.
   - "내 동아리"의 정의(활성 회원만인가). 1.1절 7번에서 보았듯 현행 코드는 상태를 보지 않는다. 전환 시 의미가 바뀐다.
   - 차단 상태를 숨길지 접을지(H7). 현행은 플래그를 내려 프론트가 정한다.
   이 둘은 AIP 엔진 설계 질문이 아니므로 창시자에게 올릴 필요는 없지만, **"앱 개발자 몫"이라고 문서에 명시**해야 에이전트가 임의로 정하지 않는다(PRINCIPLES §4 8번).
4. **창시자 질문이 될 수 있는 것이 하나 있다.** V1의 `viewer:` 블록은 `"exists bookmarks b where b.member = actor"` 형태의 **문자열 식**이다(DD-10 49~51행). handoff 핵심 문제 B는 "독립 DSL 및 컴파일러 중심으로 설계가 이동함"이고(`founder-intent-handoff-2026-10-03.md` 755행 부근, **확인 못함: 부제와 정확한 행은 744~751행 A절만 직접 열어 확인했고 B절 본문은 열지 않았다**), PRINCIPLES §4 5번은 새 언어를 배우지 않고 쓸 수 있는지를 묻는다(`PRINCIPLES.md` 43행). 문자열 안의 식은 JS/Python 사용자에게 새 표현 언어다. DD-10 이탈 방지 5행은 이를 "판정 보류(DD-04)"로 넘겼다. 보류는 허용이지만 **V1이 DD-04 결과에 따라 형태가 바뀐다는 점을 F절에 써야** 한다. 이것은 결정 요청이 아니라 의존 표기다.
5. **OI-L1(`docs/DECISIONS.md` 73행)과의 접점.** "JS/Python에서 정의를 쓰는 구체적 방식은 Open"이다. V1의 선언 형태는 이 Open의 구체화 중 하나이므로, DD-10은 형태를 확정하지 않았다고 명시해야 한다. 문서는 `개념 예시`로 표시해 부분적으로 지켰으나 G절 추천에는 "형태 미정" 표기가 없다.

정리: F절의 "없음"은 **"창시자 신규 질문 없음. 단 DD-01 F1과 DD-04 표기 결정에 종속"**으로 고쳐 쓰는 것이 정확하다. 이것은 창시자 결정을 임의로 정하라는 제안이 아니라 의존 관계를 드러내라는 제안이다.

---

## 7. 이탈 방지 점검 6문항 재평가

원문은 DD-10 122~129행이다. 같은 질문 6개를 V1(추천안) 기준으로 다시 평가한다. 평가는 `부합 / 조건부 / 미흡`으로 쓴다.

| # | 질문 | DD-10 답 | 재평가 | 근거 |
|---|---|---|---|---|
| 1 | 백엔드 개발·이중 유지보수를 줄이는가 | 예. DTO마다 반복되던 처리가 리소스 선언 한 줄 | **조건부** | 2.4절. 줄어드는 것은 구현 방식 4종과 계산 위치이지 "선언 한 줄"이 아니다. 필드 11개를 11줄로 선언한다. 그리고 V1을 `exists`만으로 시작하면 11개 중 6개만 덮어(55%) 나머지는 여전히 서버 코드가 필요하다. 게다가 서버 수정 지표(DD-01 E2)로 대안 1·3 대비 재지 않았다. 방향은 맞고 근거 수치가 없다 |
| 2 | 호출자 표현에 도움이 되는가 | 이름으로 고른다. V2를 본인 간선 범위로 열어 표현을 보완 | **조건부, 문장이 자기 모순** | "이름으로 고른다"는 호출자 표현을 *좁히는* 방향이다(PRINCIPLES §4 2번: 범위가 넓어지는가 좁아지는가). 도움이 되는 것은 작성 편의이지 표현 범위가 아니다. 호출자가 표현하는 것은 V2에서다. 또한 V2를 열면 3.2절의 내부 모순과 H2·H6 보안 문제가 따라온다 |
| 3 | 서버가 최종 권한을 통제하는가 | 예 | **조건부** | 5.1(plan·HTTP 캐시), 5.2의 N1·N3, 4.3절 H3·H5. "서버가 식을 정하므로"는 충분조건이 아니다. 정적 검사와 `private` 응답, 순서 규칙이 문장이 되면 "예"가 된다 |
| 4 | AI의 불필요한 구현 선택을 줄이는가 | 예. 이름 하나 | **미흡 (자기 평가와 추천이 충돌)** | 3.2절. 추천 자체가 V1 + V2(본인 exists) + V3이다. 같은 의도의 표면이 셋이다. 3.3절은 `viewer`/`predicate`/`publicAggregate`/`relation` 사이 분류 선택이 더 생긴다고 지적한다. "예"는 V1 단독일 때만 맞다 |
| 5 | JS/Python 생태계와 사람의 사용성 | 판정 보류(DD-04) | **보류는 허용, 단 의존 표기 필요** | 6절 4번. 문자열 식은 새 언어에 가깝다. "보류"라고만 쓰지 말고 V1이 DD-04 표기에 의존한다고 적는다. V5(소유 컬럼 선언)는 식을 쓰지 않아 이 문항에 더 낫다 |
| 6 | 복잡성을 옮기기만 했나 | viewer 평가가 엔진에 생기고 앱의 반복은 사라진다 | **조건부** | 옮겼다는 점은 정직하게 적었다. 그러나 비용 항목이 적다. 엔진 쪽에 생기는 것은 viewer 평가만이 아니라 (a) actor 의존성 추론(3.3), (b) 소유 간선 정적 검사(H5), (c) 캐시 분류와 `private` 응답(5.1), (d) 비로그인과 무효 credential 구분(N1), (e) 파생 값 종류 4개 사이의 경계다. 이 정도면 "엔진이 흡수한다"는 철학에는 맞지만(PRINCIPLES §2 AI 철학) **얼마나 옮겼는지**를 쓰지 않은 것이 문제다 |

6문항 합계: 부합 0, 조건부 4, 미흡 1, 보류 1. 원문은 전부 "예"에 가깝다. 점검의 목적이 이탈 방지이므로, 점검표가 추천을 정당화하는 쪽으로 쓰인 것이 구조적 약점이다.

---

## 8. 수정 제안과 결론

모든 제안은 `에이전트 제안`이다. 창시자 결정을 확정하지 않는다.

### P0 (이대로는 오해나 보안 구멍이 생기는 것. 문서 수정 필수)

| # | 수정 | 근거 |
|---|---|---|
| P0-1 | **C.2-2의 출처 표기를 바로잡고 불변식으로 격상.** (a) actor는 plan 매개변수여야 함(DD-03 90행·T-I2 연결), (b) viewer 값을 평가한 응답은 `private`, 공유 캐시·HTTP 캐시 금지(DD-09 97행 연결), (c) viewer 값 결과 캐시는 T13 이전에는 쓰지 않음 | 1.3, 5.1 |
| P0-2 | **C.2-1을 "소유 한정 간선만 근거"로 좁게 재정의.** 문장을 "간선에 소유 한정 정책(`owner == actor`)이 없으면 viewer 값의 근거로 못 쓴다"로. 선언 시점 정적 검사를 요구. H3, H5가 근거 | 4.3 |
| P0-3 | **C.2-3 분리.** credential이 없을 때만 기본값, 무효·만료는 인증 오류. 행 정책 통과 후에 viewer 값 평가. 비로그인의 false vs 생략은 앱 개발자 결정으로 표시 | 5.2 N1~N3 |
| P0-4 | **G절의 "V1 + V2(본인 exists)" 혼합을 철회하거나 근거를 붙임.** 표면이 셋이 되는 3.2절의 모순을 해소. 이탈 방지 4번 답을 고친다 | 3.2, 7 |

### P1 (설계 질을 크게 좌우. 다음 라운드 안에)

| # | 수정 | 근거 |
|---|---|---|
| P1-1 | **V1의 범위를 불변식으로 쓴다.** "viewer 값은 actor와 리소스 사이 도메인 관계(소유 간선, 소속)에서만 파생. 화면 이름 값 금지. 간선 한 단계까지". `canApplyNow` 같은 화면 값은 거부 | 3.1의 1, H4 |
| P1-2 | **불리언 `exists` 한 종류로 시작하지 않는다.** E1 결과(2.4절) 기준으로 K3(최신 하나), K4(내 소속 행) 표현을 V1 범위에 넣거나, 못 넣으면 "나머지 45%는 확장 읽기 또는 별도 간선 select"라고 명시 | 2.4 |
| P1-3 | **V5(소유 컬럼 선언 + `mine` 관계) 또는 V4(정책 걸린 관계 select)를 DD-10 대안에 정식 추가.** 현재 대안 구도(V1~V3)는 모두 "값을 만든다"를 전제하는 좁은 구도다. V5는 서버 수정 0, 표면 1, C.2-1이 구조로 사라지는 장점이 있고, 비용은 `owner` 개념이다. 확정이 아니라 비교표에 올리는 제안이다 | 4.1 |
| P1-4 | **viewer 값과 filter/sort의 관계를 명시.** 기본은 `select`만, 거르기는 간선 traverse(DD-01 211행 방식). `filter`를 열면 I4 비용 문제 | 3.1의 3 |
| P1-5 | **파생 값 종류(viewer/predicate/publicAggregate/relation)의 선택을 작성자에게 맡기지 않는다.** 식이 actor를 참조하는지로 서버가 추론. DD-04와 함께 결정 | 3.3 |
| P1-6 | **헤더에 전제 문장 추가**(DD-01 F1, DD-04 표기). F절을 "신규 질문 없음, 종속 있음"으로 정정. 숨은 앱 개발자 결정 2건(활성 회원 정의, 차단 표시 방식)을 명시 | 6 |
| P1-7 | **E1 결과(11개 필드, 7종류)를 DD-10에 반영하고 D절의 "선언 한 줄" 표현을 "구현 방식 4종과 계산 위치 11곳이 선언 11줄로"로 정정.** E2 지표(서버 수정 수)를 추가 | 2.4, 7의 1번 |

### P2 (정리·보완)

| # | 수정 | 근거 |
|---|---|---|
| P2-1 | B.1 사실 보정: "목록 DTO마다"를 "리소스 종류와 계산 방식 사이"로. 예외 두 곳(`RecruitmentInClubData.java` 49, `ApplyTempDetailRes.java` 59)을 적는다. "활성 회원만" 예시는 현행이 상태를 안 본다고 쓴다 | 1.1 |
| P2-2 | B.2에 PoC의 `relation` 장치(`app.aip` 88·139~144행)와 `ClubDetail.myMembership`(207행), 차단이 행 정책이라는 점(672행)을 추가 | 1.2 |
| P2-3 | `$actor`와 `actor` 표기 통일, 불변식 문장 "actor는 항상 서버가 credential에서 해석" | H8 |
| P2-4 | V3의 보정: 캐시에서는 가장 유리, 안전 근거는 북마크에만 성립, 북마크 전체 id 목록은 I4에 걸림 | 4.2 |
| P2-5 | `ClubData.schoolData`(보는 사람의 학교를 행에 붙임)는 viewer 값이 아니라 세션 정보 후보로 분리 | 2.4 해석 3 |
| P2-6 | 이탈 방지 점검을 7절의 조건부 표현으로 다시 쓴다 | 7 |

### 결론: 내 추천과 문서 추천의 차이

**문서 추천(G절 116행):** V1 기본, V2는 본인 간선 `exists`만, V3는 SDK 보조. 창시자 질문 없음.

**내 추천:**
1. **방향(서버가 이름 붙인 actor 의존 값을 리소스 단위로 제공, 호출자는 고르기만)은 지지한다.** E1에서 11개 중 9개(82%)가 같은 가족이고, DD-01의 공개 집계·PoC `relation`과 선례가 겹친다. 나쁜 방향이 아니다.
2. **그러나 "V1 불리언 exists 한 종류"로는 시작하지 않는다.** 55%만 덮는다. 단일 구조로 가려면 (가) V1에 K3·K4를 포함하거나 (나) V5 같은 소유 선언형으로 일반화해야 한다. 이 선택은 DD-04 표기와 DD-01 F1에 걸려 있어 이 라운드에서 확정하지 않고, 두 후보를 비교표에 올려 다음 라운드로 넘긴다.
3. **V2(호출자가 where를 쓰는 exists)는 허용하지 않는다.** 문서 추천과의 가장 큰 차이다. 이유: 표면이 셋이 되고(3.2), 의미 검사 문제가 영구적이며(H2), 집계 클래스 충돌이 DD-01에 새 정의를 요구한다(H6). "호출자 표현을 넓혀야 한다"(PRINCIPLES §2)는 요구는 호출자가 viewer 값 **이름을 고르고** 관계를 select/traverse하는 것으로 충족되며, 호출자가 임의 where로 소유를 증명할 필요는 없다.
4. **보안·캐시 조항 3개는 문서가 불변식으로 써야 한다**(P0-1~3). 현재 문서의 가장 큰 결함은 방향이 아니라 **조항이 안전 조건을 충분히 말하지 않는다**는 점이다.
5. **F절은 "없음"이 아니라 "신규 질문 없음, DD-01 F1·DD-04에 종속"으로 정정.**

차이를 한 줄로: 문서는 "V1 + V2 일부"를 추천하고 나는 "V1 계열 단일 표면, 범위는 E1로 넓히고 V2는 제외, 보안 조항은 불변식으로 격상"을 추천한다. 두 추천 모두 `에이전트 제안`이며 창시자 승인 전 확정이 아니다.

### 이 라운드에서 확인하지 못한 것

- 아리아리 프론트가 `isBlocked`·`isMyBookmark`를 어떻게 쓰는지 (H7의 실제 노출). 확인 못함: 프론트 코드 미열람.
- 회원당 북마크 수의 실제 분포 (V3 비용). 확인 못함: 데이터 없음.
- 연관 컬렉션의 실제 SQL (1.1절 2번). 확인 못함: 미실행.
- handoff 핵심 문제 B의 본문 (6절 4번). 확인 못함: A절 744~751행만 직접 열었다.
- 엔진이 없으므로 4.3절 공격 요청은 종이 위 분석이다. E2는 이 라운드에서 실행되지 않았다.
