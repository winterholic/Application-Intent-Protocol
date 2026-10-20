# MySQL/MariaDB adapter 조사 — WBS 5.2.4

2026-10-15 조사에서 시작한 구현 작업의 현재 기록이다. 원본 앱이 MySQL에서 실행되는 것과 AIP가 MySQL을 지원하는 것은 다른 판정이다.

**현재 상태(2026-10-20 정정): MySQL 8.4 읽기 경로(WBS 7.2.1.1)와 쓰기 경로(WBS 7.2.1.2: `/apply` 전이·`atomic` 묶음·create, `/status`)가 `aip service`의 `adopt-external`·`principal`·`serve`까지 연결돼 있다. 후속 커밋 `ec5db25`와 `1c10788`에서 relational worker 읽기·쓰기 bridge도 연결됐다. `workers` 설정 자체는 허용하고 쓰기는 `enable_writes`로 켠다. Durable job 저장소(`mysql_jobs`)와 outbox 릴레이(`mysql_outbox`)는 라이브러리 수준이며 HTTP `/jobs/*`와 비동기 job 실행은 미연결이다(WBS 7.2.1.3 일부). 서버 능력 probe는 MariaDB 11.4의 서비스 읽기·쓰기를 LATERAL·`FOR SHARE OF` 부재로 거부한다. 파일·이벤트·메시지·rate limit·`compose`·비동기 job 실행은 계속 거부한다.**

2026-10-20 [Database contracts 39398093710](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39398093710)의 Linux MySQL 8.4 HTTP 읽기 1건·쓰기 9건 및 MySQL/MariaDB 펜스 시험이 통과했다. 읽기는 DATETIME·TIMESTAMP의 UTC 출력과 Bool·Decimal을, 펜스는 ALTER·DROP·RENAME·TRUNCATE의 실제 잠금 오류 1205 및 rollback 뒤 해제를 포함한다. 같은 실행의 CLI 채택 시험은 옛 `workers` 일괄 거부 단언에서 실패했다. 이 단언을 worker 설정 허용과 `workers.enable_file_uploads` 거부의 양방향 검사로 바꿨다. 전체 workflow 성공이나 실제 격리 Node/Python 실행은 이 부분 결과만으로 주장하지 않는다. 후속 결과는 수정 커밋의 CI 실행 기록을 기준으로 한다.

## 2026-10-20 복합 키 채택 경계 보완 (WBS 7.2.1.1)

수동 매핑이 복합 FK의 컬럼을 일반 `Int`로 선언하면 기존 채택 검사가 그 물리적 관계를 무시했다. 실제 MySQL 8.4에서 `adopt-external` 계획이 잘못 허용되는 RED를 확인했다. 자동 초안과 명시적 채택이 같은 키 검사를 사용하도록 바꿨다. 매핑된 표의 복합 FK는 `MYSQL.UNSUPPORTED_COMPOSITE_FK`, 복합 PK는 `MYSQL.UNSUPPORTED_COMPOSITE_PK`로 거부하며 표·제약 이름을 메시지에 담는다. `service introspect`도 이 코드를 JSON에 보존한다. 복합 UNIQUE 인덱스는 허용한다.

실제 CLI 회귀는 단일 키 계획 허용 → 복합 FK 추가 → 계획/적용/초안 거부 → 복합 PK로 교체 → 같은 거부 → 단일 키 복원 후 계획/초안 허용을 실행한다. 거부 뒤 업무 행 보존·AIP schema 미생성·출력 파일 미생성도 검사한다. `cargo test -p aip-cli --features mysql --test mysql_adoption_flow --locked -- --ignored --nocapture --test-threads=1`은 MySQL 8.4 실DB에서 2 passed다. HTTP 읽기 1건·쓰기 9건, catalog 18건, 초안 단위 2건도 통과했다. 원시 RED/GREEN·회귀 로그는 Run `43571658-180b-4a19-895f-3fd7feea8256` outbox에 보존한다. Linux CI 결과는 해당 커밋 실행으로 별도 확인한다.

아래 날짜별 절의 미지원·미실행 표시는 당시 이력이며 현재 제품 범위는 문서 첫머리와 이 절을 따른다. MySQL 비동기 job 실행과 전체 FaceReview 포팅은 계속 미완이다.

## 2026-10-20 job 저장소의 두 엔진 CI 연결 (WBS 7.2.1.3 일부)

기존 `mysql_jobs` 라이브 시험은 실제 MySQL 8.4와 MariaDB 11.4에서 각각 3 passed였다. 40개 run을 8개 연결에서 claim하는 경쟁, lease epoch·멱등·재시도·취소·결과 만료, outbox batch rollback 및 4개 relay의 중복 방지를 실행한다. `Database contracts`에 `AIP_TEST_JOBS_URL`을 각 엔진의 전용 service URL로 지정하는 별도 두 단계를 추가했다. 값이 없으면 skip이 아니라 실패하는 기존 시험을 그대로 쓴다. 원시 로컬 실행은 Run `43571658-180b-4a19-895f-3fd7feea8256` outbox의 `jobs-mysql.log`와 `jobs-mariadb.log`다. 실제 HTTP `/jobs/*`와 worker의 비동기 실행은 이 저장소 시험에 포함하지 않으며 7.2.1.3은 미완으로 유지한다.

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

### 2026-10-17 Stage 6 DDL/transaction 경쟁 안전계약 확정 및 구현 (WBS 7.2.1.1)

- **배경**: 초기 경쟁시험에서 `INFORMATION_SCHEMA` 조회 자체는 대상 업무 테이블의 `MDL_SHARED_READ`를 획득하지 않아 외부 `ALTER TABLE`을 막지 못하는 현상이 확인되었다.
- **확정 안전계약 구현 (`MYSQL-DDL-FENCE-SAFETY-CONTRACT-DECISION.md`)**:
  0. **배포 펜스 값 객체**: `MysqlDeploymentFence::new(meta_schema, source_schema, source_tables, facts_digest, structure_digest)`가 생성 시점에 읽기용(메타 2개 + 소스 전체)과 쓰기용(메타 8개 + 소스 전체) 정규 `bound_tables`를 `canonical_bound_tables`로 한 번 계산해 고정한다. 빈 입력이나 백틱이 든 식별자는 `MIGRATE.DRIFT_DETECTED`로 거부한다. 요청 경로의 공개 진입점은 `begin_fenced_transaction(read_only, &fence)` 하나다. 테이블 목록을 직접 받는 `begin_fenced_transaction_with_bounds(read_only, meta, source, bound_tables, facts, structure)`는 `#[doc(hidden)]` 저수준 진입점으로, `fence` 진입점이 내부에서 호출하며 반례 테스트도 쓴다. 이 경로도 같은 Step 2c를 거치므로 불완전한 `bound_tables`는 트랜잭션을 만들지 못하고 `MIGRATE.DRIFT_DETECTED`로 거부된다.
  1. 풀에서 `mysql_async` 트랜잭션(`Pool::start_transaction`, `REPEATABLE READ`, 읽기 전용 여부는 `TxOpts::with_readonly`)을 열고 `SET SESSION information_schema_stats_expiry = 0`을 적용한다. 이 서버 쪽 접근 모드 지정은 `START TRANSACTION READ ONLY`와 동치가 아니라 `SET TRANSACTION READ ONLY|WRITE` 다음 `START TRANSACTION`이다.
  2. **Step 1 (선행 MDL Pin)**: `INFORMATION_SCHEMA` 조회 전에 정렬된 모든 바인딩 테이블(메타 테이블 + source schema 업무 테이블)에 대해 단일 UNION ALL 쿼리(`SELECT 1 FROM (SELECT 1 FROM \`s\`.\`t\` LIMIT 1) AS p0 UNION ALL ...`)를 실행해 트랜잭션 종료 시까지 `MDL_SHARED_READ`를 선점. (0행 빈 테이블에서도 외부 ALTER를 즉시 차단).
  3. **Step 2 (동일 Tx 내 광역 4-쿼리 검증)**: 동일 연결·트랜잭션에서 `aip_migrate_meta` 마커(`facts_digest`, `status = 'READY'`)를 확인하고, 테이블별 N회 왕복 대신 광역 4-쿼리(`SQL_COLUMNS_BY_SCHEMA`, `SQL_STATISTICS_BY_SCHEMA`/`SQL_STATISTICS_MARIADB_BY_SCHEMA`, `SQL_FOREIGN_KEYS_BY_SCHEMA`)로 전체 `CanonicalSchemaCatalog` 다이제스트를 캐시 없이(TTL=0) 검증.
  3b. **Step 2c (완전 바인딩 집합 불변식)**: 같은 트랜잭션에서 읽은 라이브 카탈로그의 테이블 집합으로 `canonical_bound_tables(meta_schema, source_schema, &live_catalog_tables, !read_only)`를 다시 계산해 Step 1에서 실제로 고정한 `bound_tables`와 정확히 비교한다. 소스 테이블이 하나라도 빠졌거나 쓰기용 메타 테이블이 빠지면 `ROLLBACK` 후 `MIGRATE.DRIFT_DETECTED`다. 8a27b94는 이 비교가 없어 호출자가 일부 테이블을 빼면 해당 테이블에 `MDL_SHARED_READ`가 없는 채로 외부 `ALTER TABLE`이 성공했다.
  4. **Fail-Closed 롤백**: 다이제스트 불일치나 MySQL 에러 `1146`(ER_NO_SUCH_TABLE), `1054`(ER_BAD_FIELD_ERROR), `1412`(ER_TABLE_DEF_CHANGED) 발생 시 즉시 `ROLLBACK` 후 `MIGRATE.DRIFT_DETECTED` 반환.
  5. `FencedMysqlTransaction`은 `Option<mysql_async::Transaction<'static>>`만 보관하고 `into_conn()`을 제공하지 않는다. 쿼리는 `as_mut()`로 빌린 트랜잭션에서만 실행한다. drop·task abort 시 mysql_async가 연결에 `RequiresRollback`을 표시하고 풀이 그 연결을 재사용하기 전에 롤백한다. 즉 `drop`이 `ROLLBACK`을 동기 실행하는 것이 아니라 풀 재사용 시점까지 지연될 수 있다(실측은 아래 테스트, 대용량 스트리밍 도중 절단은 미검증).
  6. **메타 상태 2단계 전이**: MySQL DDL은 암묵 커밋이라 롤백할 수 없으므로 `ensure_meta_schema_bootstrap`은 `aip_migrate_meta` 생성 직후 `status='MIGRATING'`을 먼저 기록하고, 나머지 메타 테이블 생성과 라이브 카탈로그 재조회가 끝난 뒤에만 `facts_digest`·`schema_sha256`과 함께 `READY`로 바꾼다. 중단되면 `MIGRATING`이나 행 없음으로 남아 Step 2a가 `MIGRATE.DRIFT_DETECTED`로 거부한다. 재실행은 멱등이며 `READY`로 수렴한다. 나머지 7개 메타 테이블은 PK 열만 가진 자리표시 DDL이고 실제 형태는 7.2.1.2/7.2.1.3에서 정한다.
- **실 DB 격리 컨테이너 실측 검증 (MySQL 8.4.11 및 MariaDB 11.4.13)**:
  - `live_pre_mdl_pin_blocks_concurrent_ddl_and_detects_prior_ddl_on_same_tx`: 두 엔진 모두 PASS. (DURING 중 외부 ALTER는 `lock_wait_timeout`(1205) 차단, 내부 SELECT 성공, ROLLBACK 즉시 MDL 해제되어 외부 ALTER 성공, 그 후 다음 검사 시 즉시 409 drift 감지, 사전 DROP TABLE 시 1146 drift 감지).
  - `live_begin_fenced_transaction_detects_unrelated_create_table_on_next_request`: 두 엔진 모두 PASS. (외부에서 `source_schema`에 무관한 신규 테이블 `CREATE TABLE unrelated` 생성 시 0ms 지연으로 다음 첫 `begin_fenced_transaction`에서 `MIGRATE.DRIFT_DETECTED` 즉시 거부).
  - `live_begin_fenced_transaction_validates_meta_marker_status_and_digest`: 두 엔진 모두 PASS. (`facts_digest` 불일치 및 non-READY status 즉시 거부).
  - `crates/aip-migrate/tests/mysql_fence_verification.rs` (8개: 오프라인 3개는 항상 실행, 라이브 5개는 `#[ignore]`라 `-- --ignored`로 명시 실행. 실행 전 `AIP_TEST_MYSQL_URL` 또는 `AIP_TEST_MARIADB_URL`이 없거나 `aip_test_support::validate_mysql_admin_url`을 통과하지 못하면 panic으로 실패하며 `SKIPPED` 통과 경로는 없다. 두 URL의 스킴은 각각 `mysql://`, `mariadb://`여야 한다): `live_fence_rejects_incomplete_bound_table_set`(일부 테이블 누락 3종과 부분 목록으로 만든 fence 모두 거부, 완전한 fence는 모든 소스 테이블의 외부 ALTER를 1205로 차단), `live_fence_detects_dropped_and_created_source_tables_before_returning`, `live_readwrite_fence_commits_rolls_back_and_runs_with_dml_only_grants`(DML 전용 계정), `live_cancelled_fenced_transaction_rolls_back_before_connection_reuse`(`pool_max=1`에서 drop과 task abort 모두 미커밋 행 0건), `live_interrupted_meta_migration_rejects_entry_until_ready_again`(`MIGRATING`·행 없음 거부, 재실행 후 `READY`), 그리고 `MysqlDeploymentFence` 오프라인 3개. Step 2c를 끄면 첫 테스트가 실패함을 확인했다.
  - `schema_wide_introspect_produces_identical_catalog_to_sequential_introspect`: 단위 테스트 PASS (광역 4-쿼리 결과가 순차 `introspect_schema`와 바이트 단위 일치).
  - `facereview_11_models_synthetic_fixture_matches_fk_manifest`: 11개 모델 전체 광역 4-쿼리 일치 검증 PASS.

| Stage 6 gate | 결과 | 상세 / 미실행 항목 |
|---|---|---|
| feature-off fail-closed | PASS | `aip-service` 기본 빌드 전체 및 `mysql_url_or_adapter_fails_closed_before_pg_connect` 통과 |
| PostgreSQL 회귀 | PASS | 기본 `aip-service` 및 `aip-migrate` 테스트 통과 |
| MySQL feature fail-closed | PASS | `aip-service --features mysql` 전체 테스트 통과; MySQL URL rejection 유지 |
| `mysql_driver` fenced transaction runtime contract | PASS (`aip-service` HTTP `serve` wiring remains gated by `DB_ADAPTER_UNSUPPORTED` until service-layer integration) | `aip-migrate::mysql_driver` 계층만 검증: 선행 MDL Pin, 4-쿼리 검증, Step 2c 완전 바인딩 집합, drop 시 롤백, `MIGRATING` 거부를 두 엔진에서 실측. `aip-service`는 MySQL 런타임에 연결돼 있지 않다. 독립 검증 미수행 |
| `aip-service`의 `MysqlDeploymentFence` 부팅 시 캐시와 요청 핸들러 주입 | 미실행 (후속) | 7.2.1.3에서 구현과 E2E 검증 필요 |
| 스트리밍 도중 `timeout`으로 Future가 절단될 때의 연결 폐기·복구 지연 | 미검증 | 유휴 상태 drop과 task abort만 실측 |
| 복합 FK 명시 거부 | PASS | `introspection_draft_rejects_composite_foreign_keys_instead_of_dropping_them` 통과 |
| WBS 7.2.1.2: Policy Read Compiler | 미실행 (후속) | MySQL/MariaDB SQL 쿼리 컴파일러, 식별자 쿼팅, pagination, relational joins |
| WBS 7.2.1.3: Apply/Write Transaction 어댑터 | 미실행 (후속) | journal/audit/outbox 트랜잭션 수명주기, HTTP serve `/ready`·`/read`·`/apply` 연동 |
| FaceReview 전체 11개 스키마 실앱 E2E | 미실행 (후속) | 전체 SQLAlchemy 11개 모델 실DB DDL 및 E2E 테스트 |

| 이번 단계 (Stage 6) | 확인 결과 | 아직 확인하지 않은 것 |
|---|---|---|
| optional driver 조달 및 feature build | `mysql_async 0.37.1` 조달 성공, `aip-migrate --features mysql` 검사 | production TLS CA configuration |
| sanitizer multi-URL regression | 이전 code에서 두 번째 비밀번호 누출 RED, 수정 후 targeted test PASS | operational callers/logging exposure |
| introspection draft renderer | synthetic fixture test PASS | 모든 FaceReview 타입/PK/FK를 draft로 변환 |
| live MySQL catalog read / request-time fingerprint | 격리 MySQL 8.4의 `aip_mysql_test` 2-table schema에서 draft·drift 차단 test PASS | FaceReview 전체 schema, HTTP boot/auth/DeploymentFence 연결, policy read, write, transaction, rollback |
| live MariaDB catalog read / request-time fingerprint | 격리 MariaDB 11.4의 `aip_mysql_test` 2-table schema에서 draft·drift 차단 test PASS | FaceReview 전체 schema, 모든 INFORMATION_SCHEMA 차이·functional index, HTTP boot/auth/DeploymentFence 연결, policy read/write/transaction/rollback |
| 일반 adapter 지원 판정 | **미지원 유지** | read/write/runtime/transaction과 FaceReview 포팅 완료 |

### 2026-10-18 읽기 경로: adopt-external·principal·serve·/read·/ready (WBS 7.2.1.1)

**구현.** `spike-v2-read`의 읽기 계획기가 `Dialect`(PostgreSQL 기본, MySql)를 받는다. PostgreSQL 출력은 바이트 단위로 같고 `mysql_dialect` 테스트가 고정한다. MySQL 방언은 식별자를 백틱으로 인용하고, 값은 SQL 텍스트 순서로 정렬한 `?` 위치 바인딩(같은 값을 여러 번 쓰면 `?`도 그만큼)으로 보내며, `JSON_OBJECT`/`JSON_ARRAYAGG`/`LEFT JOIN LATERAL`, `LOCATE`, 정렬의 NULL 순서(PostgreSQL처럼 ASC는 NULL 마지막, DESC는 처음)를 낸다. 결과는 계획의 출력 타입으로 한 번 정규화한다(TINYINT(1)→bool, DATETIME→`YYYY-MM-DDTHH:MM:SS[.f]+00:00`, DECIMAL은 문자열, 0000-00-00 거부). 전송 계층은 `RelationalBackend` trait로 저장소를 받고, `MysqlBackend`는 요청마다 `MysqlDeploymentFence`를 통과한 읽기 전용 트랜잭션 하나에서 계획을 실행한다.

`aip service adopt-external`은 PostgreSQL 채택과 같은 plan digest/`--apply <digest>` 절차다. 매핑을 실제 catalog와 대조하고(타입·nullable·FK·UNIQUE·InnoDB·UUID는 CHAR(36)·Text 최대 길이가 컬럼 길이 이내·DECIMAL 정밀도 일치), 별도 AIP schema에 엔진 이름으로 바꾼 `SQL SECURITY INVOKER` view, 신원 테이블, 배포 marker, journal을 만든다. MySQL DDL은 암묵 커밋이라 한 트랜잭션이 아니므로 marker를 `MIGRATING`으로 먼저 쓰고 모든 객체가 생긴 뒤 journal과 함께 `READY`로 바꾼다. 중단되면 런타임이 거부하고 같은 apply가 수렴한다. 정책만 바뀌면 view는 두고 marker와 journal을 한 트랜잭션에서 갱신한다. `principal bind/revoke`는 `aip_principals`(issuer·subject는 `utf8mb4_bin`)를 쓰고 기존 binding의 actor 변경을 거부한다. `serve`는 marker의 구조 digest와 실제 catalog가 다르면 기동하지 않으며, RS256 JWT 검증은 PostgreSQL 배포와 같은 `aip-auth` 검증기이고 principal 조회만 MySQL이다.

**명시 거부(2026-10-20 정정).** `workers.enable_file_uploads`, `objects`, `broker`, `rag`, `static_mounts`, `file_mounts`, `schedules`, `webhooks`, `outbox_handlers`, `message_handlers`, `job_policies`, `messaging`은 설정에 있으면 DB 접속 전에 `DB_ADAPTER_UNSUPPORTED`다. 읽기·쓰기 worker 설정 자체는 허용한다. `rate_limit`은 기본값이 켜져 있어 `"rate_limit": null`을 명시해야 한다(앞단 proxy에서 제한해야 한다는 뜻). `external_mapping`이 없으면 `EXTERNAL_MAPPING_REQUIRED`다. `migrate`·`init`·`adopt`·`jobs`·`audit`·`provision`은 MySQL에서 계속 거부한다. 실시간 구독은 `WIRE.NOT_AVAILABLE`이다.

**고친 결함 셋.** (1) `LogicalType`이 내부 태그 enum이라 ENUM·DATETIME·DECIMAL 열이 하나라도 있으면 catalog 직렬화가 실패해 지문 계산, 채택, 모든 요청이 막혔다. 인접 태그로 바꾸고 재현 테스트를 추가했다. (2) 스키마 전체 COLUMNS 쿼리가 view 컬럼도 가져와 업무 schema에 view가 하나만 있어도 지문이 실패했다. base table로 한정했다. (3) 지문이 중립 manifest만 해시해 아래 열두 가지 DDL을 보지 못했다. 원시 INFORMATION_SCHEMA 행을 지문에 넣었다.

| 실제 MySQL 8.4에서 지문이 바뀌는 것(`mysql_drift_matrix` 35건) | 바뀌지 않는 것(알려진 한계) |
|---|---|
| VARCHAR 길이, nullable, default, 정수 폭·부호, DECIMAL 정밀도, DATETIME 정밀도·TIMESTAMP 전환, ENUM 값, 컬럼·테이블 collation·charset, 컬럼 이름·순서·추가·삭제, AUTO_INCREMENT·ON UPDATE, 생성식, UNIQUE·인덱스 종류·prefix·방향·visible, 인덱스 열 순서, FK 동작·삭제, CHECK, 스토리지 엔진, 테이블 이름·추가·삭제 | 테이블 comment(의미 없음), 업무 schema의 view(AIP가 읽지 않음), **트리거** |

트리거는 `TRIGGER` 권한이 없으면 `INFORMATION_SCHEMA.TRIGGERS`에 보이지 않고, 그 권한을 런타임 계정에 주면 계정이 트리거를 만들 수 있다. 최소 권한 MySQL 배포에서는 어댑터가 업무 테이블에 몰래 추가된 트리거를 감지하지 못한다(PostgreSQL 스냅샷은 pg_catalog로 본다). 운영자가 지켜야 하며 `mysql_drift_matrix`가 이 한계를 고정한다.

**의미 차이(조용히 흉내 내지 않고 기록).** 문자열 비교·정렬·`contains`·`prefix`·UNIQUE는 컬럼 collation을 따른다. 기본 `utf8mb4_0900_ai_ci`에서는 대소문자와 악센트를 구분하지 않아 PostgreSQL의 바이너리 비교와 다르다. 어댑터는 이를 바꾸지 않고, 컬럼 collation 변경은 지문으로 드리프트 처리한다. `JSON_ARRAYAGG`에는 ORDER BY가 없어 1:N traverse는 LIMIT가 붙은 정렬된 파생 테이블을 순서대로 모으는 동작에 기대며, `traverse_many` 순서는 실DB 시험으로 확인했다. TIMESTAMP 열은 세션 시간대를 UTC로 고정해 읽는다. DATETIME은 UTC로 간주한다. view는 `FOR SHARE OF`를 지원하는 것을 확인했다.

| 이번 확인 | 결과 | 확인하지 않은 것 |
|---|---|---|
| 방언 단위(오프라인) | `mysql_dialect` 7 passed. PG 출력 바이트 고정, MySQL 구문·`?` 순서·NULL 정렬·정규화 | |
| PG 회귀 | `spike-v2-read` 전체 통과(실패 0), `aip-service`·`aip-migrate` 기본 빌드 통과 | spike-v6 전체 스위트는 아래 기록 |
| 실 MySQL 8.4.11 읽기 HTTP | `mysql_read_http` 1 passed: 읽기 전용 계정, 행·필드 정책, traverse(1:1·1:N 순서·상한·숨김 행·빈 목록), 집계, Bool·Decimal·Date·Time·Enum, drift, marker 변조, `/apply` 거부 | |
| 실 MySQL 8.4.11 CLI 전 과정 | `mysql_adoption_flow` 1 passed: plan/apply/재적용, `PLAN_CHANGED`, 설정 거부, bind/충돌/revoke, JWT serve, drift 시 `/read`·`/ready` 즉시 거부와 재기동 거부 | UUID actor·UUID resource, 외부 OIDC, 큰 schema의 catalog 비용 |
| DDL 매트릭스 | `mysql_drift_matrix` 1 passed(35건) | MariaDB 변형 |
| 드라이버·펜스 | `aip-migrate --features mysql` 라이브 13 + `mysql_fence_verification` 8, MySQL 8.4.11과 MariaDB 11.4.13 양쪽 통과 | 스트리밍 도중 절단 |
| MariaDB | 드라이버·펜스·catalog만 통과. 서빙·채택은 거부 | 읽기 경로 전체(LATERAL 부재) |

**실행.** `scripts/mysql-live-fixture.sh up`이 TLS가 켜진 MySQL 8.4와 MariaDB 11.4 컨테이너(loopback, tmpfs)를 띄우고 시드를 넣는다. `eval "$(scripts/mysql-live-fixture.sh env mysql)"` 뒤에 `cargo test -p aip-migrate --features mysql -- --include-ignored mysql --skip every_semantic_ddl`, `cargo test -p aip-migrate --features mysql --test mysql_drift_matrix -- --ignored`, `cargo test -p aip-service --features mysql --test mysql_read_http -- --ignored`, `cargo test -p aip-cli --features mysql --test mysql_adoption_flow -- --ignored`를 실행한다. MariaDB는 `env mariadb`로 같은 변수를 바꿔 드라이버·펜스 시험만 돌린다. PostgreSQL 기본 빌드가 `AIP_TEST_DATABASE_URL`을 요구하므로 격리 PG 컨테이너가 하나 더 필요하다.

### 2026-10-18 쓰기 경로: `/apply`·멱등·감사·용량·COMMIT 분류 (WBS 7.2.1.2)

**구현.** `crates/aip-migrate/src/mysql_write.rs`가 PostgreSQL 쓰기 계약(`spike-v3-write`)을 InnoDB 기본 도구로 다시 만든다. PostgreSQL 쪽은 `RETURNING`, `xmin`, advisory lock, 타입 있는 `NULL`에 기대는데 MySQL에는 없다. 요청 해석(target·patch·expect·literal·묶음 상한)은 `spike-v3-write`의 공통 함수를 그대로 써 한 계약을 유지한다.

| PostgreSQL 쪽 | MySQL 쪽 |
|---|---|
| `SELECT .. FOR UPDATE OF t`(id 순) 뒤 `UPDATE .. RETURNING`의 개수 비교 | 같은 순서로 판정·잠금, `UPDATE` 매칭 행 수(`CLIENT_FOUND_ROWS`)가 판정 수와 같아야 하고 아니면 전체 rollback(`WIRE.CONFLICT`). 매칭 행과 값이 바뀐 행을 혼동하지 않는다 |
| `xmin = pg_current_xact_id()`로 "이 트랜잭션이 바꾼 행" | 엔진이 쓰는 즉시 `touched` 집합에 기록해 커밋 시점 `check`와 용량 검사가 정확히 그 행만 본다 |
| `pg_advisory_xact_lock` | `aip_tx_locks` 1000개 bucket 중 하나의 행 잠금. COMMIT/ROLLBACK까지 유지되고 끊긴 연결과 함께 풀린다. 여러 잠금은 bucket 순서로 잡는다 |
| `RETURNING id` | 숫자 id는 `AUTO_INCREMENT`와 `LAST_INSERT_ID()`, UUID id는 엔진이 만들거나 호출자가 연 `id` 필드 값 |
| 벌크 `INSERT .. SELECT`(create 효과) | 대상 행마다 `INSERT` 하나. 새 행 id를 정확히 알아야 하기 때문(인터리브 잠금 모드의 AUTO_INCREMENT는 연속이 아니다) |
| `to_jsonb` 감사 스냅숏 | `JSON_OBJECT`를 읽어 출력 타입(Bool·Time·Decimal)으로 정규화한 뒤 `aip_audit`에 저장. 같은 트랜잭션, append-only 트리거 |
| `SERIALIZABLE`+40001 재시도 | `SERIALIZABLE`+1213(교착 victim)을 같은 재시도 규칙으로 처리. `not exists` 같은 부재 정책에서만 쓴다 |
| 멱등 키 `pg_advisory_xact_lock(hashtext)`+`aip_idem` | 키 bucket 잠금+`aip_idem`(principal·키는 `utf8mb4_bin`). 결과는 같은 트랜잭션에서 저장되어 커밋됐으면 키가 있고 아니면 없다 |

COMMIT은 보낸 시점으로 가른다. COMMIT 전 연결 단절은 `WIRE.DB_UNAVAILABLE`(커밋 없음, 같은 키 재전송 안전), COMMIT을 보낸 뒤 연결이 끊기면 `WIRE.COMMIT_UNKNOWN`, 서버가 거절한 COMMIT(교착 등)은 확정 코드다. `innodb_lock_wait_timeout`은 쓰기 기한(2초)에 맞추고 초과는 `WIRE.DEADLINE_EXCEEDED`다.

**거절하는 것(2026-10-20 정정).** `compose`(`WIRE.ADAPTER_UNSUPPORTED`), 소유 schema의 create(채택 배포만), 정책이 변경 대상 테이블을 하위 조회로 읽는 전이(MySQL 오류 1093을 그대로 `WIRE.ADAPTER_UNSUPPORTED`로 알림, 다시 쓰는 변환을 추측하지 않음). 확장 쓰기는 후속 `1c10788`에서 MySQL의 같은 fenced transaction에 연결됐으며 `workers.enable_writes`를 명시한다. 런타임 계정에 업무 테이블 `SELECT·INSERT·UPDATE·DELETE`가 없으면 `FOR UPDATE`가 1142로 실패하고 응답이 필요한 권한을 말한다. view를 거친 쓰기는 view가 `SQL SECURITY INVOKER`라 권한이 올라가지 않는다. 운영 참고: GRANT는 이미 열린 연결의 데이터베이스 권한에 즉시 반영되지 않는다(`USE` 이후).

| 이번 확인 | 결과 |
|---|---|
| 원자성·멱등·감사(실 MySQL 8.4.11) | `mysql_apply_is_atomic_idempotent_audited_and_fenced`: patch+expect, create·notify 효과, 감사 before/after, 같은 키 재생·다른 요청 거부, `/status`, 오래된 expect의 전체 rollback(효과·journal 포함), 인가·값·MISSING_TARGET, 커밋 시점 check, repeat unchanged·bulk, 증가 범위, 묶음 create와 일부 거절의 전체 취소, 감사 append-only, drift 시 쓰기 거절 |
| 동시성 | `mysql_concurrent_writes_keep_capacity_counters_and_keys_exact`: 용량(3요청 중 정확히 2개 성공, 5회), 8개 동시 증가가 범위 5에서 정확히 멈춤, 같은 키 6개 동시 요청이 한 번만 실행, 같은 expect의 한 요청만 성공 |
| 변이 확인 | 용량 잠금·키 잠금·매칭 수 확인을 각각 끄면 위 시험이 실패한다. 판정의 `FOR UPDATE`만 끄면 통과한다: 뒤의 `UPDATE`가 자체로 행 잠금을 잡고 `from`을 다시 걸어 같은 결과가 나기 때문이며, PostgreSQL 경로와 같은 설계를 유지하려고 남겼다 |
| UUID·교차 행 정책·부재 경쟁 | `mysql_uuid_resources_cross_row_policies_effects_and_absence_races`: UUID 리소스와 UUID actor handle, 소유자 정책(FOR SHARE), `update many` 효과, 위조 author 거부, 같은 slot 승인 8개 동시 요청에서 예약 정확히 1건. `serializable`을 끄면 7~8건이 통과해 이중 예약이 생기는 것을 확인 |
| COMMIT 연결 단절(W4) | `mysql_connection_loss_around_commit_...`: MySQL 프로토콜 fault proxy로 세 경우 확인. 문장 단계 차단은 `DB_UNAVAILABLE`+재전송 성공, COMMIT 차단은 `COMMIT_UNKNOWN`(서버는 rollback)+재전송이 쓰기 수행, COMMIT 응답만 유실은 `COMMIT_UNKNOWN`(서버는 commit)+같은 키가 저장 결과를 재생하고 효과는 한 번 |
| 권한 | `mysql_write_without_business_grants_...`: SELECT만 있으면 읽기는 되고 쓰기는 필요한 권한을 말하며 아무것도 커밋되지 않는다 |
| CLI 전 과정 | `mysql_adoption_flow`에 JWT `/apply`(적용·재생·익명 거부·감사 한 행) 추가 |

미확인: `where` target과 `sameScope`, `effect update`(단일 일치) 경로의 전용 시험, `compose`, 외부 OIDC 사용자의 쓰기, 큰 번들의 시간 한도, MariaDB.

### 2026-10-18 작업 저장소·outbox 릴레이·MariaDB 게이트 (WBS 7.2.1.3)

**작업 저장소.** `crates/aip-migrate/src/mysql_jobs.rs`가 `aip_execution::Store`의 상태 기계를 같은 타입(`NewRun`·`Run`·`Lease`·`Redrive`)으로 옮긴다: 멱등 enqueue(같은 키는 같은 run, 다른 요청은 `IdempotencyConflict`, 재시도 정책은 요청이 아님), lease와 fencing epoch를 가진 claim, heartbeat·finish·retry(지수 backoff와 호출자가 정한 대기)·fail, deadline, cancel, payload 만료, 운영자 list·redrive. InnoDB에 맞춘 차이는 다음과 같다.

| PostgreSQL | MySQL |
|---|---|
| `WITH candidate AS (.. FOR UPDATE SKIP LOCKED) UPDATE .. RETURNING` | READ COMMITTED 트랜잭션 하나에서 `SELECT .. FOR UPDATE SKIP LOCKED`로 후보를 잡고 같은 트랜잭션에서 갱신·재조회·커밋 |
| `clock_timestamp()` | `UTC_TIMESTAMP(6)`. 모든 만료·deadline은 DB 시계 |
| `UNIQUE(owner, operation, contract, deployment, key)` | InnoDB 인덱스 한도(3072B)를 넘어 튜플의 SHA-256을 유일 열로 두고, 중복(1062)이면 기존 run에 수렴 |
| `ON CONFLICT DO NOTHING` | 1062를 기존 run으로 해석. 입력 digest·구현·actor·timeout·ttl·delay가 같을 때만 같은 run |
| lease 만료 뒤 다른 worker가 claim | epoch가 늘어 이전 worker의 heartbeat·finish·retry·fail·check는 어떤 행도 맞추지 못한다 |

MySQL은 `UPDATE`의 `SET`을 왼쪽에서 오른쪽으로 평가하고 뒤의 식이 앞의 대입을 본다. 그래서 `state`를 읽는 열(`error`, `cancel_requested_at`, `result_expires_at`)을 먼저 대입하고 `state`를 마지막에 둔다. 처음 순서로는 `CANCEL_REQUESTED` 판정이 깨졌을 것이다.

**outbox 릴레이.** `mysql_outbox.rs`는 outbox 행 claim(`FOR UPDATE OF o SKIP LOCKED`), 행에서 정한 키(`outbox:<id>:<created_us>`)로 job enqueue, `aip_outbox_dispatch` 영수증 기록, commit을 한 트랜잭션으로 한다. outbox·job·영수증이 같은 DB라 PostgreSQL 릴레이가 두 저장소에 걸쳐 근사하던 것을 정확히 한다. 호출자 쪽 `make_run`이 한 행이라도 거절하면 배치 전체가 rollback되어 job도 영수증도 남지 않는다.

**MariaDB 게이트.** `probe_capabilities`가 어댑터가 기대는 문장을 서버에서 직접 실행해 능력을 정한다(버전 문자열을 읽지 않는다). `MysqlBackend::check_server_profile`은 빠진 능력을 이름으로 나열해 `DB_ADAPTER_UNSUPPORTED`로 거부한다. 실측:

| 능력 | MySQL 8.4.11 | MariaDB 11.4.13 |
|---|---|---|
| 8.4 이상 | 예 | 아니오(MariaDB) |
| strict mode | 예 | 예 |
| `LATERAL` | 예 | **아니오** |
| `FOR UPDATE SKIP LOCKED` | 예 | 예 |
| `FOR SHARE OF` | 예 | **아니오** |
| `JSON_ARRAYAGG` | 예 | 예 |
| 강제되는 `CHECK` | 예 | 예 |
| InnoDB | 예 | 예 |

이 probe는 연결 세션에 임시 표를 만들므로 런타임 계정에 업무 schema의 `CREATE TEMPORARY TABLES`가 필요하다. 권한이 없으면 서버 능력 부족으로 오판하지 않고 그 권한 이름을 대며 `DB_ADAPTER_UNSUPPORTED`로 거부한다.

그래서 MariaDB는 traverse(LATERAL)와 정책 잠금(`FOR SHARE OF`)을 쓰는 읽기·쓰기 경로를 지원하지 못하고, 거부는 정확히 그 둘을 이름으로 말한다. SKIP LOCKED·JSON·CHECK는 있으므로 job 저장소와 outbox 릴레이는 MariaDB에서도 돈다. 차이는 둘이다: MariaDB에는 `CAST(? AS JSON)`이 없어(JSON이 검증이 붙은 `LONGTEXT`) 문서를 그대로 바인딩하고, `FOR UPDATE OF <표>`가 없어 `FOR UPDATE SKIP LOCKED`를 쓴다. `MysqlJobStore::for_mariadb`와 `MysqlOutboxRelay::for_mariadb`가 이 방언을 고르고, `check_job_store_profile`이 job 저장소에 필요한 능력(`LATERAL`·`FOR SHARE OF`는 빼고)만 서버에서 실행해 확인한다. 실제 MariaDB 11.4.13에서 MySQL과 같은 시험 3개(상태 기계, 8 worker 동시 claim, outbox 릴레이)가 통과하고, claim의 `SKIP LOCKED`를 빼면 MariaDB에서도 시험이 실패한다. 읽기·쓰기 경로는 그대로 MariaDB를 거부한다.

| 이번 확인 | 결과 |
|---|---|
| job 저장소(실 MySQL 8.4.11) | `mysql_jobs` 3 passed: 멱등 enqueue·충돌, claim·lease·epoch fencing(만료 뒤 인계, 이전 worker의 모든 쓰기 거부), 재시도·소진, 요청된 대기와 deadline 확장, deadline 초과, delay, cancel(대기·실행·타인), redrive(상태·배포·만료), list 필터, payload 만료 뒤에도 키 수렴 |
| 동시 claim | 40개 run, 8 worker(별도 연결): 모든 run이 정확히 한 번(`claimed=40 unique=40`), epoch 1 |
| 변이 | 후보 행 잠금 제거, lease epoch 검사 제거, epoch 증가 제거를 각각 하면 시험이 실패 |
| outbox 릴레이 | 실패 배치의 전체 rollback, 4개 동시 릴레이가 30행을 정확히 한 번씩(job 30, 영수증 30, 서로 다른 job 30), 다른 topic 불변, 재실행 0건 |
| 능력 게이트 | `mysql_capability_gate` 2 passed: MySQL 8.4 통과, MariaDB 거부와 사유(LATERAL, FOR SHARE OF) |

미완료(2026-10-20 정정): `/jobs/*` HTTP 연결, 비동기 job 실행·effect journal, 메시지·파일·이벤트 저장소, schedule·webhook 트리거. 동기 relational worker의 읽기·쓰기 bridge는 후속 `ec5db25`·`1c10788`로 연결돼 있어 그 부재를 비동기 job 미연결의 근거로 쓰지 않는다. 원본 전체 포팅이나 모든 외부 capability 연결을 완료했다는 뜻은 아니다.

### 요청당 catalog 비용 (2026-10-18 측정)

배포 펜스는 캐시를 두지 않고 요청마다 바인딩된 표의 `INFORMATION_SCHEMA`와 marker를 읽어 지문을 다시 계산한다. 그래서 DDL 변경이 다음 요청에서 바로 닫히지만 비용이 표 수에 비례한다. 같은 호스트의 격리 MySQL 8.4(debug 빌드, 표당 열 10개)에서 읽기 요청 하나의 중앙값은 표 1개 8.8ms, 10개 13.5ms, 30개 22.5ms, 60개 35.7ms, 120개 62.6ms였다(요청 100회, 단일 실행). 표당 약 0.45ms다. 완화 후보는 지문을 짧은 TTL로 캐시하는 것이지만 그러면 TTL 동안 drift가 열리므로, 이 계약의 "다음 요청에서 닫힘"을 약속한 채로는 택하지 않았다. 결정은 사용자 몫이다.

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
