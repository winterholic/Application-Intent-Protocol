# FlaskBB 전체 포팅 상태

## 고정 기준

- upstream: `https://github.com/flaskbb/flaskbb`, commit `0885238df7c54826adda8cfbe8a831e846e2cb64` (2019-10-25, 2019-10-31 고정 release 시점 이전). 비교·선정 근거는 [선정 기록](../../plan-docs/porting/FLASKBB-SELECTION.md), snapshot은 `upstream/`.
- 시작한 AIP core: origin/main `d2c468dd30f258a9762365b213ca79ccbb34fe2c`; 이 작업 branch는 해당 SHA에서 분기했다. 후속 core SHA는 변경 때 갱신한다.
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
- route·endpoint 및 DB 구조 수집: 확인. [INVENTORY](INVENTORY.md)의 업무 의미 매핑은 진행 중이며 분모 변경은 source 근거와 함께 기록.
- AIP 현재 구현: `app.aip`에 사용자/그룹/카테고리/포럼/포럼 접근관계/주제/게시물의 읽기 모델과 그룹 ACL이 있다. `aip service check`, `service init`, principal bind, 실제 JWT 인증된 service, 실제 생성된 SDK의 `read` 요청으로 공개 게스트 포럼과 멤버 전용 포럼, 숨김 주제/게시물 권한을 독립 PG에서 시험했다. 복합 사용자-그룹 링크 중복도 DB에서 거부됨을 확인. 이 UI/API slice는 계속 확장 중이며 원래 FlaskBB UI는 아직 AIP SDK에 연결하지 않았다.
- AIP 전체 resource/read/apply/extension/인가와 원래 UI SDK 연결: 미완료.
- 전체 권한별 흐름, 상태 불변, 재시작 보존, 원본 backend 차단, 원본 대조: 미실행.
- clean checkout install/build/deployment/health/config/TLS 게이트: 미실행.
- 실제 SMTP/OIDC 계정 또는 공개 운영 배포: 수행하지 않음. 인증 흐름의 통합 설계는 제품 갭/결정 필요로 열어 둔다.

## 실행 경로 장애와 재현성 주의

`docker compose ... build app`은 이 Run의 macOS command environment에서 Docker BuildKit activity 파일 생성이 `operation not permitted`로 실패했다( Dockerfile 실행 전). 관측 출력은 “failed to solve: error creating temp file .../.docker/buildx/activity/...: operation not permitted”였으나 원 raw CLI log는 보존하지 못했다. 따라서 이 제한은 원문 로그가 아니라 Run 관측 기록에 근거한다. 이를 production Docker 부재로 일반화하지 않는다. 독립 경로로 OrbStack Linux 컨테이너에서 원본을 실제 install/run했다. 처음 slim-image apt install은 Debian mirror package 404, 이후 full Bullseye 공식 image에서 개발 헤더를 얻었다. 재현 경로 개선으로 Compose Dockerfile의 context COPY를 고치고, mutable upstream flask-allows branch 대신 관측한 commit SHA를 고정했다. 수정 Dockerfile은 아직 clean build 검증 전이다.

원본 public PostgreSQL schema에 대해 `aip service introspect --source-schema public`을 실제 실행했으나 `EXTERNAL_SCHEMA`로 거부됐다. DB catalog(`evidence/primary-keys.txt`)에는 `alembic_version.version_num` 자연키, 복합 PK(`forumsread`, `topicsread`)와 PK 없는 join tables가 있다. 현재 introspector는 단일 숫자/UUID PK만 자동 채택한다(`crates/aip-migrate/src/external.rs`). 이 작업의 저장소는 AIP 소유 schema로 만들고 ForumGroupAccess/UserGroup 같은 링크에 generated Id + unique pair를 둔다. 원본 데이터 import는 별도 변환으로 진행하며 이 거부를 원본 기능 축소 근거로 쓰지 않는다. Full external schema introspection output은 `evidence/introspect-full.log`.

F01 read regression은 [porting/flaskbb/tests/aip-read.test.mjs](tests/aip-read.test.mjs)에 있다. `aip-read.log`는 Linux PostgreSQL 14.18 isolated DB `aip_flaskbb`/host `55439`, dynamic owned schemas, local ephemeral RS256 test issuer/JWKS, actual AIP SDK read, role-dependent rows, duplicate relationship rejection 기록이다. TDD 초기 실행에서 회원이 guest group의 포럼까지 읽는 정책 버그가 드러났고 predicate를 바로잡아 재실행 PASS. 원본 baseline DB는 별도 `flaskbb_baseline`/port `55438`에 계속 격리.

초기화 상세, 명령 원문·raw output, 오류 캡처를 이 경로에 계속 누적하고 다음 상태 갱신 때 SHA-256을 함께 남긴다. 이 README/STATUS 변경은 기존 FaceReview UI blocker, MySQL adapter blocker, IdP 3/17 + 14 미실행 상태를 닫지 않는다.
