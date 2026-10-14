# DB 역할 분리와 최소 권한

기본 설정은 DB 역할 하나가 마이그레이션과 서버 실행을 모두 맡는다. 그 역할은 앱 schema의 소유자이므로 서버 process가 침해되면 테이블을 DROP·ALTER하고 감사 테이블의 트리거를 끄고 행을 지울 수 있다. 역할을 둘로 나누면 이 권한이 서버에서 빠진다.

| 역할 | 하는 일 | 쓰는 명령 |
| --- | --- | --- |
| migration 역할 | 앱 schema와 인프라 schema(`aip_infra_<digest>`)를 만들고 소유한다 | `init`, `migrate`, `adopt`, `provision` |
| runtime 역할 | 데이터를 읽고 쓴다. 구조는 바꾸지 못한다 | `serve`, `principal`, `jobs`, `audit` |

## 설정

```json
{
  "schema": "my_app",
  "database_url_env": "AIP_RUNTIME_DATABASE_URL",
  "migrate_database_url_env": "AIP_MIGRATE_DATABASE_URL"
}
```

`migrate_database_url_env`를 생략하면 migration 명령도 `database_url_env`를 쓴다. 그래서 역할 하나로 운영하던 배포는 설정을 바꾸지 않으면 그대로 동작한다. 두 값 모두 환경 변수 이름이며 URL 자체를 설정 파일에 쓰지 않는다. 비밀번호와 TLS 설정(`sslmode`, `database_ca_file`)은 두 URL에 각각 필요하다.

## 순서

1. migration 역할로 `aip service init app.aip --config service.json`을 실행한다. schema와 테이블은 이 역할이 소유한다.
2. 같은 역할로 `aip service provision --config service.json`을 실행한다. rate limit, outbox dispatch, 업무 감사, RAG(설정한 경우) 테이블을 인프라 schema에 만든다. 여러 번 실행해도 안전하다. rate limit을 켜거나 outbox handler·RAG를 새로 설정한 뒤에는 다시 실행한다.
3. 아래 권한을 runtime 역할에 준다.
4. runtime 역할의 URL로 `serve`를 실행한다.

`provision` 없이 runtime 역할로 `serve`를 시작하면 인프라 테이블이 없고 이 역할은 만들 수 없다는 오류와 함께 시작을 거부한다. 서버는 시작할 때 `CREATE ... IF NOT EXISTS`를 시도하고, 권한이 거부됐는데 테이블이 이미 있으면 그대로 진행한다. 역할 하나로 운영하면 예전처럼 서버가 직접 만든다.

## runtime 역할 권한

`<app>`은 설정의 `schema`, `<infra>`는 `provision` 응답의 `infrastructureSchema`다. `<runtime>`은 runtime 역할이다.

```sql
CREATE ROLE <runtime> LOGIN;                 -- 비밀번호나 인증 방식은 조직 정책을 따른다
GRANT CONNECT ON DATABASE <database> TO <runtime>;

-- 앱 schema
GRANT USAGE ON SCHEMA <app> TO <runtime>;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA <app> TO <runtime>;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA <app> TO <runtime>;

-- 인프라 schema
GRANT USAGE ON SCHEMA <infra> TO <runtime>;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA <infra> TO <runtime>;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA <infra> TO <runtime>;
REVOKE UPDATE, DELETE, TRUNCATE ON <infra>.aip_audit FROM <runtime>;   -- 감사 행은 추가만
```

`migrate`로 테이블이 늘거나 `provision`으로 새 테이블이 생기면 `GRANT ... ON ALL TABLES`를 다시 실행한다. 자동으로 주려면 migration 역할에 `ALTER DEFAULT PRIVILEGES IN SCHEMA <app> GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO <runtime>`를 설정한다. 인프라 schema도 같게 하되 `aip_audit`의 REVOKE는 테이블이 생긴 뒤 한 번 더 필요하다.

주지 않는 권한: `CREATE`(DB·schema), 테이블 소유권, `TRUNCATE`, `REFERENCES`, `TRIGGER`, superuser, `BYPASSRLS`. 이 저장소의 `crates/aip-cli/tests/least_privilege.rs`가 위 권한만 가진 역할로 `serve`, `/read`, `/apply`, `principal bind`가 동작하고 DROP·ALTER·CREATE·TRUNCATE와 감사 행의 UPDATE·DELETE가 거부되는 것을 실제 PostgreSQL에서 실행한다.

## 알아둘 점

외부 테이블을 채택할 때는 AIP view의 권한 외에 원본 테이블 권한도 필요하다. 쓰기 인가는 참조·정책 근거 행을 `FOR SHARE`로 잠근다. 그래서 생성·수정 API를 공개하지 않은 참조 테이블도 `SELECT`만 주면 쓰기 요청에서 SQLSTATE `42501`이 발생한다. PostgreSQL 행 잠금에는 잠그는 테이블의 컬럼 하나 이상에 `UPDATE` 권한이 필요하다.

[페이스리뷰 댓글 시험](tests/fixtures/facereview-comments/README.md)은 원본 사용자·영상 테이블에 `SELECT`와 `UPDATE(user_id)`·`UPDATE(video_id)`만 주고 댓글을 작성한다. 이름·영상 삭제 상태 컬럼의 UPDATE는 허용하지 않는다. 이는 DB 역할의 권한이며 호출자에게 ID 수정 API를 공개하는 것은 아니다. 변경 대상 댓글 테이블에는 `SELECT, INSERT, UPDATE`가 필요하다. 실제 원본 schema에 맞는 컬럼으로 권한을 설정하고, AIP view·metadata·인프라 권한도 함께 준다.

- `serve`는 시작할 때 배포 digest와 구조를 읽어 검사하고 `runtime_wire`를 한 번 고정한다. 둘 다 위 권한 안에서 동작한다. 앱 schema의 `aip_migrate_meta` 같은 관리 테이블에도 DML이 열리므로 runtime 역할이 침해되면 배포 metadata는 바꿀 수 있다. 이를 막으려면 PostgreSQL 컬럼 권한으로 해당 테이블의 쓰기를 줄여야 하며 이 가이드는 그 구성을 검증하지 않았다.
- 트리거는 테이블 소유자만 끌 수 있다. 역할 하나로 운영하면 서버가 소유자이지만 역할을 나누면 runtime 역할은 소유자가 아니므로 감사 테이블의 트리거를 끄지 못한다. 다만 `aip_audit`에 DELETE·TRUNCATE를 막는 트리거는 아직 없고(보안 감사 O-32), 이 가이드는 권한으로 막는다.
- `aip service principal`은 runtime URL로 실행한다. 주체 연결은 데이터 변경이라 위 DML 권한으로 충분하다.
- PostgreSQL 연결 예산에는 두 역할의 연결이 모두 포함된다. migration 명령은 짧게 실행되므로 보통 문제가 없다.
- 원격 DB에서는 두 URL 모두 `sslmode=require`와 인증서 검증을 쓴다.
