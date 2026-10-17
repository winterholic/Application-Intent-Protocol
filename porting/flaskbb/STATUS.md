# FlaskBB 전체 포팅 상태

## 고정 기준

- upstream: `https://github.com/flaskbb/flaskbb`, commit `0885238df7c54826adda8cfbe8a831e846e2cb64` (2019-10-25, 2019-10-31 고정 release 시점 이전). 비교·선정 근거는 [선정 기록](../../plan-docs/porting/FLASKBB-SELECTION.md), snapshot은 `upstream/`.
- 시작 당시 origin/main `d2c468dd30f258a9762365b213ca79ccbb34fe2c`였다. 진행 중 GitHub main 전진을 확인해 `f10c8828cac9ff38ab1ec569ca1ca61396f25f5d`까지 rebase했으며 이 SHA가 현재 branch의 upstream base다. 최신 origin을 기준으로 다시 build·read regression을 실행했다. Current work SHA는 `git rev-parse HEAD`로 확인한다.
- 전체 의미 기능 분모는 [INVENTORY.md](INVENTORY.md)에 잠근다. URL/endpoint/table 수는 별도 커버리지 축이며 기능 수로 합산하지 않는다.
- 기획의 독립 완료·출시 판정은 이 작업 이후 별도로 요구된다. 현재 전체 포팅·기능 동등성·배포 준비 완료가 아니다.

## 실행 환경과 원본 고정본 실제 결과

OrbStack Docker 29.4.0의 Linux/aarch64에서 실행. Python 3.7.17 Bullseye image digest는 `f36f6fe9ddb2622aab9803944cbe7e2bc50be75d624a081f7895e60e7c6b7f0a`. 공개 upstream은 read-only clone에서 고정 SHA로 export했다. 컨테이너 안 `/opt/flaskbb`는 실행용 사본이며 upstream snapshot은 수정하지 않았다.

전용 Compose DB `flaskbb_baseline` (PostgreSQL 14.18, host loopback `55438`)과 Redis 5.0.14 (`56379`), 원본 앱 (`58080`)만 사용한다. host PostgreSQL 5432는 사용하지 않았다. DB 데이터는 Postgres 컨테이너 tmpfs. 앱의 fixture SMTP 발송은 `MAIL_SUPPRESS_SEND=True`로 막았다.

기준선 명령/관측:

```sh
docker compose -f porting/flaskbb/compose.yaml up -d postgres redis
# upstream commit을 /upstream(ro)로 bind mount한 python:3.7.17-bullseye Linux 컨테이너에서
python -m venv /opt/flaskbb-venv
/opt/flaskbb-venv/bin/pip install -r /opt/flaskbb/requirements.txt
/opt/flaskbb-venv/bin/pip install -r /runtime/requirements.extra.txt
PATH=/opt/flaskbb-venv/bin:$PATH flaskbb --config /runtime/flaskbb.cfg install --force --initdb --populate
PATH=/opt/flaskbb-venv/bin:$PATH flaskbb --config /runtime/flaskbb.cfg populate --force --initdb --test-data
PATH=/opt/flaskbb-venv/bin:$PATH flaskbb --config /runtime/flaskbb.cfg urls
PATH=/opt/flaskbb-venv/bin:$PATH flaskbb --config /runtime/flaskbb.cfg run --host 0.0.0.0 --port 5000
```

`flaskbb urls` 관측: route rules 120 (alias/static/theme 포함), distinct endpoints 91. PostgreSQL 업무 테이블 19개와 migration 관리 `alembic_version` 1개. 근거 원본 출력은 `evidence/routes.txt`, `evidence/tables.txt`, `evidence/seed-counts.txt`. seeded roles로 test1(Administrator) 로그인 후 포럼 홈과 `/admin/` Overview의 Reports/Users/Groups/Forums/Settings/Plugins 영역을 configured `ttakkari_browser` MCP로 실제 렌더링했다. 공개 홈·portal·login·memberlist 200, 미인증 admin 302를 관측. 화면 캡처·접근성 snapshot은 이 Run outbox `browser/`에 자동 저장됐고, console errors 0. 아직 전체 route/action/권한 조합을 시험한 것은 아니다.

호환 차이(전부 runtime-only): 원본 의존성의 `flask-allows` branch를 설치 시점 commit `ee303da3f5055b3b3d66ece2316dd93976a26b33`으로 고정; PostgreSQL 드라이버 누락 보충 `psycopg2-binary==2.8.6`; Kombu 4.6.5와 최신 metadata의 비호환(`EntryPoints.get` 제거) 때문에 `importlib-metadata==4.13.0`; localization CLI에 venv `PATH`; 앱 URL이 404를 내던 합성 `SERVER_NAME=localhost:8000` 제거(원본 기본 설정에는 없음). 수정 원문 diff는 없다. `compat/`에는 고정 추가 dependency가 있다.

## 완료·미완료

- 원본 고정/출처 검토: 수행. 정확한 저자·시점 이력은 선정 기록 참고; 100% human-authorship 증명은 불가.
- 원본 전체 앱 설치/기동/초기 UI 기준선: 확인. 메일 전달·모든 UI 및 업무흐름은 미검증.
- F02 원본 인증 기준선: 고정 FlaskBB source에서 username 또는 email + Werkzeug password hash 로그인, registration uniqueness/blacklist validators, `ACTIVATE_ACCOUNT`에 따른 대기/활성화, 한 시간 operation-scoped activation/reset token, reset email, password re-auth를 확인했다. 실제 원본 runtime browser에서 test1 로그인→인증 사용자 UI→로그아웃, 가입 form과 forgot-password form을 확인했다. dedicated baseline PG 설정은 registration/activation 모두 true이고 해당 baseline runtime config는 `MAIL_SUPPRESS_SEND=True`라 이 인스턴스에서는 가입/activation/reset 메일 효과가 없다. 별도 mail-enabled 앱/DB의 실행 결과를 아래 bullet에 분리 기록하며, suppress된 baseline은 메일 증거로 세지 않는다. 상세 명령·화면 파일은 `evidence/verification.log`에 기록했다.
- F02 실제 메일·token 흐름(원본 앱 코드, 별도 검증 DB): 별도 `flaskbb_auth` DB와 앱 인스턴스, Mailpit SMTP 수신함을 구성하고 `MAIL_SUPPRESS_SEND=False`, Celery eager 설정으로 browser 가입을 실행했다. `auth_new` 계정의 activation 메시지가 Mailpit에 도착했고 그 메시지 링크를 소비해 DB `activated=true`와 활성화 완료 UI를 확인했다. 이어 password recovery 메시지가 도착했고 reset token으로 변경한 비밀번호가 실제 `auth_new` 로그인에 쓰여 사용자 nav를 확인했다. 원본 `flaskbb_baseline` DB/config는 바꾸지 않았다. 중복/차단 validator, reauth, resend, token expiry/single-use, clean reproducible container build는 미실행이다. 검증용 app은 이 Run의 원본 runtime container를 local commit한 임시 image에서 실행했으므로 재현 가능한 배포 gate를 충족하지 않는다.
- F02 AIP 경계: RFC 9068 section 2.2의 required claim 목록에는 `nbf`가 없다([RFC 9068](https://www.rfc-editor.org/rfc/rfc9068.html)). test-first 회귀로 AIP가 이를 필수로 요구하던 호환성 버그를 수정했다. 실제 Keycloak 26.2.5 test realm의 TLS issuer에서 `typ=at+jwt`, `iss`, `aud`, `sub`, 300초 수명의 서명 token을 다시 발급했고, 수정된 실제 AIP 서버에 공식 Python SDK로 `/session`→actor 6, capability discovery→`ok=true`, `User` read→`auth_new`/`activated=true`까지 성공했다. principal은 운영 CLI로 미리 연결한 synthetic 기존 actor라 새 가입자 자동 온보딩 증거는 아니다. 실제 브라우저 인증 code/PKCE는 미시험이다. 현재 격리 서버 설정은 HTTPS에서 내려 받은 공개 JWKS 파일을 사용했고 remote JWKS HTTPS fetch는 아직 미검증이다. 새 IdP subject를 AIP actor에 자동·안전하게 연결하는 제품 경로가 없어 온보딩은 핵심 제품 갭으로 남는다. 원본 비밀번호 UX를 OIDC-only라는 이유로 미정 처리하지 않는다.
- route·endpoint·CLI·plugin·메일/job·storage/search/assets source family를 [INVENTORY](INVENTORY.md)에 전수 분류해 fixed upstream + 설치된 plugin 기준 분모 v0.1을 잠갔다. 고정 route map의 91 distinct endpoint 각각을 업무 F ID에 매핑했다. 행별 원본 runtime 대조·인가/오류/빈 상태·DB 결과 전수 검증과 AIP 이식은 여전히 미완료이며, 추가 source 발견 시 새 versioned amendment로 분모에 반영한다.
- AIP 현재 구현: upstream 19개 앱 도메인 resource를 선언했고 Topic/Post/TopicFirstPost와 원자 생성·reply·edit·moderation 한 부분을 실제 SDK/서버/격리 PostgreSQL로 검증했다. Forum/User/Topic 카운터 일부는 role-visible aggregate다. 모든 resource의 변환·인가·CRUD는 미완료이며, Forum `last_post*`와 Topic 최신 활동 projection도 미완료다. 선언 수나 이 vertical slice를 전체 데이터 이식으로 부르지 않는다.
- F05 실제 AIP workflow slice: UUID Topic → UUID Post → Post.publish → 고유 TopicFirstPost relation → Topic.publish를 한 원자 bundle에서 검증. Reply create/publish/bump 및 edit/version conflict도 검증. Moderator lock/unlock/highlight/trivialize와 Topic/Post hide/unhide, member reply soft-delete, readonly 거부를 검증했다. 원본의 first-post 삭제는 CanDeletePost(=CanEditPost) 통과 후 topic 전체를 ORM cascade로 제거한다. AIP slice는 Topic.delete + 첫 Post.delete tombstone을 같은 atomic bundle에서 실행하고, 원본 첫 글 작성자의 editpost 권한도 허용한다. 삭제된 부모 아래의 Post는 해당 topic을 지울 수 있는 작성자/관리자만 정리 목적으로 읽을 수 있고 aggregate에서 제외한다. 이 저장 의미는 원본 물리 삭제와 다르며 실제 DB상태 의미 차이를 후속 gate에서 확정해야 한다. Forum last-post projection은 아직 구현되지 않았다.
- F06 partial: TopicTracker의 본인 토픽 등록·해제·재등록을 SDK atomic create/apply로 추가했고 타인의 소유자로 생성하려는 변경은 거부됨을 확인했다. AIP는 원본이 association 행을 지우는 대신 `active=false` 행을 남긴다. 이를 원본과 같은 저장 의미로 보지 않는다. AIP 노출은 `active` 필터를 지원한다. `ForumsRead`/`TopicsRead` 기록 생성·갱신, 포럼 하나/전체 mark-read, TRACKER_LENGTH·마지막 게시물 기준 unread 계산, 읽음 위치, tracker list 전체의 정렬·페이지/UI 흐름은 남아 있다.
- F08 partial: 로그인 회원의 읽을 수 있는 게시물 신고, reporter 본인 설정, submit 시 서버 `now`, moderator-only queue read/resolve(`zapped`·`zapped_by`)를 actual SDK/PG에서 시험했다. 신고자는 제출된 신고를 조회하거나 처리할 수 없고, admin이 상태를 기록했다. 미읽음/전체 목록의 원본 정렬·페이지, moderator group scope, 단건/복수 신고 삭제와 원본 UI는 아직 이식·검증되지 않았다.
- 최신 AIP regression: 2026-10-17 F06/F08 증분 후 `target/debug/aip service check porting/flaskbb/app.aip` → `ok:true`, advisories `[]`; `node --test porting/flaskbb/tests/contract.test.mjs porting/flaskbb/tests/aip-read.test.mjs` → 2 passed, 0 failed (isolated PostgreSQL 14.18, dynamic schema, actual AIP server/SDK). 출력·해시를 `evidence/verification.log`에 누적한다. 이 결과는 전체 기능 동등성/출시준비 gate가 아니다.
- AIP 전체 resource/read/apply/extension/인가와 원래 UI SDK 연결: 미완료. 현재 UI는 여전히 FlaskBB backend를 향한다.
- 전체 권한별 흐름/재시작 보존/원본 대조: 미실행. F05 slice 안의 거부 후 불변, bundle rollback, idempotent replay 및 숨김/삭제 권한 일부만 확인.
- 원본 UI의 AIP SDK 연결과 원본 업무 backend 중단 gate: 미실행.
- clean checkout install/build/deployment/health/config/TLS 게이트: 미실행.
- 실제 원본 SMTP/mail/token lifecycle: 격리 Mailpit과 DB에서 확인. 실제 Keycloak password-grant test user access token으로 bound synthetic actor의 AIP SDK `/session`, capability discovery, User read가 통과했다. 이는 원본 FlaskBB 화면의 AIP 전환 또는 browser authorization-code/PKCE 로그인 증거가 아니다. 새 사용자의 안전한 AIP actor/principal 온보딩, 원본 UI→SDK 연결, 원격 JWKS TLS 로더, 메일 이후 IdP/AIP 계정 lifecycle, 공개 운영 배포는 미완료.

## 실행 경로 장애와 재현성 주의

`docker compose ... build app`은 이 Run의 macOS command environment에서 Docker BuildKit activity 파일 생성이 `operation not permitted`로 실패했다( Dockerfile 실행 전). 관측 출력은 “failed to solve: error creating temp file .../.docker/buildx/activity/...: operation not permitted”였으나 원 raw CLI log는 보존하지 못했다. 따라서 이 제한은 원문 로그가 아니라 Run 관측 기록에 근거한다. 이를 production Docker 부재로 일반화하지 않는다. 독립 경로로 OrbStack Linux 컨테이너에서 원본을 실제 install/run했다. 처음 slim-image apt install은 Debian mirror package 404, 이후 full Bullseye 공식 image에서 개발 헤더를 얻었다. 재현 경로 개선으로 Compose Dockerfile의 context COPY를 고치고, mutable upstream flask-allows branch 대신 관측한 commit SHA를 고정했다. 수정 Dockerfile은 아직 clean build 검증 전이다.

원본 public PostgreSQL schema에 대해 `aip service introspect --source-schema public`을 실제 실행했으나 `EXTERNAL_SCHEMA`로 거부됐다. DB catalog(`evidence/primary-keys.txt`)에는 `alembic_version.version_num` 자연키, 복합 PK(`forumsread`, `topicsread`)와 PK 없는 join tables가 있다. 현재 introspector는 단일 숫자/UUID PK만 자동 채택한다(`crates/aip-migrate/src/external.rs`). 이 작업의 저장소는 AIP 소유 schema로 만들고 ForumGroupAccess/UserGroup 같은 링크에 generated Id + unique pair를 둔다. 원본 데이터 import는 별도 변환으로 진행하며 이 거부를 원본 기능 축소 근거로 쓰지 않는다. Full external schema introspection output은 `evidence/introspect-full.log`.

F01 read regression은 [porting/flaskbb/tests/aip-read.test.mjs](tests/aip-read.test.mjs)에 있다. `aip-read.log`는 Linux PostgreSQL 14.18 isolated DB `aip_flaskbb`/host `55439`, dynamic owned schemas, local ephemeral RS256 test issuer/JWKS, actual AIP SDK read, role-dependent rows, duplicate relationship rejection 기록이다. TDD 초기 실행에서 회원이 guest group의 포럼까지 읽는 정책 버그가 드러났고 predicate를 바로잡아 재실행 PASS. 원본 baseline DB는 별도 `flaskbb_baseline`/port `55438`에 계속 격리.

초기화 상세, 명령 원문·raw output, 오류 캡처를 이 경로에 계속 누적하고 다음 상태 갱신 때 SHA-256을 함께 남긴다. 이 README/STATUS 변경은 기존 FaceReview UI blocker, MySQL adapter blocker, IdP 3/17 + 14 미실행 상태를 닫지 않는다.
