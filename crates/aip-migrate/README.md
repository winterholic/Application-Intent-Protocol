# AIP facts migration

2026-10-05 구현 전 검토: `docs/PRINCIPLES.md` §4의 설계 관문 8개와 `plan-docs/decisions/DD-12-schema-evolution.md`를 대조했다. 이 크레이트는 prototype의 V1 execution facts와 `spike-v2-read::sqlgen::create_ddl_in`을 사용한다. root의 옛 Core IR 진화 코드를 제품 facts에 억지로 연결하지 않는다.

1. 원칙 1·2·4·6: 앱별 수동 DDL을 줄이고, 정의와 서버의 최종 통제를 유지한다. 잠금과 배포 절차가 늘어나는 긴장은 있다.
2. 호출자의 read/apply 표현 범위는 바꾸지 않는다. 배포 정의의 변경만 판정한다.
3. 개발자는 같은 정의와 필요한 경우 정형 backfill/convert만 쓴다. 새 서버 endpoint는 요구하지 않는다.
4. 클라이언트는 DDL·승인권을 갖지 않는다. 배포 운영자가 plans digest를 확인해 apply에 전달하며, 서버는 DB 상태와 계획을 잠금 안에서 재확인한다.
5. 다섯 작성 형식의 결과인 동일 execution facts를 받으므로 JS/TS·Python 작성 경로를 바꾸지 않는다.
6. 정본은 facts 한 개다. 계획·저널·생성 DDL은 파생물이다. migration 지시의 알 수 없는 키는 거부한다.
7. typed diff와 journal은 데이터 보존·권한 변경 검토에 필요한 내부 수단이다. 새 DSL이나 별도 Core IR을 요구하지 않는다.
8. PostgreSQL 범위(OI-DB1)와 구버전 클라이언트 공존(OI-P2)은 미결정이다. 현재 구현이 이를 영구 정책으로 정하지 않는다.

현재 API는 `init`, `plan`, `apply`, `preflight`이며 모두 `Result<serde_json::Value, serde_json::Value>`를 반환한다. `init`은 새 전용 schema만 만들고, `plan`은 읽기 전용 snapshot에서 변경과 digest를 산출한다. `apply`는 그 digest와 일치할 때만 advisory lock과 관리 테이블 잠금 안에서 DDL, metadata, journal을 한 트랜잭션으로 기록한다. 적용 후 같은 트랜잭션에서 소유 shadow schema에 새 정의를 fresh 생성해 실제 catalog와 비교한다. `preflight`는 정의·저널·실제 catalog 지문을 확인한다.

`migration` 인자는 `{"backfill":[{"resource":"Member","field":"score","value":7}],"convert":[{"resource":"Member","field":"score","to":"Int?"}]}` 형태다. SQL 본문은 받지 않는다. `backfill` 값은 대상 필드의 JSON scalar 타입이어야 하며, `convert`는 대상 타입과 정확히 일치해야 한다. 구조·타입·범위·enum·unique 변경은 PostgreSQL 검증에 실패하면 전체 트랜잭션이 취소된다. `Destructive`·`SecurityReview` 결과는 plan digest 확인을 요구한다. `Blocked`/`Unsupported`는 적용을 거부한다.

`aip_principals`의 `(issuer,subject)`와 `actor_id` 결합은 트리거가 재배정을 막는다. `enabled`와 `min_iat`는 철회 목적으로 갱신할 수 있다. 실제 catalog 지문에는 관리 schema의 함수 본문도 포함된다. 커밋 응답이 불명확할 때 같은 계획 digest를 다시 적용하면 최신 journal·facts·catalog를 대조하고 이미 반영된 경우 `alreadyApplied`를 반환한다.

배포 설정의 ID wire는 facts 바깥의 영속 설정이다. 제품 CLI의 `init_with_wire`와 prototype 이관 `apply_with_wire`는 metadata·wire·저널을 한 트랜잭션으로 저장한다. wire를 선택하지 않는 라이브러리 `init`/`adopt::apply`는 `runtime_wire=NULL`을 허용하며, 이 경우 운영 기동 전 `bind_wire(db_url,schema,"safe"|"decimal")`이 현재 정의·catalog를 잠금 안에서 검사한 뒤 한 번 고정한다. 이미 고정된 값과 다른 wire는 거부한다. 최초 고정 시 기존 멱등성 principal은 실제 전송 계층의 `actor:<id>:ids:safe-number-v13` 또는 `actor:<id>:ids:decimal-string-v13`, 같은 모드의 `anonymous:ids:<label>` 형식과 요청 wire가 일치해야 한다. 다른 wire나 알 수 없는 principal이 한 건이라도 있으면 고정을 거부해 미확정 쓰기의 다른 key 재시도를 막는다. `bind_wire` 호출은 metadata 데이터만 갱신하고 DDL이나 facts digest를 바꾸지 않는다.

현재 제약: 일반적인 애플리케이션 정책 호환성 증명, 권한 확대의 수학적 판정, 대형 테이블 무중단 backfill, 구버전 클라이언트 공존, 임의 데이터 변환, 자동 역방향 rollback은 제공하지 않는다. PostgreSQL의 `lock_timeout`과 전체 작업 기한이 적용된다. 운영 DB 연결 권한은 별도 배포 계정으로 제한해야 하며, 이 크레이트의 plan digest는 인증 주체 확인을 대신하지 않는다.

이 라이브러리의 facts 인자는 V1 sema가 산출한 execution facts를 전제로 한다. 직접 만든 JSON을 받는 공개 API에는 식별자·enum 값·비교 연산자·`exists` 대상 등의 최소 검사를 두었지만, 임의 정책 AST의 의미와 모든 표현식을 독립적으로 검증하지는 않는다. 제품 CLI는 사용자 정의를 sema로 분석한 결과만 전달해야 한다.

목표 구조 검증은 임시 schema의 catalog에서 물리 열 순서를 제외하고 schema 이름을 치환한 뒤 실제 schema와 비교한다. 이 이름 치환은 SQL 파서가 아닌 catalog 문자열 치환이므로, schema 이름이 SQL 문자열 리터럴이나 함수 본문에 등장하는 특수 정의는 의미가 같아도 검증이 실패할 수 있다. 임시 schema 이름이 facts와 겹치면 적용을 거부한다. 검증 실패는 전체 트랜잭션을 취소하며, 임시 schema 이름은 소유를 확인한 트랜잭션 안에서만 삭제한다.
