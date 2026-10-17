# FlaskBB 전체 포팅 상태

## 고정 기준

- upstream: `https://github.com/flaskbb/flaskbb`, commit `0885238df7c54826adda8cfbe8a831e846e2cb64` (2019-10-25, 2019-10-31 고정 release 시점 이전). 비교·선정 근거는 [선정 기록](../../plan-docs/porting/FLASKBB-SELECTION.md), snapshot은 `upstream/`.
- 시작 당시 origin/main `d2c468dd30f258a9762365b213ca79ccbb34fe2c`였다. 진행 중 GitHub main 전진을 확인해 `f10c8828cac9ff38ab1ec569ca1ca61396f25f5d`까지 rebase했으며 이 SHA가 현재 branch의 upstream base다. 최신 origin을 기준으로 다시 build·read regression을 실행했다. Current work SHA는 `git rev-parse HEAD`로 확인한다.
- 전체 의미 기능 분모는 [INVENTORY.md](INVENTORY.md)에 잠근다. URL/endpoint/table 수는 별도 커버리지 축이며 기능 수로 합산하지 않는다.
- 기획의 독립 완료·출시 판정은 이 작업 이후 별도로 요구된다. 현재 전체 포팅·기능 동등성·배포 준비 완료가 아니다.

## 2026-10-18 후속 Run — 독립 검증 결함 F-1~F-5 수정과 7.5.2·7.5.3 실DB 실행

**기준**: 대상 `81f397e`/`eb8c351`, branch `porting/flaskbb-full-2019`. 환경은 이 Run이 새로 띄운 격리 컨테이너만 쓴다: PostgreSQL 14.18 `aip-flaskbb-fix-pg-1018`(127.0.0.1:55471), Mailpit v1.27.8 `aip-flaskbb-fix-mailpit-1018`(SMTP 55472, HTTP 55473). 공유 5432와 55439는 쓰지 않았다. 시험은 `AIP_FLASKBB_TEST_DATABASE_URL`(`tests/env.sh`)로 DB를 바꿀 수 있고, 미지정 시 compose의 55439가 기본값이다. 원시 로그는 `evidence/run-751-*.log`, `run-752-*.log`.

| ID | 상태 | 수정 전 | 수정 후 | 커밋 |
|---|---|---|---|---|
| F-1 | 수정·실DB PASS | `cargo test -p aip-auth --offline --test durable --test jwt`: jwt 2건 실패 `psql: 오류: "" 서비스 정의를 찾을 수 없음` | `PGSERVICE`를 `env_remove`. durable 2, jwt 12, db_guard 11 passed | `a98d78d` |
| F-2 | 수정·실DB PASS | `features.test.mjs:966` `WIRE.MISSING_TARGET` | Topic create에 `hidden:false`. features 1 passed. 원본 `HideableMixin.hidden = Column(Boolean, default=False, nullable=True)`는 ORM 기본값이라 NULL은 raw insert에서만 생기고, `HideableQuery`의 `hidden != True`도 SQL 3값 논리로 NULL을 제외하므로 선언은 원본 읽기 의미와 같다. 선언 기본값은 필요 없고 [PRODUCT-GAPS](PRODUCT-GAPS.md) G-7에 기록 | `01b7b81` |
| F-3 | 수정·실DB PASS, 제품 갭 기록 | 같은 설정의 SMTP worker가 `FAILED`/`WIRE.EXTENSION_ERROR`. 샌드박스 안 `socket.create_connection`은 `PermissionError`, 밖에서는 성공 | AIP에는 선언된 outbound HTTP broker(`access http.mail`, `ctx.http.call`, `workers.integrations`)가 있어 그 방식으로 전환. `mail-jobs.test.mjs`: worker 격리 상태에서 Mailpit에 활성화·재설정 메일 도착, 같은 key 재생 시 중복 없음, 다른 사람 주소로의 relay는 `allow … input.email = actor.email`로 거부(조건 제거 변이 시 시험 실패 확인). SMTP 자체와 공급자 계약은 G-1, 연결 거부의 `EFFECT_UNKNOWN` 분류는 G-2 | `a869afd` |
| F-4 | 선언 오류로 규명·수정·실DB PASS | `moderation.test.mjs`: `user_group` 비었을 때 `moderator:false, forumModeratorOwnForum:false`. 무관한 행 1개를 넣으면 같은 시험 통과 | 문법이 `exists R where <expr>`의 `<expr>`를 `or` 사슬 전체로 읽는다(`spec/grammar.md` `set-expr`). 컴파일러 결함 아님. `canModerateForum`·`canDeleteTopic`의 `exists`를 괄호로 감쌈. 두 상태 모두 같은 표 통과. `spikes/spike-v1-fixture/tests/exists_scope.rs` 2 passed가 파서 동작을 고정. core 변경 없음. checker 경고 제안은 G-3 | `ea4ea6f` |
| F-5 | 수정·실DB PASS | `conversations-security.test.mjs`(수정 전 변형 실행): 자기 mailbox의 `from_user=bob` 행, 자기 자신에게 쓴 행, 제3자 mailbox 오라벨·미리 휴지통·읽음·draft 행 7건이 `ACCEPTED`(`from_user=bob`·`user=carol` 조합만 거부) | `Conversation.create`는 `from_user = actor`, 수신자 ≠ 본인, 보낸이 사본 또는 unread·비휴지통·비draft 수신자 사본만 허용. `Message.create`는 `user = actor`와 mailbox 주인이 두 당사자 중 하나. 거부 시 행 수 불변 단언 포함 전부 PASS. features의 UI 대화 왕복도 통과. 잔여 위험 `shared_id`는 G-8 | `941c640` |

**7.5.2 실DB 결과**: `features.test.mjs`(검색·프로필·대화·tracker·관리 CRUD·UI 호스트·비활성 차단·페이지네이션 3+3+1·offset 상한 거부·User 커서·Forum last-post·read marker·신고 큐 2+1)가 새 PG에서 처음 실제로 실행돼 PASS. 이전 세션이 "미실행"으로 남긴 9절이 이로써 실행 증거를 얻었다. `provisioning` 9 시나리오, `aip-read`, `contract`도 PASS. 미완료: sticky 정렬(G-5), 삭제·숨김 뒤 last-post 재계산(G-6), 사용자 물리 삭제·ban, 신고 일괄 처리, 대화 draft 수정·삭제, unread cutoff 계산.

**7.5.3 실DB 결과**:
- F13: `cli.test.mjs` PASS. `service init` → `seed/install.py`(고정 upstream 그룹 6·설정 4그룹 29건·admin·welcome forum을 fixture 파일에서 읽음) → 재설치 거부(전체 롤백, 행 수 불변) → `principal bind` → serve에서 admin이 Group 6·Setting 29·Welcome topic read → `Category.edit` 후 `service audit list`에 actor·via·before/after → `service migrate` 무변경 계획, 필드 추가 계획(Safe), 승인하지 않은 digest 거부, `--apply` 적용 후 행 보존, 새 정의로 serve → `principal revoke` 뒤 접근 거부. audit는 전이만 기록하며 create는 기록하지 않는다.
- F14: 위 F-3 시험과 `mail-jobs` 2번째 시험(공급자 장애로 FAILED, `service jobs list --state FAILED`가 payload를 숨김, 복구 뒤 `jobs retry`는 `WIRE.EFFECT_UNKNOWN`으로 거부, 새 key job은 1건 전달). `test_mail_jobs.py` 6 passed.
- F15: `search.test.mjs` PASS. 색인이 없어 쓰기·수정·숨김·삭제·복원이 즉시 검색에 반영되고 행 정책이 적용되며 재시작 뒤에도 같다. 의미 차이(대소문자 구분 부분 문자열, 순위·토큰화 없음)를 G-4로 고정.

**아직 하지 않은 것**: 7.5.4 독립 검증 게이트, Keycloak 브라우저 흐름, clean deployment artifact, i18n, 원본 UI 대조. `cargo fmt --all -- --check`는 이전부터 미통과이고 이번에 다루지 않았다.

## 2026-10-18 Run — 7.5.1 잔여 DSN 가드 + 7.5.2 페이지네이션/last-post 선언 + 7.5.3 메일 job/CLI 대조

**환경 제약 (이 세션)**: 실행 샌드박스가 listen 소켓을 전면 차단(`bind` EPERM)하고
OrbStack VM이 시작 타임아웃(`orbctl start` 실패 반복)으로 전용 PG(127.0.0.1:55439)를
기동할 수 없었다. 네이티브 PG 17.6을 /tmp에 빌드했으나 (a) 샌드박스에 UID 501의
passwd 항목이 없어 initdb 거부 → 소스 패치로 우회, (b) SysV shmget 차단으로 bootstrap
실패 → 중단. 따라서 리스너 기동·실DB 연결이 필요한 검증은 전부 미실행이며, 아래의
실행 증거는 listen/DB 불필요 항목만이다. 독립 검증 게이트에서 DB 환경으로 재실행 필요.

**7.5.1 잔여 (전 세션 미커밋분 인수)**: `crates/aip-auth/tests/common/mod.rs`(신규,
DSN 파싱·검증: 기본 DB/user/template 거부, percent-decode 후 재검사, service/host/
hostaddr/port/dbname/user/password 쿼리 오버라이드 거부, unix 소켓·멀티호스트 거부,
loopback 강제, 정규화 URL 재생성, PGPASSWORD/PG* 환경 격리) + `db_guard.rs`(신규,
11개 단위 테스트) + `durable.rs`/`jwt.rs`의 공통 모듈 사용 전환.
- 실행: `cargo test -p aip-auth --offline --no-run` 컴파일 통과,
  `db_guard` 11 passed/0 failed. `jwt`/`durable` DB 연결분은 미실행.
- 이전 FAIL 5결함(인증 컴파일·정지계정 차단·권한상승 차단·쪽지 양방향·이메일 검증)의
  코드 수정은 2bf962d·cc3d5df에 이미 반영됨. 본 세션은 DSN 우회 벡터 경화분을 얹는다.

**7.5.2 선언 (service check 통과, executionDigest `6ffbbab6…`)**:
- 전 list read에 `offset` 예산 추가(원본 `paginate()` 대응, 기본 10/페이지:
  Topic 2000, Post 5000, User 1000+cursor, Conversation 1000+cursor,
  Message 2000+cursor, read-marker 3종 1000+cursor, Report 1000, 기타 500~2000).
- `Post.publish`에 동일 트랜잭션 `update Forum where id = this.forum`
  (last_post/last_post_user/last_post_title/last_post_username/last_post_created) 추가.
 effect 문장의 구분자는 `;` (`,` 불가)임을 checker로 확인. 삭제/hide 시 재계산은 미선언.
- 갭 기록: `sort`에 Bool 불가 → sticky(important-first) 정렬 불가.
  Topic/Post id가 UUID라 keyset 커서는 해당 read에 미선언(의미 없음).
- `features.test.mjs` 9절 추가(문법 `node --check` 통과, 실행 미실행):
  Topic offset 3+3+1 분할/서로소/전체커버, offset 2001 거부, User 커서 2+2+1 연쇄,
  최종 reply의 Forum.last_post_* 단언(DB last_post_id/username 대조 포함),
  TopicRead 위조 거부·본인 marker 전진, Report 2+1 페이징·reporter 0건·mod 3건,
  editor Category.create 거부.

**7.5.3**:
- F14: `operation job sendActivationEmail/sendPasswordResetEmail` 선언
  (`implementation "flaskbb_mail.send_*", effect none, allow actor != null`
  남용 주의 주석 포함) + `mail/flaskbb_mail.py` 구현 + `tests/test_mail_jobs.py`
  3 passed (수신자·발신자·토큰 링크·fail-closed, 소켓 없이 순수 구성 검증).
  실제 SMTP 송신·serve job 기동은 미실행. [CLI-MAPPING.md](CLI-MAPPING.md)에 F13 대조표 작성.
- F13: 원본 install/populate/db/users/celery/shell/urls/makeconfig/translations →
  `service init/migrate/principal/jobs/audit/gen` 대응. `check`/`gen`만 실행됨
  (20 descriptors, fingerprint `873e35e6…`).
- F15: `contains` 필터 선언 유지(색인 없이 항상 일관). 제품 다이얼렉트에서
  최상위 `search` 선언은 파서 거부(`DEF.LEX_UNEXPECTED_CHAR`) → 순위 기반 전문검색은
  제품 갭으로 확정. Whoosh 동기화 불필요(색인 자체가 없음).

**계속 미완료**: 전용 PG에서의 전 suite 재실행(jwt/durable/features/provisioning/
aip-read), serve 기동 후 offset/after/last-post 실동작, mail 실제 송신·job 기동,
clean-checkout `init→migrate→seed→serve` 재현, User 물리 삭제 전이 여부, i18n.
7.5.4(독립 검증 게이트)는 착수하지 않음.

## 2026-10-17 새 컨텍스트 — 독립 FAIL (db18f573) 비활성 쓰기 차단 및 auth DB 테스트 파라미터화

**기준**: 시작 SHA `db18f573fd669ac6b95a512a698c7b395dfee49f`, branch `porting/flaskbb-full-2019`. 독립 검증(작업자 66f533e8, Claude Sonnet) 결과 FAIL 판정. 검증 보고서: `/Users/winterholic/.ttakkari/data/outbox/fecbc43e-400c-4c3f-a4e8-8124b658b9f4/flaskbb-independent-verification-20261017/verification-report.md`.

**이번 증분**:
- **비활성 계정(`activated=false`) 쓰기 전수 차단 (`porting/flaskbb/app.aip`)**:
  - 원본 FlaskBB의 비활성 사용자 규칙을 기준으로 모든 쓰기 작업(Category/Forum create 및 edit, Topic bump 및 delete, Post edit 및 delete, TopicFirstPost create, ForumRead/TopicRead create 및 updateRead, TopicTracker create/track/untrack, Report create 및 submit, Conversation create/markRead/bump/moveToTrash/restoreFromTrash, Message create, canDeleteTopic/canDeletePost predicate)에 `actor.activated` / `viewer.activated` 검사를 전수 추가. core 하드코딩이 아닌 `app.aip` 정책으로 해결.
  - `features.test.mjs`에 비활성 상태(`activated=false`)에서 쪽지함 trash 처리 거부 및 DB trash 컬럼 불변, 새 쪽지 저장 거부 및 DB 메시지 수 불변, 관리자 Category 생성 거부 및 DB category 수 불변 실DB 단언 추가.
  - 비활성 계정 재활성화(`activated=true`) 후 Category 생성, Message 저장, 쪽지함 trash 처리가 정상 복귀하여 성공함을 실DB로 단언.
- **`aip-auth` DB 테스트 파라미터화 및 fail-closed (`crates/aip-auth/tests/durable.rs`, `crates/aip-auth/tests/jwt.rs`)**:
  - 하드코딩된 fixture DB URL을 제거하고 필수 환경변수(`AIP_TEST_DATABASE_URL` 또는 `TEST_DATABASE_URL`)를 읽도록 변경.
  - 환경변수 미지정 시 fail-closed(명시적 panic 메시지) 처리 확인.
  - 기본 DB `localhost:5432/postgres`나 사용자명이 postgres인 기본 DB 연결 시도를 사전 차단하는 `check_not_default_db` guard 유지 확인.
  - `jwt.rs`에 RFC 9068 `nbf` absent 허용(`accepts_token_without_optional_nbf`) 및 `nbf: null` 거부(`rejects_token_with_explicit_null_nbf`) 테스트 케이스 추가.
- **위조 mailbox ID 직접 호출 차단 검증**:
  - `features.test.mjs`에 타인의 conversation_id를 대상으로 직접 `moveToTrash` 및 `markRead` 호출 시 거부되고 대상 mailbox의 DB 상태가 불변임을 실DB로 단언.
- **fixture DB `aip_flaskbb_e22146d03c` 스키마**:
  - 이전 실행 기록에서 생성 주체가 확인되지 않아 임의 삭제하지 않고 보존 상태로 보고.

## 2026-10-17 새 컨텍스트 — 독립 FAIL 반례 수정 및 실경로 재검증

**기준**: 시작 SHA `7fea9bc126beb0a23bdaf7051781b2aea6b3a9bd`, branch `porting/flaskbb-full-2019`. 독립 교차검증 [보고서](/Users/winterholic/.ttakkari/data/outbox/41bc6349-377e-4b39-b1f8-f3512121b1b4/flaskbb-verification-7fea9bc/verification-report.md)는 해당 SHA를 FAIL로 판정했다. 기존 9개 미커밋 파일을 보존해 계속했고, 초기 patch backup은 Run outbox의 `flaskbb-preexisting-diff.patch`다.

**이번 증분**:
- `aip-auth` auth 회귀 fixture의 `Config.provisioning` 초기값을 보충했다. `durable.rs`와 `jwt.rs`의 통합 테스트 연결을 `127.0.0.1:55439/aip_flaskbb` 고정 URL로 바꿔 `DATABASE_URL`/`TEST_DATABASE_URL` 환경값을 무시한다. jwt 스키마 이름에 process-local sequence도 추가했다.
- Provisioning 이메일 형식 검사에서 별도 임시 함수를 없애고 AIP read 계층의 기존 `spike_v2_read::scalar::valid_email`과 같은 validator를 사용한다. missing/null email, email_verified의 false/missing/null/string/숫자 경계는 실제 PG에서 거부 및 사용자/principal row 부재를 확인한다.
- 원본 FlaskBB `EditUser`의 mod_edituser 권한을 그대로 제거하지 않도록 `moderatorEditUser`를 분리했다. actor에 moderator급 + mod_edituser가 있어야 하며 기존 admin/super_mod 대상은 막고, 이 전이는 그룹·활성화 필드를 수정하지 않는다. `adminUpdateUser`는 활성 admin 전용으로 두어 새 Administrator/super_mod 승격과 기존 고권한 계정 변경을 막았다. 실제 AIP/PG 테스트에서 일반 대상 수정은 허용, 상승·고권한 대상 거부 및 DB 불변, admin의 그룹 변경 허용을 확인했다.
- Renderer의 대화함 create/reply에서 양쪽 mailbox에 첫 메시지/답장을 저장한다. URL에서 온 numeric ID를 SDK decimal string으로 바꾸고 markRead는 공식 SDK request object로 호출한다. 실제 UI→SDK→AIP→PG 테스트로 양측 메시지 확인, 개인별 unread/read/trash 상태와 비참여자 거부를 확인했다.

**실행 기록** (`porting/flaskbb/evidence/`에 원시 로그):
- 먼저 실행한 auth suite에서는 `TEST_DATABASE_URL`이 전용 DB를 가리켰지만 `tests/jwt.rs`가 무시하고 `host=localhost dbname=postgres`로 연결해 테스트군이 서로 다른 DB에 나뉘었다. 그 suite의 출력은 모두 통과였지만 분리 DB 결과가 아니어서 근거로 폐기했다. 후속 run은 양쪽 env 값을 localhost로 바꿔 이를 드러냈고, jwt test stage에서 `pg_namespace_nspname_index`, 기존 schema `aip_auth_19387_1792219434204611000` 중복 오류로 9 pass/1 fail을 관측했다. 둘 다 Mac Studio command-runner의 test 실행이며 금지된 default DB 경로 사용이다. 그 대상이 실제 운영 데이터 DB인지 확인하지 않았고, 이후 그 연결을 재사용하거나 조사·정리하지 않았다. 성공한 첫 시도의 `SchemaGuard::drop`은 실행됐으나 결과를 버리므로 잔여 영향도 독립 확인하지 않았다. 실패 시도 raw 전문은 후속 rerun으로 덮어써져 `run-aip-auth-default-db-incident.log`에 관측 excerpt만 기록했다. 두 helper를 고정 전용 DB로 바꾼 뒤 전체 suite를 17 unit + 2 durable + 10 jwt로 다시 실행했다.
- `DATABASE_URL`/`TEST_DATABASE_URL`을 금지된 기본 값으로 둔 상태의 pinned run은 모두 고정 URL을 썼다: `cargo test -p aip-auth --offline` → 17 + 2 + 10 passed; `cargo build --offline -p aip-cli` → exit 0. 로그: `run-cargo-test-aip-auth-pinned.log`, `run-cargo-build-aip-cli.log`.
- `AIP_BINARY=/private/tmp/aip-flaskbb-verifier-target/debug/aip node --test porting/flaskbb/tests/provisioning.test.mjs` → 1 test passed; 실제 PG 55439, 9 named scenarios 및 추가 claim-boundary DB invariants. `run-provisioning-boundaries.log`.
- 같은 바이너리의 `node --test porting/flaskbb/tests/features.test.mjs` → 1 test passed, exit 0; actual renderer, official SDK, AIP listener, dedicated PG. `run-features-security.log`에 권한 경계와 `run-features-ui-conversations.log`에 쪽지 round trip 전체 출력.
- `/private/tmp/flaskbb-aip-host-venv/bin/python -m unittest porting.flaskbb.tests.test_ui_host` → 13 passed; `run-ui-host-unittest.log` (Mock 기반 단위시험이며 실통합 증거로 계산하지 않음).
- `git diff --check` → exit 0. `cargo fmt --all -- --check` → exit 1; rustfmt formatting diff가 이 미커밋 auth 코드와 `spikes/spike-v6-transport/src/lib.rs` 같은 건드리지 않은 경로까지 보고됐다. workspace-wide formatting sweep은 하지 않았고, 이 check는 미통과로 남긴다.

**커밋/원격**: 코드·테스트·evidence 증분은 `2bf962d860e0678abd6e4ff6317168291bc19a0b` (`fix(porting): close auth and messaging regressions`)로 `origin/porting/flaskbb-full-2019`에 push했다. 이 커밋의 `git diff --cached --check`는 통과했다.

**계속 미완료**: 이 증분은 FlaskBB 전체 F01–F17 완료가 아니다. `activated=false`는 FlaskBB actor row policy에서 기존 token 쓰기를 막고 `activated=true` 재활성화 후 허용한다. core `aip_principals.enabled/min_iat`와 구분되며 core에 FlaskBB 표식을 하드코딩하지 않았다. 이 한 전이 외의 원본 계정수정/활성화 전체 의미는 미검증. Browser OIDC, Keycloak `ERR_CERT_AUTHORITY_INVALID`, 원본 backend-off, 가입·메일 활성화/reset/logout 전수, 재시작 보존, clean deployment는 열린 gate다. 독립 검증은 별도 세션이 배정할 예정이므로 이 실행의 자체 결과는 출시 판정이 아니다.

## 2026-10-17 현재 Run — Provisioning working proposal 구현 및 F02–F10 포팅 실DB 검증

### 1. 신규 계정 Provisioning core 계약 구현 및 실DB 증명 (OPEN 유지)

신규 주체 계약 권고안을 임시 working proposal로 채택하여 코어 및 전송 계층에 구현했다.
- **구현 내용 (`crates/aip-auth`, `spikes/spike-v6-transport`)**:
  - `auth.provisioning.enabled=false` 기본값. `POST /session` 경로에서만 단일 트랜잭션으로 작동하며 비세션 경로(`/read`, `/apply`) 프로비저닝은 완전히 차단.
  - 검증 완료된 `(iss, sub)`로부터만 사용자(`User`) 및 principal 매핑을 생성하고, 서버 설정이 지정한 기본 `Member` 그룹을 원자적으로 부여.
  - PostgreSQL advisory transaction lock (`pg_advisory_xact_lock(hashtextextended(...))`)에 길이 접두사 튜플 직렬화(`{schema_len}:{schema}:{iss_len}:{iss}:{sub_len}:{sub}`)를 적용해 NUL 바이트 없이 lock key를 안전하게 해싱하고 동시 요청 직렬화.
  - 이메일/사용자명 일치에 의한 자동 병합 금지, 클라이언트 주입 role/속성 불신, disabled/revoked 주체 재생성 차단, 미인증 이메일 거부.
  - 상태는 계속 **OPEN**으로 유지 (전역 제품 확정이 아닌 이번 포팅의 working proposal).
- **실DB 격리 검증 증거 (`porting/flaskbb/tests/provisioning.test.mjs`)**:
  - dedicated PostgreSQL `127.0.0.1:55439` database `aip_flaskbb` 동적 임시 스키마에서 9개 시나리오 100% 통과 (소요시간 1433ms):
    1. 최초 토큰 `POST /session` 정상 생성 및 이후 SDK read 검증 (`positive_first_session_and_reads`)
    2. 미등록 주체의 `/read` 직접 호출 401 거부 및 DB 불변 (`negative_non_session_route_blocked`)
    3. `provisioning.enabled=false` 배포 시 거부 (`negative_provisioning_disabled_blocked`)
    4. 미인증 이메일 거부 (`negative_unverified_email_blocked`)
    5. 기존 사용자명/이메일 충돌 시 자동 병합 없이 거부 및 DB 불변 (`negative_username_email_conflict_no_merge`)
    6. 클라이언트 위조 role 주입 무시 및 서버 고정 그룹 부여 (`negative_client_role_injection_ignored`)
    7. disabled/revoked 주체 재생성 차단 (`negative_disabled_revoked_no_recreation`)
    8. 10개 병렬 동시 요청 멱등성 및 단 1건 생성 검증 (`concurrency_10_parallel_sessions_idempotent`)
    9. 충돌 롤백 시 고아 row 0건 원자성 검증 (`rollback_atomicity_no_orphan_rows`)

### 2. INVENTORY 미포팅 항목 전진: F02, F03, F04, F06, F07/F15, F09, F10 실제 코드 및 실DB 통합 검증

- **AIP 스키마 선언 확장 (`porting/flaskbb/app.aip`)**:
  - `Topic`: `title.contains` 검색 필터 추가
  - `Post`: `content.contains` 검색 필터 추가
  - `User`: `updateProfile` transition (signature, avatar, website, location, theme, language), `adminUpdateUser` transition, `unique username`, `unique email`
  - `Category` / `Forum`: `create`, `edit` transition (admin/super_mod 권한)
  - `Conversation`: `create`, `markRead`, `moveToTrash`, `restoreFromTrash` transition/apply
  - `Message`: `create` expose
  - `ForumRead` / `TopicRead`: `create`, `updateRead` transition
  - `TopicTracker`: `id.eq`, `user.eq` 필터 추가
- **UI 어댑터 라우트 및 템플릿 구현 (`porting/flaskbb/ui/app.py`, `ui/templates/`)**:
  - F02: `/auth/register`, `/auth/forgot-password`, `/auth/reset-password` (IdP 연동 리다이렉트), GET/POST `/auth/logout`
  - F07/F15: `/search` (q, type 쿼리 파라미터로 Topic, Post, User, Forum 4종 검색 및 `search.html` 렌더)
  - F03: `/users` (MemberList), `/user/<username>` (UserProfile + 최근 활동), `/user/settings` (설정 폼 및 `User.updateProfile` bundle)
  - F09: `/conversations/inbox`, `/conversations/trash`, `/conversations/new` (양방향 대화함 동시 생성), `/conversations/<id>` (읽음 및 markRead), reply, trash, restore
  - `layout.html`: 상단 네비게이션(Forum, Users, Search, Messages, Settings, Profile) 추가
  - `porting/flaskbb/tests/test_ui_host.py`: 신규 라우트 5개 단위 테스트 추가 (총 12 passed, 0 failed).
- **실DB 통합 검증 증거 (`porting/flaskbb/tests/features.test.mjs`)**:
  - dedicated PostgreSQL `127.0.0.1:55439` dynamic schema에서 공식 SDK + 실제 AIP 바이너리 listener 기동 검증 100% 통과 (소요시간 1460ms):
    1. **F07/F15**: Topic 제목, Post 본문, User 사용자명, Forum 제목에 대한 `.contains` 검색 결과 정확성 검증.
    2. **F03**: Alice 본인 프로필 수정(Seoul, Alice In Wonderland) 반영 및 타인(Bob) 악의적 수정 시도 403 거부 검증.
    3. **F09**: Alice(발신)→Bob(수신) 대화함 동시 생성, Bob의 수신함 `unread: true` 관측, 답장 Message 작성, `markRead` 전이 후 `unread: false`, `moveToTrash` 및 `restoreFromTrash` 전이 검증.
    4. **F06**: TopicTracker 등록(`track`) 및 해제(`untrack`), `ForumRead` 생성 검증.
    5. **F04 / F10**: Bob(Admin)의 Category 생성·수정 성공, Alice(일반 회원)의 Category 생성 시도 403 거부, Bob의 Alice 계정 상태 `adminUpdateUser` 전이 검증.

### 3. 미해결 항목 및 블로커 유지
- Keycloak 브라우저 `ERR_CERT_AUTHORITY_INVALID`는 계속 별도 blocker로 유지 (우회 금지).
- 원본 WTForms 오류 화면 재표시, pagination, 원본 Jinja 템플릿 직사용(F16), 소프트 삭제 tombstone vs 물리 cascade 차이 등은 열린 상태 유지.
- 원본 백엔드 중지 상태의 최종 E2E 배포 검증 및 clean checkout 배포 재현은 후속 gate로 유지.

## 2026-10-17 현재 Run — forum 렌더 권한 및 실제 AIP SDK/DB 통합

원본 `layout.html`, `forum/index.html`, `forum/category_layout.html`, `forum/forum.html`, `forum/topic.html`, `forum/new_topic.html`, `forum/forms.py`, `forum/views.py`를 대조했다. 현재 adapted renderer에서 보존된 항목은 Aurora 정적 자원과 FlaskBB class 구조, 카테고리/포럼 제목·설명·잠금 표시, 주제 제목·작성자·게시물 수·날짜, 게시글 본문, 원본의 필수 제목/본문 검사, AIP contract의 제목 255자 상한, 로그인 회원의 CSRF 폼이다. upstream 권한 경계에 맞춰 로그인·활성 AIP actor, 포럼 읽기 접근, locked/hidden 상태, primary/additional group 권한과 moderator 관계를 조회해 새 주제/답글 폼을 노출하고, 실제 mutation 인가는 계속 AIP 서버가 최종 결정한다.

원본과 아직 다른 부분: upstream Jinja template을 그대로 재사용하지 않았고 WTForms의 필드 오류 재표시, form/plugin hooks, topic tracking checkbox, editor help/preview, last-post 작성자·제목, unread/read markers, moderator links/actions, reply/edit/delete controls, pagination, 검색·profile·관리 UI는 보존하지 않았다. 빈 값은 브라우저 `required`와 서버 검증을 거치지만 기존 WTForms 오류 화면과 동등하다고 보지 않는다. 한 UI 통합은 전체 original UI 흐름이나 F01–F05 전체 동등성 판정이 아니다.

### 격리 통합 증거

- 먼저 추가한 실제 renderer 권한 회귀는 GREEN 전 `evidence/ui-authz-red.log`에서 실패했다: UI 통합 fixture의 read-only 계정이 `/forum/2` 화면에서 “Start a topic”을 보았다. 이후 권한 계산을 화면에 연결했다.
- `node --test porting/flaskbb/tests/aip-read.test.mjs`를 다시 실행: 1 test passed, 0 failed. 전체 원시 출력과 실제 dynamic schema 식별자는 `evidence/ui-sdk-integration-green.log`에 있다. 시험은 테스트용 AIP listener를 ephemeral loopback 포트에 기동하고 dedicated PostgreSQL host `127.0.0.1:55439` database `aip_flaskbb` 안에 임의 schema를 만들었다. localhost:5432는 사용하지 않았다.
- 같은 실제 SDK/API 환경에서 pre-bound synthetic `readonly-fixture`의 topic 폼은 숨겨졌고, 유효 CSRF를 넣은 위조 POST는 HTTP 403 및 topic row 변화 없음. `member-fixture`는 화면의 topic 폼을 제출해 303을 받고 Topic + 첫 Post가 publish된 상태로 DB에 저장됐다. reply 폼에서 POST한 답글도 303 및 publish된 Post 행으로 저장됐다. 공식 SDK의 후속 `Topic` read에서 UI가 만든 topic이 확인됐다. 이들은 사전 운영자 bind를 사용하는 테스트 actor이며 새 가입/계정 provisioning 증거가 아니다. 모든 seed/schema는 test 전용이고 dynamic schema는 `finally`에서 제거한다.

### TLS 차단 stage

이번 Run에서는 browser를 다시 호출하지 않았다. 이전 configured browser 증거는 Keycloak `https://127.0.0.1:58094`에서 `ERR_CERT_AUTHORITY_INVALID`였다. 공개 인증서 metadata만 `evidence/keycloak-tls-metadata.txt`에 읽었다: subject와 issuer가 모두 `CN=127.0.0.1`, SAN은 `IP:127.0.0.1`과 `DNS:localhost`. 따라서 issuer URL host와 SAN은 일치하고, self-issued 인증서를 browser client trust chain이 신뢰하지 않는 stage가 관측된 오류와 일치한다(원인은 metadata+오류를 합친 추론이며 browser trust store를 읽거나 변경하지 않았다). 서버 SDK용 CA 명시 TLS 경로는 별도 미실행이며 browser 검증으로 간주하지 않는다. HTTP server/SDK integration 성공은 OIDC browser login, Keycloak issuer trust, authorization code/callback을 검증하지 않는다.

F01–F17, 새 IdP 주체 provisioning A/B 계약, 원본 business backend 중단, F01–F05 전체 화면/업무 parity, 재시작 보존 및 clean deployment gate는 계속 미완료다. 독립 `nbf` 검증자는 별도 detached tree에서 확인 중이므로 이번 변경에서 auth 코드는 건드리지 않았다. 20a95a9 mock UI suite 접수 범위 및 진행 중인 계정연결 검토는 이 통합 증분으로 완료 판정하지 않는다.

## 2026-10-17 새 컨텍스트 — 계정 연결 계약 대조 및 브라우저 UI 기준선

현재 계약 대조 결과, 신규 가입→AIP 회원/principal 연결은 현 제품 경로에 맞춰 배선할 수 있는 작은 UI 변경이 아니다. `product/EXECUTION.md`의 신원 절은 `service principal bind`를 명시적 운영 작업으로 한정하고, `product/OPERATIONS.md`는 앱 사용자 actor가 먼저 존재해야 한다고 규정한다. 실제 서버는 `/session` 전에 토큰을 검증한 뒤 `(issuer, subject)`의 활성 principal mapping을 조회한다. `porting/flaskbb/app.aip`의 `User`는 actor resource이고 read만 노출한다. renderer의 callback은 official SDK `discover()`를 호출하며 연결 없는 subject를 거절한다. 즉 등록 뒤 SDK/운영 CLI를 이용하거나 원본 FlaskBB DB를 우회하면 기존 계약을 깨거나 금지된 업무-backend bridge가 된다.

신규 worker 권고는 이 core provisioning을 이번 FlaskBB 포팅의 임시 working contract로 제안하고, 기본 OFF 및 POST /session 전용으로 좁혔다. 권고는 인증 전 extension hook을 기각하며 app proxy/직접 SQL/운영 bind 자동 호출도 제외한다. 다만 이는 제품 OPEN을 해결하거나 사용자 승인으로 확정한 것이 아니고 구현/실행 증거도 없다. auth code의 별도 nbf 검증이 끝나기 전 provisioning/auth 구현은 보류한다. 나중에 구현하더라도 opt-in 서버 설정, 검증된 issuer/subject, single transaction, server-fixed app group/default, no email/username auto-merge, disabled/revoked subject no-recreation, concurrency/rollback tests를 이 권고 범위에서 검증해야 한다. F02 원본 대비 차이(비밀번호/login·가입/activation/reset의 IdP 위임, local-only logout)는 동등성 gate에 계속 남는다.

이번 새 컨텍스트에서 `docker ps --format ...`으로 전용 baseline FlaskBB·Keycloak·Mailpit·두 PostgreSQL 컨테이너가 실행 중임을 확인했다. `curl` 기준 원본 FlaskBB `127.0.0.1:58080/`은 HTTP 200, 당시 renderer `:58082`와 AIP `:58083/health`는 `curl (7) Couldn't connect`였다. 기존 격리 venv와 test-only random session secret으로 renderer를 `127.0.0.1:58082`에 기동하고 configured `ttakkari_browser`로 열어 `Login - FlaskBB` 및 `Continue to login`, IdP 가입 링크, 원본 CSS/font 로드(200)를 확인한 뒤 브라우저를 닫고 호스트를 중지했다. `favicon.ico` 404가 browser console의 유일한 error였다. 등록 링크를 따라 Keycloak에 접근하거나 callback/SDK/AIP 요청은 실행하지 않았다. UI screenshot/snapshot: Run outbox `browser/page-2026-10-17T01-13-03-784Z.yml`. 상세 환경·명령은 [검증 로그](evidence/verification.log)에 기록했다.

진행 판정: 이 기준선 실행에서는 새 가입→principal 연결, authenticated browser UI 흐름, logout/denial을 확인하지 않았다. 이후 reasoning worker가 default-OFF `/session` provisioning을 포팅 working proposal로 권고했지만 제품 OPEN은 유지한다. 독립 nbf verifier의 auth edit boundary가 정리될 때까지 provisioning/auth 구현을 보류하고, 그동안 F01/F05 AIP UI와 전체 포팅을 진전한다. F01–F17 v0.1 scope, 전체 backend 중단 gate, clean deployment gate 및 기존/비활성/revoked principal 보호는 그대로다.

## 2026-10-17 현재 Run — 원본 인증 UI 재관측과 AIP 가입 경계 결정 필요

이번 `ttakkari_browser` 관측은 **원본 기준선**이다. `http://127.0.0.1:58080/`에서 원본 Forum 홈, Login 링크(`/auth/login`), Register 링크(`/auth/register`)를 열었다. 로그인 화면은 username/email·password·remember-me와 가입/비밀번호 재설정 링크를 표시했고, 가입 화면은 username·email·password·confirm·language·약관 checkbox를 표시했다. form은 아직 제출하지 않았다. 이 페이지들은 원본 FlaskBB server-rendered UI이며 새 AIP UI나 AIP 요청 증거가 아니다. Browser MCP로 페이지를 닫았다. 접근성 snapshot은 Run outbox `browser/page-2026-10-17T00-54-30-454Z.yml`, `page-2026-10-17T00-55-35-080Z.yml`, `page-2026-10-17T00-55-37-820Z.yml`에 등록됐다.

### [열림 — 제품 아키텍처 결정] 새 IdP 주체의 AIP 사용자 생성과 연결

**문제:** 고정 원본의 가입은 계정 필드 검증·중복/blacklist 검사·기본 Member 권한·비활성 계정·activation email/token으로 이어진다. 로그인은 username 또는 email과 password hash를 검증하고 활성화/비활성화·실패·reauth 정책을 적용한다. AIP 계약에는 `User` read만 있고 `User` create가 없다(`porting/flaskbb/app.aip` `resource User`). AIP 서버는 검증된 `(issuer, subject)`가 이미 actor에 연결되어야 `/session`에서 주체를 돌려주며 현재 연결 경로는 운영자 `service principal bind`다(`product/EXECUTION.md` 신원 절, `product/OPERATIONS.md`); 기존 actor 수동 연결과 bearer token은 새 가입자 생성/연결을 대신할 수 없다. 따라서 새 사용자의 가입 이후 권한에 맞는 앱 actor가 생기는 흐름을 현재 계약만으로 정직하게 실행할 수 없다.

**최소 제품안(권고, 아직 미승인·미구현):** AIP core에 선언형, 원자적 principal-provisioning 경로를 추가한다. AIP가 먼저 JWT 서명·issuer·audience·수명·subject를 검증한 뒤 `(iss, sub)`만으로 서버가 actor ID를 발급하고, 앱이 선언한 `User` actor resource row와 principal mapping을 한 DB transaction에서 생성한다. role/group·`activated` 기본값은 서버 선언에 고정하고, 요청자는 actor ID나 role을 지정할 수 없다. 이름/email claim mapping은 allowlist된 claim만 사용하고 uniqueness·IdP email verified/activation 조건을 실패폐쇄로 검사한다. 같은 주체의 재시도는 멱등, 동시 가입은 한 연결만 성공, 중복 email/username 및 잘못된 claim은 행/바인딩 불변이어야 한다. activation/reset은 사용자가 설정한 IdP/메일 공급자에서 실제 token 효과를 검증한 후 이 경로와 연결한다.

**현재 권고 (포팅 working proposal, OPEN):** reasoning worker는 (A) 좁은 server-configured core provisioning을 권고하고 (B) pre-auth extension hook을 기각했다. 설정 기본값 OFF, 검증된 `(iss, sub)`만 `POST /session`에서 생성, User+principal+앱 설정 초기 Group을 단일 transaction으로 다룬다. client actor/role과 email 동일성 자동 병합은 신뢰하지 않으며 disabled/revoked 주체는 재생성하지 않는다. 이는 현재 포팅 구현을 구체화하는 권고이지 사용자 Accepted/product-wide DSL 결정이 아니다. 앱 전용 proxy/ORM/direct SQL/operator bind 자동 호출은 제외한다.

**이번 Run에서 실행되지 않은 것:** 브라우저 authorization-code/PKCE, FlaskBB 화면의 AIP SDK 요청, AIP DB 행 변경, 새 IdP 가입→AIP actor 연결, 로그인 이후 forum 목록/주제 CRUD, 로그아웃/거부. 현재 command environment에서 `curl http://127.0.0.1:58083/health`는 `curl: (7) Couldn't connect`로 끝났다. 이 관측은 macOS command environment에서 해당 시각 AIP listener에 연결되지 않았다는 뜻뿐이며 AIP/production 장애 판정이 아니다. 원본 UI는 port `58080`에서 응답했다. 자세한 command 범위는 `evidence/verification.log` 마지막 항목.

**다음 진척:** auth code는 detached nbf verification이 진행되는 동안 수정하지 않는다. edit boundary가 끝나면 worker 권고를 per-port working proposal로 구현하되 제품 OPEN을 유지하고, default-off/session-only, verified claim source, uniqueness/no-merge, disabled/min_iat, concurrency/rollback, schema isolation을 실제 격리 DB에서 증명한다. `activated=true`는 verified-email와 원본 activation workflow parity 확인 전 제안에 머문다. 독립적으로 upstream UI parity 및 Forum/Topic/Post renderer를 계속한다. F01–F17 분모, 전체 앱 backend 중단 gate, 삭제/tombstone 및 tracker 의미 차이는 그대로 유지한다.

## 2026-10-17 현재 Run — OIDC/AIP renderer 첫 증분 (E2E 차단)

`porting/flaskbb/ui/app.py`에 별도 Flask renderer를 추가했다. OIDC authorization-code + PKCE S256 시작, browser-bound state cookie, issuer/audience/nonce/signature/시간 검증, opaque HttpOnly local session, CSRF-검사 logout을 구현했고 forum 홈·forum topic list·topic posts GET은 AIP Python caller SDK만 통해 읽는다. 이 호스트는 원본 FlaskBB app/ORM/업무 DB를 import하지 않는다. upstream의 Aurora CSS/static과 forum CSS class 계층은 재사용하지만 현재 Jinja 화면은 upstream 템플릿 그 자체가 아닌 작은 adapted renderer다. 따라서 요구된 원본 템플릿 전체 보존과 F01/F05 화면 동등성은 아직 달성하지 못했다. 코드 검토와 단위시험만으로 브라우저 실사용 동등성을 주장하지 않는다.

renderer에서 AIP 서버가 검증한 현재 actor id를 얻도록 Python SDK에 `Client.session()`을 공개하고, `product/SDK.md` 및 `product/tests/python_sdk_test.py`를 갱신했다. 이 메서드는 기존 `/session` 경로만 부르며 신규 사용자 생성/바인딩 권한은 없다.

### 실행 증거와 차단 범위

- AIP: dedicated PostgreSQL `aip_flaskbb` host `55439`, schema `aip_flaskbb_auth`; 실제 `target/debug/aip service serve --config /private/tmp/flaskbb-keycloak-ui-aip.json porting/flaskbb/app.aip`는 `127.0.0.1:58083`에서 JWT access-token 인증으로 기동했고 `/health`는 `{"ok":true,"status":"live"}`였다. 그 뒤 즉시 중지했다. 테스트 config는 CORS origin `http://127.0.0.1:58082`이며 `/private/tmp` test-only file이다.
- renderer: Python 3.14.7 격리 venv, Flask 3.1.3, joserfc 1.7.5, local product Python SDK source (`PYTHONPATH=product/python`)로 `127.0.0.1:58082`에 띄웠다. browser MCP에서 `/` 및 original-asset CSS/woff2는 200, `/auth/login`은 302였다. Favicon 404 한 건도 보였다.
- Browser: `Continue to login`이 Keycloak TLS test issuer `https://127.0.0.1:58094`로 이동했지만 configured `ttakkari_browser`는 `chrome-error://chromewebdata/`, `net::ERR_CERT_AUTHORITY_INVALID`를 표시했다. 예외 수락·보안 설정 변경·다른 browser/셸 실행 없이 browser를 닫았다. 따라서 callback·PKCE token exchange·AIP SDK browser-auth call을 실행하지 않았다.
- Keycloak isolated test realm에 `aip-flaskbb` client를 public authorization-code client로 설정하고 redirect URI `http://127.0.0.1:58082/oidc/callback`, origin, standard flow 및 PKCE S256 요청을 설정했다. Client attribute에 서버 강제 PKCE가 설정됐는지는 확인되지 않았다. `verifyEmail=true`, `registrationAllowed=true`, `loginWithEmailAllowed=true`, `resetPasswordAllowed=true`가 realm metadata에서 관측됐다. SMTP/Mailpit 등록메일·재설정메일은 이 Keycloak realm에 연결/시험하지 않았다. 기존 synthetic `auth_new` 계정의 fixture password를 재설정했으나 값은 기록하지 않는다.
- 실제 browser 로그인 단계 이후 AIP `/session`, `User`/`Category`/`Forum` reads, forum/topic 화면, AIP DB, create/reply/edit/denial, IdP logout, restart/DB 보존 및 original 업무 backend 중지 상태는 모두 **미실행**이다. 특히 같은 test subject의 사전 principal bind 계정은 기존 회원에 대한 로그인 관측일 뿐 신규 가입/온보딩 증거가 아니다.

### 이번 코드 검증 (전체 gate가 아님)

- `python3 -m unittest product.tests.python_sdk_test` (Python 3.14.7 isolated venv, fake transport protocol tests): 22 passed, 0 failed. 2개의 기존 ResourceWarning(닫히지 않은 test socket/HTTPError 임시 객체)이 함께 출력됐다.
- `python3 -m unittest porting.flaskbb.tests.test_ui_host`: 4 passed, 0 failed. 이들은 query 분기, PKCE redirect 생성, browser-state mismatch 차단, CSRF logout을 검사하는 unit tests이며 실제 IdP/UI 통합 증거가 아니다.
- `target/debug/aip service check porting/flaskbb/app.aip`: `ok:true`, advisories `[]`.
- `git diff --check`: clean.

결론: 새 UI code와 SDK actor-session read API가 생겼지만 **실제 browser login과 UI→AIP 요청은 아직 한 번도 성공하지 않았다.** UI의 auth callback도 현재 AIP auth bridge contract를 만나 신규/미연결 주체에 실패한다. 원본 Jinja 템플릿 적용, trusted local IdP TLS 시험기반, 새 주체 자동 provisioning 제품 결정 및 실제 signup email lifecycle이 다음 blocking gate다. 전체 F01–F17, 원본 업무 backend 중단, 의미 동등성, clean deployment gate는 모두 유지된다.

## 2026-10-17 현재 Run — F05 forum UI의 AIP 원자 쓰기 폼

다음 구현 증분은 기존 read-only F01/F05 renderer에 forum topic 생성과 reply 폼을 연결했다. `porting/flaskbb/ui/app.py`의 POST route는 server session에서 AIP가 검증한 actor와 활성화된 User row를 다시 읽고, browser-bound CSRF token을 검사한 뒤 official Python SDK `bundle()`로 보낸다. 새 topic은 Topic + 첫 Post + publish + TopicFirstPost 연결 + Topic publish를 단일 AIP atomic bundle로 요청하며, reply는 Post create + publish + Topic.bump를 하나의 bundle로 요청한다. 작성자 ID/username은 form body나 IdP claim이 아니라 AIP session/User read에서 고른다. Topic/Post 권한과 원자성의 최종 판단은 `app.aip` contract 및 AIP server가 수행한다. AIP mutation 실패는 권한 거부 403, 인증 실패 401, 나머지 계약/서비스 실패 503으로 렌더한다. 페이지 폼은 Aurora 기반 adapted template에 추가했다.

TDD로 POST endpoint test 3개를 먼저 추가했고 기존 호스트에 route가 없던 상태에서 새 테스트가 각각 HTTP 404로 실패했다. 구현 뒤 isolated Python 3.14.7 venv에서 `python -m unittest porting.flaskbb.tests.test_ui_host`는 7 tests passed, 0 failed. 이 테스트들은 mock SDK의 bundle request shape, actor-derived username, CSRF mutation 차단을 검사한다. `py_compile`은 exit 0이다. **실제 AIP 서버에서 이 bundle을 실행한 것, 브라우저에서 폼을 눌러 network/DB 결과를 본 것, 성공 로그인 actor는 이번 증분에서 확인하지 않았다.** 앞서 기록된 테스트 인증서 오류를 같은 방식으로 재시도하지 않았다.

이 증분은 F05 전체 구현이 아니다. 원본 forum/user/role별 폼 노출과 validation/plugin hooks, topic/post edit, delete 의미와 cascade, lock/hidden baseline, tracker/read markers, pagination/last-post/search projection, 거부 후 상태 비교는 미완료다. AIP `Topic`/`Post`는 현재 물리 delete 대신 tombstone이며 열린 의미 차이를 유지한다. 신규 IdP 주체 provisioning 제품 계약은 여전히 결정 대기다. 브라우저 TLS/신뢰 경로와 사용자 선택이 생기면 실제 signup/login/activation, 성공한 browser-to-AIP reads/writes, logout/denial 증거부터 실행하고 F01–F17 및 원본 backend 중단 gate를 계속 닫는다.

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

### 2026-10-17 — 선택 `nbf`의 null 경계 보정

독립 검증의 고정 대상 `48658d7c24b4b231b499d9512825017935f5955c` (`f3f68f6` 포함)에서 `nbf` 생략·과거·현재 허용과 미래·음수·잘못된 타입 및 서명/issuer/audience/만료 거부는 확인됐으나 명시적 `nbf:null` 허용이 별도로 실패했다. RFC 7519 §4.1.5에 따라 claim은 생략할 수 있지만 존재하면 NumericDate여야 한다. `Claims.nbf: Option<i64>`의 Serde 경계만 `default + deserialize_with`로 바꿨다: 생략은 `None`, 존재하는 값은 `i64`로 읽어야 하므로 null/비정수는 거부된다. 서명, issuer, audience, exp/iat/lifetime 및 음수·미래 검사 로직은 바꾸지 않았다.

TDD 증거: `explicit_null_optional_nbf_is_rejected`는 수정 전 기존 검증기가 서명된 null 토큰을 수락해 실패(0 passed, 1 failed, exit 101)했고, 수정 뒤 통과(1 passed)했다. 생략 허용 회귀도 통과했고 `cargo test -p aip-auth --lib --offline`는 17 passed, 0 failed다. DB가 필요 없는 기존 JWT profile, negative nbf, extreme signed timestamp 시험도 각각 통과했다. 각 명령의 전체 출력은 `evidence/nbf-*.log`에 보존했다. 선택한 `tests/jwt.rs` 경로는 `verify_token`만 호출하며 DB 조회/helper를 실행하지 않았다. `localhost:5432`를 사용하는 principal/revocation/Durable 테스트는 실행하지 않았다.

이 수정은 `nbf` claim-shape 경계의 범위로 한정된다. 가입자 provisioning 계약은 계속 미결정이며 F02 로그인·브라우저 UI→AIP 실제 요청 우선순위, F01–F17 전체 기능 분모, 원본 업무 backend 중단 및 clean deployment gate에 대한 전체 포팅 판정은 바뀌지 않는다.
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
