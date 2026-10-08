# 오픈소스 앱 백엔드 패턴 조사 (AIP 기준선 대비)

> 상태: 초안(2026-10-07). 서브에이전트가 공개 문서·소스 요약으로 조사한 자료이며 실행 검증은 하지 않았다. "AIP 상태" 열은 조사 시점 기준이고 이후 변경은 [점검 기록](2026-10-07-pattern-security-audit.md)을 따른다. 제안 절은 결정이 아니다.

기준선: P1~P72 (coverage-audit.md 끝 표, pattern-security-audit.md §4). 상태 표기: 직접/우회/확장 필요/불가.
기준선 요약: 직접=단건·목록·정렬·범위·FK eq/in/isNull(최근 추가)·N:1 traverse·행별 count·상태머신·소유자/역할/테넌트 정책·유니크·초대·감사로그(전이마다)·멱등키. 불가=offset·검색·sum/avg/group by·카운터증감·patch/hard delete/호출자값 create(OPEN)·기본값·계산필드·webhook·rate limit·스케줄·cascade.

## 1. Discourse (포럼) — https://docs.discourse.org/
- 리소스: topics, posts, categories, users, notifications, search, invites, groups, tags, likes/reactions (WebFetch 요약; 세부 파라미터는 확인 못함: 요약만 반환).
- 동작: 토픽 목록(카테고리·태그 필터, page), 글 작성/수정/삭제, 좋아요/반응, 검색, 초대 발급, 그룹 멤버십, 알림 목록, 웹훅.
- 패턴: page/offset 페이지네이션, 전문검색, 계층(카테고리·답글 트리), 태그 N:M, 반응, 그룹/역할 권한(카테고리별 읽기/쓰기), 초대, 알림 fan-out, rate limit, 웹훅, 소프트 삭제(글 삭제 복구), 버전 이력(글 revision; 추정: 일반 지식).
- AIP: offset 불가, 검색 불가, 반응 우회, 알림 직접(outbox), webhook/rate limit 불가, 태그 우회.

## 2. Mastodon (SNS) — https://docs.joinmastodon.org/methods/timelines/
- 리소스: statuses, accounts, timelines(home/public/hashtag/link/list), lists, notifications, polls, bookmarks, filters.
- 동작: 타임라인 조회(max_id/since_id/min_id), 즐겨찾기/부스트/북마크, 팔로우/언팔로우, 리스트 관리, 투표, 필터 설정.
- 패턴: keyset(cursor) 페이지네이션(id 기반 max_id/min_id, limit 기본 20 최대 40), 가시성(public/unlisted/private/direct), 팔로우 그래프 기반 피드(fan-out on write 홈 타임라인; 추정), 반응 토글, 투표, 해시태그 N:M, 알림.
- AIP: cursor는 id gt/lt 불가라 우회(동률 누락), 팔로우 그래프 피드(조인 필터)는 우회, 가시성은 직접, 토글 우회.

## 3. Ghost (블로그/CMS) — https://docs.ghost.org/admin-api
- 리소스: posts, pages, tags, members, tiers, newsletters, offers, labels, users, images, themes, webhooks.
- 동작: 글 browse/read/add/edit/delete, 이미지 업로드(/images/), 멤버 관리, 웹훅 등록.
- 패턴: NQL 필터 문법, page+limit(기본 15) offset 페이지네이션 + meta, 상태 draft/published/scheduled(예약 발행), updated_at 낙관적 동시성, 파일 업로드, 웹훅, include 관계 확장, 구독 tier(결제 Stripe; 추정).
- AIP: 예약 발행 우회(노출 시각만), 낙관적 잠금 부분, 파일 우회, 웹훅 불가, offset 불가.

## 4. Medusa v2 (이커머스) — https://docs.medusajs.com/api/store
- 리소스: products, carts(line items, promotions, shipping, payment sessions), orders, customers, regions.
- 동작: 상품 목록(fields/필터/정렬), 장바구니 생성/라인아이템 추가·수정·삭제, 프로모션 적용, 배송수단, 결제세션, 카트 완료→주문, 주문 이력.
- 패턴: limit/offset/count, `-` 접두 정렬, fields dot-notation 관계 확장, publishable key로 sales channel 스코프(테넌트), 카트→주문 트랜잭션 워크플로(재고 예약·결제), 가격 계산(리전·프로모션), 메타데이터 JSON 병합, locale.
- AIP: 카운터/재고·산술 불가, 계산 필드 불가, 결제 불가, 다중 resource 트랜잭션은 효과 기반 직접, count total 우회.

## 5. Plane (이슈/프로젝트) — https://developers.plane.so/api-reference/introduction
- 리소스: workspace, project, work-item(issue), state, label, cycle, module, comment, link, attachment.
- 동작: 이슈 CRUD(PATCH/DELETE), 상태 변경, 라벨 부여, 사이클/모듈에 이슈 추가, 코멘트, 링크·첨부.
- 패턴: cursor(`value:offset:is_prev`)+per_page+total_results, fields/expand, API key/OAuth, 분당 60 rate limit, workspace>project 테넌트, 상태(State 그룹) 머신(사용자 정의 상태), 라벨 N:M. 필터·정렬·웹훅 세부는 확인 못함(문서 미기재).
- AIP: 테넌트·상태 직접, 라벨 N:M 우회, patch/delete 불가, rate limit 불가, offset/total 불가.

## 6. Outline (문서/위키) — https://www.getoutline.com/developers
- 리소스: documents, collections, shares, memberships(user/group), comments, revisions, stars, subscriptions.
- 동작: documents.list/info/search/search_titles/create/update/archive/restore/delete/move, collections.add_user/remove_user/export/import, shares.create/revoke, comments(resolve, reaction), revisions 복원, 접근요청 승인.
- 패턴: RPC형 POST, limit/offset+nextPath, 전문검색, 컬렉션>문서 트리 + move, 소프트 삭제(archive/restore/휴지통), 버전 이력+복원, 공유 링크(토큰 공개), 컬렉션/문서별 멤버 권한(사용자·그룹), 즐겨찾기, 구독, 댓글 resolve+반응, export/import, 웹훅.
- AIP: 트리(self-ref FK)는 부분, 검색/offset 불가, archive는 상태필드 우회, 이력 우회(전이마다 기술), 공유 링크 토큰은 확인 필요(직접 표현 근거 없음, 추정 불가에 가까움), 그룹 권한 우회, export 불가.

## 7. Cal.com (일정 예약) — https://cal.com/docs/api-reference/v2/introduction
- 리소스: event-types, bookings, slots, schedules(availability), teams, calendars, webhooks, OAuth.
- 동작: 예약 생성/재일정/취소/확정, 슬롯 조회(가용 계산), 스케줄 CRUD, 팀·멤버십, 웹훅 등록.
- 패턴: 계산형 조회(가용 슬롯=스케줄−예약−외부캘린더 충돌), 예약 상태머신(pending/accepted/cancelled/rescheduled), 이중예약 방지(시간 구간 겹침 제약), 팀/조직 테넌트, 분당 120 rate limit, 웹훅, 외부 캘린더/결제 연동, 시간대, 반복 일정(추정: 일반 지식, 문서 요약엔 없음).
- AIP: 상태머신 직접, 구간 겹침 제약(exclusion)은 유니크로 표현 불가=불가(추정), 계산 슬롯 불가, webhook/rate limit/스케줄 불가.

## 8. Zulip (채팅) — https://zulip.com/api/rest
- 리소스: messages, channels(streams), topics, subscriptions, users, reactions, flags, uploads, event queue.
- 동작: 메시지 전송/수정/삭제, narrow 조회(anchor+num_before/num_after), 읽음 플래그 일괄 갱신, 반응, 채널 생성/구독, 토픽 mute/삭제, 타이핑/presence, 파일 업로드, event queue 롱폴링.
- 패턴: anchor 기반 양방향 keyset 페이지네이션, narrow(다중 필터+검색 문법), 읽음 상태 per-user 플래그(대량 갱신), 채널 구독 멤버십+역할, 실시간 push(롱폴링), 파일 업로드, rate limit, 소프트 삭제 없음(추정).
- AIP: 양방향 cursor 불가(id gt/lt 불가), 검색 불가, 읽음 일괄=bulk 전이 우회, 실시간 구독 불가(범위 밖 추정), 파일 우회.

## 9. Formbricks (설문/폼) — https://formbricks.com/docs/api-reference/rest-api
- 리소스: surveys, responses, contacts, webhooks, action classes, environments.
- 동작: 설문 CRUD, 응답 목록(설문별)/수정/삭제, 컨택트 속성, 웹훅 설정. API key 개인 발급.
- 패턴: 익명/공개 클라이언트 API로 응답 제출(호출자 값 create), 환경(prod/dev) 스코프 테넌트, 웹훅, 응답 export·집계(요약 통계; 문서엔 없음, 추정). 필터·limit/skip 세부 확인 못함: 요약에 미기재.
- AIP: 호출자 값 create 불가(OPEN), 집계 count 외 불가, 웹훅 불가.

## 10. Plausible (분석) — https://plausible.io/docs/stats-api
- 리소스: /api/v2/query(읽기 전용), Events API.
- 동작: metrics(visitors, pageviews, bounce_rate, visit_duration…) × dimensions(page, source, country, time:day…) 쿼리, filters(is/contains/matches, and/or/not), date_range, order_by, limit(기본 10000)+offset, 총 행수.
- 패턴: 요청자가 표현하는 **집계 쿼리 DSL**(metrics/dimensions/filters)이 핵심 = AIP 취지와 가장 유사, group by 임의 차원, 시간 버킷, 비율 지표, 팀 스코프 키, 시간당 600 rate limit, 이벤트 ingestion(호출자 값 create, 익명 허용).
- AIP: group by/sum/avg/시간버킷 불가, offset 불가, 익명 ingest는 create OPEN, rate limit 불가.

## 11. Miniflux (RSS) — https://miniflux.app/docs/api.html
- 리소스: feeds, entries, categories, users, OPML.
- 동작: entries 목록(status/starred/search/category_id/before/after/published_*/order/direction/limit/offset), 단건 수정, bookmark 토글, 다건 상태 일괄 변경(PUT /v1/entries ids+status), mark-all-as-read(피드/카테고리 단위), feed refresh, fetch-content, OPML import/export, 읽음/안읽음 counters.
- 패턴: offset 페이지네이션, 검색, 시간 범위 필터, 토글, ids 일괄 상태 변경, 조건(범위) 일괄 전이(mark all as read by feed), per-feed unread 카운터(집계 group by), 소유자 스코프(전부 user_id), 외부 fetch(스케줄러/크롤), 중복 제거(해시 유니크).
- AIP: 소유자 목록 직접, 필터 직접, 검색/offset 불가, 일괄은 ids 필터 불가·where만이라 우회, group by counters 불가, 외부 fetch 불가(확장 영역).

## 12. Firefly III (가계부) — https://api-docs.firefly-iii.org/
- 리소스: accounts, transactions(splits), budgets, budget limits, categories, bills, piggy banks, recurrences, rules, tags, webhooks, charts, summary/insight, search.
- 동작: 거래 생성(복수 split, 출금/입금/이체), 계좌 잔액(기간 기준), 예산 한도 대비 지출 요약, 정기 거래(recurrence) 실행, 규칙 자동 적용(트리거→액션), 저금통 입출금, 차트 데이터.
- 패턴: page/limit, 기간 필터, 집계(sum by category/budget/period), 계좌 잔액=거래 합계 파생값, 이체=두 계좌 대칭 쓰기(복식), 반복 일정(cron), 규칙 엔진(이벤트→조건→액션), 웹훅, 태그 N:M, 중복 탐지, 다통화.
- AIP: sum/group by 불가, 파생 잔액 불가(계산 필드 불가), 두 행 쓰기는 효과 기반 직접(단 호출자 금액 값은 OPEN), 반복/스케줄 불가, 규칙 엔진 불가, 웹훅 불가.

## 13. Immich (사진) — https://api.immich.app/introduction
- 리소스: assets, albums, search, people/faces, timeline buckets, trash, stacks, memories, partners, tags, libraries, jobs, shared links.
- 동작: 업로드(+중복 체크 bulk-upload-check), 앨범에 asset 추가/제거, 앨범 공유 사용자·공유 링크, 메타/CLIP 검색, 타임라인 버킷(월별 그룹), trash 이동/복원, 스택, 태그 부여, 파트너 공유.
- 패턴: 파일 업로드(해시 중복), 소프트 삭제 trash+복원, N:M(album-asset, tag-asset), 공유 링크(토큰, 만료·비밀번호; 추정 일반 지식), 사용자/파트너 공유 권한(viewer/editor), 시간 버킷 그룹 집계, 벡터 유사도 검색, 백그라운드 잡 큐.
- AIP: 파일 우회, trash 우회(상태), N:M 우회(조인 resource), 버킷 집계 불가, 벡터/전문 검색 불가, 공유 링크 불가 추정.

## 14. Twenty (CRM) — https://www.mintlify.com/twentyhq/twenty/developers/api/rest-api (검색 결과 요약, docs.twenty.com 직접 fetch는 리다이렉트 후 미실행)
- 리소스: people, companies, opportunities, tasks, notes, 사용자 정의 object, 워크스페이스.
- 동작: 목록(필터·orderBy·cursor), 단건, 생성/수정, 삭제(soft)·복원, 배치 생성/수정, 관계 연결(people↔companies), 웹훅.
- 패턴: cursor 페이지네이션(limit, cursor/starting_after, has_more), 필터 문법 `filter[email][contains]=`, 다중 orderBy, soft delete(+복원), 워크스페이스 테넌트, 사용자 정의 스키마(런타임 객체/필드 정의), 칸반 파이프라인 stage 상태 이동(+position 정렬; 추정), 웹훅. 세부는 검색 스니펫 기준이라 불완전.
- AIP: contains 불가(prefix만), soft delete 우회, 테넌트 직접, 런타임 스키마 변경은 범위 밖(추정), 웹훅 불가.

## 15. Chatwoot (헬프데스크) — https://developers.chatwoot.com/api-reference/introduction
- 확인 못함: 개별 엔드포인트 문서(소개 페이지에는 Application/Client/Platform API 3분류만 있음). 아래는 일반 지식, 모두 추정.
- 리소스(추정): conversations, messages, contacts, inboxes, agents, teams, labels, canned responses, webhooks. 계정(account) 단위 테넌트.
- 패턴(추정): 상태 open/pending/snoozed/resolved 머신, 담당자/팀 배정, 라벨 N:M, 대화 필터 쿼리(다중 조건 AND/OR 배열), page 페이지네이션, 메시지 public/private note, Client API는 inbox identifier+contact identifier로 비인증 위젯 호출(호출자 값 create), 웹훅.
- AIP: 상태머신/테넌트 직접, 배정은 자기 지정만 직접(P70), 복합 필터 OR 불가(추정), 익명 create OPEN.

## 16. Gitea issues (이슈/코드호스팅) — https://docs.gitea.com/api/1.24/
- 리소스: repository issues, comments, labels, milestones, reactions, dependencies, tracked time, subscriptions, attachments, lock.
- 동작: 이슈 목록(state/labels/milestones/assignee/q/since/before/type), 생성/수정(deadline), 코멘트 CRUD, 라벨 추가·제거, 반응 추가/삭제, 의존(blocks) 관계, 시간 기록, 구독, 잠금.
- 패턴: page/limit + Link 헤더 + X-Total-Count(offset+total), 전문 q 검색, 복합 필터(라벨 다중), 이슈 번호는 repo 내 순번(per-parent sequence counter), 반응 N:M(user×comment×emoji 유니크), 의존 관계(자기참조 N:M), 잠금(상태 플래그), 구독, repo 수준 권한(owner/collaborator/org team), 첨부 업로드.
- AIP: 순번 채번 불가(카운터), offset/total 우회(count 별도), 검색 불가, 라벨 필터 N:M 우회, 반응 우회, 잠금은 상태 직접.

## 17. Saleor (이커머스, GraphQL) — https://docs.saleor.io/api-usage/overview
- 리소스: channels, products/variants, checkout, orders, warehouses/stock, 권한, 웹훅, metadata. 개별 동작 문서는 확인 못함: 개요 페이지에 목차만 있음. 아래 추정.
- 패턴: public/private API 분리(권한 그룹별 허용 목록과 유사), Relay cursor 페이지네이션(first/after, 추정), filter/where/sortBy, channel 스코프 가격/재고, checkout 라인 추가→완료 시 재고 allocation(원자적 차감), 웹훅(비동기/동기), metadata(public/private JSON), 다국어(translations).
- AIP: 재고 차감·산술 불가, 결제 불가, 웹훅 불가, 정의 허용 목록 개념은 AIP와 동일 계열.

## 18. Payload CMS (CMS) — https://payloadcms.com/docs/rest-api/overview
- 리소스: collections(설정에서 자동 생성), globals, auth collections, upload collections.
- 동작: find/findByID/count/create/PATCH(where 조건 일괄 또는 id)/DELETE(where 일괄 또는 id), login/logout/refresh/verify/forgot/reset password, globals 읽기/갱신, 커스텀 endpoint.
- 패턴: where 연산자 DSL(요청이 표현)+limit/page+sort+depth(관계 populate)+select+joins(역관계 1:N), 필드·컬렉션 단위 access control 함수, drafts/versions(이력·복원), localization, upload, count 엔드포인트, bulk update/delete by where. 정의(config)에서 API 자동 생성 = AIP 취지와 가장 가까운 사례 중 하나.
- AIP: count 별도 직접(count 한정), where 일괄 update는 전이만, patch/delete 불가, 버전 이력 우회, 다국어 불가, 역관계 joins 불가(FK eq로 우회), 파일 우회.

## 19. Linkding (북마크) — https://linkding.link/api/
- 리소스: bookmarks, archived, assets, tags, bundles, user profile.
- 동작: 목록(q 검색·태그·날짜·bundle, limit/offset), 단건, URL 존재 체크(check), 생성(자동 메타 스크랩·자동 태깅), PUT/PATCH, archive/unarchive, delete, 첨부 업로드, 태그 CRUD.
- 패턴: 검색 문법, offset, URL 유니크(upsert 성격 check), 태그 N:M, 저장된 검색(bundle), 외부 fetch/스크랩, 자동 규칙 태깅, 파일 첨부, 소유자 스코프(+shared 공개 플래그), 사용자 설정.
- AIP: 소유자/공개 직접, URL 유니크 직접, 검색/offset 불가, 태그 N:M 우회, 스크랩 불가(확장), archive 상태 우회.

## 20. Canvas LMS (LMS) — https://canvas.instructure.com/doc/api/
- 리소스: courses, enrollments, assignments, submissions, grades, modules, quizzes, discussion topics, groups, roles/permissions, masquerading.
- 동작: 과제 제출(호출자 값 create+파일), 채점, 수강 등록(role별), 모듈 순서 배치, 퀴즈 응시, 토론 스레드, 그룹 멤버십.
- 패턴: Link 헤더 페이지네이션, 코스 단위 enrollment 롤(teacher/student/TA)로 권한, 제출 상태(submitted/graded/late), 성적 집계(가중 평균; 추정), 모듈 position 재배치(추정), 마감일 기반 late 계산, 대리 접속(masquerade), throttling(rate limit, 문서에 세부 없음), OpenAPI 제공.
- AIP: 역할/테넌트 직접, 제출 상태머신 직접, 성적 평균 불가, position 재배치 불가(추정), late 계산 필드 불가, 제출 값 create OPEN.

## 21. Umami (분석) — https://docs.umami.is/docs/api
- 리소스: websites, stats, pageviews timeseries, metrics(type별), sessions, events, reports, teams, send event.
- 동작: 기간(startAt/endAt)+unit+timezone 시계열, 타입별 상위 N 지표, 세션·이벤트 목록(page/pageSize), 이벤트 전송(익명).
- 패턴: 시간 버킷 집계+타임존, 상위 N group by, 필터, 팀 스코프, 익명 ingest. 세부 파라미터는 확인 못함(요약만).
- AIP: 집계·버킷 불가, 익명 ingest OPEN.

## 22. Strapi (CMS) — https://docs.strapi.io/cms/api/rest/filters
- 리소스: Content-type Builder가 만든 컬렉션, locale, draft/publish.
- 동작: find 필터($eq,$contains,$between,$gt/$lt,$and/$or/$not), sort, page/pageSize 또는 start/limit, populate(관계·미디어), locale, status(draft/published).
- 패턴: 정의에서 자동 생성되는 CRUD, 복합 불리언 필터, 두 방식 offset 페이지네이션, 명시적 populate(허용 관계만 확장), 다국어, 초안/발행 상태.
- AIP: contains 불가, and/or/not 필터 불가(추정, 기준선에 필터 조합 언급 없음), populate는 N:1 traverse만 직접, 다국어 불가, 초안/발행은 상태머신 직접.

## 23. OpenStreetMap API v0.6 (지도/위치) — https://wiki.openstreetmap.org/wiki/API_v0.6
- 리소스: changesets, nodes/ways/relations, map(bbox), GPS traces, notes, changeset discussion.
- 동작: changeset 열기/닫기, diff upload(다건을 단일 트랜잭션), 요소 CRUD(version 필수), bbox 조회(노드 50,000 상한), 이력, GPX 업로드, 노트 생성·코멘트.
- 패턴: 낙관적 잠금(version 불일치 409), 묶음 트랜잭션(changeset 자원 상한: 24h/10,000건), 공간 범위 쿼리(bbox), 하드 삭제 대신 visible=false+이력 보존, rate limit(429/509), 개인정보 처리(트레이스 랜덤화).
- AIP: 낙관적 잠금 부분, 공간 쿼리 불가(추정), 상한 조회는 직접(행 상한), 다건 단일 트랜잭션은 직접(효과 기반) 가능하나 호출자 값 OPEN, rate limit 불가.

## 24. Nakama (게임 리더보드) — https://heroiclabs.com/docs/nakama/concepts/leaderboards/
- 리소스: leaderboards, records, tournaments, owners.
- 동작: 점수 제출(operator best/set/incr/decr), 정렬(ASC/DESC), 랭킹 조회(cursor), 특정 사용자 목록(친구), around owner(내 주변 순위), 만료·CRON 리셋, 지난 주기 조회, authoritative 모드(서버만 제출).
- 패턴: incr/decr/best 산술 갱신(upsert), 순위 계산(rank), 주변 윈도 쿼리, 주기 리셋(cron), 서버 전용 쓰기 모드(AIP 원칙과 같은 방향: 호출자 신뢰 최소), 메타데이터.
- AIP: incr/best 불가, 순위 계산 불가(집계 정렬 불가 P53), around owner 불가, 리셋 스케줄 불가, authoritative=서버 정의 전이로 직접 근접.

---
# 패턴 빈도 표 (조사 24개 중, 개수는 본 문서 기술 기준이며 일부는 문서 요약 부재로 추정 포함)

| 패턴 | 프로젝트 수 | 현재 AIP 상태 | 대표 사례(URL) |
|---|---|---|---|
| 호출자 값 create / patch / delete (CRUD의 C·U·D) | 22 | 불가(OPEN, 제품 /apply는 전이만) | Payload https://payloadcms.com/docs/rest-api/overview |
| 역할/소유자/테넌트 권한 | 20 | 직접 | Plane https://developers.plane.so/api-reference/introduction |
| offset/page 페이지네이션 (+ total) | 12 | 불가(총건수는 별도 count 우회) | Gitea https://docs.gitea.com/api/1.24/ |
| 검색(전문/부분일치/검색 문법) | 11 | 불가(prefix 우회) | Outline https://www.getoutline.com/developers |
| 파일 업로드/첨부 | 10 | 우회(Url 무검증) | Immich https://api.immich.app/introduction |
| 태그/라벨 N:M | 9 | 우회(조인 resource), N:M 필터 불가 | Linkding https://linkding.link/api/ |
| 웹훅·외부 연동 | 9 | 불가 | Ghost https://docs.ghost.org/admin-api |
| 상태 머신/워크플로 | 8 | 직접 | Cal.com https://cal.com/docs/api-reference/v2/introduction |
| 집계(sum/avg/group by/시간 버킷/지표 쿼리) | 7 | count만 직접, 나머지 불가 | Plausible https://plausible.io/docs/stats-api |
| rate limit | 7 | 불가 | Cal.com (120/min) 위 URL, Plane (60/min) |
| 복합 필터 DSL(and/or/not, contains, between) | 6 | 부분(단일 조건 AND만 추정) | Strapi https://docs.strapi.io/cms/api/rest/filters |
| cursor/keyset 페이지네이션 | 6 | 우회(동률 누락) | Mastodon https://docs.joinmastodon.org/methods/timelines/ |
| 관계 확장(populate/expand/include) | 6 | N:1만 직접, 1:N 불가 | Medusa https://docs.medusajs.com/api/store |
| 알림 fan-out | 6 | 직접(outbox, 소비자 제품 쪽 미확인) | Discourse https://docs.discourse.org/ |
| 일괄(ids/where) 변경 | 5 | 우회(ids 필터 불가, where만) | Miniflux https://miniflux.app/docs/api.html |
| 반응/투표 | 5 | 우회(전이 3개) | Gitea 위 URL |
| 소프트 삭제/휴지통·복원 | 5 | 우회(상태 필드) | Outline, Immich, Twenty |
| 버전 이력/복원 | 5 | 우회(전이마다 기술) | Payload, Outline |
| 예약/반복/주기 실행(스케줄러) | 5 | 불가(노출 시각만) | Nakama https://heroiclabs.com/docs/nakama/concepts/leaderboards/ |
| 카운터·순번·재고·점수 증감(산술 갱신) | 5 | 불가 | Nakama(incr/decr), Gitea 이슈 번호 |
| 공개 범위·공유 링크(토큰) | 5 | 가시성 직접, 공유 링크 불가(추정) | Outline shares.create |
| 익명/공개 ingest(비인증 호출자 값 쓰기) | 4 | 불가(익명 쓰기 정책 운영 결정 미정) | Plausible, Umami, Formbricks |
| 다국어(locale) | 4 | 불가 | Strapi, Payload, Medusa |
| 트리/계층(폴더·댓글·카테고리) | 4 | 부분(self-ref 정의는 직접, 생성 경로는 OPEN) | Outline documents.move |
| 낙관적 동시성(version/updated_at) | 3 | 부분 | OSM https://wiki.openstreetmap.org/wiki/API_v0.6 |
| 결제 | 3 | 불가(범위 밖 추정) | Medusa, Saleor, Ghost |
| 순위(rank)·around-me 조회 | 1 (+Canvas 성적 추정) | 불가 | Nakama |
| 공간(bbox) 쿼리 | 1 | 불가(추정) | OSM |
| 실시간 push(롱폴링/스트리밍) | 2~4 | 범위 밖 추정 | Zulip event queue |
| position/rank 재배치 | 근거 약함 (Outline move·Canvas modules·Plane 추정) | 불가(추정) | 확인 못함: 요약에 순서 재배치 명시 없음 |

비고: 근거 약함 항목(Chatwoot, Saleor, Twenty 세부, position 재배치)은 문서 접근 제한 때문이며 빈도에서 보수적으로 셌다. 모든 "AIP 상태"는 기준선 표 인용이고 실행 재검증은 하지 않았다.

# 상위 15개 빈칸 표준화 선택지 (제안일 뿐, 결정 아님. W0/W1/W2·최종 WRITE API는 창시자 OPEN)

공통 전제: 호출자는 연산·필드·값만 표현하고, 서버 정의가 허용 목록·상한·정책을 정해 최종 결정한다.

1. 호출자 값 create/patch/delete — (A) 정의의 `expose create/update/delete`가 필드 허용 목록·기본 소유자 바인딩·check를 선언, 호출자는 허용 필드 값만 보냄. (B) 기존 전이를 확장해 전이 파라미터(타입 검증)로 값을 받음(전이만 쓰기 면). (C) 쓰기를 named command로만 노출(자유 patch 없음). 쓰기 조합 결정과 직결.
2. offset 페이지네이션+total — (A) `page offset`을 정의에서 opt-in, 서버 최대 offset/limit 상한. (B) cursor만 지원하고 offset은 "최대 N까지" 제한적 제공. (C) total은 `with total` 선언 시 별도 count(비용 상한 명시).
3. 검색 — (A) 필드별 `search`(prefix/contains/FTS 중 정의가 선택) 허용 선언, 호출자는 q만 보냄. (B) 정의가 검색 인덱스 종류(trigram/tsvector)를 지정해야만 허용. (C) 확장(extension)으로 외부 검색 엔진 위임.
4. 파일 첨부 — (A) `File` 타입: 서버가 업로드 슬롯(서명 URL)을 발급, 크기·MIME 허용 목록은 정의. (B) 외부 스토리지 참조 Url에 호스트 허용 목록 검증만 추가.
5. 태그/라벨 N:M — (A) `relation many via join`으로 선언하고 조인 resource 자동 생성, `filter tags.has` 제공. (B) 현재처럼 조인 resource 수동 선언에 `has/in` 필터 표준 연산자만 추가.
6. 웹훅/외부 연동 — (A) `emit event` 선언(대상 URL은 서버 설정 허용 목록, 서명 포함), 호출자는 구독 불가. (B) 기존 outbox를 공식 소비자(웹훅 전달기)와 함께 제품화. 호출자가 임의 URL 등록하는 방식은 SSRF라 비권장.
7. 집계(sum/avg/min/max/group by/시간 버킷) — (A) `aggregate` 선언에 허용 metric·dimension·time unit 조합 열거(Plausible식 쿼리 DSL을 허용 목록 안에서만). (B) 정의된 named stat(서버 정의 쿼리)에 입력 파라미터만 받음. 
8. rate limit — (A) 정의 수준 `limit per actor/window`(read/apply별) 선언을 런타임이 집행. (B) 프록시 몫으로 두고 정의엔 힌트 메타데이터만(추정: 운영 문서와 일관).
9. 복합 필터(and/or/not, contains, between) — (A) filter 허용 목록에 연산자별 선언 + 요청은 깊이·항 수 상한 있는 불리언 트리. (B) 평면 AND만 유지하고 or는 `in`으로 대체. 정책 필드 거부 규칙(POLICY_FIELD_NOT_FILTERABLE)은 그대로 유지해야 함.
10. cursor 동률 처리 — (A) (정렬키, id) 복합 keyset을 서버가 자동 부여. (B) id gt/lt 필터 허용. 첫 안이 호출자 표현 범위를 덜 넓힘.
11. 관계 확장(1:N, populate) — (A) `traverse many` 선언에 자식 상한(limit)과 정렬 고정. (B) 부모-자식을 별도 read 두 번으로 두고 FK eq 필터(이미 존재)를 표준 패턴으로 문서화.
12. 일괄 변경 — (A) `target.ids`(상한 있음)+where를 정의가 허용한 전이에만. (B) 요청 단위 batch(최대 N건 단일 트랜잭션, 멱등키 하나). 현재 DoS 점검 결과(상한 검사 선행)를 유지할 것.
13. 카운터·재고·점수 증감 — (A) 전이 안에 산술 표현 허용(`stock - 1`, `score + n`)과 `check stock >= 0`, 서버 정의 안에서만 (호출자 금액은 파라미터 범위 선언). (B) 전용 `counter` 필드 타입(incr/decr/best만, 원자 갱신). 
14. 소프트 삭제/휴지통·복원 — (A) `softDelete` 선언이 deleted_at 필드·기본 필터·restore 전이를 자동 생성. (B) 현재처럼 상태 필드+정책 `where`로 표준 템플릿만 제공.
15. 예약/반복/만료 실행 — (A) `schedule` 선언(cron 또는 필드 시각 도달)로 서버 정의 전이를 서버가 실행, 호출자는 트리거 불가. (B) 외부 cron이 서버 전이를 호출하는 운영 패턴을 문서화(엔진 변경 없음). 버전 이력·공유 링크·반응·익명 ingest는 다음 순위 후보로 남김.


## 변경 이력

- 2026-10-07: 초안. 오픈소스 24개 조사.

## 실제 소스에서 추출한 규칙의 실행 대조

후속 작업에서는 API 문서 요약과 별개로 아래 공개 GitHub 소스·테스트의 commit을 고정해 읽었다. 외부 프로젝트 코드는 실행하지 않았다. 필요한 업무 규칙을 AIP 독립 도메인으로 옮겨 의미 검사와 실제 PostgreSQL 전이를 실행했다. 전체 프로젝트와의 동등성을 주장하는 비교는 아니다.

| 원본 규칙과 근거 | AIP 실행 대조 | 남은 경계 |
|---|---|---|
| [pretix 정원 경쟁 테스트](https://github.com/pretix/pretix/blob/fe43862fa473c43b139bc00710aa1fc4a06ef726/src/tests/concurrency_tests/test_order_creation_locking.py#L91-L126) | Ticket의 정원 2 초과 시 전이 전체 취소, 같은 그룹 동시 쓰기의 하나만 커밋 | 원본의 cart·expired reservation·quota 종류 전체를 이식한 것은 아님 |
| [django-appointment 시간 검증](https://github.com/adamspd/django-appointment/blob/3e176df7141d10ea9447730b5e4d7b7d5dfc4899/appointment/models.py#L468-L492) | 시작 < 종료 check, 잘못된 구간에서 취소·outbox까지 rollback | [구간 겹침 검사](https://github.com/adamspd/django-appointment/blob/3e176df7141d10ea9447730b5e4d7b7d5dfc4899/appointment/tests/test_services.py#L750-L780)는 실행 불변식으로 지원하지 않으며 UNSUPPORTED_INVARIANT 대조를 남김 |
| [DocuSeal 서명자 role 중복 거부](https://github.com/docusealco/docuseal/blob/c6a7555f545ea8ea96207f198c2d8e42d16b091f/spec/requests/submissions_spec.rb#L172-L197) | 같은 submission·role의 복합 유일 제약 위반, 다른 submission의 같은 role 허용 | template별 서명자 수 상한 전체를 재현한 것은 아님 |
| [InvenTree 오래된 주문 취소 재시도](https://github.com/inventree/InvenTree/blob/575fbdc9072dee89bc5624cb7b2604829826f552/src/backend/InvenTree/order/test_sales_order.py#L311-L341) | 주문 상태·할당 상태·outbox를 원자 변경, 재시도 거부로 outbox 중복 방지. [bounded 다중 효과](2026-10-07-bounded-update-effects.md)로 여러 활성 할당 해제와 동시성·상한·rollback을 추가 검증 | 원본은 allocation 삭제, AIP probe는 RELEASED 상태로 보존한다. 기존 exact-one update는 다중 할당을 계속 거부하며 새 many 선언에서만 허용 |
| [django-helpdesk 공개·비공개 FollowUp](https://github.com/django-helpdesk/django-helpdesk/blob/5c2808dec2ba3b63d107b715d7d5b326feda666f/src/helpdesk/models.py#L948-L999), [comment 저장 회귀](https://github.com/django-helpdesk/django-helpdesk/blob/5c2808dec2ba3b63d107b715d7d5b326feda666f/tests/test_ticket_actions.py#L423-L441) | 제출자·직원·제삼자·익명별 root 및 1:N 댓글 조회, 서버 정형 답변의 따옴표·역슬래시·개행 보존, 비공개 알림 생략, 재시도 시 댓글·outbox 중복 방지 | 서버가 선언한 정형 답변으로 대조했다. 호출자 임의 댓글 입력·메일 발송·첨부파일·전체 helpdesk 동등성을 검증한 것은 아님 |

테스트 정본은 `spike-v1-fixture/tests/oss_business_patterns.rs`, `spike-v3-write/tests/oss_business_domains.rs`, `oss_cancel_idempotence.rs`, `oss_helpdesk_visibility.rs` 및 정원 경쟁 전용 `cardinality_capacity.rs`다. 재고 예약은 별도 StockItem 정의에서 `allocated <= onHand`와 원자 증감·실패 rollback을 확인한다. 이 실행 대조로 앞의 24개 조사 빈도나 전체 지원 비율을 다시 계산하지 않는다.
