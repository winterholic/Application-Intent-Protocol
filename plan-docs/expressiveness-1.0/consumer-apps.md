# 소비자 앱 3유형 표현력 검증 (협업형 SaaS, 콘텐츠·커뮤니티, 주문·재고)

측정: 2026-11-21, 브랜치 `test/expr-consumer-apps-20261121`(1차 기준 `design/expressiveness-1.0-20261121` 50f2bc27, 주문·재고 2차는 P0-A·B·C 통합 2e94daa0을 merge한 b719bb42 이후), aip 1.0.0 debug 빌드(`CARGO_TARGET_DIR=/tmp/aip-target-apps`), PostgreSQL 17.11 전용 컨테이너, Node v26.9.0, Python 3.14.7, TypeScript 7.0.2.

목표: AIP가 일반 백엔드 업무를 얼마나 적은 반복 코드로, 안전하고 예측 가능하게 구현하게 해 주는지를 소비자 앱 세 유형으로 본다. 문법 통과가 아니라 실제 HTTP(`aip service serve`)·PostgreSQL·SDK 경로 실행을 근거로 쓴다. 협업형 SaaS와 콘텐츠·커뮤니티는 현재 기능으로 끝까지 실행했다. 주문·재고는 1차에서 현재 문법의 우회만 실행하고 P0 목표 정의를 미실행으로 남겼으며, 2차(2e94daa0 merge 뒤)에 목표 정의를 실제로 실행해 이 문서의 1절과 8절에 반영했다.

## 1. 실행 결과

명령 한 번: `node product/tests/fixtures/expr-apps/run.mjs`(전용 PG 17 컨테이너 기동, 앱마다 새 schema `service init`, 시드, principal 바인딩, `serve`, 시나리오, 정리). P0 통합(2e94daa0) merge 뒤 마지막 전체 실행 출력:

```
실제 PostgreSQL: PostgreSQL 17.11 (Debian 17.11-1.pgdg13+2) on aarch64-unknown-linux-gnu, ...
DSN 대상: 전용 컨테이너 aip-expr-apps-43515 (host=127.0.0.1 port=57247 user=… dbname=aip_test_expr_apps)
revision: f90a000112fd, aip 1.0.0, HTTP=aip service serve, SDK=@aip/sdk(tarball 설치)+python aip_sdk
합계: PASS 64 / FAIL 0 / SKIP 0
```

이 revision은 시험 코드를 커밋한 시점이고 이 문서를 커밋하기 전이다. 1차 실행(50f2bc27 기반, PASS 47 / SKIP 1)과 달리 SKIP이 없다. 주문·재고 목표 정의가 현재 바이너리에서 `service check`를 통과하기 때문이다(P0 merge 전에는 `DEF.PARSE_UNKNOWN_KEY` `input`).

| 단위 | 시나리오 | 결과 |
| --- | --- | --- |
| saas/ts | typecheck + 16개 | PASS 17 |
| saas/py | 7개 | PASS 7 |
| community/ts | typecheck + 11개 | PASS 12 |
| community/py | 3개 | PASS 3 |
| orders/ts (현재 문법과 우회) | typecheck + 8개 | PASS 9, INFO 2(동시 주문, 단가 위조) |
| orders/target-a (변형 A: 선언만, P0-A·B) | typecheck + 8개 | PASS 9, INFO 다수(거절 코드, 동시 주문) |
| orders/target-b (변형 B: 확장, P0-B·C) | typecheck + 6개 | PASS 7, INFO 다수 |

"PASS"는 서비스 응답과 함께 psql로 DB 행을 직접 읽어 교차 확인한 것이다. 시험이 실패할 수 있는지는 정의를 일부러 망가뜨려 확인했다(RED, 4절).

실행 환경 전제: 이 저장소의 `spikes/spike-0-ts/node_modules`에 darwin용 TypeScript 7이 있어야 한다. 원래 checkout의 것은 linux-arm64 바이너리만 있어서 쓸 수 없었고, 다른 worktree의 설치본을 이 worktree의 무시되는 `node_modules`로 복사했다(추적 파일 변경 없음).

## 2. 재사용과 신규 작성의 근거

`product/tests/fixtures`의 기존 앱을 먼저 검토했다.

| 기존 자산 | 판단 | 근거 |
| --- | --- | --- |
| `memoir-notes`, `memoir-folder-rename` | 문법만 참고 | 단일 소유자(`owner = actor`) 앱이라 테넌트·역할·집계가 없다. `patch`·`expect version` 전이 문법을 가져왔다 |
| `facereview-comments`, `facereview-auth`, `facereview-mysql` | 문법만 참고 | 외부(MySQL) 스키마 포팅용이다. `default database` 필드는 신규 스키마에서 못 쓴다(`EXTERNAL_DEFAULT_REQUIRED`, 5절 F1). `traverse`·`refIds` 패턴을 참고 |
| `external-effects`, `mysql-worker-*`, `password-auth` | 사용 안 함 | 외부 효과·MySQL worker·비밀번호 검증 범위라 이 검증과 겹치지 않는다 |
| `examples/*/app.aipoc` | 사용 안 함 | PoC 언어(`.aipoc`)이고 제품 경로(`.aip`)가 아니다 |
| `product/tests/sdk-semantic-e2e.test.mjs` | 패턴 재사용, 파일은 재사용 안 함 | 정의를 파일 안에 한 개만 박은 `node:test`다. service 기동, JWT, principal 바인딩, SDK tarball 설치, Python 생성 binding 방식을 `run.mjs`로 일반화했다. `product/tools/build-sdk.mjs`와 `product/test-support/postgres-guard.mjs`는 그대로 호출한다 |
| `LIMITS.md`의 주문·재고 정의(E1~E3) | 주문·재고 앱의 기초 | 이미 실행으로 확인된 정의라 현재 우회 앱(`orders.current.aip`)의 뼈대로 썼다 |

그래서 협업형 SaaS와 콘텐츠·커뮤니티는 새로 작성했고, 주문·재고는 LIMITS 정의를 확장했다.

파일 구성(`product/tests/fixtures/expr-apps/`): `run.mjs`(실행기), `lib/check.ts`·`lib/check.py`(결과 줄 규약과 `sql()`), `saas/`, `community/`, `orders/`.

## 3. 협업형 SaaS (`saas/workspace.aip`)

모델: Org(테넌트), User(조직 소속, 역할 OWNER·ADMIN·MEMBER·VIEWER, 이메일은 필드 정책), Project, Task, Comment. 테넌트 격리는 자동이 아니므로 모든 resource에 `rows read when org = actor.org`와 생성 `allow`의 `org = actor.org`를 손으로 쓴다. 쓰기는 `expose create`, 부분 수정 `patch`+`expect version`+`to version = version + 1`, 상태 전이(`start`·`complete`), 삭제 전이, 선언 효과 cascade(`update many`, `delete many`)다. 읽기는 to-one·to-many `traverse`, 댓글 수 집계, `cursor` 페이지다.

실행한 시나리오(TypeScript 16개, Python 7개)와 부정 시나리오:

| 구분 | 내용 | 확인한 결과 |
| --- | --- | --- |
| 테넌트 읽기 격리 | Acme 사용자가 Org·User·Task를 읽음 | Globex 행 0건. Globex의 carol은 자기 행만 |
| 필드 정책 | MEMBER와 OWNER가 같은 User 목록을 읽음 | MEMBER는 본인 이메일만, 나머지 `null`. OWNER는 전부 |
| 관계 생성·조회 | 담당자·프로젝트가 있는 Task 생성 후 `traverse` | `project`·`assignee` 객체와 `commentCount` 0 |
| 부분 수정 | `title`만 보내고 `expect version: 0` | 생략한 `description`·`assignee` 유지, version 1 |
| 낙관적 잠금 | 낡은 `expect version: 0` | `WIRE.CONFLICT`, DB 값 불변. 새 version으로 재시도 성공, `assignee: null`로 Ref 해제 |
| 멱등 | 같은 key 재전송 | `replayed: true`, version 한 번만 증가 |
| 상태 전이 | `start`·`complete` 순서, 순서 위반, DONE 뒤 edit | 순서 위반과 DONE 뒤 수정은 거부(`from`) |
| 역할 부정 | VIEWER의 생성, MEMBER의 남의 Task 수정·프로젝트 보관·사용자 초대, 관리자 권한자(alice)의 OWNER 역할 부여 | 전부 `WIRE.FORBIDDEN`, DB 불변 |
| 다른 테넌트 부정 | 수정, 보관, 삭제, 읽기, 다른 조직 프로젝트로 생성, `org` 위조, 다른 조직 사용자를 담당자로, 다른 조직 Task에 댓글 | 전부 `WIRE.MISSING_TARGET`(존재를 숨김), DB에 흔적 없음 |
| cascade | 프로젝트 보관 시 Task DONE, Task 삭제 시 댓글 삭제 | 같은 트랜잭션에서 실행됨 |
| 페이지 | `sort id desc`, `limit 3`, `after {id}` 반복 | 이어 읽은 id 열이 DB 내림차순과 같음, 중복·누락 없음 |
| 클라이언트 | TypeScript(`--strict` 컴파일 통과), Python 생성 helper | 같은 의미 |

DB에서 `task.org_id <> project.org_id`인 행이 없음도 확인했다(비교 대상이 9건 이상 있을 때만 통과하도록 단언).

## 4. 콘텐츠·커뮤니티 (`community/community.aip`)

모델: Member(역할 USER·MODERATOR), Board, Post(DRAFT→PUBLISHED→HIDDEN, 되돌리기), Comment, Reaction. 게시글 읽기 정책은 `status = PUBLISHED or author = actor or isModerator(actor)`다.

| 구분 | 내용 | 확인한 결과 |
| --- | --- | --- |
| 초안 가시성 | 작성자 12개, 독자·익명 0개, 운영자 12개 | 일치 |
| 상태 전이 | 일괄 `publish`(ids 10개), 중복 게시 시도, `hide`·`restore`, 숨김 상태에서 `edit` | 중복 게시가 섞이면 묶음 전체 롤백. 숨김 글은 독자에게 사라지고 작성자는 `HIDDEN`으로 봄. 숨김 글 수정은 거부 |
| 권한 | 독자의 hide·unpublish·edit, 작성자의 hide | `FORBIDDEN`. 남의 초안 publish는 `MISSING_TARGET` |
| 조회수 | 서로 다른 key로 3번, 같은 key 재전송 | 3(재전송은 증가 없음). 초안 조회는 `MISSING_TARGET` |
| 관계·집계 | `traverse author·board·comments`, 댓글 수, 게시판별 공개 글 수 | 숨긴 댓글은 `where hidden = false`로 제외. 글 숨김 시 게시판 count 감소 |
| count와 sum | 반응 2건(5점, 3점) | noah 시점 `count=2, sum=3`, 운영자 시점 `count=2, sum=0`(5절 F6) |
| 필터·정렬 | `board.eq`, `views.gte`, `title.contains`, `views desc` | 기대한 행과 순서 |
| cursor | `sort views desc`, `after {views, id}` | DB의 (views desc, id asc 동률)과 같음, 10건 중복·누락 없음 |
| offset | 댓글 12개를 5개씩 offset 0·5·10, offset 200 | 12개 전체, 범위 밖은 0건 |
| 거부 | `limit` 초과, sort 필드가 빠진 `after` | 코드 있는 예외(`WIRE.BAD_CURSOR` 포함) |

### RED 확인(시험이 실패할 수 있는가)

정의를 하나씩 망가뜨리고 같은 시나리오를 돌려 실패를 확인한 뒤 원복했다(`git status` 깨끗함, GREEN은 마지막 전체 실행).

| 변형 | 관찰한 실패 |
| --- | --- |
| Task `rows read when org = actor.org`를 `true`로 | `tenant-read-isolation`: Globex 할 일 1건 노출. `cross-tenant-negatives`: `MISSING_TARGET`가 `FORBIDDEN`으로 바뀜(allow의 org 조건이 한 겹 더 막지만 존재 은폐는 사라짐) |
| `Task.edit`에서 `expect version` 제거 | TypeScript 컴파일 실패(`{version: number}`를 받을 자리가 계약에서 사라짐) |
| `canWrite`를 `true`로 | VIEWER의 할 일 생성이 성공해 `role-negatives` 실패 |
| Post `rows read`를 `true`로 | 초안이 독자에게 12개 보여 가시성 시나리오 7개 실패 |
| `Post.hide`의 `allow`를 `actor != null`로 | 독자의 숨김 성공으로 `moderation-hide-restore` 실패 |
| 주문 `setStock`에서 `expect stock` 제거 | 확장 호출이 `EXTENSION_ERROR`로 실패 |

## 5. 작성량과 일반 백엔드와의 비교

### 5.1 작성자가 쓴 것

줄 수는 빈 줄과 `//`·`#` 주석 줄을 뺀 값이다. 괄호는 전체 줄 수다. 정의는 한 줄에 항목 여러 개를 쓴 곳이 있어 줄 수가 작성 분량을 그대로 뜻하지는 않는다.

| 앱 | 정의(.aip) | 확장 코드 | 클라이언트 호출 코드(시나리오 파일) | 그 외 |
| --- | --- | --- | --- | --- |
| 협업형 SaaS | 88줄(94) | 0줄 | TS 178줄(207), Python 108줄(138). SDK 호출 지점 TS 50곳(apply 24, read 13, bundle 13) | 시드 SQL 12줄 |
| 콘텐츠·커뮤니티 | 63줄(69) | 0줄 | TS 162줄(181), Python 71줄(88). 호출 지점 TS 47곳(apply 20, read 19, bundle 8) | 시드 SQL 2줄 |
| 주문·재고(현재 우회) | 33줄(41) | 17줄(19)(`shop.mjs`) | TS 106줄(124). 호출 지점 17곳(apply 5, bundle 4, writeExtension 8) | 시드 SQL 3줄 |
| 주문·재고(P0 변형 A, 선언만) | 26줄(32) | 0줄 | TS 122줄(134). 호출 지점 18곳(apply 8, bundle 10) | |
| 주문·재고(P0 변형 B, 확장) | 36줄(42) | 12줄(16)(`shopb.mjs`) | TS 72줄(80). 호출 지점 10곳(apply 2, bundle 2, writeExtension 6) | |

주의: 클라이언트 줄 수는 앱 클라이언트가 아니라 시험 코드라서 단언과 부정 시나리오가 섞인 상한이다. 한 기능의 호출 코드는 보통 SDK 호출 한 번(3~6줄)이다. 정의에 `expose`로 열지 않은 것(읽기·쓰기·전이)은 서버가 만들어 주지 않는다. 서버 쪽에 사람이 쓴 Controller·DTO·Service·Repository 코드는 두 앱에서 0이고, 주문·재고에서는 확장 한 파일뿐이다(변형 A는 0). 주문·재고 세 구현의 비교는 8절이다.

### 5.2 같은 기능을 NestJS 또는 Spring으로 구현할 때 필요한 계층

일반 백엔드 쪽 줄 수는 추정하지 않았다. 아래는 같은 요구를 만족하려고 **필요한 계층과 파일 종류**의 비교다. 일반 백엔드 열은 Spring(JPA, Spring Security)과 NestJS(TypeORM, Guard)에서 공통으로 필요한 것을 적은 것이며 이 저장소에서 구현해 보지 않았다.

| 항목 | AIP에서 한 일 | 일반 백엔드에서 필요한 계층·파일 |
| --- | --- | --- |
| 리소스별 API | `expose read/create/apply`(resource마다 몇 줄) | resource마다 Controller(목록·단건·생성·수정·전이 엔드포인트) |
| 요청·응답 형식 | 필드 타입·길이·범위 선언(`fields`) | Create·Update·Query·Response DTO와 검증 어노테이션·파이프, 응답 매핑 |
| 비즈니스 규칙·상태 전이 | `transition`의 `from`·`to`·`allow`·`patch` | Service 메서드(전이마다), 상태 검사, 트랜잭션 경계 |
| 저장소 접근 | 정의에서 SQL 생성(읽기·쓰기·집계) | Entity와 Repository(목록 필터·정렬·페이지 쿼리, 집계 쿼리, 비관적·낙관적 락 쿼리), 마이그레이션 |
| 테넌트 격리 | resource마다 `rows read when org = actor.org`와 `allow` 조건을 손으로 반복(누락하면 누출, 4절 RED) | 쿼리마다 tenant 조건 또는 전역 필터·Hibernate filter·Row Level Security. 이것도 누락하면 누출 |
| 권한(역할·소유자) | `predicate`, `allow`, `field ... read when` | Security 설정·Guard·Policy 클래스, 메서드별 권한 어노테이션, 필드 마스킹 매핑 |
| 존재 은폐(다른 테넌트에 404/403 정책) | 기본 동작(`MISSING_TARGET`) | 서비스마다 조회 후 분기 또는 예외 변환 |
| 낙관적 잠금 | `expect version` + `to version = version + 1`, 충돌은 `WIRE.CONFLICT` | `@Version` 또는 버전 컬럼, 충돌 예외를 응답으로 바꾸는 핸들러 |
| 멱등 재전송 | 모든 쓰기에 key가 내장(`replayed`) | 멱등 키 저장 테이블·인터셉터를 직접 구현 |
| 일괄 쓰기·원자성 | `bundle`, 선언 효과 `update many`·`delete many` | 별도 배치 엔드포인트·트랜잭션 Service |
| 집계(count/sum) | `aggregate` 선언 | Repository 집계 쿼리와 DTO, 캐시 무효화 |
| 페이지(cursor/offset) | `budget { cursor; offset }` | 페이지 DTO, 정렬 허용 목록 검증, keyset 쿼리 |
| 클라이언트 타입 | `service gen`이 TS·Python binding 생성 | OpenAPI 생성 또는 수기 타입, SDK 별도 |
| 쓰기 중 복잡한 계산(주문 확정) | 확장 한 파일 17줄 | Service 메서드(계산·재고 확인·락) |

AIP가 줄여 주지 않는 것: 인증 서버(JWT 발급·JWKS), principal과 actor 연결(`aip service principal bind`는 운영자 명령), 시드·온보딩 데이터, 확장이 필요한 계산, 테넌트 조건 반복. 이 부분은 일반 백엔드와 같거나 그대로 앱이 맡는다.

## 6. 막히거나 번거로웠던 표현 (분류)

분류: **한계** = AIP 표현력·기능 한계, **작성** = 앱 작성 방식으로 풀림, **문서·SDK** = 문서·SDK·진단 문제. 번호는 아래 audit 대조에서 참조한다.

| # | 현상 (재현) | 분류 |
| --- | --- | --- |
| F1 | 신규 schema에서 `default database` 필드를 쓰면 `service init`이 `EXTERNAL_DEFAULT_REQUIRED`. 서버가 생성 시각을 채울 방법이 없고, `create`는 Time 필드를 받지 않으며, 전이의 `to publishedAt = now`로만 기록된다. 이 필드는 nullable이라 cursor 정렬에 못 쓴다(E5). 그래서 커뮤니티 목록은 `id`·`views`로 cursor 정렬하고 `publishedAt`은 select만 한다 | 한계(E5의 흔한 변형) |
| F2 | 단건 조회에도 `filter id.eq`를 정의에 적어야 한다. 빠뜨리면 TypeScript가 컴파일 오류로 알려 준다 | 작성(낮음) |
| F3 | `budget cost`가 모자라면 `WIRE.COST_EXCEEDED`. Task 읽기(to-many traverse 1개와 count 집계)가 `cost 600`에서 실패해 1500으로 올렸다. 비용 산정 규칙을 이번에 읽은 문서에서 찾지 못했다(확인 필요) | 문서 |
| F4 | `access visibleTotal = totalOfVisible(Comment)`를 Task의 `commentCount`에 쓰면 `DEF.ACCESS_MISMATCH`("Comment 기준 groupKey 집계에만 사용"). 올바른 인자는 집계를 선언한 쪽인 `Task`다. 메시지가 반대로 읽힌다 | 문서(진단) |
| F5 | `where` 없는 count는 `DEF.ADVISORY_PUBLIC_COUNT` 경고. `where org = actor.org`로 해소(actor를 where에 쓸 수 있음) | 작성(audit #14 일치) |
| F6 | 같은 Post에서 `reactionCount`(count)는 모두의 반응 2건, `scoreSum`(sum)은 호출자가 읽을 수 있는 반응만 합산해 noah 3, 운영자 0이다. 반응을 본인만 읽게 하면 공개 점수 합계를 만들 수 없다. 설계된 계약이지만 같은 정의 안에서 비대칭이 놀랍다. 공개 합계가 필요하면 반응 행을 공개해야 한다 | 문서(+decisions.md의 count 비상속 결정 항목과 연결) |
| F7 | TypeScript `bundle`의 `atomic`은 비어 있지 않은 튜플 타입이라 `Array.from`으로 만든 배열은 캐스트가 필요하다. `created[].id`는 resource별 타입이 아니라 합집합(`Id<"Order"> \| Id<"OrderLine">`)이라 다음 호출에 쓰려면 캐스트가 필요하다. `internal` 전이는 계약 타입에 없어 직접 호출 시험에 `as never`가 필요하다(의도). `replayed`는 성공 변형에만 있어 `ok`로 좁혀야 읽을 수 있다 | SDK |
| F8 | cursor 응답에 다음 cursor가 없다. 클라이언트가 마지막 행에서 정렬 필드 값과 `id`로 `after`를 만들어야 하고, 정렬 필드를 select에 넣어야 한다. 동률 처리는 `id` 오름차순으로 고정이다(문서는 "id로 동률"만 적어 방향이 없다) | 문서·SDK |
| F9 | SDK 읽기 cache(`maxAgeMs` 1초)는 같은 client의 쓰기로만 무효화된다. 다른 사용자의 변경을 같은 client로 곧바로 읽으면 이전 결과가 나온다(익명 client가 방금 게시된 글을 0건으로 읽은 것이 이 원인으로 보이며, 읽을 때마다 새 client를 만들자 해결됐다) | 문서 |
| F10 | 테넌트 조건을 resource마다 `rows read`, 생성 `allow`, 참조 일관성(`project.org = actor.org`)에 손으로 반복한다. 빠뜨리면 누출되는 것을 RED로 확인했다. `allow`에도 org 조건을 두면 `FORBIDDEN`으로 한 겹 더 막지만 존재 은폐는 `rows read`만 한다. task와 project의 org 일치는 DB 제약이 아니라 생성 `allow`에만 기댄다 | 한계(자동 격리 없음, SUPPORT 13)·작성(audit #17 일치) |
| F11 | 자식이 있는 행을 지우면 `WIRE.REFERENCE_CONFLICT`. 선언 효과 `delete many Comment maxRows 500 where task = this`로 cascade했다. 500건을 넘는 경우의 동작은 실행하지 않았다 | 작성(상한은 한계, 미확인) |
| F12 | 효과 `update many Task ... { status = DONE }`로 cascade했다. 효과 값에는 산술이 없어(audit #6) `version = version + 1`을 함께 올릴 수 없다. 이번 시나리오는 DONE 뒤 edit가 `from`으로 막혀 영향이 없고, version이 안 오르는 것 자체는 단언하지 않았다 | 한계(E2) |
| F13 | 사용자 초대(`expose create User`)는 행만 만든다. JWT subject와 actor를 잇는 `aip service principal bind`는 운영자 CLI라 앱 API로 로그인 연결까지 끝낼 수 없다. 시나리오는 시드 시점에 바인딩했고 초대 직후 로그인은 실행하지 않았다 | 한계(범위)·문서 |
| F14 | 확장 입력 타입에 배열이 없다(`List<Int>`와 `Int[]`는 파싱 오류, `Json`만 통과하며 모양 검증은 확장이 한다). 다건 주문 줄은 줄 행을 먼저 만들어야 하고(E1과 결합), P0-C 변형 B도 `Json` 입력이 된다 | 한계(audit에 없던 새 항목) |
| F15 | 동시 주문 우회: 재고 4개, 2개짜리 주문 6건을 동시에 보냈더니 반복 실행마다 성공은 1건이었고 나머지는 `WORKER_BUSY` 2건과 `EXTENSION_ERROR` 3건이었다(가능했던 성공은 2건). 재고가 음수가 되거나 이중 차감되지는 않았다(안전성 유지, 활성은 나쁨). `WORKER_BUSY`는 한 사용자의 동기 worker 동시 호출 한도(기본 2) 때문이다. 1차(50f2bc27)에는 낙관적 충돌이 호출자에게 `EXTENSION_ERROR`로 보였고, P0 merge 뒤 같은 시험에서는 `WIRE.CONFLICT`로 보인다(P0-C의 코드 전달 수정). P0-B 변형 A는 같은 상황에서 성공 2건이다(8절) | 한계(E2), P0로 해소 |
| F16 | `build-sdk.mjs`가 컴파일에 실패하면 `BUILD_FAILED`만 알려 줘 원인을 알 수 없었다. 실패 메시지에 컴파일러 출력(stderr/stdout) 요약을 붙이도록 수정했고, 일부러 문법 오류를 넣어 `TS1110`·`TS1109`가 출력되는 것을 확인했다 | 문서·SDK(진단, 수정) |
| F17 | JWT의 `exp - iat` 3600초는 `crates/aip-auth/src/lib.rs:648-659`의 `1..=900` 검사로 거부되고, 300초는 허용된다. 하드코딩 제한이며 설정 변경 지점은 없다. `Invalid`는 `spikes/spike-v6-transport/src/lib.rs:1416-1422`에서 `WIRE.UNAUTHENTICATED` / "토큰 검증 실패"로 뭉뚱그려지고 거부 이유 로그도 남기지 않는다. 900초 상한은 `product/IDP-RUNBOOK.md:23,29`, `product/GETTING-STARTED.md:49`, `product/OPERATIONS.md:22`에 이미 적혀 있다 | 문서(진단, 원인 확정) |

## 7. audit.md 판정과의 대조

| audit # | audit 판정 | 이번 실행 | 차이 |
| --- | --- | --- | --- |
| 1 게시글 CRUD·부분 수정·낙관적 잠금 | A | Task·Post의 부분 수정, 충돌, 재시도, 재전송 실행 PASS | 같음 |
| 2 주문+줄 원자 생성 | D | 우회는 호출 2회. 줄을 만들기 전 끊기면 줄 없는 미확정 주문이 남는 것을 DB에서 확인 | 같음(고아 주문을 실행으로 확인) |
| 3 입력 수량 재고 차감 | C | 확장 우회 실행 PASS(10, 5 → 7, 3) | 같음 |
| 7 주문 확정 | C(LIMITS 문서만 근거, 재실행 안 함) | 이번에 실행 PASS(합계 700, 부족 시 전체 롤백, 재전송 재차감 없음) | 문서 근거가 실행 근거로 바뀜. 등급은 C 유지 |
| 8 다단계 승인 등 상태 전이 | A | enum 상태와 predicate allow, 일괄 `publish`·롤백 실행 PASS | 같음 |
| 11 목록 필터·정렬·cursor·관계 | A | 실행 PASS. 단 F8(다음 cursor 없음, 동률 방향 미문서)이 번거로움 | 같음, 문서 보강 필요 |
| 12 nullable 정렬+cursor | B | F1: 신규 schema에서는 서버가 시각을 채울 길이 없어 nullable Time이 불가피하고 그래서 cursor 정렬이 막힌다 | **차이**: B(작성 우회)가 아니라 한계 쪽에 가깝다. 생성 시각 정렬은 흔한 요구다 |
| 14 count·sum 집계 | B(작성) | count는 공개, sum은 `rows read` 재적용으로 실제 값이 달라짐(F6) | 분류 추가: 작성에 더해 문서 |
| 16 역할·소유자·필드 권한 | A | 필드 정책, 역할 부정, 존재 은폐 실행 PASS | 같음 |
| 17 테넌트 격리 | B(작성) | F10: 누락 시 누출을 RED로 증명 | 같음, 위험이 실행으로 확인됨 |
| 18 다른 사용자를 지정한 공유 생성 | B | 담당자·초대·댓글 대상 참조의 가시성 거부 실행 PASS | 같음 |
| 21 확장 호출 SDK | B | `writeExtension` 실행 PASS. F7: 실패 결과의 `appCode`·`replayed`를 읽으려면 좁히기 필요 | 같음 |
| 9, 10, 13, 15, 19, 20 | | 이번 범위 밖이라 실행하지 않음(13의 관계 필드 필터는 시나리오가 필요로 하지 않아 확인하지 못함) | 하지 않음 |

audit의 "범위 밖 발견"인 LIMITS E2 본문과 `ctx.reject`의 불일치는 이번에도 확인됐다. 확장이 `ctx.reject("INSUFFICIENT_STOCK", ...)`를 던지면 호출자는 `WIRE.EXTENSION_REJECTED`와 `appCode`를 받았고(시나리오 PASS), LIMITS는 아직 "모든 실패가 `EXTENSION_ERROR`"라고 쓴다.

### 7.1 P0 병합 뒤(2e94daa0) audit 대조

| audit # | 1차 등급 | 2차 실행 | 결과 |
| --- | --- | --- | --- |
| 2 주문+줄 원자 생성 | D | 변형 A: `bundle` 한 번으로 주문+줄 생성, 부모 실패·자식 실패 모두 고아 없음(A1~A3) | D에서 A로 올라간다(선언만, 단 합계는 클라이언트 값) |
| 3 입력 수량 재고 차감 | C | `input { qty }`와 `to stock = stock - input.qty`로 확장 없이 실행(A1, A5, A7) | C에서 A로 올라간다 |
| 7 주문 확정(재고·합계·상태) | C | 변형 A는 재고·원자 생성까지 선언으로 되나 합계 검증은 안 됨, 변형 B가 서버 단가 합계까지 확장 12줄로 한다(B1) | 합계를 서버가 신뢰해야 하면 C(확장) 유지. 확장 입력의 배열 부재(F14)로 다중 줄은 못 한다 |
| 14 count·sum | B | 변화 없음 | |

## 8. 주문·재고: 우회와 P0 목표 실행 결과

세 구현을 모두 실제 serve와 PostgreSQL 17에서 실행했다. P0-A·B·C 통합 2e94daa0을 merge하고 바이너리를 다시 빌드한 뒤, 이전에 `DEF.PARSE_UNKNOWN_KEY`(`input`)로 멈추던 `orders.target.aip`가 `service check`를 통과하는 것으로 새 소스를 확인했다. 문법과 wire는 `wp1-notes.md`(`input` 블록, 바인딩 규칙)와 `wp2-notes.md`(`refs: {필드: {created: 연산번호}}`, `ctx.data.apply`의 일곱 번째 인자 `input`)를 따랐다.

| 구현 | 정의 | 확장 | 파일 |
| --- | --- | --- | --- |
| 우회(현재 문법) | `orders.current.aip` | `extensions/shop.mjs` | `scenario.current.ts` |
| 변형 A: 선언만(P0-A·B) | `orders.target.aip` | 없음 | `scenario.target-a.ts` |
| 변형 B: 확장(P0-B·C), 서버가 단가 계산 | `orders.target-b.aip` | `extensions/shopb.mjs` | `scenario.target-b.ts` |

### 8.1 실행한 시나리오와 결과

| 요구 | 실행 결과 |
| --- | --- |
| 주문+줄 원자 생성(bundle refs) | A1: 호출 한 번으로 주문 1, 줄 2(만든 주문 id를 가리킴), 재고 10→7, 5→3. A3: 줄의 상품이 없으면 `MISSING_TARGET`이고 주문·줄 수 불변(고아 주문 없음) |
| 입력 수량 재고 차감 | A1·A5: 서버가 잠근 행에서 계산. 수량 0, 5000(범위 밖), -1, 누락은 `BAD_VALUE`, 재고(3)보다 큰 4는 `INVALID_STATE`, 재고 불변 |
| 재고 부족 시 전체 롤백 | A2: 묶음 안 두 번째 `take`가 모자라면 `INVALID_STATE`이고 주문·줄·첫 `take`까지 모두 남지 않음. B2: 확장이 `EXTENSION_REJECTED:INSUFFICIENT_STOCK`, 주문·줄·재고 불변 |
| 같은 key 재전송 | A4: `replayed: true`, 처음 만든 `created` id를 그대로 돌려주고 재고는 한 번만 차감, 같은 key에 다른 본문은 `IDEMPOTENCY_MISMATCH`로 실행 안 됨. B4: 확장 재전송이 같은 주문 id를 재생, 주문 1건 |
| 가격 위조 | 아래 8.3 |
| 동시 주문 | 아래 8.2 |

### 8.2 동시 주문: 재고 4개, 2개짜리 6건 동시 전송

| 구현 | 성공 건수(관찰) | 실패 코드 | 비고 |
| --- | --- | --- | --- |
| 우회(1차, 50f2bc27) | 1 | `WORKER_BUSY` 2, `EXTENSION_ERROR` 3 | 5회 모두 1건 |
| 우회(2차, merge 뒤) | 1 | `WORKER_BUSY` 2, `CONFLICT` 3 | 실행마다 1건. 충돌이 `CONFLICT`로 보이는 것만 달라짐 |
| 변형 A, `take`를 먼저 | **2** | `INVALID_STATE` 4 | 6회 이상 모든 실행에서 2건(40~60ms). 기대치 2건과 일치 |
| 변형 A, 줄을 먼저 | 0, 1, 2로 실행마다 다름 | `DEADLINE_EXCEEDED` 다수 | 아래 교착 설명 |
| 변형 B, 사용자당 1건씩 2건 | 2 | | 관찰한 모든 실행(5회) |
| 변형 B, 6건(`take`를 줄 생성보다 먼저) | 2 | `WORKER_BUSY` 2, `EXTENSION_ERROR` 2 | 관찰한 모든 실행(5회)에서 2건. `WORKER_BUSY`는 한 사용자의 동기 worker 한도(기본 2) |

모든 구현에서 재고는 음수가 되지 않았고 성공 건수 x 2만큼만 줄었으며 실패한 호출의 주문·줄은 남지 않았다(안전성). 차이는 활성이다. 우회는 낙관적 충돌 때문에 가능한 2건 중 1건만 성공하고, 목표 변형은 행 잠금 직렬화로 가능한 만큼 성공한다.

**잠금 순서 함정(실행으로 발견).** 변형 A에서 `create OrderLine`을 `take`보다 먼저 두면 줄의 FK가 상품 행에 공유 잠금을 먼저 잡고, 뒤의 `take`가 같은 행을 갱신 잠금으로 올리려 해서 동시 요청끼리 서로 기다린다. PG 로그에 `deadlock detected`가 찍히고(1초 deadlock_timeout), 서버 재시도가 이어지는 동안 요청 한도에 걸려 `WIRE.DEADLINE_EXCEEDED`가 된다. `take`를 묶음의 맨 앞에 두면 같은 요청들이 2건 성공으로 40ms 안에 끝난다. 정확한 락 모드 설명은 PG 로그의 교착 기록과 락 순서에서 추정한 것이고 `pg_locks`로 직접 확인하지는 않았다. 한 줄 순서가 결과를 가르므로 문서에 "같은 행을 갱신하는 연산을 묶음 앞에 둔다"는 지침이 필요하다(제품 코드는 수정하지 않았고 기록만 한다). 변형 B의 확장도 `take`를 줄 생성 앞에 두었다.

### 8.3 가격 위조 문제를 막는 방법

| 구현 | 호출자가 가격을 속이면 | 실행 결과 |
| --- | --- | --- |
| 우회 정의 | 줄의 `unitPrice`와 확장이 계산하는 합계가 그대로 속은 값이 된다 | `unitPrice` 1(실제 200)로 보낸 주문이 합계 1로 확정됨(`workaround-price-forgery-is-accepted`). **우회 정의에도 위조 구멍이 있었다** |
| 변형 A | 줄 `create allow`에 `unitPrice = product.price`를 두어 단가를 서버가 강제한다. 합계 `total`은 곱셈이 없어(E3) 선언으로 검증할 수 없다 | 단가 위조는 `WIRE.FORBIDDEN`이고 흔적 없음. 합계 위조는 막지 못한다: 정당한 줄 1개(합계 200)에 `total: 1`을 보낸 주문이 그대로 저장됨(A6). 한계로 기록 |
| 변형 B | 호출자는 단가·합계를 보내지 않는다. 확장이 `Product.price`를 읽어 합계를 계산하고 `internal` 전이 `settle`로 쓴다. 직접 create는 합계 0의 미확정 주문과 단가가 맞는 줄만 허용한다 | 합계 999 직접 create(placed 참과 거짓 모두), 단가 1 줄은 `FORBIDDEN`. `Product.take`·`Order.settle` 직접 호출은 `NOT_EXPOSED`. 남의 `owner`를 넣은 확장 호출은 거절. 서버가 계산한 합계 400(`unit_price` 200, 수량 2)이 저장됨(B1, B3) |

변형 B의 남은 틈: 직접 create로 합계 0의 줄 없는 미확정 주문은 클라이언트가 스스로 만들 수 있다(B3, 재고와 합계에는 영향 없음). `expose create`에는 `internal` 표지가 없어 확장만 만들 수 있게 닫지는 못한다. 확장은 호출자 id를 알 수 없어 `owner`를 입력으로 받고, 다른 사람의 id를 넣으면 create의 `allow owner = actor`가 거절한다. 확장 입력에 배열이 없어(F14) 변형 B는 한 번에 한 상품만 주문한다. 우회와 변형 A는 줄 여러 개를 다룬다.

### 8.4 작성량과 보장 차이 (실제 파일 기준)

줄 수는 5.1절과 같은 방식(빈 줄·`//` 주석 제외, 괄호는 전체 줄)이다. 일반 백엔드 줄 수는 추정하지 않았다.

| 항목 | 우회(현재) | 변형 A(선언만) | 변형 B(확장) |
| --- | --- | --- | --- |
| 정의 줄 | 33(41) | 26(32) | 36(42) |
| 확장 코드 줄 | 17(19) | 0 | 12(16) |
| 주문 한 건을 넣는 클라이언트 호출 수 | 3: 부모 `bundle`, 줄 `bundle`, `writeExtension` | 1: `bundle` | 1: `writeExtension` |
| 시나리오 파일(시험 코드) | 106줄(124), 호출 지점 17 | 122줄(134), 호출 지점 18 | 72줄(80), 호출 지점 10 |
| 다중 줄 주문 | 가능 | 가능 | 한 상품만(F14) |
| 고아 주문 | 줄 생성 전에 끊기면 줄 없는 미확정 주문이 남음(DB 확인). 정리는 앱 몫 | 남지 않음(A2, A3) | 확장 실패 시 남지 않음(B2). 단 클라이언트가 합계 0 미확정 주문을 직접 만들 수는 있음 |
| 갱신 유실 | 읽고 계산해 쓰는 확장이 `expect`로 보호해 유실은 없지만 충돌은 거짓 실패가 된다 | 계산을 서버가 잠근 행에서 하므로 읽은 값이 낡을 일이 없다 | `take`는 같다. `settle`의 합계는 확장이 계산한 값을 `patch`로 쓴다 |
| 동시성(재고 4, 2개짜리 6건) | 성공 1(가능 2) | 성공 2(`take` 먼저). 줄 먼저면 교착 | 성공 2(6건 중 `WORKER_BUSY` 2, `EXTENSION_ERROR` 2) |
| 재고 부족 판정 | 확장이 읽고 비교 | `from stock >= input.qty` | 확장 사전 확인과 `take`의 `from` |
| 단가·합계 위조 | 가능(실행 확인) | 단가 차단, 합계는 클라이언트 값 | 둘 다 차단 |
| 멱등 재전송 | 가능(각 호출마다 key. 부모 `create`와 확장 호출이 따로) | 요청 하나의 key가 전체를 덮음 | 확장 호출 하나의 key |

정의 줄이 변형 B에서 가장 많은 것은 직접 create를 좁히는 조건(`total = 0`, `placed = false`, `unitPrice = product.price`)과 `settle` 전이, 확장 계약 선언 때문이다. 줄 수만 보면 변형 A가 가장 작지만 합계를 선언으로 검증할 수 없다는 보장 차이가 있어, 신뢰가 필요한 합계에는 변형 B가 맞다.

## 9. 범위 밖 발견과 후속 조치 (`build-sdk.mjs`만 수정)

| # | 재현 | 비고 |
| --- | --- | --- |
| P-1 | 별도 임시 worktree에서 `product/sdk/index.ts`에 `const deliberatelyBroken: = ;`를 덧붙이고 `node product/tools/build-sdk.mjs --out /tmp/aip-sdk-build-fail-out-4` 실행 → `BUILD_FAILED`와 `TS1110: Type expected`, `TS1109: Expression expected`를 출력. `build-sdk.mjs:83-94`가 컴파일 예외의 stderr/stdout에서 최대 8줄·1200자를 요약한다. 성공 출력 형식은 변경하지 않았다 | 수정·컴파일러 출력 확인 |
| P-2 | JWT TTL을 주입한 임시 `run.mjs` 사본으로 `CARGO_TARGET_DIR=/tmp/aip-target-apps AIP_BIN=/tmp/aip-target-apps/debug/aip AIP_TEST_TOKEN_TTL=3600 node product/tests/fixtures/expr-apps/.tmp-run-jwt.mjs --app saas --lang ts` 실행 → `FAIL 16`, 모두 `WIRE.UNAUTHENTICATED`; `AIP_TEST_TOKEN_TTL=300` → `PASS 17 / FAIL 0`. 원인은 `crates/aip-auth/src/lib.rs:657`의 900초 상한으로 확정했다. 제한은 설정 불가이며, 응답은 `WIRE.UNAUTHENTICATED` / "토큰 검증 실패"로 일반화되고 로그에 원인은 없다. 제품 문서에는 최대 900초가 이미 명시돼 있다 | 원인 확정(F17) |
| P-3 | `access x = totalOfVisible(Comment)`를 `Task`의 `commentCount`에 사용하면 `spikes/spike-v1-fixture/src/sema.rs:1721`이 `DEF.ACCESS_MISMATCH`와 실제 문구 `` `visibleTotal`은 `Comment` 기준 groupKey 집계에만 사용 ``을 만든다. 판정은 `p != r.name || a.group_key.is_none()`이며 여기서 `r.name`은 집계 선언 resource(`Task`), `p`는 접근 원본(`Comment`)이다. 즉 Comment 행이 Task를 가리키는 groupKey 집계라는 뜻이 반대로 읽힌다 | `sema.rs`·`codes.rs`는 다른 작업자 소유라 수정하지 않음. 제안: `` `{n}`은 `{r.name}`을 참조하는 `{p}`의 groupKey 집계에서만 사용 ``(예: “`visibleTotal`은 `Task`를 참조하는 `Comment`의 groupKey 집계에서만 사용”) |
| P-4 | LIMITS.md E2 본문 대 `ctx.reject` 동작 | audit.md가 이미 기록한 항목, 이번에도 재현 |
| P-5 | 확장의 낙관적 충돌이 호출자에게 `WIRE.EXTENSION_ERROR`로 보임(1차 동시 주문 시험, F15) | P0 merge 뒤 같은 시험에서 `WIRE.CONFLICT`로 보임을 확인했다. 해소됨 |
| P-6 | 변형 A 묶음에서 `create OrderLine`을 `take`보다 앞에 두고 같은 상품을 6건 동시에 주문 → 0~2건 성공, 나머지 `WIRE.DEADLINE_EXCEEDED`, PG 로그 `deadlock detected`. `take`를 맨 앞으로 옮기면 정확히 2건 | 잠금 순서에 따라 교착이 나는 작성 함정이다. 문서에 순서 지침이 없다. 응답은 `DEADLINE_EXCEEDED`라 교착이 원인임을 호출자가 알기 어렵다 |
| P-7 | 변형 B 확장 안에서 `take`가 `INVALID_STATE`로 실패하거나 낙관적 충돌이 나면 호출자는 `WIRE.EXTENSION_ERROR`를 받는다. 사전 확인이 못 거른 경쟁 패배는 `EXTENSION_REJECTED`가 아니라 일반 오류로 보인다(6건 동시 시험의 `EXTENSION_ERROR` 원인이 `take` 실패라는 것은 확장 로그를 보지 않은 추정) | 사유 구분이 필요하면 확장이 `ctx.reject`를 쓰도록 `take` 실패를 잡을 수 없다(호출이 poison됨, SDK.md 업무 거절 절). 호출자는 재시도 가능 여부를 알기 어렵다 |

## 10. 미실행과 한계

- 실행하지 않은 것: 효과 `delete many`·`update many`의 `maxRows` 초과 동작, 초대 직후 로그인(`principal bind`), 비동기 job·파일·실시간, 관계 필드 필터(audit #13), 변형 B의 다중 줄 주문(확장 입력에 배열이 없어 시나리오로 만들지 못함), Python SDK로 P0 기능 호출(`created(i)` helper, `apply_*`의 `input=`). 이 SKIP 사유는 모두 범위 밖이며 `run.mjs` 출력의 SKIP은 0이다.
- Python 시나리오는 SaaS 7개, 커뮤니티 3개로 TypeScript보다 적다. 주문·재고는 TypeScript만 실행했다.
- 동시 주문 결과는 어느 호출이 성공하는지가 실행마다 다르다. 시험은 불변식(재고 음수 없음, 성공 건수와 차감량 일치, 성공한 주문의 줄만 남음)을 매번 단언하고, 건수 자체는 변형 A의 `take` 먼저 경우만 정확히 2건으로 단언한다(줄 먼저와 변형 B 6건은 실행마다 달라 건수를 단언하지 않고 기록만 한다).
- 변형 A `take` 먼저가 항상 2건이라는 결론은 약 6회 관찰에 근거한다. 다른 부하·다른 PG 설정에서는 확인하지 않았다.
- RED 확인(시험이 실패할 수 있는지): 변형 A의 `unitPrice = product.price` 제거는 A6가, 변형 B의 `total = 0` 제거는 B3가, `take`의 `from` 제거는 A2·A5(`INVALID_STATE`가 `BAD_VALUE`로 바뀜)가 잡았다. 처음 시험은 `from`이 없어도 stock의 `Int(0..)` 범위 검사가 음수를 막아 통과했기 때문에 코드 단언을 보강했다.
- 일반 백엔드 비교는 계층 목록이며 줄 수나 개발 시간은 측정하지 않았다.
