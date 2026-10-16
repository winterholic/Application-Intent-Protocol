# MySQL/MariaDB adapter 조사 — WBS 5.2.4

2026-10-15. 범위는 계약 조사다. **AIP MySQL adapter는 아직 구현되지 않았다.**
원본 앱이 MySQL에서 실행되는 것과 AIP가 MySQL을 지원하는 것은 다른 판정이다.

## 2026-10-17 실제 제품 경로 확인

`aip-migrate::external::introspect`가 PostgreSQL `tokio_postgres` 연결과 `pg_catalog` 쿼리만 사용한다. MySQL/MariaDB URL을 넘기는 검사는 실제 driver/catalog 시험이 아니며, 이전에는 PostgreSQL connect 단계의 일반 `DB_CONNECT` 오류로 끝났다. 이제 이 두 scheme은 `DB_ADAPTER_UNSUPPORTED`로 명시 거부한다. 테스트는 오류에 합성 URL 비밀번호가 노출되지 않는 것도 고정한다.

후속 순수 변환 단위에서 `aip_migrate::mysql_catalog::normalize_table`을 추가했다. INFORMATION_SCHEMA COLUMNS/STATISTICS에서 온 typed row를 driver 없이 받으며, 컬럼 순서·정규화 타입·부호/표시 폭·null/default/generated·PK/unique index 순서를 반환한다. 중복/비연속 column ordinal, 없는 column을 참조하는 index, index 멤버 간 `NON_UNIQUE` 불일치는 오류다. `CHAR`/`VARCHAR` 길이는 현재 `Text`로 축약되므로 길이 비보존 진단을 남긴다. 작은 현재 manifest 경계 밖인 collation, generation expression, on-update, auto_increment, functional index expression, 미지원 타입은 diagnostics에 남긴다. `tinyint(1)`은 값이 0/1로 제한됐다는 뜻으로 단정하지 않고 정수 metadata와 alias 표식·진단을 함께 보존한다.

`normalize_foreign_keys`는 future driver가 `KEY_COLUMN_USAGE`와 `REFERENTIAL_CONSTRAINTS`를 결합해 제공하는 driver 비의존 row를 받아 constraint 단위 manifest를 만든다. constraint/source schema·table·이름을 분리 키로 유지하고 ordered source/target column pairs, referenced schema/table, `ON DELETE`/`ON UPDATE` actions를 보존한다. 비연속 순서·누락 source/target/action·action 또는 대상 불일치는 오류로 닫는다. 현재 인식하는 action은 `CASCADE`, `SET NULL`, `RESTRICT`, `NO ACTION`; `SET DEFAULT`와 알 수 없는 값은 unsupported 오류다. 원본 11개 RDB 모델의 선언 기준은 [FK manifest](facereview-fk-manifest.json)다. 기존 댓글 PostgreSQL fixture는 이 중 Comment의 두 참조만 담는다. 이름과 실제 참조 schema는 원본 선언에 없으므로 manifest 예상치에서 만들지 않고, catalog 입력/출력에서만 보존한다.

`COLUMN_DEFAULT = NULL`은 MySQL 8.4에서 명시적 `DEFAULT NULL`과 DEFAULT 절 부재 양쪽에 쓰이므로 `NullOrAbsent`로 표현하고 모호성 진단을 남긴다. 드라이버 연결 시 `SHOW CREATE TABLE` 등으로 구분해야 한다 ([MySQL 8.4 COLUMNS](https://dev.mysql.com/doc/refman/8.4/en/information-schema-columns-table.html)).

현재 catalog/FK 코드 revision: `28bb463f4bb73f461ff813ec8da3f57ad726e165`.

이 구현의 catalog 회귀 행은 모두 synthetic이며 MySQL/MariaDB 관측 증거가 아니다. FK/action 입력은 추가했지만 check·charset 또는 index prefix 등의 입력 모델은 아직 없다. FK/action 변환 API의 미래 소비자는 driver이며 현재 제품 경로는 이를 호출하지 않는다. MySQL driver query 연결과 INFORMATION_SCHEMA row extraction은 다음 연결 단계다. URL guard와 전체 adapter 미지원 판정은 그대로다.

| 이번 확인 | 결과 | 확인하지 않은 것 |
|---|---|---|
| MySQL/MariaDB scheme 요청의 fail-closed 경계 | `aip-migrate` 회귀 1 passed | 서버 연결, schema introspection, 정책 read/write |
| 순수 catalog 변환 및 metadata 오류 경계 | synthetic `mysql_catalog` 회귀 8 passed | 실제 MySQL/MariaDB catalog 행·호환성 |
| adapter 지원 판정 | **미지원** | 실제 MySQL·MariaDB 호환성 |

다음 구현 slice는 이 문서 §구현 위치와 작업 순서의 2번, MySQL driver를 이용한 반복 가능한 read-only introspection이다. 원본 11개 모델에서 만든 synthetic schema를 실제 MySQL 8.4에 두고, catalog manifest·UUID 문자열·타입/default/nullable·PK/FK/unique·collation을 대조한다. MariaDB 성공은 별도 실행 전까지 미지원 표식으로 둔다. Read-only 단계가 실제 adapter 경로에서 통과하기 전에 7.2.2 인증·권한으로 건너뛰지 않는다.

2026-10-17 구현 착수 때 `cargo search mysql_async --limit 3`는 통과했으나, `cargo info mysql_async`의 crate 다운로드 후 cache open이 nested worker sandbox에서 `/Users/winterholic/.cargo/registry/cache/...`의 `Operation not permitted`로 막혔다. 이건 현재 실행 환경의 dependency cache 쓰기 제한이며 production의 MySQL 가용성 증거가 아니다. 이 제한 아래서 driver를 우회 조달하거나 다른 실행 경로를 사용하지 않았고 adapter 구현은 시작하지 않았다.

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
