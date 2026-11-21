# 소비자 앱 3유형 표현력 검증 (협업형 SaaS, 콘텐츠·커뮤니티, 주문·재고)

측정: 2026-11-21, 브랜치 `test/expr-consumer-apps-20261121`(기준 `design/expressiveness-1.0-20261121` 50f2bc27), aip 1.0.0 debug 빌드(`CARGO_TARGET_DIR=/tmp/aip-target-apps`), PostgreSQL 17.11 전용 컨테이너, Node v26.9.0, Python 3.14.7, TypeScript 7.0.2.

목표: AIP가 일반 백엔드 업무를 얼마나 적은 반복 코드로, 안전하고 예측 가능하게 구현하게 해 주는지를 소비자 앱 세 유형으로 본다. 문법 통과가 아니라 실제 HTTP(`aip service serve`)·PostgreSQL·SDK 경로 실행을 근거로 쓴다. 협업형 SaaS와 콘텐츠·커뮤니티는 현재 기능으로 끝까지 실행했고, 주문·재고는 현재 문법으로 가능한 부분만 실행하고 P0 기능이 필요한 부분은 목표 정의만 준비했다(미실행).

## 1. 실행 결과

명령 한 번: `node product/tests/fixtures/expr-apps/run.mjs`(전용 PG 17 컨테이너 기동, 앱마다 새 schema `service init`, 시드, principal 바인딩, `serve`, 시나리오, 정리). 마지막 전체 실행 출력:

```
실제 PostgreSQL: PostgreSQL 17.11 (Debian 17.11-1.pgdg13+2) on aarch64-unknown-linux-gnu, ...
DSN 대상: 전용 컨테이너 aip-expr-apps-19286 (host=127.0.0.1 port=54532 user=… dbname=aip_test_expr_apps)
revision: 9ec531c70b87, aip 1.0.0, HTTP=aip service serve, SDK=@aip/sdk(tarball 설치)+python aip_sdk
합계: PASS 47 / FAIL 0 / SKIP 1          (전체 11초)
```

| 단위 | 시나리오 | 결과 |
| --- | --- | --- |
| saas/ts | typecheck + 16개 | PASS 17 |
| saas/py | 7개 | PASS 7 |
| community/ts | typecheck + 11개 | PASS 12 |
| community/py | 3개 | PASS 3 |
| orders/ts (현재 문법과 우회) | typecheck + 7개 | PASS 8, INFO 1(동시 주문 관찰값) |
| orders/target (P0-A·B·C 목표) | 5개 | SKIP 1: `미실행: P0 미병합. 현재 check 결과 DEF.PARSE_UNKNOWN_KEY transition 안 알 수 없는 항목 input` |

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
| 주문·재고(현재 우회) | 33줄(41) | 17줄(`shop.mjs`) | TS 99줄(115). 호출 지점 16곳(apply 5, bundle 4, writeExtension 7) | 시드 SQL 2줄 |
| 주문·재고(P0 목표, 미실행) | 34줄(44) | 10줄(`shop.target.mjs`) | 미실행(scenario.target.ts 66줄은 실행 전 초안) | |

주의: 클라이언트 줄 수는 앱 클라이언트가 아니라 시험 코드라서 단언과 부정 시나리오가 섞인 상한이다. 한 기능의 호출 코드는 보통 SDK 호출 한 번(3~6줄)이다. 정의에 `expose`로 열지 않은 것(읽기·쓰기·전이)은 서버가 만들어 주지 않는다. 서버 쪽에 사람이 쓴 Controller·DTO·Service·Repository 코드는 두 앱에서 0이고, 주문·재고에서는 확장 한 파일뿐이다.

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
| F15 | 동시 주문 우회: 재고 4개, 2개짜리 주문 6건을 동시에 보냈더니 반복 실행마다 성공은 1건이었고 나머지는 `WORKER_BUSY` 2건과 `EXTENSION_ERROR` 3건이었다(가능했던 성공은 2건). 재고가 음수가 되거나 이중 차감되지는 않았다(안전성 유지, 활성은 나쁨). `WORKER_BUSY`는 한 사용자의 동기 worker 동시 호출 한도(기본 2) 때문이고, 낙관적 충돌은 호출자에게 `CONFLICT`가 아닌 `EXTENSION_ERROR`로 보여 재시도할지 구분할 수 없다 | 한계(E2, P0-B가 해소할 항목)·문서 |
| F16 | `build-sdk.mjs`가 컴파일에 실패하면 `BUILD_FAILED "SDK 컴파일 실패"`만 알려 준다. 원인(원래 checkout의 TypeScript가 linux 바이너리뿐)은 `tsc`를 직접 실행해서야 보였다 | 문서(진단) |
| F17 | JWT의 `exp - iat`를 3600초로 만들었을 때 모든 호출이 이유 없이 `WIRE.UNAUTHENTICATED`였고, 300초로 바꾸자 해결됐다. 수명 한도 때문이라고 추정하며 코드로는 확인하지 않았다(다른 조건은 동일). 거부 사유가 응답에 없다 | 문서(진단, 추정) |

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

## 8. 주문·재고: 현재 우회와 P0 목표를 나란히

현재 우회는 모두 실행했고(`orders.current.aip`, `extensions/shop.mjs`, `scenario.current.ts`), 목표 정의(`orders.target.aip`, `extensions/shop.target.mjs`, `scenario.target.ts`)는 **미실행**이다. 목표 정의의 `input` 문법은 decisions.md와 `feat/expr-p0b-arith-20261121`의 시험(`spikes/spike-v3-write/tests/expr_arith.rs`)에서 가져왔다. 이 worktree의 바이너리는 P0를 포함하지 않아 `DEF.PARSE_UNKNOWN_KEY`로 멈춘다(`run.mjs`가 매번 기록). 문법이 실제 구현과 맞는지는 병합 뒤 확인해야 한다.

| 시나리오 | 현재 우회(실행) | P0 병합 뒤 목표(미실행) |
| --- | --- | --- |
| 주문과 줄 원자 생성 | 호출 2회: 부모 `create`, 그 id로 줄 `bundle`, 확장이 확정. 줄 생성 전에 끊기면 줄 없는 주문이 남음(DB로 확인) | P0-A: `bundle` 한 번에 `create Order`, `create OrderLine ... refs {order: {created: 0}}`. 실패 시 부모도 남지 않음(A1) |
| 입력 수량만큼 재고 차감 | 확장이 읽고 계산해 `setStock`(patch+expect)으로 씀. 확장 17줄 | P0-B: `transition take { input { qty }; from stock >= input.qty; to stock = stock - input.qty }`와 apply의 `input: {qty}`. 확장 없이 선언만(A2) |
| 동시 주문 | 재고 4, 주문 6건(각 2개): 성공 1건, `WORKER_BUSY` 2, `EXTENSION_ERROR` 3, 재고는 음수 아님(F15) | 행 잠금으로 직렬화되어 가능한 만큼 성공해야 한다. 기대: 재고 3, 수량 1 요청 5건이면 정확히 3건 성공(B1) |
| 멱등 재전송 | 확장 key 재전송은 `replayed: true`로 재고 한 번만 차감, 확정 주문의 다른 key 재확정은 거부, 부모 `create` 재전송은 중복 주문 없음 | 묶음 전체가 한 key. 재전송이 저장된 `created`를 재생하고 재고를 다시 줄이지 않아야 한다(A3) |
| 서버가 단가로 합계 계산 | 확장이 줄을 읽어 합계 계산(700) | 변형 A(선언만)는 `total`과 `unitPrice`를 클라이언트가 보내므로 가격 위조가 가능하다. 변형 B(P0-C `ctx.data.create` + `take`)가 서버 단가로 계산하고 원자적이다(B2). 다만 확장 입력에 배열이 없어 `Json` 입력이 된다(F14) |

병합 뒤 후속 작업 순서: (1) 새 바이너리로 `orders.target.aip` check, (2) 문법 차이(`refs`, `input`, `ctx.data.apply`의 input 인자 이름. 확인 필요)를 맞춤, (3) `run.mjs`의 `targetOnly` 단위를 일반 단위로 바꾸고 `scenario.target.ts`를 연결, (4) 위 기대값(특히 B1의 정확히 3건)을 실행으로 확인, (5) `shop.mjs`와 현재 우회 시나리오를 목표와 대조해 줄일 수 있는 코드 양을 이번 절의 줄 수와 비교.

## 9. 범위 밖 발견(제품 코드는 수정하지 않았다)

| # | 재현 | 비고 |
| --- | --- | --- |
| P-1 | `node product/tools/build-sdk.mjs --out /tmp/x`를 linux 전용 `node_modules`에서 실행 → `BUILD_FAILED "SDK 컴파일 실패. 소유한 출력 디렉터리를 정리함"` | 원인 출력 없음(F16) |
| P-2 | 시험 JWT를 `exp = iat + 3600`으로 만들어 `/session` 호출 → `WIRE.UNAUTHENTICATED`(로그에 `status 200`). `+300`은 통과 | 수명 상한이 원인이라는 추정(F17) |
| P-3 | `access x = totalOfVisible(Comment)`를 `Task`의 `commentCount`에 사용 → `DEF.ACCESS_MISMATCH` | 진단 문구가 반대로 읽힘(F4) |
| P-4 | LIMITS.md E2 본문 대 `ctx.reject` 동작 | audit.md가 이미 기록한 항목, 이번에도 재현 |
| P-5 | 확장의 낙관적 충돌이 호출자에게 `WIRE.EXTENSION_ERROR`로 보임(동시 주문 시험, F15) | `feat/expr-p0c-ext-create-20261121`의 "CONFLICT·ALREADY_EXISTS를 코드 그대로 전달" 커밋이 다루는 것으로 보이나 이 worktree에서 확인하지 못함 |

## 10. 미실행과 한계

- 주문·재고 목표 정의와 시나리오 5개: 미실행(P0-A·B·C 병합 전).
- 효과 `delete many`·`update many`의 `maxRows` 초과 동작, 초대 직후 로그인(`principal bind`), 비동기 job·파일·실시간, 관계 필드 필터(audit #13)는 실행하지 않았다.
- Python 시나리오는 SaaS 7개, 커뮤니티 3개로 TypeScript보다 적다. 주문·재고는 TypeScript만 실행했다.
- 동시 주문 결과는 실행마다 어느 호출이 성공하는지가 달라지고 성공 건수는 모든 실행에서 1건이었다(5회 확인). 시험은 불변식(재고 음수 없음, 성공 건수와 차감량 일치)만 단언하고 성공 건수 자체는 단언하지 않는다.
- 일반 백엔드 비교는 계층 목록이며 줄 수나 개발 시간은 측정하지 않았다.
