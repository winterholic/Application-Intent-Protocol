# 페이스리뷰 전체 포팅 실행 기록

2026-10-15. **앱 포팅은 미완료다.** 전체 기능 추적표와 MySQL adapter 계약 조사를 먼저 수행하고,
원본 앱의 실제 DB 기준선으로 이어간다. 댓글 PostgreSQL fixture는 엔진 회귀이며 전체 포팅 분모에 넣지 않는다.

2026-10-17 후속에서 MySQL `mysql` feature에 선택 `mysql_async` 의존성과 live INFORMATION_SCHEMA introspection draft 경로를 추가하고, 격리 MySQL 8.4의 작은 synthetic schema 조회를 확인했다. 이는 FaceReview 원본 전체 포팅이나 서비스 read/write/transaction 지원이 아니다. MariaDB도 미검증이다. 댓글 UI는 브라우저 연결/preflight 차단으로 미실행이며 YouTube 모달 원인은 추정이다. 세부 및 다음 gate는 [브라우저 기준선](BROWSER-BASELINE.md#2026-10-17-댓글-ui-실행-환경-갭)과 [adapter 계약](MYSQL-ADAPTER-CONTRACT.md#2026-10-17-mysql-드라이버-및-live-catalog-단계)을 따른다.

## 고정 원본 — WBS 7.1.1

| 역할 | 저장소 | 고정 SHA | 선정 근거 |
|---|---|---|---|
| 사용자 | facereview-front-joowon | 2e7c021fa5abebd72278d841b8066a33d45cfbb3 | README의 facereview.net·Vercel 운영 설명, vercel.json API rewrite, Router와 현재 v2 호출이 백엔드에 대응 |
| 관리자 | new-facereview-admin-front | ab0386c878a5ba5c7c079a262b987c10d211d22d | README의 별도 운영 관리자 제품, App의 login/dashboard/users/videos/requests/comments 경로와 실제 admin API |
| 서버 | new-facereview | 7948b7e721837fdb73c427cb54e12cad4ec2d5af | application factory의 blueprint registration과 현재 v2 route/service/model, Socket.IO와 Beat |

현재 체크아웃의 분석 대상 파일은 HEAD와 LF 정규화 후 바이트가 같다. Git의 dirty 표시는 CRLF 차이다.
원본 작업 트리는 수정하지 않는다. 운영이 지금 이 SHA라는 주장은 하지 않으며 **실험 baseline**으로 고정한다.
각 파일의 SHA-256과 dependency lock hash는 [inventory JSON](facereview-inventory.json)의 repositories에 있다.

`facereview-front`는 이전 사용자 프런트(README·Docker 배포 workflow), `-ref`는 `-joowon`과 같은 upstream의 참조 사본이다.
두 저장소를 별도의 현재 제품으로 더해 분모를 늘리지 않는다. `origin-codes/`는 레거시 구현이므로 실행 제품이 아니다.
Swagger/Redoc/OpenAPI와 Flask static은 프레임워크 생성 경로이며 업무 포팅 분모에서 제외하고 runtime route 비교에는 별도 표시한다.
반면 `/admin/dummy-data`는 production에 실제 등록되므로 이름만 보고 개발 전용으로 제외하지 않는다.

## 추적표 구조와 재현

```sh
python3 -m unittest discover -s tools -p test_facereview_inventory.py
python3 tools/facereview_inventory.py --root /path/to/facereview --output /tmp/facereview-inventory.json
cmp /tmp/facereview-inventory.json plan-docs/porting/facereview-inventory.json
```

JSON의 entries는 기능 ID, 원본 파일/행, HTTP method/path·화면·socket·job, 인증 decorator,
service call과 transitive service/모델 근거, AIP 처리 방식, WBS, 상태·gate·실행 증거를 연결한다.
models는 11개 RDB 모델 및 Mongo 문서/Repository 선언, redis는 key 표현식, schedules는 Beat schedule,
integrations는 메일/YouTube/ML/DB 접점을 보존한다. 서비스 내부의 동적 객체 alias 추론은 하지 않으므로
`data_models_static`은 코드 근거 탐색용이며 runtime DB oracle을 대신하지 않는다.

고정 revision에서 추출한 항목은 HTTP 52개(제품 등록 49개·개발 전용 3개), socket 3개, job 3개,
사용자·관리자 route 선언 18개(redirect/layout/index/fallback 포함), client HTTP call site 47개, client socket call site 3개다.
서로 다른 종류를 더해서 기능 개수나 포팅 대체율이라고 부르지 않는다.
47개 client HTTP는 `/api` prefix와 path parameter를 정규화해 모두 원본 handler ID에 연결된다.
알 수 없는 blueprint, 동적 Python route, 인식할 수 없는 TS 호출, backend 없는 client 호출은 추출 실패다.
이 도구는 고정된 프로젝트의 코드 패턴을 다루며 임의 TS 프로그램 전체를 해석하는 parser가 아니다.

## 교체 방식·권한 소유자 — WBS 7.1.4

| 영역 | 처리 방식·데이터 책임 | 선행 gap·검증 gate |
|---|---|---|
| 로그인/회원/세션 | 공식 Python 인증 확장 + AIP principal binding/revoke. 비밀번호 검사·HS256→제품 인증 전환은 서버가 담당 | 7.2.2. refresh cookie/만료/replay/logout 후 구토큰·정지계정. 프런트가 role/owner를 결정하지 못함 |
| SQL 목록/조회/작성 | resource read/apply·서버 row/field policy, MySQL 업무 DB 유지 | 7.2.1/3. [adapter 계약](MYSQL-ADAPTER-CONTRACT.md)의 원자성·drift·권한 matrix. 원본 ORM proxy 금지 |
| 댓글/like/bookmark | 선언 read/transition + 물리 삭제/토글 지원 gap 분리 | 7.2.4. 원본 댓글은 사용자·관리자 모두 is_deleted, like/bookmark 취소는 실제 row 삭제. 원본 on/off/on 의미 대조 |
| 추천/감정/마이페이지 | 공식 Python 확장에 순수 score/aggregation; Mongo/Redis는 서버가 등록한 capability | 7.2.5/8. MySQL+Mongo saga 단계/보상/journal. 임의 network/raw SQL 허용으로 우회하지 않음 |
| 영상/프레임 | Socket.IO transport adapter 또는 공식 SDK realtime 전환 + 전문 ML runtime | 7.2.6. 실제 player/camera/frame/model.h5 inference와 centisecond timeline. 가짜 감정값 성공 금지 |
| 예약 업무 | Beat 3개 원본 일정→managed durable job/등록된 YouTube provider | 7.2.7. retry/중복/부분실패/quota/추천 pool. 외부 전문 API 유지량 공개 |
| 이메일 | 등록된 SMTP/HTTP 전송 capability + receipt/suppression/멱등성 | 7.2.8. sink·실provider를 구분. notify outbox만 기록한 것은 메일 발송 아님 |
| 관리자/운영 | ADMIN/SUPER_ADMIN 서버 정책 + read/transition + 제한된 집계/host metrics capability | 7.2.9. 역할상승·탈퇴·삭제·승인 transaction과 Redis/Mongo/호스트 집계 oracle |
| UI 전수 | 원본 React 사본의 data access를 SDK로 전환, 현재 route 및 상태 유지 | 7.2.10. HAR/trace·기본 endpoint 고정. 원본 backend 종료 후 앱 E2E가 최종 gate |

입력 validation·인가·비용·최종 transaction은 AIP 서버가 소유한다. 외부 ML/YouTube/SMTP 전문 경계는
유지할 수 있으나 원본 업무 controller/service를 그대로 HTTP proxy한 결과와는 구분한다.
별도 PG job store와 MySQL business write를 쓴다면 업무 journal/outbox를 MySQL transaction에 묶고
relay 수락 receipt로 연결해야 한다. 이 구현·장애 gate는 adapter 작업에 남으며 두 DB atomic commit을 주장하지 않는다.

설계 관문: 원칙 1·2·3·7을 적용하고 client 표현은 유지한다. 서버 앱 코드의 반복을 resource·공식 capability로 모은다.
caller 권한/URL/실행계획은 신뢰하지 않는다. TS/Python SDK를 유지하고 별도 업무 DSL을 만들지 않는다.
IR 변경을 목표로 삼지 않으며 기존 OPEN 결정을 닫지 않는다. 새 provider 계약과 transport 구현은 각각의 실행 task에 귀속한다.

## 확인 과정에서 바로잡은 계획

원본 `WatchService.delete_comment`는 `comment.is_deleted=1`이다. 이전 WBS의 '원본 댓글 물리 DELETE'는 잘못된 전제였다.
`HomeService.toggle_bookmark`·`WatchService.toggle_like`의 취소는 `db.session.delete`다.
인증 password hash는 requirements에 bcrypt가 있어도 실제 서비스는 Werkzeug를 쓴다.
패키지명·이전 설명 대신 고정 source와 DB 결과를 비교 기준으로 삼는다.

실제 원본 API에서 댓글 생성 응답 ID가 null인 문제와, Redis blacklist에 기록한 refresh token의
재발급 replay가 허용되는 문제를 별도로 기록했다. 전자는 후속 조회의 실제 DB UUID로 검사를 이어가고,
후자는 포팅본에서 거부해야 할 보안 차이로 관리한다. 원본 access token의 만료까지 유지되는 수명과는 구분한다.

## 다음 실행 gate

7.1.2의 격리 Compose는 MySQL/Mongo/Redis를 loopback 전용 포트·tmpfs에서 실행한다.
원본 API 기준선은 [결과 JSON](facereview-api-baseline.json)에 저장한다.
API test client, 브라우저, socket 실제 ML, worker/provider를 구분하며 UI/ML/provider 미실행 상태에서는 환경 작업 전체를 done으로 세지 않는다.
재현 절차와 현재 확인 범위는 [BASELINE.md](BASELINE.md)에 둔다.

2026-10-15 후속: 7.1.3.1의 API 하위 gate는 23개 case의 계약·의미값과 12개 SQL/Redis 상태를
독립 oracle에 대조했다. 누락/중복/부분실행/기대값변조/저장오류를 unit 음성 control로 검사한다.
7.1.3 전체는 browser·Socket.IO/ML·worker/provider 기준선이 남아 있어 완료하지 않는다.

### 2026-10-17 원본 Socket.IO polling 부분 기준선

WBS 7.1.3.3.1은 고정 원본에서 허용된 로컬 Origin의 polling connect/disconnect와 빈 `watch_frame` 입력 거부를 확인했다.
실행 결과는 [facereview-socket-baseline.json](facereview-socket-baseline.json), 재현·범위는 [브라우저 기준선](BROWSER-BASELINE.md#원본-socketio-polling-입력-거부-기준선-2026-10-17)에 기록했다.
원본 결과와 AIP 구현은 혼동하지 않는다. 유효 프레임·WebSocket·ML·timeline·worker·provider는 미검증이며 7.1.3과 7.1.5.3 전체 gate는 계속 열린다.
이 부분 실행에서 AIP 본체의 제품 갭은 판정하지 않았다. 다음 포팅 단계에서 원본 기능과 AIP를 실제로 대조해 갭이 나타나면 코어 회귀로 고정한다.

7.1.5.1·7.1.5.2는 API 하위 증거 gate·반복 runner를 추가했다. 고정 원본 추출과 lock 확인→새 실DB 실행→
API/저장 oracle→자체 컨테이너 정리→증거 hash/exit/범위 검사를 한 명령으로 수행한다.
부분 결과·실패 exit·잘못된 scope·oracle/lock 불일치·누락/변조 파일은 성공으로 보고하지 않는다.
사용자 요청대로 작고 선행 효과가 있는 과제를 순서대로 처리하며 원본·화면·ML·인프라의 완료 조건을 줄이지 않는다.
남은 browser/runtime oracle와 전체 CI는 하위 WBS에 예정으로 유지한다.
