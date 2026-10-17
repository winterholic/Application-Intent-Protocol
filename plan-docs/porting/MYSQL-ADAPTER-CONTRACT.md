# MySQL/MariaDB adapter 조사 — WBS 5.2.4

2026-10-15 조사에서 시작한 구현 작업의 현재 기록이다. 원본 앱이 MySQL에서 실행되는 것과 AIP가 MySQL을 지원하는 것은 다른 판정이다.

**현재 상태: optional `mysql_async` 기반 live catalog introspection과 요청 시점 catalog fingerprint 비교 primitive가 구현됐고 격리된 MySQL 8.4 및 MariaDB 11.4에서 읽기를 확인했다. 2026-10-17 추가 경쟁시험에서 INFORMATION_SCHEMA catalog 조회는 트랜잭션 안에서도 대상 업무 테이블의 DDL을 막지 않음을 두 엔진 모두 확인했다. 따라서 이 primitive를 서비스 fence로 연결할 조건이 아직 없으며, HTTP service boot/auth/fence 통합, 일반 policy read/write 및 트랜잭션 어댑터는 미구현이다. MySQL/MariaDB 저장 어댑터 지원 완료로 판정하지 않는다.**

## 2026-10-17 실제 제품 경로 확인

`aip-migrate::external::introspect`는 기본 빌드에서 MySQL/MariaDB URL을 `DB_ADAPTER_UNSUPPORTED`로 거부한다. `mysql` feature 빌드는 `mysql_async` 연결과 catalog 조회를 사용해 introspection draft 및 mapping을 만든다. 이 예외는 `service introspect` 전용이다. 일반 service/serve/migrate/adopt 경로는 계속 fail-closed다. URL 파싱·연결 오류에 원본 비밀번호를 포함하지 않는다.

후속 순수 변환 단위에서 `aip_migrate::mysql_catalog::normalize_table`을 추가했다. INFORMATION_SCHEMA COLUMNS/STATISTICS에서 온 typed row를 driver 없이 받으며, 컬럼 순서·정규화 타입·부호/표시 폭·null/default/generated·PK/unique index 순서를 반환한다. 중복/비연속 column ordinal, 없는 column을 참조하는 index, index 멤버 간 `NON_UNIQUE` 불일치는 오류다. `CHAR`/`VARCHAR` 길이는 현재 `Text`로 축약되므로 길이 비보존 진단을 남긴다. 작은 현재 manifest 경계 밖인 collation, generation expression, on-update, auto_increment, functional index expression, 미지원 타입은 diagnostics에 남긴다. `tinyint(1)`은 값이 0/1로 제한됐다는 뜻으로 단정하지 않고 정수 metadata와 alias 표식·진단을 함께 보존한다.

`normalize_foreign_keys`는 future driver가 `KEY_COLUMN_USAGE`와 `REFERENTIAL_CONSTRAINTS`를 결합해 제공하는 driver 비의존 row를 받아 constraint 단위 manifest를 만든다. constraint/source schema·table·이름을 분리 키로 유지하고 ordered source/target column pairs, referenced schema/table, `ON DELETE`/`ON UPDATE` actions를 보존한다. 비연속 순서·누락 source/target/action·action 또는 대상 불일치는 오류로 닫는다. 현재 인식하는 action은 `CASCADE`, `SET NULL`, `RESTRICT`, `NO ACTION`; `SET DEFAULT`와 알 수 없는 값은 unsupported 오류다. 원본 11개 RDB 모델의 선언 기준은 [FK manifest](facereview-fk-manifest.json)다. 기존 댓글 PostgreSQL fixture는 이 중 Comment의 두 참조만 담는다. 이름과 실제 참조 schema는 원본 선언에 없으므로 manifest 예상치에서 만들지 않고, catalog 입력/출력에서만 보존한다.

`COLUMN_DEFAULT = NULL`은 MySQL 8.4에서 명시적 `DEFAULT NULL`과 DEFAULT 절 부재 양쪽에 쓰이므로 `NullOrAbsent`로 표현하고 모호성 진단을 남긴다. 드라이버 연결 시 `SHOW CREATE TABLE` 등으로 구분해야 한다 ([MySQL 8.4 COLUMNS](https://dev.mysql.com/doc/refman/8.4/en/information-schema-columns-table.html)).

### 2026-10-17 MySQL 드라이버 및 live catalog 단계

- `crates/aip-migrate/Cargo.toml`의 `mysql` feature가 선택 의존성 `mysql_async = 0.37.1`을 활성화한다. 기본 빌드에서는 해당 드라이버가 제외되며, TLS backend는 `default-rustls-ring`이다. 라이선스는 MIT OR Apache-2.0이다.
- `MysqlDriverConfig`는 `mysql://`와 `mariadb://` URL, pool min/max, TLS mode, connect timeout을 읽는다. TLS를 요구하면 인증서 검증을 켜고, 추가 CA bundle은 `AIP_MYSQL_CA_CERT`로 지정한다. 이 CA 환경변수는 DB 연결 설정이며 Cargo crate 다운로드 설정과 별개다.
- feature build의 `aip_migrate::external::introspect`는 `mysql_driver`를 통해 INFORMATION_SCHEMA의 table/column/index/FK row를 조회하고 기존 normalizer에 연결해 introspection definition/mapping을 반환한다. `service introspect`만 이 설정을 통과하며 MySQL 서빙·정책 read/write·migration/adopt 경로는 지원하지 않는다.
- 새 격리 MySQL 8.4 컨테이너의 `aip_mysql_test` schema에서 `users`와 `posts` 두 테이블, unique/ordinary index, FK action을 조회해 CLI draft mapping에 담는 live test가 통과했다. 이는 FaceReview 원본 schema 검증이 아니다.
- 다중 URL error sanitizer의 두 비밀번호 누락 회귀를 테스트 우선으로 재현(exit 101)한 뒤 수정하고 같은 targeted test를 통과시켰다. 호출자/운영 로그에서 이 helper가 실제 사용됐는지는 확인하지 않았다.
- `mysql_wire.rs`는 과거 parser 실험으로 파일에 보존되어 있으나 모듈 export가 없고 테스트도 compile 대상에서 제외한다. 제품 driver로 계산하지 않는다.
- FaceReview 11개 모델/81개 컬럼/13개 FK 검사는 기존 synthetic fixture와 manifest 범위다. 원본 MySQL DDL과 live catalog를 대조하지 않았다.

### 2026-10-17 요청 시점 catalog fence increment

- `MysqlDriver::catalog_fingerprint`는 INFORMATION_SCHEMA를 매번 다시 읽어 카탈로그 digest를 계산한다. `verify_catalog_fingerprint`는 호출 때마다 재조회하고 저장된 digest와 다르면 오류로 닫는다. TTL·stale cache는 사용하지 않는다. 이 primitive는 서버의 `DeploymentFence`에 아직 연결되지 않았다.
- 실제 격리 MySQL 8.4 및 MariaDB 11.4에서 baseline fingerprint 뒤 외부 `ALTER TABLE`을 적용하고 바로 다음 fence 검사에서 schema 변경을 거부하는 것을 확인했다.
- MariaDB는 MySQL의 `STATISTICS.EXPRESSION` 열을 제공하지 않고 `COLUMNS.GENERATION_EXPRESSION`이 NULL일 수 있어, 공통 catalog 쿼리 shape를 그대로 쓸 수 없다. column expression은 `COALESCE`하고, MariaDB index 조회는 `NULL AS EXPRESSION` dialect query를 사용한다. functional index와 전체 metadata parity는 검증하지 않았다.
- 두 엔진 모두 disposable TLS 연결에서 구성 CA 성공, 기본 신뢰 저장소 거부, 별도 untrusted CA 거부, SAN/hostname mismatch 거부를 확인했다. 시험은 catalog driver 수준이며 service 부팅이나 authentication을 증명하지 않는다.
- 각 엔진의 live catalog draft, TLS, drift-fence targeted tests 통과. 기본 `aip-migrate`, `aip-service` 및 `aip-service --features mysql` 회귀도 통과했다. PostgreSQL service 경로는 기존 구현을 유지하며 기본 feature 테스트를 통과했다.
- 기존 task/work-history 저장 위치 `/Users/winterholic/work-history`는 git repository가 아니며 이번 command sandbox에서 writable root가 아니다. 이 실행의 이어받을 상태는 이 계약 문서와 커밋에 기록한다.

### 2026-10-17 Stage 6 DDL/transaction 경쟁시험 — 서비스 연결 보류 근거

- 재현 가능한 ignored live test `mysql_driver::tests::live_information_schema_reads_do_not_pin_target_table_metadata_for_the_business_transaction`를 추가했다. 서로 다른 테스트 테이블에서 트랜잭션을 연 채 `INFORMATION_SCHEMA.COLUMNS` 조회 후 경쟁 `ALTER TABLE`을 수행하고, 별도 케이스에서 업무 테이블 `SELECT` 후 같은 경쟁 DDL을 수행한다.
- 실제 loopback Docker의 MySQL 8.4.11과 MariaDB 11.4.13에서 각각 통과했다. `lock_wait_timeout=2`인 원시 CLI 경쟁시험 및 Rust test 원시 출력은 Run outbox `stage6-live/mdl-race-live.log`, `stage6-live/test-mysql-mdl-retry.log`, `stage6-live/test-mariadb-mdl.log`에 보존했다. 엔진 데이터 디렉터리는 컨테이너 tmpfs였고, DB는 테스트 전용 fixture였다.
- 관측: 두 엔진 모두 INFORMATION_SCHEMA 조회 중엔 ALTER가 0.05초 만에 성공했다. 같은 열린 트랜잭션에서 업무 테이블 SELECT를 한 경우 ALTER는 두 엔진에서 약 2초 뒤 lock wait timeout으로 실패했고, 트랜잭션 종료 후 재시도는 성공했다.
- 이는 catalog fingerprint의 여러 INFORMATION_SCHEMA statement를 일반 transaction snapshot으로 감싸도 DDL fence가 되지 않는다는 직접 반례다. 업무 query가 테이블을 처음 여는 때부터 그 transaction 종료까지의 MDL은 보호하지만, fingerprint 확인과 첫 업무 table access 사이의 경쟁 window는 여전히 존재한다. 현재 primitive/service 구조는 두 작업을 같은 connection/transaction으로 결합하지도 않는다.
- 공식 근거: [MySQL 8.4 metadata locking](https://dev.mysql.com/doc/refman/8.4/en/metadata-locking.html)과 [online DDL locking](https://dev.mysql.com/doc/refman/8.4/en/innodb-online-ddl-performance.html)은 객체 사용 시 MDL과 DDL의 호환성/대기를 설명한다. [MariaDB metadata locking](https://mariadb.com/docs/server/reference/sql-statements/transactions/metadata-locking)은 transaction이 실제로 사용하는 table의 metadata를 transaction 종료까지 잠근다고 설명한다. 두 엔진의 catalog 조회 자체가 해당 업무 table을 transaction 종료까지 잠근다고 문서가 보장하지 않으며 live 결과도 이를 부정했다.
- 결과: 이 Run에서는 `serve`, `/ready`, auth, policy path를 MySQL에 연결하지 않았다. service 경로에 catalog fingerprint만 호출하거나 인증/fence를 우회해 HTTP boot를 성공시키는 것은 fail-closed 계약을 약화한다. PostgreSQL의 기존 경로는 수정하지 않았다.
- **설계가 필요한 계약 경계**: “다음 요청에서 외부 DDL drift를 무조건 막는다”가 요구사항이면 AIP가 그 DDL 주체와 함께 쓸 수 있는 lock/maintenance protocol이 필요하다. DB advisory/user lock은 해당 lock을 획득하는 caller끼리만 협조시키며 임의의 DBA DDL은 자동으로 참가하지 않는다. 지원 가능한 선택지는 (a) 외부 DDL 동시 실행은 보장 범위 밖으로 명시하고 운영자가 service drain/maintenance 경계를 지키게 하거나, (b) 모든 허용된 schema 변경 주체가 따르는 별도 배포 프로토콜·권한 모델을 설계하고 검증하는 것이다. 앱 사용자 조회의 직렬화와 관리형 스키마 변경의 직렬화를 어떻게 함께 보장할지 정하기 전에는 immediate drift-blocking으로 표현하지 않는다.
- 이 경쟁 test는 table-level MDL의 두 상황만 증명한다. view/trigger/FK graph, DROP/RENAME/CREATE namespace 경합, transaction isolation별 차이, 첫 business query 전 race의 service-level 결과는 미검증이다. 복합 FK는 계속 명시 거부하며 FaceReview 전체 schema/실앱은 후속 게이트다.

| Stage 6 gate (이번 Run) | 결과 | 미실행/한계 |
|---|---|---|
| feature-off fail-closed | `aip-service` 기본 빌드 전체 및 `mysql_url_or_adapter_fails_closed_before_pg_connect` PASS | 해당 service URL rejection test는 pass; MariaDB service boot 아님 |
| PostgreSQL 회귀 | 기본 `aip-service` 38 unit + 2 integration PASS | 본 Run에서는 전체 workspace DB-backed ignored cases 재실행 안 함 |
| MySQL feature fail-closed | `aip-service --features mysql` 전체 36 unit + 2 integration PASS; MySQL URL rejection PASS | MySQL runtime은 계속 unsupported |
| MySQL/MariaDB HTTP boot, readiness, deployment facts 대조 | 미실행 | 둘 다 실제 service adapter 경로가 없음 |
| principal/session 유효·잘못된 JWT, disabled/revoked, TLS/CA/hostname | 미실행 | 이번 Run에서 auth 연결/HTTP 요청 없었음 |
| `/read`·`/apply` allow/deny와 거부 뒤 DB 불변, rollback·동시성·멱등성·연결 끊김 | 미실행 | policy/write transaction adapter 없음 |
| DDL before/during/after 실제 service fence | 부분 관측: catalog scan과 업무 SELECT 중의 MDL 차이를 두 엔진에서 재현 | service fence 자체와 check 직후/첫 업무 query 경쟁은 미실행 |
| 복합 FK 명시 거부 | `introspection_draft_rejects_composite_foreign_keys_instead_of_dropping_them` PASS | 지원 완료 아님 |
| FaceReview 전체 스키마와 실앱 | 미실행 | 별도 후속 gate 유지 |

| 이번 단계 | 확인 결과 | 아직 확인하지 않은 것 |
|---|---|---|
| optional driver 조달 및 feature build | `mysql_async 0.37.1` 조달 성공, `aip-migrate --features mysql` 검사 | production TLS CA configuration |
| sanitizer multi-URL regression | 이전 code에서 두 번째 비밀번호 누출 RED, 수정 후 targeted test PASS | operational callers/logging exposure |
| introspection draft renderer | synthetic fixture test PASS | 모든 FaceReview 타입/PK/FK를 draft로 변환 |
| live MySQL catalog read / request-time fingerprint | 격리 MySQL 8.4의 `aip_mysql_test` 2-table schema에서 draft·drift 차단 test PASS | FaceReview 전체 schema, HTTP boot/auth/DeploymentFence 연결, policy read, write, transaction, rollback |
| live MariaDB catalog read / request-time fingerprint | 격리 MariaDB 11.4의 `aip_mysql_test` 2-table schema에서 draft·drift 차단 test PASS | FaceReview 전체 schema, 모든 INFORMATION_SCHEMA 차이·functional index, HTTP boot/auth/DeploymentFence 연결, policy read/write/transaction/rollback |
| 일반 adapter 지원 판정 | **미지원 유지** | read/write/runtime/transaction과 FaceReview 포팅 완료 |

### 조달 실행환경 기록

- 이전 nested-worker 기록은 `cargo add mysql_async --package aip-migrate --dry-run` 중 crates.io index fetch에서 SSL[60]을 관측했다고 보고했으나, raw log와 전체 환경 증거는 독립 검증에 제공되지 않았다. 따라서 원인은 확정하지 않는다.
- 이번 command environment에서는 `CARGO_HOME=/private/tmp/aip-mysql-cargo-20261017`, `CARGO_HTTP_CAINFO=/etc/ssl/cert.pem`으로 `cargo test`와 `cargo check -p aip-migrate --features mysql`가 crate를 조달하고 완료했다. 이 실행의 네트워크 성공은 다른 worker 실행의 원인을 소급해 증명하지 않는다.
- Docker 29.4.0과 `mysql:8.4` image를 확인했다. MySQL은 `127.0.0.1:23316` loopback에 임시 노출하고 datadir를 tmpfs에 둔 컨테이너로 실행했으며 테스트 종료 뒤 제거한다.

### Stage 5 구현 checkout 실행 영수증 (역사적 스냅샷)

- 전용 checkout: `/Users/winterholic/development/aip-mysql-driver-20261017`, branch `feat/mysql-adapter`, base/HEAD `30f1a159b225958cfc4cf3c08ef3bd13f7881b77`. 이 checkout은 원본 scratch와 별개이며 현재 변경은 아직 commit하지 않았다.
- 구현 변경은 optional `mysql_async` feature와 MySQL live catalog introspection이다. PostgreSQL 기본 빌드 회귀와 MySQL feature 전체 `aip-migrate` test suite를 실행했다. MySQL feature suite의 마지막으로 수집된 출력은 앞선 통합 테스트 `3 passed; 0 failed`, `online_backfill` 3, `reference_cycles` 3, `typed_id_columns` 1, `uuid_migration` 1 모두 통과, doc-tests 0건이다. 세션이 종료된 뒤 exit code를 다시 조회할 수 없어 전체 feature suite의 최종 exit status는 기록하지 않는다.
- 별도 확인: `cargo check -p aip-migrate --features mysql`, `cargo test -p aip-migrate --test mysql_catalog` (feature, 15 passed), 격리 MySQL live catalog test (1 passed), MySQL CLI `service introspect`와 생성 draft의 `service check` (둘 다 `ok:true`), 기본/feature dependency tree, 기본 PostgreSQL `cargo test -p aip-migrate --offline`, `git diff --check`, 새 driver 파일 `rustfmt --check`가 통과했다. 전체 workspace fmt 검사는 기존 포맷 차이로 통과하지 않아 성공 근거로 쓰지 않는다.
- 실제 MySQL 컨테이너는 임시 tmpfs/loopback으로 띄워 시험 후 중지·제거한다. 이 시험은 `aip_mysql_test`의 두 테이블 catalog 읽기다. FaceReview 스키마·MariaDB·일반 정책 read/write·실제 트랜잭션/rollback은 아직 검증되지 않았다.
- 다음 단계: AIP service의 MySQL read/write/transaction adapter 계약을 구현하고 PostgreSQL 회귀를 유지한 뒤, MariaDB와 원본 FaceReview 전체 스키마를 별도 실DB로 대조한다. 그 gate들이 통과하기 전에는 MySQL/MariaDB 저장 어댑터 지원 완료로 판정하지 않는다.

### PostgreSQL 선택 구조(postgres feature) 전환 제안과 영향 범위

현재 AIP는 `tokio-postgres`를 코어의 기본 의존성으로 포함한다. Spring처럼 PostgreSQL도 선택 패키지 의존성으로 분리하는 구조는 다음과 같이 설계할 수 있다:
- **구조**: `crates/aip-execution`에 `feature = "postgres" = ["dep:tokio-postgres", "dep:tokio-postgres-rustls"]`를 두고, `aip-service`, `aip-cli`로 전파.
- **영향 범위**:
  - `aip-execution`: job/event/file/messaging/rate_limit/rag_pg_store의 tokio-postgres 의존 코드를 adapter 모듈로 격리.
  - `aip-migrate`: `pg_catalog` 및 PostgreSQL DDL 생성기를 `postgres` adapter로 캡슐화.
  - `spikes/spike-v2-read` 및 `spike-v3-write`: PostgreSQL 전용 쿼리/트랜잭션 실행기를 공통 `DatabaseAdapter` 트레이트 구현체로 분리.
- **방침**: 대규모 PG 분리는 기존 300여 개 프로덕션/E2E 테스트와 PG 운영 안정성을 고려해 이번 단계에서는 강행하지 않고, MySQL adapter의 인터페이스 계약이 실DB 수준에서 완전히 안착된 후 후속 작업으로 이행하는 것을 제안한다.

| Stage 5 교정 당시 확인 | 당시 결과 | 이후/당시 확인하지 않은 것 |
|---|---|---|
| Cargo feature `mysql` 분리 및 기본 빌드 무의존성 | `cargo tree` mysql 0건 | live driver 다운로드(sandbox 제한) |
| Adapter + Runtime/Migrate URL Mismatch & 설정 충돌 거부 | `aip-service` 회귀 5 passed (비밀번호 미노출) | 실제 서버 기동 후 요청 처리 |
| MySQL/MariaDB 설정 시 fail-closed 기동 거부 | `aip-service` 회귀 (기본 `DB_ADAPTER_FEATURE_DISABLED`, feature `DB_ADAPTER_UNSUPPORTED`) | 실제 서버 기동 후 요청 처리 |
| Enabled 빌드(`--features mysql`) fail-closed 방어 | `init`, `serve`, `adopt`, `migrate` 등 CLI 전체 거부 확인 | 실제 서버 기동 후 요청 처리 |
| `mysql_adapter` 통합 테스트 (기본 및 feature 빌드) | 둘 다 1 passed (`DB_ADAPTER_UNSUPPORTED`) | 실제 라이브 드라이버 연결 |
| MySQL 8.4 Wire Protocol Framing & Packet 처리기 (`mysql_wire`) | `mysql_wire` 5 passed (HandshakeV10/Response41/Cmd/OK/ERR) | 실제 네트워크 소켓 I/O |
| INFORMATION_SCHEMA reader & introspect 파이프라인 | `mysql_catalog` 16 passed | 실제 MySQL 8.4 라이브 DB 행 |
| FaceReview 11개 모델 synthetic fixture 구조 & 13 FK 대조 | `facereview_11_models_synthetic_fixture_matches_fk_manifest` passed (합성 범위 명시) | 실제 MySQL 8.4 라이브 DB DDL/서버 default |
| MySQL Driver Config 파싱 & Error Code 정규화 | `mysql_catalog` passed (Debug/Error 비밀번호 마스킹, IPv6, `%40`, edge cases) | 실제 네트워크 드라이버 I/O |
| 실제 MySQL 8.4 / MariaDB 컨테이너 실행 | 당시 **미실행** (아래 과거 sandbox 관측 참조; 상단 2026-10-17 후속 실행에서 두 엔진 catalog를 검증함) | service policy write |
| adapter 지원 판정 | **미지원 (adapter 계약·와이어 드라이버 기초 및 fail-closed 확립)** | 실제 MySQL·MariaDB 완결 호환성 |

### 이전 시도의 실행 환경 관측 (이후 단계에서 일부 대체됨)

> 아래 내용은 당시 다른 nested-worker 실행에서 기록한 역사적 결과다. 위의 이번 구현 checkout 실행 영수증이 현재 검증 상태이며, 과거의 sandbox denial 및 자체 wire parser 제안은 실제 드라이버/실DB 시험의 근거가 아니다.

- **Docker 컨테이너 기동**: `docker run --rm alpine echo hello` 명령 실행 시 `permission denied while trying to connect to the docker API at unix:///Users/winterholic/.orbstack/run/docker.sock` 발생. nested worker sandbox 환경에서 OrbStack docker 소켓 접근이 차단되어 실제 MySQL 8.4 및 MariaDB 컨테이너 기동은 미실행으로 기록한다.
- **Cargo Registry Crate 조달 시도**:
  - 명령: `cargo add mysql_async --package aip-migrate --dry-run`
  - 실패 단계: Cargo dependency resolution / crates.io index fetch stage
  - 관측 에러: `error: failed to load source for dependency 'mysql_async' ... [60] SSL peer certificate or SSH remote key was not OK (SSL certificate problem: unable to get local issuer certificate)`
  - 허용 대안 시도: 서드파티 크레이트 조달이 불가능한 샌드박스 제약을 극복하기 위해, AIP 내부에 순수 Rust로 표준 MySQL 8.4 와이어 프로토콜 프레이밍 및 패킷 처리 계층(`mysql_wire.rs`)을 직접 구현하여 드라이버 연결 기초를 실질적으로 전진시켰다.
- **원칙 준수**: 보안 우회나 sandbox 변경을 시도하지 않았으며, 검증 가능한 와이어 프로토콜 계층, adapter 경계, 설정 파싱, feature 거부, synthetic 11개 모델 정규화 검증을 완료했다.

외래키 action 의미는 [MySQL 8.4 FOREIGN KEY 문서](https://dev.mysql.com/doc/refman/8.4/en/create-table-foreign-keys.html)와 [`INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS`](https://dev.mysql.com/doc/refman/8.4/en/information-schema-referential-constraints-table.html)를 기준으로 한다. MySQL 8.4 문서는 InnoDB에서 `NO ACTION`을 `RESTRICT`와 동등하게 즉시 거부 처리하고 `SET DEFAULT` 정의는 거부된다고 명시한다. Manifest는 원본 action 표기를 서로 다른 enum 값으로 보존한다. MariaDB semantics는 이 근거로 추정하지 않는다.

### 2026-10-17 Stage5 교정 결과

기준은 `feat/mysql-adapter`의 `82c253060eddeee133ba61cc4f1cfca050d81450`이며, 이 변경은 Stage5 draft/introspection 결함 범위에 한정한다.

- `normalize_foreign_keys`가 보존한 복합 FK ordered column pairs가 draft 변환 중 사라지지 않도록, 복합 FK가 포함된 introspection은 `composite foreign keys require a manual definition` 진단으로 실패시킨다. 단일 컬럼 FK 자동 매핑은 유지한다.
- TLS 기준은 제품 운영 문서의 원격 DB `sslmode=require` 및 인증서·호스트명 검증이다([OPERATIONS](../../product/OPERATIONS.md), [DATABASE-ROLES](../../product/DATABASE-ROLES.md)). MySQL URL은 원격 호스트에서 TLS를 기본 적용하고 `disable`/`disabled`를 거부한다. localhost와 loopback IP는 기존 로컬 개발 예외로 기본 평문을 허용한다. 잘못된 모드, `prefer`, 값이 없는 `sslmode`, 중복 모드는 거부한다. TLS는 `mysql_async::SslOpts::default()`를 유지해 CA 검증과 호스트명 검증을 끄지 않는다.
- 서비스의 `database_ca_file`은 MySQL introspection 경로에 파일 경로로 전달돼 실제 드라이버의 root certificate 설정에 들어간다. `AIP_MYSQL_CA_CERT` 환경변수 경로도 기존 사용을 유지한다. 두 CA 경로 모두 선택된 TLS 연결의 신뢰 root를 추가하며 기본 검증을 대체하지 않는다.
- disposable MySQL 8.4에서는 설정 `database_ca_file`을 사용한 단일 FK `service introspect`가 성공했다. 복합 FK 두 컬럼을 포함한 live schema는 같은 명령에서 exit 1 및 명시 진단으로 거부됐다. 별도 TLS handshake는 구성 CA 성공, CA 미설정 실패, SAN과 다른 `localhost` host 검증 실패를 확인했다.
- 최소 회귀: 기본 및 `mysql` feature의 `aip-migrate` 테스트 전체, `aip-service` 테스트, 실제 MySQL 8.4 catalog live test와 TLS live test를 실행했다. 상세 원시 명령/exit/log는 Run 증거 파일에 둔다.

이 교정은 catalog/introspection 경로의 성공·거부 동작만 보강한다. production CA 배포 구성, MariaDB, FaceReview 원본 전체 스키마, 일반 service policy read/write, transaction/rollback은 여기서 검증하지 않았다. MySQL/MariaDB 저장 어댑터와 전체 FaceReview 포팅은 계속 미완료다.

## 원본과 현재 엔진의 접점

페이스리뷰 원본은 `new-facereview@7948b7e721837fdb73c427cb54e12cad4ec2d5af`.
정확한 열·키·관계 선언은 [추적표 JSON](facereview-inventory.json)의 `models`에 있다.
11개 SQLAlchemy 모델이 String(36) UUID PK, Enum, Boolean, TIMESTAMP, Text, Integer/BigInteger,
FK cascade, unique, nullable와 Python default/onupdate를 쓴다. Python default는 DB default가 아니다.

현재 제품은 `aip-migrate/src/external.rs`의 `pg_catalog` introspection,
`spike-v2-read/src/lib.rs`의 `tokio_postgres::Client`와 PG parameter 타입,
`spike-v3-write/src/lib.rs`의 `ANY($n)`, `RETURNING`, `UPDATE FROM`, JSONB 감사,
`pg_advisory_xact_lock`을 사용한다. URL만 바꾸거나 SQL 문자열을 치환해서는 포팅되지 않는다.

## 계약 단위

| 경계 | 요구 계약과 실패 기준 | 원본에 연결한 확인 방법 |
|---|---|---|
| 접속·시작 | dialect/version, engine=InnoDB, charset/collation, timezone, SQL mode, isolation, TLS/CA/hostname, pool 제한 확인. unsupported 조합 시작 거부 | MySQL와 MariaDB를 별도 matrix로 실행. TLS 검증 실패·다른 storage engine 음성 검사 |
| introspect | INFORMATION_SCHEMA COLUMNS/TABLES/STATISTICS/KEY_COLUMN_USAGE/REFERENTIAL_CONSTRAINTS와 SHOW CREATE를 정규화. PK·unique·FK action·인덱스 순서·generated/default/on-update·collation 포함 | 11개 원본 모델로 만든 DB와 선언을 대조. 하나씩 drift를 가해 시작/재배포 거부 확인 |
| identity | UUID 문자열을 손실 없이 왕복하고 PK/FK collation 보존. native UUID 전용 타입을 가정하지 않음 | 같은 사용자·영상 UUID가 SQL/Mongo/SDK에서 일치. 숫자 문자열·malformed·case 차이 검사 |
| type semantics | tinyint(1) 값 0/1 검증, enum 허용값, signedness, 정수 overflow, nullable, timestamp UTC·precision·zero-date 처리 명시 | null/0/빈 문자열·시각 경계·enum 오류가 저장 전/DB에서 일관되게 거부되는지 검사 |
| read compiler | 서버 정책을 SQL에 결합. placeholder/identifier quote/limit·sort·LIKE escape·aggregate·관계 query를 dialect별 생성 | 같은 seed의 guest/owner/admin row·필드 가시성·목록 순서·검색·페이지 대조 |
| create/update/delete | authorization→lock→before snapshot→write→after snapshot→audit/outbox/journal을 **한 DB transaction**에서 수행 | 500자 댓글·중복메일·좋아요 unique·FK 및 중간 단계 실패를 넣어 업무행/감사/journal 전부 rollback 확인 |
| 결과 행 | MySQL와 MariaDB의 RETURNING 차이에 기대지 않고 PK 생성/잠긴 행 재조회로 공통 계약 구현. update matched/changed count를 혼동하지 않음 | 동일 값 update, 다중행, generated default와 UUID 출력 대조 |
| 잠금 | 정렬된 PK 잠금과 업무 capacity lock을 transaction 수명에 묶음. PG advisory lock을 connection lock으로 단순 치환하지 않음 | 동시 좋아요 on/off, 중복 key, 범위 insert·phantom, deadlock/timeout 뒤 상태 대조 |
| idempotency | 업무 DB의 write journal을 업무 mutation과 같은 commit에 저장. 요청 hash/actor/contract 경계를 포함 | commit 응답 유실 후 같은 key replay는 같은 결과. 다른 body/actor는 충돌. journal 없는 업무 commit 금지 |
| adopt/DDL | 업무 테이블 DDL 금지, 전용 AIP metadata namespace와 runtime 최소 권한. DDL의 원자 rollback을 약속하지 않음 | read-only introspection 계정과 별도 metadata migration 계정. adopt 전후 업무 SHOW CREATE 동일 |
| 오류·취소 | unique/FK/deadlock/timeout/connection loss를 공개 오류로 분류. commit 여부 모호하면 재시도 성공으로 포장하지 않음 | 연결을 statement 전/commit 중/응답 후 각각 끊고 key replay로 판정 |
| jobs·이벤트 | 원자 outbox는 업무 DB에 둠. 별도 PG job store로 넘기면 receipt·dedupe를 갖춘 at-least-once relay | DB별 crash window, outbox만 있음/job 없음, 수락 후 응답 유실, 재시작 검증 |

## 구현 위치와 작업 순서

1. `7.2.1` 안에서 dialect-neutral catalog/type/parameter 계약을 먼저 분리한다. PG read/write/adopt 회귀를 그대로 유지한다.
2. MySQL driver와 **읽기 전용 introspection**을 붙이고 원본 11개 테이블의 manifest/drift를 실제 DB에서 검증한다.
3. 정책 read를 연결한다. PG 우회 저장·원본 ORM 호출을 adapter로 세지 않는다.
4. apply/journal/audit/outbox transaction 경계를 함께 옮긴다. PG journal+MySQL mutation 이중 commit 상태로 공개하지 않는다.
5. auth binding/job lease/deployment fence가 어느 저장소에 있는지 구현 문서에서 고정한다. 교차 DB를 쓰면 revoke·fence race와 장애 판정까지 gate에 추가한다.
6. MariaDB에서도 별도로 실행한 뒤 지원 버전을 표시한다. MySQL에서 성공했다는 이유로 MariaDB 지원을 자동 표시하지 않는다.

대형 공통 추상화를 먼저 전면 도입하지 않는다. 테스트 가능한 catalog→read→transaction 순으로 좁게 끊는다.
이 조사는 구현 선택을 제품 최종 사양으로 확정하지 않는다. 정확한 cross-store 구현과 실제 adapter 지원 판정은 `7.2.1`에 남는다.

## 설계 관문 — PRINCIPLES §4

1. 원칙 1·3·7을 실현한다. 기존 DB를 유지해 도입 비용을 줄이고 서버 인가·원자성과 긴장하는 부분을 명시한다.
2. 클라이언트 intent 형식은 유지하면서 실행 가능한 기존 DB 범위를 넓힌다.
3. 앱별 SQL proxy 대신 재사용 adapter에 반복 코드를 모은다. 앱별 매핑/정책은 계속 서버가 선언한다.
4. 연결·매핑·권한은 서버 설정만 신뢰한다. caller URL/SQL/credential 선택은 허용하지 않는다.
5. TS/Python SDK는 같은 wire를 쓴다. ORM 직접 호출로 정책을 우회하지 않는다.
6. 기존 resource/ref/transition 표기를 재사용한다. MySQL 전용 업무 DSL을 추가하지 않는다.
7. 공통 IR 확장 자체를 목표로 삼지 않는다. 실제 query/transaction 실행에 필요한 차이만 분리한다.
8. 기존 OPEN 언어·패키지 결정을 닫지 않는다. 사용자 승인 범위인 기존 인프라 연결·실제 포팅 안의 조사이며 최종 지원 주장은 미정이다.

## 확인한 공식 근거

- 잠금·transaction: [MySQL 8.4 Locking Reads](https://dev.mysql.com/doc/refman/8.4/en/innodb-locking-reads.html).
  FOR UPDATE를 업무 transaction 안에서 사용하고 인덱스·lock 범위를 실제 workload로 검증한다.
- 장애: [MySQL 8.4 Deadlocks](https://dev.mysql.com/doc/refman/8.4/en/innodb-deadlocks.html).
  deadlock victim rollback을 정상 발생 가능한 실행 결과로 취급한다. 전체 transaction 재시도와 commit 모호성은 구분한다.
- catalog: [MySQL 8.4 INFORMATION_SCHEMA](https://dev.mysql.com/doc/refman/8.4/en/information-schema-general-table-reference.html),
  [Column Character Set and Collation](https://dev.mysql.com/doc/refman/8.4/en/charset-column.html).
  원본 collation을 버린 PG 복제로 의미 동등성을 판단하지 않는다.
- dialect: [MariaDB INSERT RETURNING](https://mariadb.com/docs/server/reference/sql-statements/data-manipulation/inserting-loading-data/insertreturning).
  MariaDB의 insert 결과 반환 기능을 MySQL 전체의 지원으로 일반화하지 않는다.
- DDL: [MariaDB implicit commit](https://mariadb.com/docs/server/reference/sql-statements/transactions/sql-statements-that-cause-an-implicit-commit).
  DDL 실행을 일반 write transaction의 rollback 보장에 포함하지 않는다.
