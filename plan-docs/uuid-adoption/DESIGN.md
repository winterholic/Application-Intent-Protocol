# UUID Id와 외부 PostgreSQL 채택

기준: 2026-10-14 사용자 승인. 숫자 `Id`와 AIP 생성 schema를 권장 기본으로 유지하면서, 일반 앱의 UUID와 기존 운영 DB를 같은 제품 경로에 연결한다. 기존 prototype marker 채택을 느슨하게 바꾸지 않는다.

## 설계 관문

1. PRINCIPLES의 1·2·3·4·6·7을 실현한다. 설정·인가 검토는 추가되지만 UUID 변환 DB나 앱별 CRUD 서버 복제를 요구하지 않는다.
2. 기존 공개 read/apply/create/compose·일반 수정 입력 계약에 UUID 대상을 더한다. 물리 테이블 이름이나 임의 SQL은 요청에 받지 않는다.
3. introspect가 정의와 매핑 초안을 만든다. 개발자는 인가·노출을 검토하며 controller/DTO/SQL adapter를 복사하지 않는다.
4. 서버가 typed facts·계약·정규 Id·물리 구조·FK·권한을 확인한다. 초안은 deny-all이고 운영자가 노출을 결정한다.
5. TS/Python 생성 타입·설치 SDK·Node/Python 확장은 공통 Id 경계를 사용한다.
6. 기본키 철자는 `id: Id` 또는 `id: Uuid`; 참조 철자는 resource 이름이다. `X.Id`는 비컬럼 인자에 사용한다.
7. 기존 제품 execution facts/SQL/SDK 엔진에 종류별 Id 어댑터를 추가한다. 별도 UUID 서버를 만들지 않는다. PoC `aip-ir::Type::Uuid`는 원래 있던 별도 경로다.
8. 방향과 구조 확장은 사용자가 승인했다. 생성·채택의 세부 선택은 이 승인 안의 구현 결정이고 DECISIONS D-A31·D-A32에 기록한다.

## Id 경계

| 경계 | 숫자 기본 | UUID |
|---|---|---|
| 선언 | `id: Id` | `id: Uuid` |
| 제품 IR | `Id<R>` | 동일 `Id<R>` + resource `idKind: uuid` |
| FK/비컬럼 `X.Id` | 대상 종류를 따름 | 대상 종류를 따름 |
| wire | 기존 safe/decimal 배포 설정 | 두 설정 모두 소문자 8-4-4-4-12 문자열 |
| TS/Python | 기존 number/int 또는 string/str | 리소스별 branded string / NewType(str) |
| SQL | bigint(외부 int2/int4도 읽기·쓰기 지원) | native uuid; 바인드 값 쪽만 uuid로 cast |
| 정렬·필터·커서 | 기존 동작 | PG UUID 비교·정렬; after의 Id도 UUID 검증 |
| 생성 | 기존 identity | owned `DEFAULT gen_random_uuid()`; 외부 DB default 유지 |
| 캐시 | principal + 요청 JSON | 실제 actor UUID 문자열과 정규 UUID 요청을 그대로 사용 |
| 멱등 | principal + key + 요청 본문 | UUID actor의 영속 숫자 handle을 내부 principal namespace로 사용. 같은 UUID의 handle은 유지되며 공개 Id를 숫자로 바꾸지 않음 |
| 감사·notify | 기존 bigint 저장 | 혼합 배포에서는 target/source/recipient를 text로 보존. job/outbox 자체 순번·내부 신원 handle은 숫자 유지 |

UUID는 nil과 여러 UUID 버전을 허용하되 정규 문자열만 수용한다. 대문자·축약형·잘못된 하이픈·다른 스칼라는 DB 전에 거부한다. SDK도 UUID와 숫자 resource 성공 응답을 구별하며, 잘못된 성공 응답으로 pending을 해제하지 않는다. V13 숫자 반올림 방지 원칙은 바뀌지 않는다.

클라이언트 지정 Id는 `expose create { fields id, ... }`처럼 서버가 명시적으로 열었을 때만 허용한다. 보통 id는 열지 않아 DB가 생성한다. 외부 GENERATED ALWAYS 컬럼에는 호출자 쓰기를 열 수 없다. PK 변경 전이는 여전히 거부한다. 숫자↔UUID migration은 identity/FK 재매핑 의미가 없으므로 Unsupported다. 기존 숫자 앱에 UUID 리소스를 추가하는 owned migration은 outbox Id 저장을 text로 올리며 기존 값을 손실 없이 보존한다. 이 단계는 Breaking으로 보고하고 worker 재시작을 요구한다. 이후 provision으로 기존 감사 target_id도 text로 갱신한다. 마지막 UUID 리소스 제거에는 outbox 이력 보존 계획이 필요하다.

`EntityId`는 Number/Uuid를 분리한다. 종류별 parsing/cast/emission이 확장 지점이며 Text·자연키를 UUID나 숫자로 암묵 해석하지 않는다. 자연키·복합키는 후속 설계 대상이다.

## 외부 DB 소유권과 이름 매핑

`service introspect`는 catalog SELECT만으로 단일 숫자/UUID PK, 스칼라 필드, 같은 schema의 단일 FK를 정의 초안과 mapping JSON으로 만든다. 정책은 추론하지 않고 `rows read when false`로 닫는다. 지원하지 않는 타입·복합키는 오류로 보고하며 필드를 조용히 버리지 않는다.

매핑은 resource→physical table, field→physical column이다. `adopt-external`은 **별도 새 AIP schema**에 `security_invoker=true`인 단순 updatable view와 metadata를 만든다. view는 물리 컬럼을 기존 엔진의 이름으로 alias하고 실제 xmin을 노출한다. 원본에는 CREATE/ALTER/DROP을 하지 않는다. 기존 PK·FK·CHECK·UNIQUE·trigger·RLS·default가 실제 테이블에서 계속 작동한다. view는 권한을 올리지 않으며 실행 계정에 원본의 필요한 DML 권한이 있어야 한다.

PG 단순 view의 기본값·수정과 security-invoker 권한 동작은 [공식 CREATE VIEW 문서](https://www.postgresql.org/docs/17/sql-createview.html), 기본 UUID 생성은 [UUID 함수 문서](https://www.postgresql.org/docs/17/functions-uuid.html)를 따른다. PostgreSQL 15 이상이 필요하다.

`default database`는 외부 필드의 실제 default를 선언한다. 읽기 nullable은 바꾸지 않고 create 입력을 생략할 수 있게 한다. 외부 create는 INSERT로 실제 default·generated 값을 얻은 뒤 같은 transaction에서 allow를 확인해, 거부되면 변경도 rollback한다. owned 경로에서 이 modifier를 쓰면 거부한다.

업무 actor가 UUID면 `aip_actor_identities`에 UUID→내부 numeric handle을 저장한다. JWT subject는 명시적으로 bind하고 재배정은 막는다. 인증·재검증은 실제 업무 actor가 존재하는지도 확인한다. `/session`은 원래 UUID를 반환하고 정책의 actor는 원래 resource를 가리킨다. `actor principal`의 관리형 신원은 업무 resource와 함께 사용할 수도 있다.

## 계획·드리프트·재배포

계획 digest는 현재 배포, 정의, 매핑, wire, 원본 catalog snapshot에 묶인다. apply는 원본 ACCESS SHARE 잠금 아래 다시 검증하며, 달라진 계획은 거부한다. snapshot은 컬럼 타입·nullable·default·identity/generated·PK/FK/CHECK·index·trigger·rule·RLS·schema 함수 본문 등을 포함한다. 업무 데이터와 sequence 현재 값은 포함하지 않는다.

serve는 저장된 baseline을 읽어 새 기준으로 자동 덮지 않는다. 시작 시 owned marker/view와 원본 구조를 확인한다. 요청 fence와 `/ready`도 원본 drift를 거부한다. catalog 실패도 허용으로 처리하지 않는다. snapshot은 원본 schema 전체를 대상으로 하므로 매핑하지 않은 테이블의 DDL에도 재검토가 필요하며 큰 schema에서는 catalog 비용을 측정해야 한다. 다른 schema의 함수 본문 변경·GRANT 변경 전체를 fingerprint로 보증하지는 않는다.

같은 매핑·resource/필드/Id 종류·actor·wire의 정책 변경은 `adopt-external`로 metadata와 journal만 갱신한다. 명시적 plan/apply로 구조 baseline을 재검토할 수 있다. 같은 최신 plan의 재시도는 alreadyApplied로 확인하며 신원·멱등 기록을 유지한다. 매핑/필드 종류·operation 구조 변경은 새 AIP schema 채택이 필요하다. 기존 DB에 없는 AIP unique 선언과 부분 unique index의 암묵 추가는 거부한다. 일반 owned `migrate`는 external marker에서도 거부하므로 config에서 매핑을 빼도 업무 DDL 경로로 우회되지 않는다.

prototype `adopt`는 기존 구조 marker·실제 catalog 검증과 멱등 이관을 그대로 수행한다. 외부 adoption과 서로 바꾸어 사용하지 않는다.

## 사용 예

```sh
aip service introspect --config service.json --source-schema public \
  --out notes.aip --mapping-out notes.mapping.json
# notes.aip의 정책·expose 검토; service.json에 external_mapping 추가
# mapping 경로는 config 파일 기준 상대 경로도 가능하다.
aip service adopt-external notes.aip --config service.json
# 출력 digest를 검토해 정확히 그 계획 적용
aip service adopt-external notes.aip --config service.json --apply <digest>
aip service provision --config service.json
aip service principal --config service.json bind --subject <subject> --actor <uuid>
aip service gen notes.aip --out binding.ts
aip service gen notes.aip --lang python --out binding.py
aip service serve notes.aip --config service.json
```

설정의 `schema`는 AIP 소유 schema다. 원본 schema는 mapping의 `schema`에 있다. `database_url_env`는 런타임 DML 계정, `migrate_database_url_env`는 AIP schema 생성/관리 계정을 가리킨다. 채택 계정에는 원본 SELECT·USAGE만으로 충분하다. AIP infrastructure의 실제 schema 이름은 `provision` 출력 `infrastructureSchema`를 사용해 GRANT한다. 토큰·DB URL은 정의/매핑에 쓰지 않는다.

검증 코드: `product/tests/uuid-external.test.mjs`, `spike-v3-write/tests/uuid_ids.rs`, `spike-v6-transport/client/uuid.test.ts`. 실제 새 PostgreSQL DB·제한 역할·설치 SDK를 사용한다. 운영 원본이나 .env를 사용하지 않는다.

## 현재 제한

- 자동 introspect: 단일 숫자/UUID PK, Text/Bool/Int/Time/Date 및 같은 schema의 단일 FK. enum·numeric·JSONB·domain·복합키 등은 자동 초안에서 명시적으로 거부한다. 읽기만 매핑할 일부 테이블/필드 선택은 수동 정의/매핑으로 구성한다.
- 외부 매핑은 ordinary/partitioned table을 대상으로 한다. 임의 기존 view·foreign table adapter는 후속이다.
- 소유하지 않는 테이블의 DDL migration, PK 종류 변환, cross-schema FK·자연키·복합키, 자동 정책 추론은 제공하지 않는다.
- 기존 DB의 독립 trigger가 외부 시스템에 부작용을 보내는 경우까지 AIP rollback이 되돌리지 않는다. 운영자가 기존 DB 동작을 관리한다.
- PostgreSQL 외 공급자와 별도 서버 캐시는 이번 구현 범위가 아니다. 목록과 우선순위는 INFRASTRUCTURE-CONNECTIONS.md에 있다.
