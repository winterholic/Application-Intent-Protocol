# 09. 검증 로그

> 검증-보충 루프(`07` 4절)의 기록. 회차마다 **무엇을 검사했고, 무엇이 나왔고, 어느 쪽을 고쳤는지**를 남긴다. 판정 근거가 된 명령과 출력은 그대로 적는다.

## Pass 1 (2026-09-30)

### 범위
- `docs/design/00-08`, `spec/grammar.md`의 모든 ` ```aip ` 예제 블록 (36개)
- 방법: (1) `tools/doccheck.py` 기계 검사, (2) 예제 ↔ 문법 수동 교차 검토, (3) 커버리지 카탈로그 판정 기계 집계

### 1. 기계 검사

첫 실행:
```
$ python3 tools/doccheck.py
...
36 aip blocks checked, 16 finding(s)
```

| 규칙 | 건수 | 내용 | 고친 쪽 |
|---|---|---|---|
| DURATION_WORD | 8 | `6 months`, `7 days`, `1 day` 등 | 예제 (`6mo`, `7d`, `1d`) |
| LATEST_NO_BY | 3 | `latest X where ...`에 정렬 기준 없음 | 예제 (`by f.createdAt`). 1건은 `exists`로 대체 |
| LAMBDA | 2 | `all(xs, x => p)` | 예제 (`all(xs x: p)`) |
| TZ_BARE | 1 | `at 00:00 Asia/Seoul` | 예제 (`tz "Asia/Seoul"`) |
| COUNT_STAR | 1 | `count(*)` | 예제 (`count()`) |
| PARAM_REBIND | 1 | Case 8에서 엔티티 파라미터 `r`을 `from Recruitment r`로 다시 바인딩 | 예제 (`from r`) + 문법(`from ident` 허용) |

규칙 추가 후 재실행(TIMERANGE, CURSOR_PARAM, SAME_NO_BINDER, ALLOW_MISSING):
```
$ python3 tools/doccheck.py
docs/design/08-coverage-catalog.md:557: [ALLOW_MISSING] command ExportMyData has no allow clause
36 aip blocks checked, 1 finding(s)
```
원인은 한 줄로 쓴 intent를 검사기가 여러 줄 블록으로 잘못 잡은 것. 예제를 포매터 형태(여러 줄)로 바꿨다. 한 줄 intent 처리 개선은 파서 도입(M1) 시 해소.

최종:
```
$ python3 tools/doccheck.py
36 aip blocks checked, 0 finding(s)
```

### 2. 수동 교차 검토에서 나온 불일치

| # | 발견 | 판정 | 조치 |
|---|---|---|---|
| V01 | `allow authenticated`, `allow public`이 문법 식에 없음 | 명세 누락 | 문법 primary에 `public`, `authenticated` 추가 (allow 전용 술어) |
| V02 | `same(applies.recruitment.club)`: 다값 경로를 단일 값처럼 사용하고, 결과를 `allow`에서 다시 경로로 반복 | 설계 결함 | **SC-1** `let` 절 도입 (`let club = same(applies a: a.recruitment.club) else CODE`). 불변 바인딩이며 가변 변수가 아님 |
| V03 | `TimeRange` 타입(02, 01)과 문법의 `Range<Time>` 불일치 | 예제 오류 | 예제 수정, 검사 규칙 추가 |
| V04 | `page: Cursor?`를 파라미터로 선언(01 Case 9, 11, 15) | 설계 결정 필요 | **SC-2** cursor는 `page` 절이 있으면 transport 수준에서 자동. 파라미터 금지 |
| V05 | `s3.put(file, max 10MB, types [...])`: 인자 문법 위반, 제약이 효과 호출에 붙음 | 예제 오류 | 제약을 `Upload(max 10MB, types [...])` 파라미터 타입으로 이동 |
| V06 | `on reservation.expired(order: Order)`: on-event 문법과 다른 모양 | 예제 오류 | inventory 형식이 `StockReservationExpired` 이벤트를 내보내는 것으로 정의, `on StockReservationExpired e`로 수정 |
| V07 | `webhook payments.captured(ref)`: webhook 문법과 다른 모양 | 예제 오류 | `webhook PaymentCaptured via payments.webhook { on CAPTURED(e) do {...} }` |
| V08 | `via payments.toss.webhook`처럼 괄호 없는 via 대상이 문법(call)과 불일치 | 명세 경직 | **SC-3** `call-target`(인자 없으면 괄호 생략) 도입, actor/webhook/notify/deliver에 적용 |
| V09 | 파라미터 이름 `at`이 `at ... run` 키워드와 충돌 (`at at run`) | 예제 오류 | `publishAt`으로. 문맥 키워드 목록 정리는 M1 |
| V10 | 파라미터 `status`와 대상 필드 `status`가 `set status = status`에서 섞임 | 명세 모호 | **SC-4** `update X set f = e`의 좌변은 대상 필드, 우변 맨 이름은 파라미터/let/별칭. 예제는 `newStatus`로 개명 |
| V11 | `dynamic schema from questions`를 엔티티 멤버로 썼는데 문법은 필드 수식어 | 명세 오류 | 문법에서 entity-trait로 이동 |
| V12 | 쿼리 `Weather`에 `from`이 없음(외부 fetch만) | 명세 경직 | **SC-5** `fetch`가 있으면 `from` 생략 가능 |
| V13 | `no: sequence per board ...`에 필드 타입 없음 | 예제 오류 | `no: Text sequence ...` |
| V14 | `stock: Int counter sharded 8`에 `via` 없음 | 예제 오류 | `counter via postgres sharded 8` |
| V15 | `upcast ... with channel = WEB` 대입 표기 | 예제 오류 | `with channel: WEB` (필드 대입 표기 통일) |
| V16 | `sign hmac-sha256`: 하이픈은 식별자에 불가 | 예제 오류 | `hmac_sha256` |
| V17 | `target: Email where domain in School.emailDomain`: 엔티티 타입을 값처럼 경로 접근 | 예제 오류 | `where exists School s where s.emailDomain = domain(target)` |
| V18 | `running_sum(...) over club order by at`: `at`이 식이 아님 | 예제 오류 | `order by f.at` |
| V19 | `sum(order.items.price * order.items.quantity)`: 다값 경로 산술 | 예제 오류 | `sum(order.items i: i.price * i.quantity)` |
| V20 | `do each rows r partial {`: `do` 뒤에 블록 없음 | 예제 오류 | `do { each ... partial { ... } }` |
| V21 | 02의 "Intent 문법 요약"이 명세와 이중 관리됨 | 문서 구조 | 요약을 정본 참조 + 요지로 교체 |
| V22 | Case 1의 `is open = ...` 설명이 문법의 `predicate`와 다름 | 예제 오류 | `predicate open = ...` |
| V23 | IR(03)에 `let`, `upsert`, `purge`, `each partial`, `at ... run`, `capacity`, 정책 거절 코드가 없음 | IR 누락 | IR에 추가. L3 형식은 IR 노드 없이 하강된다는 원칙(10.1절) 명시 |

### 3. 커버리지 카탈로그 집계

```
$ awk -F'|' '/^\| [A-S][0-9][0-9] /{v=$(NF-1); n++; if(v~/✅/)a++; if(v~/🆕/)b++; if(v~/🔌/)c++; if(v~/⚠/)d++; if(v~/⛔/)e++} END{print "rows",n,"ok",a,"new",b,"ext",c,"open",d,"out",e}' docs/design/08-coverage-catalog.md
rows 190 ok 95 new 76 ext 12 open 9 out 3
```

카탈로그의 합계 표는 이 출력으로 교체했다(처음 초안의 수작업 숫자는 틀려 있었다).

### 4. 이번 회차에 **검증하지 못한** 것

- 문법의 모호성(LL/LR 충돌), 키워드와 식별자 충돌 전수: 파서 구현(M1) 전에는 기계 검증 불가. doccheck는 정규식 수준이다.
- 하강 규칙의 정확성: 문서상 서술만 있고 실행 가능한 하강기가 없다.
- 효과 배치, 잠금 순서, 캐시 무효화 도출의 정확성: M2 런타임 테스트 대상.
- 타 제품 비교표(00 6절)의 셀별 사실 확인: 공개 문서 대조 필요.
- 한국 PG(토스페이먼츠)의 authorize/capture 분리 지원 여부: 확인 필요.

### 5. Open Issues (다음 회차 입력)

| ID | 이슈 | 관련 케이스 |
|---|---|---|
| OI-01 | `each ... partial`의 결과 모양(항목별 성공/오류 목록)의 계약 표현 | F04, Case 2 |
| OI-02 | `snapshot(...)`의 의미: 깊은 복사 vs 버전 참조 | Case 7, D08 |
| OI-03 | 캐시 분할 기준(`actor-class`)의 정의 방법 | Case 8 |
| OI-04 | 불변식 복구 규칙(`repair on erase`) 문법 | Case 5 |
| OI-05 | deferred 효과 영구 실패 시 도메인 상태 선언 문법 (W-EFFECT-NO-FAILURE-PATH의 해결 형태) | Case 15 |
| OI-06 | inventory 형식의 일반성(옵션, 창고, 묶음 상품) | Case 13, M02 |
| OI-07 | `dynamic schema` 질문 정의 변경 시 기존 답변 해석 (스냅샷 필수 여부) | D08 |
| OI-08 | 한국어 형태소 검색 엔진 선택(Postgres 확장 vs 외부 검색 엔진) | E06 |
| OI-09 | 정기 결제(`billing`) 형식 범위 | H07 |
| OI-10 | 대규모 fan-out 피드 전략 | E14 |
| OI-11 | 영업일/공휴일 데이터 원천 | J05 |
| OI-12 | 계약 버전 공존(구버전 클라이언트 지원 기간) | P06 |
| OI-13 | 낙관적 업데이트/오프라인 큐의 클라이언트 계약 | R04, R05 |
| OI-14 | `relation` 재귀 허용과 종료 증명 | A10 |
| OI-15 | 필드 가시성에서 `self`의 정확한 정의(Member 행 자신 vs 소유자) | C05 |

### 명세 변경 요약 (Pass 1)

- SC-1 `let` 절 (불변 바인딩, 실패 코드)
- SC-2 cursor 파라미터 금지, `page` 절이 transport 입력 생성
- SC-3 `call-target` (via 뒤 괄호 생략)
- SC-4 `update ... set`의 좌/우변 이름 해석 규칙
- SC-5 `fetch`가 있는 query의 `from` 생략
- 기타: `dynamic schema`를 entity-trait로, `public`/`authenticated` 술어, 예약어 목록 갱신

---

## Pass 2 (2026-09-30)

### 범위
아리아리 백엔드 중 Pass 1에서 보지 않은 서비스: `ClubActivityService`(활동, 댓글, 좋아요, 차단), `AttendanceService`, `InviteService`, `ClubNoticeService`, `ApplyTempService`, `ClubService`. 목적은 카탈로그에 없는 케이스와 AIP가 막아야 할 결함 유형을 찾는 것.

### 새로 확인한 결함 (코드 대조)

| # | 위치 | 내용 | AIP에서의 처리 |
|---|---|---|---|
| D01 | `ClubActivityService.modifyClubActivityComment` / `deleteClubActivityComment` | `[결함]` 동아리 회원이면 **남의 댓글도 수정/삭제 가능**. 작성자 확인이 없고, 댓글이 경로의 활동 소속인지도 확인하지 않는다 | 엔티티 파라미터의 권한은 그 엔티티에서 계산(`allow comment.author = actor or managerOf(...)`). 경로 id와 엔티티 불일치라는 개념 자체가 없음 (C21) |
| D02 | `saveClubActivityComment` | `[결함]` 부모 댓글이 같은 활동 소속인지 확인하지 않는다. 대댓글 깊이 제한 없음 | 참조 일관성 불변식 → 복합 FK (G10), `tree max depth 2` |
| D03 | `InviteService.createInviteAlarm` | `[결함]` `isHigherRoleTypeThan(clubMember, clubMember)`: 자기 자신과 비교한다. `ClubMember.isHigherRoleTypeThan`은 ADMIN이면 항상 true, 그 외에는 자기 자신보다 높을 수 없으므로 **MANAGER는 초대 알림을 항상 보낼 수 없다** | 관계 술어 `outranks(actor, target)`는 두 인자가 다른 행임을 타입으로 강제할 수 없지만, 분석기가 같은 식을 양변에 쓴 비교를 W-SELF-COMPARE로 경고 (분석 규칙 추가) |
| D04 | `InviteService.acceptInviteAlarm` | `[확인 필요]` 초대 코드가 `AES(clubId)`라서 특정 초대 대상에 묶이지 않는다. 같은 동아리의 코드를 받은 누구나 재사용 가능 | `grant link ... to invitee uses 1` (C19) |
| D05 | `ClubService.modifyClub`, `ApplyTempService.modifyApplyTemp` | `[결함]` 새 파일 저장 전에 **이전 파일을 먼저 삭제**한다(외부 비가역 효과가 커밋 전). 트랜잭션이 실패하면 DB는 삭제된 파일을 가리킨다 | `s3.Object` 필드 소유권: 이전 객체 삭제는 커밋 후 deferred (H13) |
| D06 | `ClubActivityService.deleteClubActivity` | `[결함]` 이미지 행은 지우지만 S3 객체는 지우지 않는다(고아 파일) | 같은 필드 소유권 규칙. 행 삭제 시 객체 deferred 삭제 |
| D07 | `ClubNoticeService.toggleClubNoticeFix` | `[결함]` "고정 공지 최대 3개"가 조회 후 변경이라 동시 요청 시 4개 이상 가능 | `capacity count(ClubNotice n where n.club = club and n.fixed) <= 3` (A26) |
| D08 | `ClubNoticeService` | `[결함]` 공지 상세와 일반 목록은 시스템 관리자를 허용하지만 고정 공지 목록은 허용하지 않는다(정책 불일치, Pass 1의 회계와 같은 유형) | `superuser` 선언 1회로 전역 일관 적용 (C20) |
| D09 | `ClubActivityService.saveClubActivityCommentBlock` | `[결함]` 자기 자신을 차단할 수 있다. 중복 차단 검사는 조회 후 저장 | `invariant not_self`, `unique (blocker, blocked)` (G11) |
| D10 | `ClubActivityService.modifyClubActivity` | `[결함]` 이미지 최대 10장 계산에 쓰는 삭제 id 수를, 그 id들이 이 활동 소속인지 확인하기 **전에** 사용한다 | `capacity`는 변경 후 상태로 검사되므로 계산 순서 문제가 없음 |
| D11 | `ApplyTempService.modifyApplyTemp` | 운영 코드에 `System.out.println` | 사용자 코드가 없으므로 해당 없음. 구조화 로그는 런타임 소유 |
| D12 | `AttendanceService.findAttendees` | 동아리 전체 멤버를 로딩(`findAllByClub`) 후 페이지 응답 구성 | 선택 모양 기반 계획, 무제한 로딩 표현 불가 |

Pass 1과 합쳐 아리아리에서 확인한 결함 유형:

| 유형 | 건수 | AIP의 대응 범주 |
|---|---|---|
| 권한 누락/불일치 (IDOR, 규칙 이중 구현, 정책 불일치, 자기 비교) | 8 | 표현 불가 또는 정적 경고 |
| check-then-act 경쟁 (중복 지원, 기간 겹침, 고정 공지 수, 중복 차단) | 4 | DB 제약/capacity로 경쟁 불가 |
| 커밋 전 외부 비가역 효과 (카카오 해제, 파일 선삭제) | 3 | 효과 등급으로 배치 강제 |
| 고아 외부 자원 (S3) | 2 | 필드 소유권 + deferred 정리 |
| 불변식 위반 (관리자 0명, 자기 차단, 부모 불일치) | 4 | 제약 하강 + 지연 검사 |
| 구현 실수 (조회수 키, 빈 목록 get(0), 누락 id 무시) | 3 | 형식화/입력 검증으로 제거 |
| N+1, 전체 로딩 | 4 | 구조적으로 불가 |

(건수는 이 로그의 항목을 손으로 분류한 것. 한 결함이 두 유형에 걸치면 주 유형 하나로만 셌다.)

### 추가된 명세/형식
- `Snapshot<T>` 타입 (OI-02, OI-07 해소)
- 효과 호출 `on failure do { ... }` (OI-05 해소)
- `actor ... superuser when <pred> audited`
- `grant link`: `grants` 선택화, `to <expr>`, `require`, `on redeem do`, `holder` 바인딩
- 참조 경로 `invariant`의 복합 FK 하강 규칙
- 분석 규칙 W-SELF-COMPARE (같은 식을 비교 양변에 사용)

### Open Issue 처리

| ID | 결과 |
|---|---|
| OI-01 | **닫음.** `each ... partial` 결과는 계약에서 `{ results: [{ index, ok: true, value } \| { index, ok: false, error }] }`. 명령 전체는 한 트랜잭션이고 항목별 savepoint. `emit`은 성공 항목만 |
| OI-02 | **닫음.** `snapshot`은 깊은 복사가 아니라 `Snapshot<T>` 버전 참조. `history`가 선언된 엔티티만 가능 |
| OI-05 | **닫음.** 효과 호출의 `on failure do`. 재시도 한도 초과 후 dispatcher가 블록을 로컬 트랜잭션으로 실행. 없으면 W-EFFECT-NO-FAILURE-PATH 유지 |
| OI-07 | **닫음.** `Json validated by <path>`의 path는 `Snapshot<T>` 타입이어야 한다(E-DYNSCHEMA-UNPINNED) |
| OI-15 | **닫음.** 필드 가시성의 `self`는 "이 행이 actor 자신"(actor 엔티티에서만 사용 가능). 다른 엔티티에서는 소유자 경로를 명시해야 하며 `self`를 쓰면 E-SELF-CONTEXT |
| 나머지 | OI-03, 04, 06, 08, 09, 10, 11, 12, 13, 14 유지 |

### 검증 명령

```
$ python3 tools/doccheck.py
37 aip blocks checked, 0 finding(s)
$ awk -F'|' '/^\| [A-S][0-9][0-9] /{...}' docs/design/08-coverage-catalog.md
rows 203 ok 101 new 83 ext 12 open 9 out 3
```

### 검증하지 못한 것 (Pass 1과 동일 + 추가)
- D04의 실제 악용 가능성(초대 코드가 어디에 노출되는지): 알림 API 응답 경로 확인 필요
- 복합 FK 하강이 soft delete, tree와 함께 쓰일 때의 제약 충돌: 파서/하강기 구현 후 검증

---

## Pass 3 (2026-10-01): Rust 구현으로 검증

ADR-001 결정: Rust. `rustup`으로 Rust 1.98.1 설치 후 워크스페이스 구성.

### 구현된 것

| crate | 역할 | 검증 |
|---|---|---|
| `aip-syntax` | lexer, AST, 재귀 하강 파서 (명세 전체 문법) | 단위 테스트 13, `aip check-docs`로 문서 예제 29개 파싱 |
| `aip-sema` | 심볼 테이블, 타입 검사, 정적 규칙 | conformance 24건(기대 진단 코드 일치), 아리아리 정의 오류 0 |
| `aip-ir` | 실행 계획(IR), 계약(describe) | - |
| `aip-pg` | DDL 생성, 식→SQL 컴파일러, intent→계획 | 아리아리 DDL 158문장이 PostgreSQL 17에 적용됨 |
| `aip-runtime` | 실행기, 멱등, 재시도, 감사, 페이지네이션, outbox 디스패처, 스케줄러, HTTP | 단위 테스트 4 |
| `aip-cli` | `aip parse/check/check-docs/ddl/ir/explain/contract/migrate/run/token` | 아리아리 end-to-end 테스트 1 (단언 60여 개) |

### 기계 검증 명령과 결과

```
$ cargo test -q --workspace
test result: ok. 13 passed   (aip-syntax)
test result: ok. 2 passed    (aip-sema conformance + ariari clean)
test result: ok. 4 passed    (aip-runtime units)
test result: ok. 1 passed    (aip-pg)
test result: ok. 1 passed    (ariari end-to-end on PostgreSQL)
$ aip check-docs docs spec
36 blocks: 29 parsed, 0 failed, 7 skipped (fragments with '...')
```

### 파서가 문서에서 찾은 불일치 (정규식 검사기가 놓친 것 8건)

`;` 구분자, `do {}` 단독 조각, 옛 `repair on erase:` 문법, 옛 `emit` 조각, extension 형식 정의(`form`)를 앱 문법으로 표기, `use auth-oidc`의 하이픈, 엔티티 멤버로 쓴 `slug/position`, 최상위 `toggle`. 전부 문서 쪽을 고쳤다. `form` 정의는 별도 언어(extension manifest)로 분리 표기(`aip-form` 펜스).

### 검사기와 백엔드가 아리아리 정의에서 찾은 **정의 쪽 실수**

컴파일러가 사람(이번에는 설계자 자신)의 실수를 실제로 잡는지가 AIP의 핵심 가설이다. 아리아리를 옮기면서 쓴 정의에서 다음이 걸렸다.

| 발견 | 잡은 곳 | 내용 |
|---|---|---|
| 출석 취소에서 `a.member in members` | 검사기 E201 | Member와 ClubMember를 비교. `members.member`가 맞다 |
| 게시/댓글/공지/임시저장 생성 명령 4개 | 검사기 W501 | 재시도 시 중복 생성. `idempotent` 추가 |
| 무제한 목록 2개 | 검사기 W403 | `page` 추가 |
| 대댓글 선택 안의 `count(comments.likes)` | 백엔드 E600 | 바깥 활동 전체 댓글의 좋아요를 세는 식. `count(likes)`가 맞다 |
| `ClubMember.member on erase anonymize` | e2e 설계 검토 | 익명화된 행이 ADMIN으로 남아 복구 규칙이 발동하지 않는다. 멤버십은 `cascade`가 맞다 |

### 설계 결정 (구현 중 확정)

- **컴파일 시점 SQL 고정**: 런타임은 SQL을 만들지 않는다. 모든 문장은 IR에 고정되고 `aip explain`이 그대로 보여준다. 파라미터는 이름 표식으로 컴파일하고 문장 완성 시 `$n`으로 번호를 매긴다(조각 재사용 때문).
- **모든 파라미터는 text로 바인딩하고 SQL에서 캐스트**: 드라이버 타입 매핑 문제를 없앤다.
- **`=`는 `=`, `!=`는 `IS DISTINCT FROM`, 술어 최상위는 `coalesce(..., false)`**: 인덱스 사용과 "NULL은 아무것도 허용하지 않는다"를 함께 만족.
- **가시성(`visible to`)은 클라이언트에게 돌아가는 데이터와 엔티티 파라미터 로딩에만 적용**, 정책/사전조건 내부의 조회에는 적용하지 않는다(정책은 시스템 관점).
- **lifecycle은 엄격**: 같은 상태로의 "전이"도 선언되어야 허용. (`ApproveRecruitment` 재호출이 `RECRUITMENT_STATUS_INVALID_TRANSITION`)
- **엔티티 파라미터 잠금**: 쓰기 대상은 `FOR UPDATE`, 읽기만 하는 파라미터는 `FOR SHARE`, 전역 테이블 순위 순서.
- **capacity 트리거**: 부모 행을 `FOR UPDATE`로 잠가 그룹 단위 직렬화. 새 행이 필터에 해당할 때만 +1.

### 아직 실행되지 않는 형식 (Pass 4 대상)

`verification`, `grant link`, `approval`, `expose`, `job`, `subscribe`, `webhook`, `consume`, `projection`, `rule`, `search`, `outbound webhooks`, `consent`, `impersonate`, `migration`, `versioned`(`expect version`), `tenant`, `slug/sequence/position` 값 생성, 동적 스키마 런타임 검증, deferred 효과의 `on failure`, 외부 브로커(kafka) 전달, 실제 S3/메일/OIDC 제공자.

### 새로 드러난 설계 이슈

- **OI-16 `grants` 설탕의 한계**: `grants: membership(holder, club) as GENERAL`은 멤버십 엔티티의 다른 필수 필드(아리아리 `ClubMember.name`)를 채울 방법이 없다. 링크 사용 시 입력을 받는 `redeem with (params)`가 필요하다.

---

## 다음 회차 계획 (Pass 3, 원래 계획 — 일부 Pass 4로 이월)

1. 카탈로그 🆕 83건 각각의 **하강 결과 스케치**를 작성하고, 하강 결과가 L2만 쓰는지 확인한다(L2 밖이 필요하면 그 형식은 설계 결함).
2. 아리아리 남은 영역(Q&A, FAQ, 후기, 합격 후기, 알림, 신고, 시스템 공지, 백오피스)을 대조한다.
3. 아리아리 외 도메인 하나(예약/부킹 또는 SaaS 멀티테넌트)로 카탈로그 편향을 점검한다.
4. doccheck에 문장 첫 토큰 검사를 추가한다.
5. 남은 OI 중 설계로 닫을 수 있는 것(OI-03, 04, 14)을 닫는다.

---

## Pass 4 (2026-10-01): 실행 범위 확장과 계약 출력

### 새로 실행되는 형식

| 형식 | 하강 결과 | 검증 |
|---|---|---|
| `expose` | 시맨틱 단계에서 `Get/List/Create/Update/Delete<E>` 의도로 전개. 새 IR 없음 | e2e: FAQ 노출, `position` 순서 |
| `grant link` | `Issue`(토큰 해시 저장) / `Redeem`(1회성, 대상 제한, `redeem with` 입력) | e2e: 재사용 거부, 대상 불일치 거부 |
| `verification` | `Request`(코드 해시 저장, 메일 효과) / `Verify`(시도 횟수는 실패해도 커밋) | e2e: 해시 저장, 실패 시도 커밋 |
| 동적 스키마 | `ValidateDynamic` 단계. 스키마 행을 고정 버전(`form_version`)으로 참조 | e2e: 버전 2 고정 후 검증 |
| 값 생성 | `position`/`sequence`/`slug` 트리거. 순번은 `_aip_sequence` upsert | e2e |
| 업로드 | 스테이징 → 커밋 후 active 승격, 교체 시 이전 객체 지연 삭제 | e2e: `.exe` 거부, 항목별 3개 |
| 계약 출력 | `/aip/describe`에 출력 형태(null 가능 여부 포함)와 오류 사유 | TS 클라이언트 생성 후 `tsc --strict` 통과 |

### 생성 TS 클라이언트를 실제 서버에 붙여서 드러난 결함

- **multipart `input` 파트가 파일로 해석됨**: 클라이언트가 `input`을 `Blob`으로 붙이면 파일명(`blob`)이 생기고, 서버는 파일명이 있는 파트를 업로드로 취급한다. 결과는 `'input' is required`. 문자열 파트로 바꿔 해결했다. 타입 검사와 Rust e2e만으로는 잡히지 않았고, **생성 클라이언트를 실제 서버에 붙이는 검증 단계가 필요하다**는 근거가 됐다.
- 확인한 흐름: 파일 포함 생성(멱등 키), keyset 목록, 비회원 가시성(`myMembership: null`), 권한 거부 `403 ONLY_ADMIN_CAN_DELETE`, 비인증 `401`.

### 알려진 약점

- **업로드 후 롤백 테스트가 약하다**: 현재 테스트는 업로드 이전 단계에서 실패하므로 "스테이징된 객체가 롤백 시 지워지는가"를 실제로 검증하지 못한다. 업로드 뒤 정책 실패를 일으키는 시나리오가 필요하다.
- `encrypted` 필드는 아직 평문 저장(W602로 경고).

### 아직 실행되지 않는 형식 (Pass 5 대상)

`approval`, `job`, `subscribe`, `webhook`, `consume`, `projection`, `rule`, `search`, `outbound webhooks`, `consent`, `impersonate`, `migration`, `versioned`(`expect version`), `tenant`, `publishable`, deferred 효과의 `on failure`, kafka 전달, 실제 S3/메일/OIDC 제공자, 필드 암호화, `--reset` 없는 스키마 마이그레이션.

---

## Pass 5 (2026-10-01): `approval`, `job`

| 형식 | 하강 결과 | 검증 (e2e, 실제 PostgreSQL) |
|---|---|---|
| `approval` | `Request/Approve/Reject/Cancel` 명령 + `Status` 조회. `_aip_approval`(대상당 PENDING 하나, 부분 UNIQUE) + `_aip_approval_vote`(투표자당 하나, PK) | 요청 권한(`requested by`), 비가시 행 NOT_FOUND, 중복 요청, 자기 승인 금지, 비승인자, 중복 투표, 1표로 미확정, 2표 확정, 반려 1표 확정, 요청자만 철회, 만료 커밋, 동시 최종 투표 2건에서 확정 1회 |
| `job` | 시작 명령(동일 작업 진행 중이면 재사용) + `Status` 조회 + 워커 계획(`Job` IR). 항목 스냅숏, 100건 배치 커밋, heartbeat 기반 재개, 항목별 재확인·잠금 | 권한, 재사용, 요청자만 조회, CSV(가시성 범위, 따옴표 이스케이프, ISO UTC), 7일 뒤 삭제 예약, 완료 알림, 스냅숏 이후 승인된 항목은 건너뜀 |

### 설계 결정

- **승인 대상 행은 투표 동안 `FOR UPDATE`로 잠근다.** "N번째 승인"이 동시성에서도 정확하고, `on approved`가 대상 행을 바꾸는 경우와도 순서가 맞는다.
- **승인자 판정은 집합의 원소가 actor이거나 actor 참조 필드가 정확히 하나인 경우만 허용**(검사기 E201). 모호한 연결을 추측하지 않는다.
- **만료는 지연 처리한다.** 별도 스케줄러 없이 투표·재요청 시점에 만료를 확정하고 커밋한다. `Status`는 만료 여부를 계산해서 보여준다. 만료는 `on rejected`를 실행하지 않는다.
- **job 항목은 "최소 1회" 실행이다.** 워커가 죽으면 커밋된 진행 지점부터 재개하므로 배치 중간 항목이 다시 실행될 수 있다. 대신 항목마다 원래 조건을 다시 확인하고 잠그므로, 스냅숏 이후 조건을 벗어난 항목(예: 그사이 합격 처리된 지원)은 실패가 아니라 건너뛴다.
- **내보내기 파일은 요청자의 가시성으로 만든다.** 필드 단위 `visible to`가 걸린 필드는 제외한다.

### 발견한 결함

- 조회(prelude)에서 엔티티 파라미터를 `FOR SHARE`로 읽으면 읽기 전용 트랜잭션에서 실패한다(`AIP.INTERNAL`). 상태 조회는 잠그지 않도록 고쳤다. e2e가 없었다면 운영에서 처음 드러났을 결함이다.

### 이어서 구현: `versioned`, `webhook`, 두 번째 도메인(Shop)

| 형식 | 하강 결과 | 검증 |
|---|---|---|
| `versioned` | 파라미터로 받은 행을 **절대값으로** 고치거나(`set x.f = v`) 지우는 명령에 `<param>Version` 입력이 자동으로 붙고, 잠금 뒤 버전을 비교한다. 불일치는 `409 AIP.CONFLICT.STALE_VERSION` | 아리아리 공지(누락 400, 낡은 버전 409, 재조회 후 성공), Shop 상품 가격 |
| `webhook` | 검증(HMAC-SHA256 또는 Stripe `t=,v1=` + 5분 창) → `_aip_webhook_seen`으로 중복 제거 → outbox `webhook` 행 → 즉시 2xx. 디스패처가 `_aip_processed`로 한 번만 적용 | 위조·재전송 거부(저장 전), 확인 응답 후 디스패치 전 미적용, 재전달 no-op, 핸들러 없는 이벤트 확인 후 폐기, 이미 결제된 주문에 늦게 온 실패 이벤트는 무변경 |

Stripe 서명 규칙은 공식 문서(docs.stripe.com/webhooks, 2026-10-01 확인)로 검증했다: 헤더 `t=…,v1=…`(v1 여러 개 가능, v0 무시), 서명 대상 `"{t}.{body}"`, HMAC-SHA256, 상수 시간 비교, 기본 허용 오차 5분.

**Shop 예제**(`examples/shop`)를 추가했다. 아리아리에 없는 모양(공유 재고, 결제 웹훅, 여러 직원의 가격 편집)으로 카탈로그 편향을 점검하는 두 번째 도메인이다.

### 설계 결정

- **상대 갱신(`+=`, `-=`)은 편집으로 치지 않는다.** 교환 가능하므로 서로의 변경을 잃지 않는다. 버전 입력도 요구하지 않는다.
- **프로그램 전체에서 상대 갱신으로만 움직이는 필드는 버전을 올리지 않는다**(`aip-pg/src/writes.rs`). 컴파일 시점 전역 분석으로 정한다. 주문이 재고를 가져가도 직원의 가격 편집이 낡은 것이 되지 않는다. 그 필드를 절대값으로 쓰는 명령이 하나라도 생기면 자동으로 버전 대상이 된다.
- **웹훅은 받는 즉시 저장하고 응답한다.** 처리 실패가 제공자 타임아웃이나 중복 재전송 폭주로 번지지 않는다. Stripe 문서의 권고(빠른 2xx, 비동기 처리, 이벤트 id 중복 제거)와 같다.

### `rule`

| 하강 결과 | 검증 (Shop e2e) |
|---|---|
| 조건이 읽는 테이블마다 트리거가 소유 행 id를 `_aip_rule_pending`에 기록. 모든 커밋 지점(명령, 디스패처, job 배치, 스케줄) 직전에 `settle_rules`가 조건을 평가하고 `_aip_rule_state`로 에지를 판정 | 재고 0에서 한 번만 알림, 재입고 뒤 재무장, 다시 0이면 재발화. 웹훅 디스패처 경로에서 관련 테이블(`count(p.orders ...)`) 변화로 발화. 미정산 pending 0 |

- **설계 문서(08)의 "커밋 직전 평가"를 그대로 따랐다.** 커밋이 끝나면 규칙은 이미 반영되어 있다. 결과적 일관성이 아니다.
- **추적할 수 없는 조건은 컴파일 오류다.** `e.ref.field`처럼 다른 행을 거쳐 읽는 조건은 트리거가 소유 행으로 되돌아갈 길이 없어서 거부한다. 조용히 발화하지 않는 규칙보다 낫다.

### 남은 형식 (Pass 5 시점 목록 — Pass 6에서 tenant, publishable, consent, impersonate, search, outbound webhooks 해제)

`subscribe`, `consume`, `projection`, `search`, `outbound webhooks`, `consent`, `impersonate`, `migration`, `tenant`, `publishable`, deferred `on failure`, kafka, 실제 S3/메일/OIDC, 필드 암호화, 무중단 스키마 마이그레이션.

## Pass 6 (2026-10-01): 설계 철학 인계 반영, Core Semantic IR 도입

입력: `docs/origin/philosophy_handoff_2026-10-01.md`. 검토 결과는 `10-philosophy-alignment.md`, 결정 상태는 `docs/DECISIONS.md`로 분리했다.

### 구조 변경
| 변경 | 이유 | 검증 |
|---|---|---|
| 옛 `aip-ir`(SQL 담은 실행 계획) → `aip-plan`으로 개명 | IR이라는 이름이 실행 계획을 가리켜 철학 §4.3의 층 구분을 흐렸다 | 개명 후 29 passed |
| 새 `aip-ir` = Core Semantic IR v0 → 0.2 | 언어 독립 의미 계약이 코드에 없었다(10 §3.1) | 아래 |
| `aip-sema/src/to_core.rs`: `.aip` → Core IR 하강 | 첫 프런트엔드 | 두 예제 validate 진단 0, Unknown 타입 0, 미해석 이름 0 |
| `aip-ir/src/validate.rs`: 프런트엔드 독립 구조 검증 `AIP-I101`~`I119` | 다른 프런트엔드가 만든 IR도 같은 검사를 받는다 | 규칙마다 negative 테스트 |
| `aip core <file> [--digest]` | 기계가 읽는 의미 계약 출력 | ariari `sha256:…`, shop `sha256:…` 출력 |

Core IR 성질 검증(`crates/aip-sema/tests/core_ir.rs`): JSON 왕복 동일, 두 번 하강한 digest 동일, **공백·주석만 다른 소스의 digest 동일**(위치 정보가 IR에 새지 않음), enum 값 하나 추가하면 digest 다름. Shape는 aip-pg의 계약 output과 두 예제 84개 intent에서 구조 일치(일회성 비교, 스크립트 미보존).

1차 하강에서 IR이 의미를 잃던 10곳을 0.2에서 메웠다: 식별자 인자 `Symbol`, `RichText{policy}`, `Update.via`, `TypeTest`(`x is Entity`), `RecordField`, `Type::Event`, `RefUnion.on_erase`, `Shape::List.nullable`, `Counter.store`(옛 설명 "inverse relation"은 틀렸다: `via redis`는 저장 extension), `InRangeValue`.

### 결함 수정: Decimal/Money (D-P4)
- 재현: 수정 전 Money 입력 `"NaN"`이 검증을 통과(단위 테스트로 확인). PostgreSQL `numeric`은 NaN·Infinity를 저장한다.
- 수정: 입력은 10진 문자열 `^-?(0|[1-9][0-9]*)(\.[0-9]+)?$` 또는 |n| ≤ 2^53-1 정수 number만. 응답은 `to_jsonb` 4곳을 문자열 직렬화로 바꿈. TS 타입 `string`. 계약에 `wire`/`format` 표기.
- e2e: 아리아리 회계 금액을 문자열로, `"12345678901234567.89"` 저장·조회 왕복 정확, 거부 5건. 출력 수정을 되돌리면 이 테스트가 실패함을 확인(negative control).
- 남음: `Decimal(p,s)` 자릿수 초과 거부(실행 계획 `TypeSpec::Decimal`에 정밀도 없음).

### S3: 백엔드가 Core IR만 읽는다
- `aip-pg`의 의존이 `aip-ir`, `aip-plan`뿐이다(`grep -rn "aip_syntax\|aip_sema" crates/aip-pg` → 0줄). 파이프라인: `analyze → to_core → validate → aip_pg::compile(&Program, &SourceMap)`.
- 검증: 전환 전 실행 계획 전체를 골든으로 고정(`crates/aip-pg/tests/golden/*.plan.json`: 두 예제 + 오류 없는 conformance 3건, `crates/aip-cli/tests/golden.rs`), 골든 한 바이트 변조 시 실패 확인(negative control). 전환 후 바이트 동일, 단 한 줄 예외.
- 예외 한 줄 = **기존 버그 발견**: 옛 `lookup_param`이 같은 이름의 파라미터를 가진 첫 query의 타입을 가져왔다. `GetClubEvent`가 `club_faq`를 읽어 title/at/place를 NULL로 내보내고 있었다. IR은 이름이 해석돼 있어 이 부류의 오류가 구조적으로 사라진다.
- 하강 버그 수정: `produce csv to s3 bucket "exports"`의 store/bucket 뒤바뀜.
- IR 0.3: `Dur{n, unit}`(달력 단위 보존. "6 months"는 초로 환산하면 의미가 바뀐다), `Constraint.ordinal`(기존 DB 객체 이름 `__u7`이 선언 위치에 의존하기 때문. OI-IR4).
- 골든이 덮지 못한 차이(예제에 없는 입력): `visible to`+`masked`를 고정 순서로 적용, 필드당 generator 하나, `Json<X>`와 `Json validated by`를 IR이 구분하지 않음, E600/E601 위치가 식이 아닌 선언 단위. 다음 회차 대상.
- `check-docs` 전후 동일(36 blocks, 29 parsed, 0 failed).

### Presentation을 Core IR에서 파생 (철학 §4.1)
- 새 `crates/aip-contract`(의존: `aip-ir`만): `describe`(공개 계약), `gen_ts`, `operator`. `aip-plan`에서 `contract.rs`, `tsgen.rs` 삭제. 런타임은 CLI가 계산한 계약 JSON을 받아 서빙하고 Core IR을 보지 않는다.
- 계약에 실리는 사실(writes, effects, 오류 코드, 버전 파라미터)은 `aip-ir/src/facts.rs`, extension 기본값은 `builtin.rs`, 폼이 만드는 intent(IssueX, RequestX, XStatus…)는 `form_intents.rs` 한 곳에서 도출하고 `aip-pg`와 `aip-contract`가 함께 쓴다(중복 제거).
- 검증: 옮기기 전 계약·TS 출력을 골든으로 고정(`crates/aip-cli/tests/contract_golden.rs`), 바이트 동일. 실행 서버의 `GET /aip/describe`도 골든과 같았다.

### 공개 계약 위생 (철학 §8, §10)
- `webhooks` 절(서명 방식, 헤더, 비밀 환경변수 이름)을 공개 계약에서 빼고 `aip contract <file> --operator`로만 출력. 수정 전 shop 계약에 `"secret_env": "STRIPE_WEBHOOK_SECRET"`이 실려 있었다(negative control로 확인).
- 프로토콜 버전 분리: 계약 최상위 `"aip": "0.1"`(옛 실행 계획 버전 문자열) → `"protocol": "aip-protocol/0.1"`, `"core_ir": "aip-core/0.4"`.
- 의도된 골든 변경은 계약 2개와 TS 헤더 주석 2개뿐, 실행 계획 골든 불변.

### IR 0.4
- `Type::Json{schema, validated_by}`: `Json<Record>`와 `Json validated by <field>`(동적 스키마)를 구분. 검사 `AIP-I121`.
- 한 필드에 generator 둘 이상이면 `AIP-E222`(의미 없는 선언). conformance `two_generators`.

### 진단·오류 코드 레지스트리 (철학 §11)
- `crates/aip-ir/src/codes.rs` 한 표에 79개(E 31, W 6, I 21, 런타임 21): title, explain, fix, example, 런타임은 http_status·retryable. 소스의 코드 리터럴을 상수로 바꿨고 `AipError::status()`가 레지스트리를 읽는다.
- 완전성 테스트: 소스 스캔으로 모든 코드가 등록됐는지, 중복, 죽은 항목, conformance·계약 errors 코드 등록 여부. W404를 빼면 실패함을 확인.
- `aip explain-code <CODE>`, `aip diagnostics --markdown`. `spec/diagnostics.md`는 생성물이며 동일성 테스트로 묶었다.
- 관찰: 한 코드가 여러 의미로 쓰였다(E201은 59곳). AI가 코드만 보고 고칠 수 없다.
- 분리(발생 지점 119곳 전수 분류, 기존 번호는 가장 일반적인 의미에 유지): E107 actor 선언 둘, E108 타입 옵션 오류(Money 통화 누락 등), E208 문맥 밖 사용(`this`, query의 Upload), E210 최소 1개가 필요한 곳의 빈 값(enum, `in` 목록, approval), E211 인자 개수, E212 형식별 구문 오용, E214 webhook 옵션 오류, E302 Snapshot 대상에 동적 스키마 없음, E502. 새 코드마다 conformance 케이스.
- 회귀 방지: 코드별 사용 지점 수를 `crates/aip-ir/tests/code_sites.snap`에 고정. 지점이 늘면 테스트가 실패해 같은 의미인지 재검토를 강제한다.
- E212는 여전히 서로 다른 구문 오용(count body, same binder, 이벤트 spread, insert from as, approvers alias)을 묶는다. 수정 방법이 구문마다 달라 다음 분리 후보.
- `AIP.UNAVAILABLE`: 409 → 503, retryable. 닫힌 포트 풀로 엔진 호출과 실제 HTTP 응답 모두 503 확인(`crates/aip-cli/tests/unavailable.rs`).
- 관찰만: 멱등 오류 코드가 `AIP.IDEMPOTENCY.*`와 `AIP.INPUT.IDEMPOTENCY_KEY_*` 두 네임스페이스로 나뉘어 있다. 런타임 코드 이름 변경은 클라이언트 호환을 깨므로 보류.

### S2: 의미 규칙을 Core IR 위로 (언어 독립 검사)
- `check.rs`/`model.rs` 진단 지점 172곳을 분류: 프런트엔드 고유(F: 이름 해석, 타입 추론, 구문 형태) 127곳, 의미 규칙(S: 해석된 IR만으로 판단 가능) 45곳. S는 `crates/aip-ir/src/analyze.rs`로 옮기고 `check.rs`에서 지웠다(이중 실행 없음). 코드·메시지 불변.
- 파이프라인 단일화: `crates/aip-sema/src/pipeline.rs::check_source`(sema → F 오류면 중단 → to_core → validate → analyze → SourceMap으로 줄·열) 하나를 CLI 전 명령, conformance, 테스트 헬퍼가 쓴다.
- **언어 독립성 증거**: conformance의 S 규칙 케이스를 하강한 IR을 JSON으로 왕복시킨 뒤 `validate`+`analyze`만 돌려 `.expect`와 같은 코드가 나온다(`ir_json_alone_yields_the_semantic_codes`). 다른 작성 언어가 IR을 만들면 받게 될 검사와 같다. 규칙 하나를 끄면 실패(E207, E309로 두 번 확인). 이를 위해 conformance 19건 추가.
- 위치: 기존 conformance 34건과 두 예제의 `aip check` 출력이 줄·열·메시지까지 동일. 예외는 여러 줄 relation/fn 본문 안의 W402.
- 동작 변화: F 오류가 있으면 S 규칙은 보고되지 않는다. `aip check`에 I1xx가 나올 수 있다.
- 남음: lifecycle 검사는 F에 두었다. IR `validate`의 I113과 같은 노드를 검사해 코드 역할 정리가 먼저다(OI-D1).

### `tenant` 실행 (E602 해제, D-P18)
- 참조 무결성 트리거(경로 중간 행 이동 시 하위 행까지 검사), intent 단일 테넌트(Load 뒤 검사 Step), 앵커 테넌트 스캔 필터 자동 삽입, 파라미터에서 참조로만 도달한 집합은 필터 생략. 새 코드 E313(경로 불량), E314(앵커 없는 스캔), E315(`internal` 아닌 `cross tenant`), 런타임 `AIP.TENANT.MISMATCH`(404, 가시성 기반 NOT_FOUND 정책과 같은 이유). 문법 `internal cross tenant`. IR `aip-core/0.5`(`cross_tenant`).
- 부수 수정: `internal` intent를 호출할 경로가 없어 `at … run`이 실패하던 것을 `Engine::call_internal`로 해결.
- 검증: 새 예제 `examples/saas`(Workspace/Project/Task/Comment) 침투 e2e. 규칙 끔·트리거 끔·둘 다 끔 세 구성에서 각 공격이 실제로 성공함을 상시 테스트로 고정(negative control). 필터를 `true`로 바꾸거나 트리거·검사 Step을 빼면 e2e 실패(변이 확인).
- 한계: actor는 테넌트에 묶이지 않는다. 남의 테넌트 id 접근의 1차 방어선은 앱의 `allow`/`visible to`이고, 테넌트 규칙은 두 테넌트에 모두 속한 사용자가 둘을 섞는 것을 막는다. webhook/consume/retain은 암묵적 cross tenant, L3 폼은 필터 미적용 → 아래 "트랜잭션 테넌트 고정"으로 보강.

### 트랜잭션 테넌트 고정 (D-P18 보강)
- 테넌트 범위 엔티티와 루트 테이블의 INSERT/UPDATE/DELETE 트리거(`<table>__tenant_pin`)가 행의 테넌트를 트랜잭션 로컬 `aip.tenant`에 고정한다. 다른 테넌트의 쓰기는 `AIP.TENANT.MISMATCH`. UPDATE는 떠나는 테넌트도 검사한다. `aip.tenant_cross = on`이면 이 검사만 건너뛰고 참조 무결성 트리거는 그대로다.
- 플랜이 문맥 시작에 `tenant: pin|reset|cross` Step을 둔다. 앵커 intent와 L3 폼(approval, job, grant link, verification)은 앵커 테넌트로 고정, 항목을 여럿 담는 문맥(이벤트 핸들러, schedule 항목, rule 행, job 항목)은 항목마다 초기화, `cross tenant`는 스위치를 켠다. retain은 한 문장이 여러 테넌트 행을 지우므로 스위치를 켠 채 실행한다.
- 문법: `cross tenant`를 `on Event`, `schedule`, `rule`, `consume` 앞과 webhook 핸들러 앞에 허용. IR `aip-core/0.6`(`cross_tenant`).
- 검증: `saas_e2e`에 웹훅(섞인 페이로드 거부, 같은 테넌트 성공, cross 핸들러), 이벤트 핸들러, 스케줄 스윕, job 진행 범위, approval 승인자 범위, rule 정산을 추가. 핀만 끈 구성, 항목별 초기화를 뺀 구성에서 각각 공격과 배치 실패가 실제로 나타나는 것을 상시 테스트로 고정.
- 남음: actor 없는 문맥의 **읽기**는 필터하지 않는다. 웹훅·핸들러가 다른 테넌트 값을 읽어 이벤트·알림으로 내보내는 경로는 막히지 않는다(OI-TN1). grant link·verification은 실행 e2e 없음. consume은 미실행 형식(E602).

### `publishable`, `consent`, `impersonate` 실행 (E602 해제)
- 문법은 정본 그대로: `consent X version N required for [..]`, `impersonate E by <cond> audited reason required ttl <dur>`.
- publishable: 작업본(엔티티 테이블)과 게시본(`<table>_published`). query는 게시본을 읽고 초안은 `drafts query` 수식어로만 본다. 권한 `publishable by <조건>`, 생략 시 superuser만(없으면 E301). 생성 intent `Publish<E>`, `Discard<E>Draft`.
- consent: `AIP.CONSENT.REQUIRED`(403), 이력 `_aip_consent`, 버전을 올리면 이전 동의 무효. `Give/Withdraw<X>Consent`, `<X>ConsentStatus`.
- impersonate: 세션 `_aip_impersonation`, 토큰은 세션 id를 서명(`aip1.<b64 target.exp.session>.<sig>`), 매 호출 세션 유효성 확인. 대리 중 command는 감사에 운영자·대상 둘 다. 중첩·자기 자신·권한 있는 대상 거부, superuser 우회 꺼짐, 본인 데이터 export/erase·동의·approval 투표 거부.
- 검증: 새 예제 `examples/cms`, `crates/aip-cli/tests/cms_e2e.rs`(위조 토큰은 실제 소켓, ttl 만료는 실제 대기). 상시 negative control(`cms_negative_controls`) + 코드 변이 14건 전부 e2e 실패로 잡힘. 기존 실행 계획 골든 불변, IR `aip-core/0.7`.
- 보강(같은 날): 대리 세션이 시작 시에만 `by`를 검사해 운영자가 권한을 잃어도 ttl까지 유효했다. 이제 매 호출에 세션 확인과 같은 쿼리로 `by`(또는 superuser)를 운영자에 대해 재평가하고, 거짓이면 세션을 끝내고 인증 실패로 응답한다. e2e `impersonation_rechecks_the_operator`(DB로 role을 내린 뒤 대리 호출 거부, `ended_at` 기록, 데이터 불변), 재평가를 무력화하면 실패(negative control).
- 남음: consent가 걸린 job 시작 e2e 없음. 게시본의 touch 카운터·version은 다음 게시 전까지 작업본과 다르다.

### `search`, `outbound webhooks` 실행 (E602 해제)
- search: tsvector 생성 컬럼 + GIN, 가중치 A~D, `websearch_to_tsquery`, rank 정렬(동률 id). 일치도는 엔티티 필드가 아닌 IR 노드 `SearchRank`(Decimal). 길이 0 질의는 입력 오류, 불용어·구두점만이면 빈 결과. 한국어는 `simple` + 단어 접두 일치 근사이며 W603 경고와 계약 `guarantees.search`로 형태소 검색이 아님을 밝힌다(OI-08 유지). e2e: 가중치 순서, 다중어, 숨은 행 제외, 테넌트 격리, 페이지 도중 행 추가 시 중복·누락 0.
- outbound webhooks: outbox 비동기, `AIP-Signature: t=,v1=`(inbound Stripe와 대칭, 타임스탬프로 재전송 방지), 이벤트 id 헤더, 두 배씩 늘어 마지막이 `over`에 닿는 재시도, `disable after` 초과 시 비활성 기록. 엔드포인트 비밀은 저장하지 않고 `AIP_SECRET`과 엔드포인트 id로 파생(평문 저장 문제 W602 회피), 등록 응답에서 한 번만 노출. SSRF: http(s)만, 전달 직전 DNS 해석 결과가 사설·루프백·링크로컬이면 거부, 검사한 주소로 연결 고정(DNS rebinding 차단), 리다이렉트 미추종. 사설 대상은 `--allow-private-webhook-targets`로만.
- 검증: `search_e2e.rs` 2개, `outbound_e2e.rs` 5개. 상시 negative control 8건, 구현 변이 16건 전부 e2e 실패로 잡힘. 실서버 스모크에서 서명을 별도 Python 구현으로 재검증. ariari/shop 실행 계획 골든 불변. IR `aip-core/0.9`. 새 의존성 reqwest 0.12(rustls), url 2.
- 남음: 비활성 엔드포인트 재활성화, 비밀 회전, 전달 행 정리, https 대상 e2e, 다중 프로세스 러너 경합.

### `subscribe`, `migration` 실행 (E602 해제)
- subscribe: WebSocket `GET /aip/subscribe`, 첫 메시지 `auth`(브라우저 WebSocket은 헤더를 못 붙이고, URL 토큰은 로그에 남는다). `subscribe` → `snapshot` → 결과가 바뀔 때만 전체 재전송. 변경 감지는 `LISTEN/NOTIFY` 트리거(읽기 집합 테이블), 재조회는 매번 구독자 기준 allow·가시성·필드 가시성·테넌트 재적용, 권한 철회 시 error 후 종료. 상한 코드 `AIP.SUBSCRIPTION.LIMIT`, `AIP.SUBSCRIPTION.TOO_LARGE`. 프로토콜 `aip-protocol/0.1` 유지.
- migration: `migrate()`에서 스키마 뒤 선언 순서대로 각각 한 트랜잭션, `_aip_migration`에 이름·본문 digest 기록, 적용된 본문이 바뀌면 기동 실패(`AIP.MIGRATION.CHANGED`), 실패는 롤백(`AIP.MIGRATION.FAILED`), advisory lock으로 동시 기동 1회. 테넌트는 cross tenant.
- 검증: `subscribe_e2e` 5개(실서버 WebSocket), `migration_e2e` 4개, 두 Part negative control. 기존 실행 계획 골든 불변, IR `aip-core/0.10`.
- 남음: FK cascade 삭제의 NOTIFY, 리스너 재연결 후 재조회, 대규모 구독 부하, 클라이언트 자동 재연결. **기존 DB에는 새 트리거가 `--reset` 없이 생기지 않는다** → IR 비교 기반 스키마 진화로 해결 진행 중. 남은 미실행 형식: `consume`, `projection`.

### IR 비교 기반 스키마 진화, 계약 호환성 비교
- `--reset` 없이 스키마를 바꾼다. 배포마다 Core IR과 digest를 `_aip_deployment`에 같은 트랜잭션으로 기록하고, 다음 배포는 "이전 IR → 새 IR" 의미 차이(`aip-ir/src/diff.rs`, SQL 모름)를 `aip-pg/src/evolve.rs`가 DDL 단계로 바꾼다. 불변식: 옛 DB + 계획 = 새 프로그램을 빈 DB에 배포한 스키마(e2e가 카탈로그 덤프로 비교).
- 자동: 엔티티·optional 필드·default 있는 필수 필드·인덱스·enum 값 추가, 길이·범위 완화, 트리거·함수는 항상 현재 텍스트로 교체(기존 DB에 새 알림 트리거가 안 생기던 문제 해결). 데이터 검사 후 허용: unique/check 추가, 길이·범위 축소, 쓰이지 않는 enum 값 삭제. 거부: 선언 없는 삭제·이름 변경, 호환되지 않는 타입 변경, trait 추가·삭제, 필드 종류 변경.
- 의도 선언 문법(한 표현씩): 이름 변경 `was 옛이름`(필드, `entity New was Old`), 삭제 `removed field x`, `removed entity X`. 모순은 AIP-E111. 선언이 없는 프로그램의 IR·digest는 이전과 같다(IR `aip-core/0.11`).
- 데이터 변환이 필요한 조임(필수 전환, unique, 옛 필드 삭제)은 `migration` 다음 배포로 나눈다(expand/contract). 거부 코드 `AIP.SCHEMA.*`의 fix가 이 순서를 안내.
- `aip migrate --plan [--json]`은 읽기 전용, `aip run`은 거부 대상이 있으면 기동 실패. 기록 없는 기존 DB는 스키마가 맞으면 채택, 아니면 `AIP.SCHEMA.UNRECORDED`.
- `aip diff --against <core-ir.json|deployed> [--json]`(`aip-contract/src/compat.rs`): 03 §10 규칙(intent·입력 삭제, 필수 입력 추가, 출력 삭제·nullable화, 멱등성 강화=깨짐 / 오류 코드·정책 변경=경고 / 그 외 호환). 깨짐이면 종료코드 1.
- 검증: e2e 14개(실제 PostgreSQL에서 v1→v2→v3, 선언 없는 삭제 거부·무변경, 이름 변경 데이터 이동, unique 위반 개수 보고, `--plan` 무변경), diff 단위 8개, conformance 9개. negative control 6종(선언 없는 삭제를 허용하면 `note` 값이 실제로 사라짐). 기존 실행 계획 골든 불변. 네 예제 TS 클라이언트 tsc 통과(메인에서 실행).
- 남음: 대형 테이블 잠금 시간 미측정(`CREATE INDEX CONCURRENTLY` 미사용: 한 트랜잭션 보장과 양립 불가), trait 변경·필드 종류 변경 지원.

### 결과
`cargo test -q --workspace` → passed 167 failed 0 (스키마 진화·aip diff 후, 2026-10-02). 142 (subscribe·migration 후). 131 (search·outbound 후). 120 (대리 세션 재확인 후). 119 (publishable·consent·impersonate 후). 111 (트랜잭션 테넌트 고정 후). passed 110 (tenant 후). S2 후 108. 그 전: passed 106 failed 0, `check-docs` 62 blocks 0 failed (코드 분리 후, 2026-10-02). 레지스트리 직후 103. 이전: passed 90 failed 0, clippy 경고 0, `check-docs` 36 blocks 0 failed (2026-10-02).

### `publishable`, `consent`, `impersonate` 실행 (E602 해제, D-P19~D-P21)

- 새 예제 `examples/cms`(Site 테넌트, Article `publishable by editorOf(actor, site)`, `consent Terms`, `impersonate Member`). ariari, shop, saas의 실행 계획 골든은 바이트 불변이고 계약과 클라이언트 골든은 `core_ir` 버전 문자열(`aip-core/0.7`)만 바뀌었다.
- 저장 방식: 같은 테이블에 초안 JSON 컬럼을 두는 안과 `<table>_draft` 섀도 테이블 안을 버리고, **엔티티 테이블을 작업본, `<table>_published`를 게시본**으로 두었다. 이유는 모든 command, 제약, 트리거, rule, FK, 테넌트 고정이 지금 그대로 작업본에서 도는 것이다. query만 `Schema::published()`(같은 컬럼, 테이블 이름만 게시본)로 컴파일하므로 읽기 쪽 변경은 한 곳이다. 비용은 게시본 테이블 하나와 게시 때 행 복사다.
- 새 런타임 코드 `AIP.CONSENT.REQUIRED`(403). 409(PRECONDITION)가 아닌 이유는 호출 상태가 아니라 사용자가 할 일이 남은 거부여서다. `reason`은 동의 이름.
- 대리 토큰은 같은 형식 `aip1.<b64>.<서명>`에 본문 `<target>.<exp>.<session>`을 싣는다(일반 토큰은 두 부분 그대로). 세션 id가 서명에 들어가므로 세션을 바꾸거나 떼면 서명이 깨진다. 토큰 서명은 HTTP 계층이 하고(엔진은 비밀을 모른다), 세션 유효성(종료, ttl, target 일치)은 엔진이 호출마다 DB에서 확인한다.
- 검증: `crates/aip-cli/tests/cms_e2e.rs` 7개(게시 가시성과 편집/되돌리기/권한, consent 거부-동의-버전 상승-철회, 대리 거부 규칙 전부와 감사 행, ttl 1초 실제 대기, 실제 소켓의 토큰 위조 5종). 상시 negative control(`cms_negative_controls`)과 구현 변이 14건을 로그에 기록.
- 남음: 게시본의 `touch` 카운터는 다음 게시 때 반영된다. 게시본의 `version`은 작업본과 다를 수 있다. 세션 중 조회(query)는 감사하지 않는다(command만). 운영자가 `by` 조건을 잃어도 열린 세션은 ttl까지 유효하다.

### `search`, `outbound webhooks` 실행 (E602 해제, D-P22, D-P23)

- 예제 `examples/saas`에 추가: `Task.notes`, `search TaskSearch`(english, 제목 A와 노트 B), `search ProjectSearch`(korean), `SearchTasks`(keyset), `SearchProjects`(offset), `Endpoint` 엔티티와 `AddEndpoint`, `outbound webhooks for Endpoint`. ariari와 shop의 실행 계획 골든은 바이트 불변이고 그 계약과 클라이언트 골든은 `core_ir`(`aip-core/0.9`) 한 줄만 바뀌었다. IR은 `0.8`(검색)과 `0.9`(`SigningSecret`)로 올렸다.
- `search`: 인덱스는 생성 tsvector 컬럼 + GIN. 트리거 대신 생성 컬럼을 택한 이유는 행이 바뀌는 모든 경로(raw SQL 포함)에서 색인이 뒤처질 수 없어서다. history 엔티티의 버전 스냅숏과 `versioned` 변경 판정에서는 이 컬럼을 뺀다. 한국어는 PG에 형태소 분석이 없어(OI-08) `simple` 설정과 질의 단어 접두 일치(`'단어':*`)로 근사하고 `pg_trgm`(로컬 사용 가능 확인)은 쓰지 않았다: 확장 의존이 없고 가중치와 일치도 순위가 tsvector에서만 되기 때문이다. 대가로 단어 중간 일치와 합성어는 못 찾으며 AIP-W603과 계약에 적었다. 검증: `search_e2e.rs`(가중치, 다중어/or/구절/제외, 어간, 빈 질의, 숨은 행과 다른 테넌트, keyset 도중 행 추가, 한국어, 계약), 상시 negative control 5건과 구현 변이 6건 모두 실패로 잡힘.
- `outbound webhooks`: 전달은 outbox 뒤 러너(`aip-runtime/src/outbound.rs`)가 reqwest(rustls)로 보낸다. 서버는 해석한 주소가 전부 공개일 때만 그 주소로 연결하고 리다이렉트와 프록시를 쓰지 않는다. 검증: `outbound_e2e.rs` 5개(로컬 수신 서버에서 서명 검증과 변조/타 엔드포인트/재전송 거부, 5xx 재시도 스케줄 1s, 3s, 7s 후 성공, 계속 실패 시 비활성, 사설 주소 7종 거부, where와 테넌트, 비밀이 DB 어디에도 없음)와 실서버 스모크(python 수신, 서명을 별도 구현으로 재검증). 구현 변이 10건 모두 e2e 실패.
- 남음: 비활성 엔드포인트를 다시 켜는 방법(새로 등록만 가능), 전달 행 보관 정리, 비밀 회전, 엔드포인트 URL의 등록 시점 DNS 검사(전달 때 검사), 한국어 형태소 검색(OI-08), 수신자 쪽 상태를 앱이 읽는 방법(비활성 여부 노출).

### `subscribe`, `migration` 실행 (E602 해제, D-P24, D-P25)

- 예제 `examples/saas`에 추가: `subscribe LiveProjectTasks`(allow가 멤버십), `subscribe LiveWorkspaceTasks`(allow authenticated, 행 가시성에 기댐), `migration backfill_done_notes`. ariari, shop, cms의 실행 계획 골든은 바이트 불변이고 계약과 클라이언트 골든은 `core_ir`(`aip-core/0.10`) 두 줄만 바뀌었다. saas 골든은 구독, 마이그레이션, 알림 트리거 DDL이 더해졌다. 새 conformance `ok_subscribe_migration`, `tenant_subscribe_without_anchor`(AIP-E314).
- IR `aip-core/0.10`: `Subscribe.output`(목록 Shape)와 `Subscribe::as_query()`(구독이 뜻하는 list query), `Migration::digest()`. 구독은 pg에서 그 query의 계획을 재사용하고(allow, 가시성, 필드 가시성, 테넌트 필터가 query와 같은 코드), 읽는 테이블은 계획 SQL에서 `"table"`로 나타나는 이름(별칭의 컬럼 `x."table"`은 제외)을 뽑아 정한다. IR에서 따로 도출하지 않은 이유는 관계, 가시성, 테넌트 경로가 닿는 테이블을 SQL이 이미 전부 담고 있어서다. 과대 추정은 불필요한 재조회뿐이고(결과가 같으면 전송 없음) 누락은 없다.
- e2e `subscribe_e2e.rs` 5개(실서버와 tokio-tungstenite): 스냅숏과 다른 사용자의 변경 뒤 갱신(전체 결과), 구독이 읽지 않는 테이블 변경에는 재조회 없음, 다른 워크스페이스 변경은 재조회하되 메시지 없음, 보이지 않는 행 미포함(allow는 열려 있지만 가시성이 막는 구독, 타 워크스페이스 프로젝트 구독은 NOT_FOUND), 두 워크스페이스 멤버의 구독에 다른 테넌트 행이 섞이지 않음, 멤버십 삭제로 권한이 철회되면 error 후 구독 종료(가시성에 기댄 구독은 빈 목록으로 계속), 인증 없는 연결, 잘못된 토큰 3종, 인증 전 subscribe, 선언되지 않은 이름 3종, 상한(연결, 연결당 구독, 결과 크기), 대리 세션 종료 뒤 갱신에서 IMPERSONATION_ENDED. 같은 파일의 `a_change_wakes_only_the_subscriptions_that_read_it`는 두 구독 중 읽는 쪽만 다시 조회됨을 `Hub::reruns()`로 센다.
- e2e `migration_e2e.rs` 4개: 스키마만 있던 DB에서 첫 기동 실행과 기록(digest), 재기동 시 미실행, 서식 변경은 digest 불변, 본문 변경 시 `AIP.MIGRATION.CHANGED`(그 기동에서 아무것도 실행되지 않음), 선언 순서 실행, 실패하면 롤백과 미기록과 중단(고친 뒤 이어서 실행), 15만 행에서 동시 기동 2개 중 1회만 실행, saas의 migration이 두 워크스페이스를 한 트랜잭션에서 고침.
- negative control: 구독은 wake가 reads를 무시, 동일 결과 dedupe 제거, 재조회 오류에도 구독 유지, 대리 세션 재확인 제거, 인증 게이트 제거, NOTIFY 트리거 제거. 마이그레이션은 advisory lock 제거, digest 비교 제거, cross pin 제거. 모두 해당 e2e가 실패했고 원복했다. 실서버(`aip run --dev-auth`)와 Node의 전역 `WebSocket`으로 스냅숏과 갱신 수신을 확인했다.
- 남음: 자동 재연결과 백오프(클라이언트), 변경 행으로 거르기와 같은 입력 구독 공유(OI-10), 검색(`from X.match`)을 읽는 구독(AIP-E601), 스키마를 이미 가진 DB에는 알림 트리거가 `--reset` 없이는 생기지 않음, 선언에서 지운 migration의 `_aip_migration` 행 정리, 갱신 사이의 변경 행 diff 프로토콜.
