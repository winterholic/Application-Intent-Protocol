# 08. 커버리지 카탈로그: 백엔드에서 개발하는 모든 케이스

> 상태: Living document · 검증-보충 루프(`07` 4절)의 입력이자 출력
> 목표: 백엔드 개발에서 반복적으로 나오는 케이스를 전수 열거하고, 각각을 AIP 문법의 어느 구성요소가 담당하는지 판정한다. 담당자가 없으면 새 형식을 정의한다.

## 판정 기호

| 기호 | 뜻 |
|---|---|
| ✅ | 기존 L2 Core 또는 이미 정의된 L3 형식으로 표현됨 |
| 🆕 | 이 문서에서 새 L3 형식(또는 L2 확장)을 정의함. `spec/grammar.md`에 반영 |
| 🔌 | extension 효과 또는 순수 함수(`fn`)로 표현. 언어가 할 일은 서명과 경계 |
| ⚠ | 표현 방향은 있으나 설계 미결. 09 검증 로그의 open issue |
| ⛔ | 현재 범위 밖. 이유를 적음 |

형식 이름 옆 `[L2]`는 Core 언어 개정, 표시 없으면 L3 형식이다.

---

## A. 데이터 모델링

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| A01 | 엔티티, 필드, 기본값, 선택 필드 | `entity`, `T?`, `= default` | ✅ |
| A02 | 1:N, N:1, 역참조 | `ref`, `T[] via f` | ✅ |
| A03 | N:M과 조인 속성 | 조인 엔티티 명시 | ✅ |
| A04 | 참조 삭제 정책 (cascade, restrict, set null) | `on delete` | ✅ |
| A05 | 생성/수정 시각, 생성자 | `track created, updated [by actor]` | 🆕 |
| A06 | 변경 이력(감사 로그) | `history` | 🆕 |
| A07 | 소프트 삭제와 복원, 휴지통 | `soft delete [retain 30d]` | 🆕 |
| A08 | 낙관적 동시성(버전, ETag) | `versioned` + 명령 입력 `expect version` | 🆕 |
| A09 | 다형 참조(댓글 대상이 여러 타입) | `ref A \| B` 태그 유니온 참조 | 🆕 [L2] |
| A10 | 트리/계층(카테고리, 대댓글, 조직도) | `tree via parent [max depth N]` | 🆕 |
| A11 | 반정형 데이터 | `Json<Schema>` (스키마 있는 JSON만 경로 접근) | ✅ |
| A12 | 다국어 콘텐츠 | `Localized<Text>` 타입, 요청 locale 폴백 | 🆕 |
| A13 | 순서 있는 enum, 역할 서열 | `enum X ordered` | ✅ |
| A14 | 계산 필드(파생 값) | `= expr` 파생 필드, `count(...)` | ✅ |
| A15 | 금액과 통화 | `Money(KRW)` | ✅ |
| A16 | 다중 통화 환산 | `fx.convert` 효과 (read 등급) | 🔌 |
| A17 | 슬러그, 사람이 읽는 고유 키 | `slug from title unique [per scope]` | 🆕 |
| A18 | 채번(주문번호 `2026-000123`) | `sequence per <scope> format "..."` | 🆕 |
| A19 | 사용자 지정 정렬 순서(드래그) | `position within <scope>` (분수 인덱스) | 🆕 |
| A20 | 파일 첨부 메타데이터 | `s3.Object` 타입 | ✅ |
| A21 | 이미지 변형(썸네일, 리사이즈) | `s3.Object variants { thumb: 200x200 }` (deferred 처리) | 🆕 (s3 ext) |
| A22 | 복합 기본키/자연 키 | 금지. `id` 고정 + `unique (a, b)` | ✅ (의도적 제한) |
| A23 | 대용량 텍스트/바이너리 | `Text` 상한, 바이너리는 s3 | ✅ |
| A24 | 지리 좌표 | `geo.Point` (postgis capability) | 🔌 (geo ext) |
| A25 | 반복 일정 규칙 | `Recurrence` 타입 (RRULE) | 🆕 (stdlib) |
| A26 | 하위 컬렉션 개수 상한(이미지 10장, 고정 공지 3개) | `capacity count(...) <= 10` | ✅ (G04 형식) |
| A27 | 특정 시점 버전 참조(지원서 양식 스냅샷) | `Snapshot<T>` 타입 (`history` 엔티티 필요) | 🆕 |

## B. 인증과 신원

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| B01 | 소셜 로그인(OIDC) | `actor Member via auth.oidc(kakao)` | ✅ (auth ext) |
| B02 | 이메일+비밀번호 | `auth.password` provider (argon2id) | ✅ (auth ext) |
| B03 | 인증 코드(이메일/SMS, 학교 인증) | `verification` 형식 | 🆕 |
| B04 | 토큰 회전, 로그아웃, 전체 로그아웃 | auth ext 표준 intent | ✅ |
| B05 | MFA, 패스키 | auth ext provider | ⚠ (M4 이후) |
| B06 | 머신 클라이언트(API 키, 스코프) | `actor ApiClient via auth.apiKey scopes [...]` + 다중 actor | 🆕 [L2] |
| B07 | 서비스 간 내부 호출 | `internal` intent (공개 계약에서 제외, mTLS) | 🆕 [L2] |
| B08 | 관리자 대리 로그인(impersonation) | `impersonate` 형식 (감사 필수, 명시 승인) | 🆕 |
| B09 | 계정 연결/병합 | 일반 command + `erase`/`reassign` | ✅ |
| B10 | 가입 후 온보딩 단계 | `lifecycle` | ✅ |
| B11 | 세션 목록, 강제 로그아웃 | auth ext | ✅ |
| B12 | 익명 사용자 → 가입 전환(장바구니 유지) | `actor Guest via auth.anonymous` + `merge` 명령 | 🆕 |
| B13 | 시간 한정 코드로 행위 허용(출석 코드, QR) | `grant link ... on redeem do {...}` | 🆕 (C08 확장) |

## C. 인가

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| C01 | 전역 역할(RBAC) | `actor.role = ADMIN` | ✅ |
| C02 | 리소스 소유자 | `x.owner = actor` | ✅ |
| C03 | 관계 기반(동아리 역할) | `relation` | ✅ |
| C04 | 상태 의존 권한 | `allow` + `require`/`lifecycle` | ✅ |
| C05 | 필드 단위 읽기 권한(이메일은 본인과 관리자만) | 필드 `visible to` | 🆕 |
| C06 | 필드 단위 마스킹(010-****-1234) | 필드 `masked unless <pred> as <fn>` | 🆕 |
| C07 | 테넌트 격리 | `tenant` 형식 | 🆕 |
| C08 | 초대 링크, 공유 링크(기간 한정 권한) | `grant link` 형식 (서명 토큰 → 관계 부여) | 🆕 |
| C09 | 시간 제한 권한 | relation 안의 시간 조건 | ✅ |
| C10 | 다자 승인(2인 승인, 결재선) | `approval` 형식 | 🆕 |
| C11 | 권한 상승/관리자 소멸 탐지 | authority graph 분석 | ✅ (분석기) |
| C12 | 관리자 행위 감사 | intent 수식어 `audited` | 🆕 |
| C13 | 요청 빈도 제한 | intent 절 `limit N per <window> per <key>` | 🆕 (redis ext) |
| C14 | 봇 방지(CAPTCHA) | admission 효과 `captcha.verify` | 🔌 |
| C15 | 차단 사용자 상호 비노출 | 엔티티 `visible to` | ✅ |
| C16 | 권한 거절 사유를 사용자에게 설명 | `allow ... else REASON` (정책별 거절 코드) | 🆕 |
| C17 | 기능 플래그/점진 공개 | `flag` 선언 + `flag(name)` 술어 | 🆕 |
| C18 | 정책 테스트(이 사용자가 이걸 할 수 있나) | `aip can <actor> <intent> <input>` CLI + 계약 | 🆕 (도구) |
| C19 | 특정 대상에게만 유효한 초대 | `grant link ... to <member>` | 🆕 (C08 확장) |
| C20 | 시스템 관리자 전역 우회(일관 적용) | `actor Member superuser when ... audited` | 🆕 [L2] |
| C21 | 작성자만 수정/삭제 | `allow x.author = actor or ...` | ✅ |

## D. 입력과 검증

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| D01 | 타입, 범위, 길이, 정규식 | 정제 타입 `Text(1..30)`, `Int(0..)`, `Text matches /.../` | ✅ |
| D02 | 이메일, URL, 전화번호 | `Email`, `Url`, `Phone(KR)` | ✅ (stdlib) |
| D03 | 교차 필드(시작 < 종료) | `Range<T>` 또는 입력 레코드 `check` | ✅ |
| D04 | 조건부 필수 | 입력 레코드 `check when a: b != null` | 🆕 |
| D05 | 파일 크기/형식 | `Upload(max 10MB, types [...])` | ✅ |
| D06 | 리치 텍스트 정화(XSS) | `RichText(policy: basic)` 타입 | 🆕 (stdlib) |
| D07 | 중복 제출 | `idempotent` | ✅ |
| D08 | **동적 폼**(관리자가 정의한 질문, 답변을 그 정의로 검증) | `dynamic schema` 형식 | 🆕 |
| D09 | 금칙어/욕설 필터 | `moderation.check` 효과 또는 `fn` | 🔌 |
| D10 | 입력 정규화(trim, 소문자) | 정제 타입 옵션 `Text(trim, lower)` | ✅ |

## E. 조회

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| E01 | keyset 페이지네이션 | `page N by keyset` | ✅ |
| E02 | 오프셋 페이지네이션(페이지 번호 UI) | `page N by offset max page 100` | 🆕 (작은 확장) |
| E03 | 동적 필터, 선택적 필터 | `where x in filter.xs` (빈 목록 = 조건 없음 규칙) | ✅ |
| E04 | 동적 정렬 | `sort by param of { ... }` | ✅ |
| E05 | 전문 검색 | `search` 형식 (postgres tsvector 또는 search ext) | 🆕 |
| E06 | 한국어 형태소 검색 | search ext capability | ⚠ (엔진 선택 미결) |
| E07 | 집계/통계/대시보드(group by) | `query ... group by` | 🆕 [L2] |
| E08 | 내보내기(CSV/Excel) | `export` 형식 (job + s3 + notify) | 🆕 |
| E09 | 반경 검색 | `geo.within(p, 3km)` | 🔌 (geo ext) |
| E10 | 추천/개인화 | 외부 서비스 read 효과 | 🔌 |
| E11 | 실시간 구독(live query) | `subscribe` intent | 🆕 [L2] |
| E12 | 읽기 모델/프로젝션 | `projection` 형식 (이벤트로 유지) | 🆕 |
| E13 | 사용 가능 여부 확인(닉네임 중복) | query + `limit` | ✅ |
| E14 | 팔로우 기반 피드 | query(pull) 또는 projection(push) | ✅ / ⚠ (대규모 fan-out) |
| E15 | 외부 데이터 결합 조회 | read 등급 효과 `fetch` in query | 🆕 (효과 등급 `read` 추가) |
| E16 | 조회 전략 힌트 | `plan batch \| join` | 🆕 (작은 확장) |
| E17 | 복제본 읽기 | `consistency eventual` | 🆕 (M5 이후) |
| E18 | 단건 조회 + 권한별 다른 모양 | 필드 `visible to` + 선택 | ✅ |
| E19 | 목록 항목별 "내가 좋아요 했나" | 선택 안 `exists ...` (lateral) | ✅ |

## F. 명령과 쓰기

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| F01 | 단건 생성/수정/삭제 | `insert/update/delete` | ✅ |
| F02 | 기본 노출(관리 화면용 CRUD) | `expose` 형식 | 🆕 |
| F03 | 일괄 처리(전부 또는 전무) | `Set<T> max N` | ✅ |
| F04 | 일괄 처리(항목별 성공/실패 보고) | `each ... partial` 형식 (항목별 savepoint, 상한 필수) | 🆕 |
| F05 | upsert | `upsert E by (k1, k2)` | 🆕 [L2] |
| F06 | 복제(fork) | `insert from` | ✅ |
| F07 | 토글(좋아요) | `toggle E { ... }` | 🆕 |
| F08 | 오래 걸리는 작업 + 진행률 | `job` 형식 | 🆕 |
| F09 | 대량 가져오기(CSV) | `job` + 파싱 `fn` + `upsert` | 🆕 / 🔌 |
| F10 | 예약 실행(특정 시각에 게시) | `at <time> run <Intent>` | 🆕 |
| F11 | 초안/게시 | `publishable` 형식 | 🆕 |
| F12 | 연쇄 상태 전이(모든 항목 배송 → 주문 완료) | `rule` 형식 (파생 전이) | 🆕 |
| F13 | 되돌리기/휴지통 | `soft delete` + restore | 🆕 (A07) |
| F14 | 쓰기 후 결과 모양 지정 | `returns x { selection }` | ✅ |
| F15 | 복합 생성(동아리 + ADMIN 멤버 + 기본 양식) | 한 명령의 여러 insert | ✅ |
| F16 | 삭제 전제조건(본인만 남았을 때만 삭제) | `require count(...) = 1` | ✅ |

## G. 트랜잭션과 동시성

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| G01 | check-then-act | 컴파일러 잠금 집합 | ✅ |
| G02 | 재고 차감 | `-=` + CHECK | ✅ |
| G03 | 중복 예약(시간대 겹침) | `no overlap` | ✅ |
| G04 | 선착순 한정 수량(쿠폰 1000장) | `capacity` 형식 (데이터에서 온 상한) | 🆕 |
| G05 | 인기 행 경합(hot row) | `counter sharded N` 힌트 | 🆕 |
| G06 | 분산 락 | 제공하지 않음. DB 잠금으로 표현 | ✅ (의도적) |
| G07 | 멱등 소비 | 자동 | ✅ |
| G08 | 직렬화 격리 필요 불변식 | 자동 격상 | ✅ |
| G09 | 경매 최고가 | 잠금 + `require bid > current` | ✅ |
| G10 | 참조 일관성(대댓글의 부모는 같은 글 소속) | 참조 경로 `invariant` → 복합 FK 하강 | 🆕 (하강 규칙) |
| G11 | 자기 참조 금지(자기 차단) | `invariant not_self: blocker != blocked` | ✅ |

## H. 외부 연동과 부작용

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| H01 | 동기 응답이 필요한 외부 호출 | reservable / irreversible 효과 | ✅ |
| H02 | 외부 조회 | `read` 등급 효과 | 🆕 (E15) |
| H03 | 웹훅 수신(서명 검증, 중복 제거) | `webhook` intent | 🆕 [L2] |
| H04 | 웹훅 발신(고객 구독, 서명, 재시도) | `outbound webhooks` 형식 | 🆕 |
| H05 | 메일/SMS/푸시 | mail/sms/push ext | ✅ |
| H06 | 결제/환불 | payments ext | ✅ |
| H07 | 정기 결제/구독 | `billing` 형식 (payments ext) | ⚠ |
| H08 | 외부 데이터 주기 수집 | `schedule` + `read` 효과 + `upsert` | ✅ |
| H09 | 사용자별 외부 계정 토큰 저장/갱신 | `Credential` 타입 (암호화, 자동 갱신) | 🆕 |
| H10 | 타임아웃, 서킷 브레이커, 재시도 정책 | 효과별 런타임 정책 (`aip.toml`) | ✅ |
| H11 | LLM 호출 | `ai` ext 효과 (비결정, 비용, observational 또는 irreversible) | 🔌 |
| H12 | 외부 호출 결과를 모름(타임아웃) | `lookup` 서명 | ✅ |
| H13 | 파일 교체/삭제 시 이전 객체 정리(커밋 후) | `s3.Object` 필드 소유권: 덮어쓰기·행 삭제 시 이전 객체 deferred 삭제 | 🆕 (s3 ext) |
| H14 | deferred 효과가 끝내 실패했을 때 도메인 처리 | 효과 호출 `on failure do {...}` | 🆕 |

## I. 이벤트와 비동기

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| I01 | 도메인 이벤트 발행 | `emit` | ✅ |
| I02 | 내부 구독 반응 | `on Event` | ✅ |
| I03 | 외부 브로커 발행 | `emit ... to kafka topic` | ✅ |
| I04 | 외부 이벤트 소비 | `consume` intent | 🆕 [L2] |
| I05 | 이벤트 소싱 | 범위 밖. 이벤트는 사실 기록이지 상태의 원천이 아님 | ⛔ |
| I06 | saga | 자동 생성 | ✅ |
| I07 | 지연 실행 | `at` / 타이머 | ✅ |
| I08 | 실패 이벤트 재처리 | `aip outbox retry` | ✅ (도구) |
| I09 | 이벤트 스키마 버전 | `event X v2` + 업캐스트 `fn` | 🆕 |

## J. 시간과 배치

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| J01 | cron | `schedule every ...` | ✅ |
| J02 | 보존 기한 | `retain` | ✅ |
| J03 | 만료(토큰, 예약, 초대) | 타이머 / `expires` | ✅ |
| J04 | 대량 배치 청크 처리 | 하강 시 자동 청크 | ✅ |
| J05 | 영업일/공휴일 계산 | `calendar` stdlib | ⚠ (공휴일 데이터 원천 미결) |
| J06 | 반복 일정 인스턴스 생성 | `Recurrence` + schedule | 🆕 (A25) |
| J07 | 놓친 실행 처리 | `catch up once \| skip` | ✅ |

## K. 알림과 커뮤니케이션

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| K01 | 앱 내 알림 + 읽음 | `notify` + notify ext | ✅ |
| K02 | 수신 설정/거부 | `notify ... respecting preferences` | 🆕 |
| K03 | 묶음 알림(다이제스트) | `notify ... digest every 1h` | 🆕 |
| K04 | 채팅/메시지 | 엔티티 + `subscribe` | ✅ |
| K05 | 템플릿 다국어 | ext 템플릿 + `Localized` | ✅ |

## L. 콘텐츠와 커뮤니티

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| L01 | 게시글/댓글/대댓글 | 엔티티 + `tree` | ✅ |
| L02 | 좋아요/북마크 | `toggle` + unique | ✅ |
| L03 | 신고/모더레이션 큐 | lifecycle + `retain` | ✅ |
| L04 | 조회수 | `counter` | ✅ |
| L05 | 인기순(시간 감쇠) | 파생 점수 `fn` | 🔌 |
| L06 | 태그 | N:M | ✅ |
| L07 | 멘션 알림 | 파싱 `fn` + `notify` | 🔌 + ✅ |
| L08 | 공개 범위(전체/멤버) | `visible to` | ✅ |

## M. 커머스와 금융

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| M01 | 장바구니 | 엔티티 | ✅ |
| M02 | 재고 예약/확정/해제 | `reserve/confirm/release stock` (inventory 형식) | ✅ |
| M03 | 쿠폰/프로모션 규칙 | 가격 `fn` + `capacity` | 🔌 + 🆕 |
| M04 | 포인트/적립금 원장 | `ledger` 형식 (복식부기, 불변, 잔액 파생) | 🆕 |
| M05 | 배송 추적(외부 웹훅) | `webhook` | ✅ |
| M06 | 환불/부분 환불 | payments ext | ✅ |
| M07 | 세금/수수료 | `fn` | 🔌 |
| M08 | 판매자 정산 | `schedule` + `ledger` | ✅ |
| M09 | 회계 장부 누적 잔액 | `running_sum` | ✅ |

## N. 멀티테넌시와 SaaS

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| N01 | 테넌트 격리 | `tenant` | 🆕 (C07) |
| N02 | 플랜별 한도(프로젝트 3개까지) | `capacity` | 🆕 (G04) |
| N03 | 조직/팀/초대 | 엔티티 + `grant link` | ✅ |
| N04 | 사용량 계량(metering) | `counter` + `export` | ✅ / ⚠ |
| N05 | 테넌트별 설정 | 엔티티 + `config` 폴백 | ✅ |

## O. 개인정보와 컴플라이언스

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| O01 | 삭제권 | `erase` | ✅ |
| O02 | 열람권(내 데이터 내보내기) | `export personal data of actor` | 🆕 |
| O03 | 약관 동의와 재동의 | `consent` 형식 | 🆕 |
| O04 | 필드 암호화 저장 | `encrypted` 필드 수식어 (kms ext) | 🆕 |
| O05 | 출력 마스킹 | C06 | 🆕 |
| O06 | 개인정보 조회 기록 | `personal ... access audited` | 🆕 |
| O07 | 보존 기한 | `retain` | ✅ |

## P. 운영과 관측

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| P01 | health/readiness | 런타임 기본 | ✅ |
| P02 | 메트릭, 트레이싱, 구조화 로그 | 런타임 기본 | ✅ |
| P03 | 타입 있는 설정값 | `config` 선언 | 🆕 [L2] |
| P04 | 백오피스 | `expose` + 역할 | ✅ |
| P05 | 데이터 마이그레이션/백필 | `migration` 블록 (집합 연산) | 🆕 |
| P06 | 계약 버전 공존 | 계약 해시 + deprecated intent | ⚠ |
| P07 | 멀티 리전 | 범위 밖 | ⛔ |
| P08 | 부하 제한/백프레셔 | 런타임 동시성 한도 | ✅ |

## Q. 실시간과 협업

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| Q01 | live query | `subscribe` | 🆕 (E11) |
| Q02 | 접속 상태(presence) | redis ext `presence` | 🆕 (ext) |
| Q03 | 동시 편집(CRDT) | 범위 밖 | ⛔ |

## R. 클라이언트 계약

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| R01 | 타입 생성 | 계약 → SDK | ✅ |
| R02 | 오류 reason 타입 | 계약 | ✅ |
| R03 | 클라이언트 캐시 무효화 | 계약에 `invalidates` 포함 → `@aip/react`가 자동 refetch | 🆕 |
| R04 | 낙관적 업데이트 | 계약의 쓰기 집합으로 힌트 | ⚠ |
| R05 | 오프라인 큐 | 멱등 키 기반 재전송 | ⚠ |

## S. 도메인 패턴

| ID | 케이스 | 표현 | 판정 |
|---|---|---|---|
| S01 | 시간 슬롯 예약 | `no overlap` + `capacity` | ✅ |
| S02 | 투표(1인 1표) | `unique` | ✅ |
| S03 | 출석 체크(코드, 시간 창) | `verification` + `require now in window` | ✅ |
| S04 | 추첨/무작위 | `random(seed recorded)` stdlib | 🆕 |
| S05 | 추천인 보상 | `grant link` + `ledger` | ✅ |
| S06 | 결재선 | `approval` | 🆕 (C10) |

---

## 판정 합계 (v0.1)

케이스 203건 (Pass 2 기준). 한 케이스에 판정이 둘 이상 붙은 경우(예: `🔌 + 🆕`) 각각 센다.

| 판정 | 건수 |
|---|---|
| ✅ | 101 |
| 🆕 | 83 |
| 🔌 | 12 |
| ⚠ | 9 |
| ⛔ | 3 |

집계 명령: `awk -F'|' '/^\| [A-S][0-9][0-9] /{...}' 08-coverage-catalog.md` (09 검증 로그에 전문).

---

## 새 형식 정의 (문법 스케치)

아래는 🆕 항목의 표현이다. 공식 문법은 `spec/grammar.md`.

### 엔티티 수식어

```aip
entity Post {
  track created, updated by actor           // A05: createdAt, updatedAt, createdBy, updatedBy
  history                                   // A06: post_history 테이블, 쓰기 집합마다 전후 값 기록
  soft delete retain 30d                    // A07: deletedAt, 기본 가시성에서 제외, 30일 후 purge
  versioned                                 // A08: version 필드, 명령 입력 `expect version`
  tenant via board.workspace                // C07: 모든 읽기/쓰기에 테넌트 조건 강제

  title: Localized<Text(1..100)>            // A12
  body: RichText(policy: basic)             // D06
  email: Email  personal  encrypted  visible to self or actor.role = ADMIN
                masked unless self as mask.email          // C05, C06, O04
  parent: Post?  tree max depth 5           // A10
  target: ref Club | Recruitment            // A09
  no:    Text sequence per board format "{board.code}-{n:06}"  // A18
  slug:  Text slug from title unique per board                  // A17
  rank:  Text position within board                             // A19: 분수 인덱스 문자열
}
```

- `self`는 필드 가시성 문맥에서 "이 행이 actor 자신인 경우"(Member 엔티티) 또는 `owner`를 가리키는 예약어다. 정의 규칙은 grammar 명세.
- `soft delete`가 있는 엔티티에 대한 `delete`는 소프트 삭제로 하강하고, 영구 삭제는 `purge`로만 가능하다.
- `tenant`가 선언된 엔티티를 다른 테넌트 조건 없이 읽는 쿼리는 표현 불가능하다(하강 시 조건 삽입, 우회 문법 없음). 테넌트 간 작업은 `cross tenant` 수식어가 붙은 `internal` intent만 가능하다.

### 인증·인가

```aip
actor Member   via auth.oidc(kakao)
actor ApiClient via auth.apiKey scopes [orders.read, orders.write]      // B06
actor Guest    via auth.anonymous                                       // B12

command CreateOrder(...) {
  allow actor is Member or (actor is ApiClient and actor has scope orders.write)
}

verification SchoolEmail {                                              // B03
  subject: Member
  target: Email where exists School s where s.emailDomain = domain(target)
  code: digits 6  ttl 10m  attempts 5  resend after 60s
  deliver via mail.template("school-code")
  on verified(m, email) do { set m.school = the School s where s.emailDomain = email.domain }
}
// 하강: 코드 해시 저장 엔티티 + Request/Verify 두 intent + rate limit + 시도 횟수 lifecycle

grant link ClubInvite {                                                 // C08
  grants: membership(holder, club) as GENERAL
  scope: club: Club
  issued by managerOf(actor, club)
  expires 7d  uses 50
}
// 하강: 서명 토큰 엔티티 + Issue/Redeem intent. Redeem 시 unique(club, member) 등 불변식 그대로 적용

approval ExpenseApproval for FinancialRecord r {                        // C10
  approvers: ClubMember m where m.club = r.club and m.role >= MANAGER
  require 2 approvals, no self approval
  on approved do { set r.status = APPROVED }
  on rejected do { set r.status = REJECTED }
  expires 7d
}

command ForceClose(club: Club) audited { ... }                          // C12
command RequestCode(email: Email) limit 5 per 10m per actor, 20 per 10m per client { ... }   // C13
command DeleteClub(club: Club) {
  allow membership(actor, club).role = ADMIN else ONLY_ADMIN_CAN_DELETE  // C16
}

flag newCheckout default off rollout 10% by actor                       // C17
command Checkout(...) { allow flag(newCheckout) and authenticated ... }

impersonate Member by actor.role = SUPPORT audited reason required ttl 30m   // B08
internal command RecalculateStats(club: Club) { ... }                    // B07
```

### 입력과 동적 스키마

```aip
record PeriodInput {
  start: Time
  end: Time?
  openEnded: Bool
  check when not openEnded: end != null else END_REQUIRED               // D04
}

entity ApplyForm {
  club: Club
  questions: List<Question> max 50          // record Question { key, label, kind: TEXT|CHOICE|FILE, required, maxLength, choices }
  dynamic schema from questions             // D08 (엔티티 trait): 이 엔티티의 행이 다른 데이터의 스키마가 된다
}

entity Apply {
  form: ApplyForm
  answers: Json validated by form           // 저장 시 form의 질문 정의로 런타임 검증
}
```

`dynamic schema`는 정적 타입이 아닌 **데이터로 정의된 스키마**다. 정적으로 보장하는 것은 "검증이 반드시 실행된다"와 "질문 정의 자체가 유효하다"(질문 key 중복 금지 등)이고, 개별 답변의 타입은 런타임 검증(R 등급)이다. 질문 정의가 바뀐 뒤 기존 답변의 해석은 `form` 참조가 스냅샷이어야 안전하므로, `validated by`는 스냅샷 참조를 요구한다(Case 7의 `snapshot`과 연결).

### 조회

```aip
query ClubStats(club: Club) {                                           // E07
  allow managerOf(actor, club)
  from Apply a where a.recruitment.club = club
  group by a.recruitment, a.status
  select { recruitment { title } status count: count() }
}

search RecruitmentSearch on Recruitment fields [title weight A, body weight B] language korean   // E05
query SearchRecruitments(q: Text(1..50)) {
  allow public
  from RecruitmentSearch.match(q) r
  page 20 by keyset
  select { id title rank: r.rank }
}

subscribe ChatRoom(room: Room) {                                        // E11
  allow exists membership(actor, room)
  from Message m where m.room = room
  select { id body author { nickname } sentAt }
}
// 하강: 초기 스냅샷 query + outbox의 Message 쓰기 이벤트를 조건 필터 후 WebSocket 전달, 전달 시점에 가시성 재평가

projection ClubFeed from events [ActivityPosted, NoticePosted] key club {   // E12
  on ActivityPosted e: insert FeedItem { club: e.activity.club, kind: ACTIVITY, ref: e.activity }
  on NoticePosted e:   insert FeedItem { club: e.notice.club, kind: NOTICE, ref: e.notice }
}

query Weather(city: Text(1..50)) cached 10m {                                  // E15
  allow public
  fetch weather.current(city) as w                                      // read 등급 효과
  select { temp: w.temp }
}

query Heavy(...) { plan batch ... }                                     // E16
query Feed(...) { consistency eventual ... }                            // E17
query List(...) { page 20 by offset max page 100 ... }                  // E02
```

### 쓰기

```aip
expose Notice {                                                          // F02
  read:   visible
  create: managerOf(actor, club)  fields [title, body, pinned]
  update: managerOf(actor, club)  fields [title, body, pinned]
  delete: managerOf(actor, club)
}
// 하강: GetNotice, ListNotices, CreateNotice, UpdateNotice, DeleteNotice intent. 필드 목록 밖은 입력 불가

command ImportMembers(club: Club, rows: List<MemberRow> max 1000) {
  allow managerOf(actor, club)
  do {
    each rows r partial {                                               // F04: 항목별 성공/실패 결과
      upsert ClubMember by (club, member) { club, member: r.member, name: r.name, role: GENERAL }   // F05
    }
  }
}

command ToggleBookmark(r: Recruitment) {                                // F07
  allow authenticated
  do { toggle Bookmark { member: actor, recruitment: r } }
}

job ExportApplicants(recruitment: Recruitment) {                        // F08, E08
  allow managerOf(actor, recruitment.club)
  progress over Apply a where a.recruitment = recruitment
  produce csv to s3 bucket "exports" expires 7d
  notify actor via notify.template("export-ready")
}

command SchedulePublish(post: Post, publishAt: Time) {                  // F10
  allow post.author = actor
  do { at publishAt run PublishPost(post) }
}

entity Article { publishable }                                          // F11: draft/published 두 버전, Publish/Discard intent 생성

rule OrderCompleted on Order o                                          // F12
  when o.status = SHIPPED and all(o.items i: i.delivered)
  do { set o.status = COMPLETED }
// 하강: o.items.delivered 또는 o.status를 쓰는 명령의 커밋 직전에 규칙 평가 삽입 (연쇄 깊이 상한)
```

### 동시성·용량

```aip
entity Campaign {
  quantity: Int(1..)
  capacity count(Coupon c where c.campaign = this) <= quantity else SOLD_OUT    // G04
}
// 하강: Coupon insert 시 Campaign 행 정렬 잠금 + 카운트 검사. 또는 remaining 컬럼 차감 + CHECK (planner 선택)

entity Product { stock: Int counter via postgres sharded 8 }             // G05: 합산 읽기, 분산 쓰기
```

### 외부 연동

```aip
webhook TossPaymentEvent via payments.toss.webhook {                    // H03
  // 서명 검증, 이벤트 id 중복 제거는 extension이 하강에서 삽입
  on DONE(e)     do { update Order o where o.paymentRef = e.paymentKey set o.settledAt = now }
  on CANCELED(e) do { ... }
}

outbound webhooks for Workspace w {                                     // H04
  events [OrderCreated, OrderCancelled] where event.order.workspace = w
  sign hmac_sha256  retry 10 over 24h  disable after 3d failing
}

entity Member { calendar: Credential<google.calendar>? }                // H09: 암호화 저장, 만료 시 refresh 효과 자동

consume kafka topic "inventory.adjusted" as InventoryAdjusted {         // I04
  key: sku  dedupe by event_id
  do { update Product p where p.sku = msg.sku set p.stock = msg.stock }
}

event OrderCreated v2 { order: Order, channel: Channel }                // I09
upcast OrderCreated v1 -> v2 with channel: WEB
```

### 알림

```aip
notify managersOf(club) via notify.template("new-apply")
  respecting preferences(category: RECRUITMENT)                         // K02
  digest every 1h                                                       // K03
```

### 개인정보·설정·운영

```aip
consent Terms version 3 required for [SubmitApply, CreateClub]          // O03: 미동의 시 CONSENT_REQUIRED
command ExportMyData() {                                                // O02
  allow authenticated
  do { export personal data of actor to s3 notify actor }
}
entity Member personal access audited { ... }                           // O06

config maxAttachments: Int = 10                                         // P03: aip.toml/환경변수에서 타입 검증 후 주입

migration 2026_10_01_backfill_slug {                                    // P05
  update Post p where p.slug = null set p.slug = slugify(p.title)
}

fn pickWinners(entries: Set<Entry>, n: Int): Set<Entry> = sample(entries, n, seed: recorded)   // S04
```

### Pass 2 추가 형식

```aip
actor Member via auth.oidc(kakao) superuser when role = SUPER_ADMIN audited   // C20
// 모든 allow와 visible to에 "or actor is superuser"가 일관되게 붙고, 사용 시 감사 기록이 남는다.
// 아리아리에서 공지 상세는 시스템 관리자를 허용하고 고정 공지 목록은 허용하지 않던 불일치가 사라진다.

entity Apply {
  form: Snapshot<ApplyForm>                          // A27: 제출 시점 양식 버전에 고정
  answers: Json validated by form
}

entity ActivityComment {
  activity: ClubActivity
  parent: ActivityComment?  tree max depth 2
  invariant same_activity: parent = null or parent.activity = activity   // G10
  // 하강: (parent_id, activity_id) → (id, activity_id) 복합 FK. 경쟁 없이 DB가 강제
}

entity Block {
  blocker: Member  on erase cascade
  blocked: Member  on erase cascade
  unique (blocker, blocked) else ALREADY_BLOCKED
  invariant not_self: blocker != blocked            // G11
}

entity Club {
  profile: s3.Object?                                // H13: 필드가 객체를 소유
}
command ChangeClubProfile(club: Club, file: Upload(max 5MB, types [png, jpg])) {
  allow managerOf(actor, club)
  do { s3.put(file, bucket: "club") into club.profile }
  // 하강: 새 객체 staged 업로드 → 커밋 → 이전 객체 deferred 삭제. 커밋 전에 이전 파일을 지우는 순서는 표현 불가
}

command CancelOrder(order: Order) idempotent by order {
  allow order.customer = actor
  do {
    payments.refund(order.paymentRef, order.total, key: order.id) on failure do {   // H14
      set order.refundStatus = FAILED
    }
    set order.status = CANCELLED
  }
}

grant link AttendanceCode {                          // B13: 관계를 부여하지 않고 행위만 허용
  scope: event: ClubEvent
  issued by managerOf(actor, event.club)
  expires 2h
  require exists membership(holder, event.club) else NOT_CLUB_MEMBER
  on redeem do { insert Attendance { event, member: holder } }   // unique(event, member)로 중복 출석 차단
}

grant link DirectInvite {                            // C19: 특정 회원 전용 초대
  grants: membership(holder, club) as GENERAL
  scope: club: Club, invitee: Member
  issued by managerOf(actor, club)
  to invitee
  expires 7d  uses 1
}
```
