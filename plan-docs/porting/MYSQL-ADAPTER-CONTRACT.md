# MySQL/MariaDB adapter 조사 — WBS 5.2.4

2026-10-15. 범위는 계약 조사다. **AIP MySQL adapter는 아직 구현되지 않았다.**
원본 앱이 MySQL에서 실행되는 것과 AIP가 MySQL을 지원하는 것은 다른 판정이다.

## 2026-10-17 실제 제품 경로 확인

`aip-migrate::external::introspect`가 PostgreSQL `tokio_postgres` 연결과 `pg_catalog` 쿼리만 사용한다. MySQL/MariaDB URL을 넘기는 검사는 실제 driver/catalog 시험이 아니며, 이전에는 PostgreSQL connect 단계의 일반 `DB_CONNECT` 오류로 끝났다. 이제 이 두 scheme은 `DB_ADAPTER_UNSUPPORTED`로 명시 거부한다. 테스트는 오류에 합성 URL 비밀번호가 노출되지 않는 것도 고정한다.

후속 순수 변환 단위에서 `aip_migrate::mysql_catalog::normalize_table`을 추가했다. INFORMATION_SCHEMA COLUMNS/STATISTICS에서 온 typed row를 driver 없이 받으며, 컬럼 순서·정규화 타입·부호/표시 폭·null/default/generated·PK/unique index 순서를 반환한다. 중복/비연속 column ordinal, 없는 column을 참조하는 index, index 멤버 간 `NON_UNIQUE` 불일치는 오류다. `CHAR`/`VARCHAR` 길이는 현재 `Text`로 축약되므로 길이 비보존 진단을 남긴다. 작은 현재 manifest 경계 밖인 collation, generation expression, on-update, auto_increment, functional index expression, 미지원 타입은 diagnostics에 남긴다. `tinyint(1)`은 값이 0/1로 제한됐다는 뜻으로 단정하지 않고 정수 metadata와 alias 표식·진단을 함께 보존한다.

`normalize_foreign_keys`는 future driver가 `KEY_COLUMN_USAGE`와 `REFERENTIAL_CONSTRAINTS`를 결합해 제공하는 driver 비의존 row를 받아 constraint 단위 manifest를 만든다. constraint/source schema·table·이름을 분리 키로 유지하고 ordered source/target column pairs, referenced schema/table, `ON DELETE`/`ON UPDATE` actions를 보존한다. 비연속 순서·누락 source/target/action·action 또는 대상 불일치는 오류로 닫는다. 현재 인식하는 action은 `CASCADE`, `SET NULL`, `RESTRICT`, `NO ACTION`; `SET DEFAULT`와 알 수 없는 값은 unsupported 오류다. 원본 11개 RDB 모델의 선언 기준은 [FK manifest](facereview-fk-manifest.json)다. 기존 댓글 PostgreSQL fixture는 이 중 Comment의 두 참조만 담는다. 이름과 실제 참조 schema는 원본 선언에 없으므로 manifest 예상치에서 만들지 않고, catalog 입력/출력에서만 보존한다.

`COLUMN_DEFAULT = NULL`은 MySQL 8.4에서 명시적 `DEFAULT NULL`과 DEFAULT 절 부재 양쪽에 쓰이므로 `NullOrAbsent`로 표현하고 모호성 진단을 남긴다. 드라이버 연결 시 `SHOW CREATE TABLE` 등으로 구분해야 한다 ([MySQL 8.4 COLUMNS](https://dev.mysql.com/doc/refman/8.4/en/information-schema-columns-table.html)).

### DB Adapter 경계, Adapter-URL Mismatch 방어 및 드라이버 연결 계약 구현

1. **선택적 패키지 feature 분리**:
   - `crates/aip-execution`, `crates/aip-migrate`, `crates/aip-service`, `crates/aip-cli`에 Cargo feature `mysql`을 추가했다.
   - 기본 빌드에서는 `mysql` feature가 비활성화되며, `cargo tree --offline -p aip-cli -e normal | grep -i mysql` 결과가 비어 있음을 확인했다(의존성 0건).
2. **Adapter + Runtime/Migrate URL Mismatch 방어 및 환경 충돌 검사 (Secret-Free)**:
   - `resolve_database_adapter`에 Adapter와 Runtime URL(`database_url_env`) 및 Migration URL(`migrate_database_url_env`) 간 일치성 검증을 도입했다.
   - `postgres` 어댑터에 `mysql://`/`mariadb://` URL이 지정되거나, `mysql`/`mariadb` 어댑터에 `postgres://` URL이 지정되면 `DB_ADAPTER_MISMATCH`로 즉시 거부한다.
   - Runtime URL과 Migrate URL 간 scheme이 불일치하는 경우(예: Runtime은 PG인데 Migrate URL은 MySQL) 역시 `DB_ADAPTER_MISMATCH`로 즉시 차단한다.
   - `database_adapter` 필드와 `database_adapter_env` 환경변수가 서로 다른 어댑터를 지정하면 `BAD_CONFIG`로 거부한다.
   - 오류 메시지 및 로깅 시 URL 비밀번호(`super_secret_pw` 등)는 일절 노출되지 않으며 scheme과 어댑터 식별자만 안전하게 표기된다.
3. **Enabled 빌드(`--features mysql`)에서의 Fail-Closed 전체 진입점 누출 방어**:
   - `#[cfg(feature = "mysql")]` 빌드에서도, 미구현된 MySQL 라이브 커넥터가 기존 PostgreSQL 연결(`tokio-postgres`)로 조용히 흘러가지 않도록 `require_built_features`에서 모든 명령(`serve`, `service init`, `migrate` 등)에 대해 사전 명시적 `DB_ADAPTER_UNSUPPORTED`로 fail-closed 거부한다.
4. **INFORMATION_SCHEMA Query Reader 및 Driver 연결 계층**:
   - MySQL 8.4 공식 Reference에 따른 `SQL_TABLES`, `SQL_COLUMNS`, `SQL_STATISTICS`, `SQL_FOREIGN_KEYS` 쿼리 상수를 확립했다.
   - 비동기 행 수집 계약 `MysqlCatalogReader` trait(`fetch_tables`, `fetch_columns`, `fetch_indexes`, `fetch_foreign_keys`)과 이를 `normalize_table`/`normalize_foreign_keys`로 연결하는 `introspect_table`, `introspect_schema` 파이프라인을 구현했다.
   - 드라이버 설정 `MysqlDriverConfig`: URL 파싱, connect timeout(5s), statement timeout(10s), pool size(min 1, max 10), TLS 지원, `display_sanitized`를 통한 credential 마스킹(`***`)을 구현했다.
   - 에러 정규화 `normalize_mysql_error_code`: 1045(`DB_AUTH_FAILED`), 1049(`DB_NOT_FOUND`), 1146(`TABLE_NOT_FOUND`), 1062(`UNIQUE_VIOLATION`), 1213(`DEADLOCK_DETECTED`), 1205(`LOCK_TIMEOUT`), 1451/1452(`FOREIGN_KEY_VIOLATION`), 2002/2003(`DB_CONNECT_FAILED`) 및 raw credentials sanitize.
5. **FaceReview 11개 모델 전체 81컬럼 선언, 실제 PK, 고유/복합 인덱스 및 13개 FK 대조**:
   - [facereview-inventory.json](facereview-inventory.json) 및 [facereview-fk-manifest.json](facereview-fk-manifest.json)의 11개 SQLAlchemy 모델 전체 선언을 대조하는 `facereview_11_models_full_schema_matches_declaration` 테스트를 작성했다.
   - 합성 2컬럼 round-trip이 아닌 11개 모델의 총 81개 컬럼(정확한 타입, nullable, default), 실제 PK(특히 `UserEmotionSummary`의 PK `user_id` 포함), 고유 제약(`uk_user_genre`, `uk_bookmark_user_video` 등), 복합 인덱스(`idx_video_comments`, `idx_user_history`, `idx_video_history`), 총 13개 외래키의 완전 일치를 검증했다(13 passed).

### PostgreSQL 선택 구조(postgres feature) 전환 제안과 영향 범위

현재 AIP는 `tokio-postgres`를 코어의 기본 의존성으로 포함한다. Spring처럼 PostgreSQL도 선택 패키지 의존성으로 분리하는 구조는 다음과 같이 설계할 수 있다:
- **구조**: `crates/aip-execution`에 `feature = "postgres" = ["dep:tokio-postgres", "dep:tokio-postgres-rustls"]`를 두고, `aip-service`, `aip-cli`로 전파.
- **영향 범위**:
  - `aip-execution`: job/event/file/messaging/rate_limit/rag_pg_store의 tokio-postgres 의존 코드를 adapter 모듈로 격리.
  - `aip-migrate`: `pg_catalog` 및 PostgreSQL DDL 생성기를 `postgres` adapter로 캡슐화.
  - `spikes/spike-v2-read` 및 `spike-v3-write`: PostgreSQL 전용 쿼리/트랜잭션 실행기를 공통 `DatabaseAdapter` 트레이트 구현체로 분리.
- **방침**: 대규모 PG 분리는 기존 300여 개 프로덕션/E2E 테스트와 PG 운영 안정성을 고려해 이번 단계에서는 강행하지 않고, MySQL adapter의 인터페이스 계약이 실DB 수준에서 완전히 안착된 후 후속 작업으로 이행하는 것을 제안한다.

| 이번 확인 | 결과 | 확인하지 않은 것 |
|---|---|---|
| Cargo feature `mysql` 분리 및 기본 빌드 무의존성 | `cargo tree` mysql 0건 | live driver 다운로드(sandbox 제한) |
| Adapter + Runtime/Migrate URL Mismatch & 설정 충돌 거부 | `aip-service` 회귀 5 passed (비밀번호 미노출) | 실제 서버 기동 후 요청 처리 |
| MySQL/MariaDB 설정 시 fail-closed 기동 거부 | `aip-service` 회귀 5 passed (기본 및 `--features mysql`) | 실제 서버 기동 후 요청 처리 |
| Enabled 빌드(`--features mysql`) fail-closed 방어 | `aip-service` 회귀 passed (`DB_ADAPTER_UNSUPPORTED`) | 실제 서버 기동 후 요청 처리 |
| INFORMATION_SCHEMA reader & introspect 파이프라인 | `mysql_catalog` 13 passed | 실제 MySQL 8.4 라이브 DB 행 |
| FaceReview 11개 모델 전체 81컬럼, PK, 복합인덱스, 13개 FK 선언 대조 | `facereview_11_models_full_schema_matches_declaration` passed | 실제 MySQL 8.4 라이브 DB 행 |
| MySQL Driver Config 파싱 & Error Code 정규화 | `mysql_catalog` 회귀 passed (비밀번호 마스킹) | 실제 네트워크 드라이버 I/O |
| 실제 MySQL 8.4 / MariaDB 컨테이너 실행 | **미실행** (이유 아래 참조) | 실제 컨테이너 연결 및 정책 write |
| adapter 지원 판정 | **미지원 (adapter 계약·driver 연결 계층 및 fail-closed 확립)** | 실제 MySQL·MariaDB 호환성 |

### 2026-10-17 실행 환경 제약 및 관측 증거

- **Docker 컨테이너 기동**: `docker run --rm alpine echo hello` 명령 실행 시 `permission denied while trying to connect to the docker API at unix:///Users/winterholic/.orbstack/run/docker.sock` 발생. nested worker sandbox 환경에서 OrbStack docker 소켓 접근이 차단되어 실제 MySQL 8.4 및 MariaDB 컨테이너 기동은 미실행으로 기록한다.
- **Cargo Registry Crate 조달**: `cargo search` / download 실행 시 `[60] SSL peer certificate or SSL remote key was not OK (SSL certificate problem: unable to get local issuer certificate)` 및 CA 파일 접근 거부 발생. 쓰기 가능한 `$CARGO_HOME` 경로에 로컬 레지스트리를 미러링하여 오프라인 검증은 정상 수행했으나, 신규 외부 크레이트의 라이브 네트워크 조달은 sandbox 보안 정책에 의해 제한된다.
- **원칙 준수**: 보안 우회나 sandbox 변경을 시도하지 않았으며, driver 없이 검증 가능한 adapter 경계, 설정 파싱, feature 거부, synthetic 11개 모델 정규화 검증을 완료했다.

외래키 action 의미는 [MySQL 8.4 FOREIGN KEY 문서](https://dev.mysql.com/doc/refman/8.4/en/create-table-foreign-keys.html)와 [`INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS`](https://dev.mysql.com/doc/refman/8.4/en/information-schema-referential-constraints-table.html)를 기준으로 한다. MySQL 8.4 문서는 InnoDB에서 `NO ACTION`을 `RESTRICT`와 동등하게 즉시 거부 처리하고 `SET DEFAULT` 정의는 거부된다고 명시한다. Manifest는 원본 action 표기를 서로 다른 enum 값으로 보존한다. MariaDB semantics는 이 근거로 추정하지 않는다.

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
