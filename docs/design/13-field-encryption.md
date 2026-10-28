# 13. 제품 서비스 방언의 필드 암호화 설계

> Rev.3 — 2026-10-19. 상태: **Proposed**. 제품 암호화 구현·출시·창시자 승인을 뜻하지 않는다.
> 정본은 이 문서다. Rev.1은 git `a257b8d`에 보존한다. 과거 Rev.2 보완 보고의 내용은 실제 main 문서에 없었으며 이번에 재대조했다.
> 원칙: [PRINCIPLES](../PRINCIPLES.md), [DECISIONS](../DECISIONS.md). 구현 작업은 PMS 8.18.1~8.18.6.

## 0. 배경과 설계 관문 점검

### 0.1 현재 코드 관측과 목표를 구분한다

2026-10-19 현재 코드 관측:

- 독립 검증 정정: `6f5f277`·`34fa1df`의 SQL builder와 전수 header scan, `1418944`의 고정 키 map 기반 내부 실행은 제품 암호화 완료가 아니다. header는 AEAD/AAD/키 가용성을 인증하지 않는다. 실제 listener는 키 map이 있어도 실행을 거부하며, 배포·채택·마이그레이션 차단도 유지한다. 내부 fixture 테스트와 실제 CLI/배포 경로를 구분한다. 8.18.3·8.18.4는 미완이다.
- 제품 `spikes/spike-v1-fixture/src/parser.rs`의 `type_ref`는 `encrypted(scope: ...)`를 인식한다. `.aip`/`.e.ts`/`.e.py`는 추출 후 `parser::parse_spec`, `.h.ts`/`.h.py`는 `hmap::to_spec`→`parse_type_str`를 거쳐 같은 `type_ref`로 모인다. Text/Email/Url/Decimal·service/ref/owner 선언의 정적 제약과 5형식 동일 facts/digest를 검증했다. 개발 검사를 켜면 `DEF.ADVISORY_ENCRYPTION_RUNTIME_DISABLED`로 아직 배포할 수 없음을 알린다. 서비스 CLI의 PG/MySQL 기동 경로와 모든 transport listener의 공통 진입점은 개발 검사 옵션과 관계없이 `DEF.ENCRYPTION_RUNTIME_UNAVAILABLE`로 실행을 차단하고, migration/adopt는 metadata를 거부한다. 저장·읽기·저널 연결 전 평문 실행을 허용하지 않는다.
- [aip-crypto](../../crates/aip-crypto/README.md)는 v2 셀·KEK wrapping·구조화 지문·TTL 캐시·feature-gated 명시적 v1 import를 제공한다. 제품 저장 경로와는 연결하지 않았다.
- `aip-execution::encryption::DekStore`는 scope/DEK metadata DDL, caller transaction 안의 최초 키 생성·동시 생성 승자 확정·KEK-only rewrap API를 제공한다. `service provision`의 migration 역할이 해당 테이블을 생성한다. 이 증분의 실제 PostgreSQL 동시성·최소권한 검증은 CI 대기이며, 업무 행 암복호화·키별 사용량 예약·writer epoch 검사·durable rekey는 아직 연결하지 않았다.
- PoC `crates/aip-runtime/src/crypto.rs`의 AES-GCM/v1과 `rekey.rs`는 제품 엔진에 연결돼 있지 않다. 제품을 평문 `valueCiphertext: Text`로 우회하면 암호화 지원으로 세지 않는다.
- `spikes/spike-v2-read/src/scalar.rs::parse`는 Text/Email/Decimal 문자열을 검증하되 원문 문자열을 반환한다. Email 소문자화·Text NFC·Decimal 정규화를 이미 수행한다고 보지 않는다.
- `aip service serve`가 현재 명령이다. 기존 문서의 `service run`과 실제 CLI를 혼동하지 않는다.
- [Infisical 키 모델](../../plan-docs/porting/INFISICAL-KEYMODEL.md)의 기본 로컬 KMS+프로젝트 DEK는 env/file KEK 봉투 암호화로 표현할 수 있다. 벤더별 원격 KMS는 별도 구현이다.

2026-10-28 구현 증분: `DekStore`에 별도 연결에서 먼저 commit하는 `reserve_for_write`(키당 2^20회, 배치 최대 500셀), 업무 transaction의 `seal_in`, 전체 DEK identity로 조회하는 `open_cell`, 이전 키를 READABLE로 보존하며 epoch를 올리는 `promote_in`을 추가했다. rollback·동시 상한·구 epoch·KEK rewrap·RETIRED 읽기 거부를 전용 PostgreSQL 17의 `encryption_cells` 7개로 검사했다. 이 API는 호출자에게 행/필드 인가·scalar 검증을 요구하며 아직 제품 resource writer/read 호출자가 사용하지 않는다. 회전 실행기·전수 adopt·최종 RETIRED 판정도 미완이다. 설계 상태와 실행 readiness gate는 유지한다.

2026-10-28 봉인 writer 증분: native create/update가 인가·scalar 검증·ID/scope 확정 뒤 같은 트랜잭션에서 `seal_in`하도록 `WriteCellSealer` 경계(`StoreSealer`)를 연결했다. 다중 행은 행별 CASE로 나누고 생성은 명시적 id를 요구한다. expect/`to`/effect 대입·lookup:eq·owner는 거부하며, sealer 없이는 기존 `v2:` 통과가 유지된다. 같은 트랜잭션의 두 번째 예산 예약이 멈추지 않도록 DEK 잠금을 KEY SHARE/NO KEY UPDATE로 낮췄다. 정적 검증만 마쳤고 실DB 실행은 샌드박스 차단으로 대기한다. read·adopt·rekey 미연결이므로 gate와 Proposed 상태는 유지한다.

2026-10-28 읽기 복호화 증분: 인가 SQL 실행 뒤 `execute_opened`가 root·1:1·1:N 각 층의 프레임을 `ReadCellOpener` 경계(`StoreOpener`)로 `open_cell`한다. 정책 소거 셀은 NULL로 남아 열지 않고, 숨은 `__aip_id`/`__aip_scope`는 응답에서 지운다. 혼합 scope 참조·owner·위조 셀은 읽기를 실패로 닫는다. opener 없이는 기존 출력이 유지된다. 정적 검증만 마쳤고 실DB 실행은 샌드박스 차단으로 대기한다. adopt·rekey 미연결이므로 gate와 Proposed 상태는 유지한다.

2026-10-28 검증 이관 증분: `adopt::apply_verified`가 계획 승인 일치 뒤·변경 전 같은 트랜잭션에서 모든 암호화 열의 전수 AEAD 검증을 수행한다. 키셋 페이지 순회로 평문은 버리고 개수만 남기며, 위조 셀은 마커를 건드리기 전 이관 전체를 실패로 닫는다. owner scope는 거부하고, 기본 `creation`·이관·초기화·마이그레이션 게이트는 그대로 닫혀 있다. 게이트 단위 검사는 실행했고 실DB 이관 검사는 샌드박스 차단으로 대기한다. rekey 미연결이므로 Proposed 상태는 유지한다.

2026-10-29 Durable Rekey 증분: `crates/aip-execution::rekey`에 `RekeyContext`, `rekey_target_column`, `rekey_scope` 및 `RekeyLease`를 구현했다. `READABLE` 구 키 세대를 keyset pagination(`LIMIT 100`)으로 순회하며 원자적 CAS(`WHERE col = <old_frame>`)로 `ACTIVE` 키 세대로 재암호화한다. 레이스 행 재스캔(multi-pass)과 잔여 구 셀 전수 검증(`final 0`)을 거쳐 `DekStore::retire_dek_in`으로 안전하게 `RETIRED` 상태로 전이한다. infrastructure 스키마의 `aip_rekey_leases` 테이블과 epoch 검증으로 동시 rekey 작업자 및 stale writer를 차단한다. 단위 테스트 3건 통과. Proposed 상태와 배포 게이트는 유지한다.

`aip-execution::encrypted_journal`은 create/apply/atomic/compose의 values·expect·target(s).where 및 compose `{const}`를 같은 방문자로 처리한다. `/apply`·`/status`·확장 멱등 저장/재생과 durable 효과 marker에 연결했다. 지문 오류의 평문 fallback, 구 지문 키 부재를 mismatch로 취급하는 처리, 저장된 marker와 caller 입력의 문자열 같음만으로 replay하던 처리를 제거했다. 이 증분도 8경로 전체·worker 실제 실행·암호화 제품 통합 완료를 뜻하지 않는다.

이하 표·포맷·상태 기계는 **Proposed 목표 계약**이다. 라이브러리·정적 진단·DEK metadata 기반을 단계적으로 구현하며 제품 실행 증거는 §7의 각 게이트에서 별도로 수집한다.

### 0.2 원칙 관문 8문항

1. 새 화면에는 resource 필드·정책·공개 계약을 선언한다. 화면별 암호 API는 만들지 않는다.
2. 호출자는 허용된 필드를 일반 read/create/patch로 사용한다. 비지원 암호 검색·정렬은 진단하며 범용 SQL로 우회하지 않는다.
3. 행/필드 인가 뒤에만 복호화한다. `lookup: eq`와 unique의 존재 확인 오라클·동일 값/길이 누출은 남으므로 절대적인 비누출을 주장하지 않는다.
4. 평문은 타입 검증 후 AEAD API와 SQL 바인딩에만 전달하며 실행 코드로 해석하지 않는다.
5. 읽기·쓰기 셀 수, 입력 길이, 키 캐시·키링 크기, 작업 기한을 제한하고 키 미설정은 시작 단계에서 거부한다.
6. 선언은 타입 뒤 `encrypted(scope: ...)` 하나다. scope는 명시하며 별도 resource 절을 만들지 않는다.
7. 파서는 단일 typed facts를 만들고 Rust 서버가 인가·암호화 경계를 소유한다. 워커는 키를 받지 않는다.
8. 안전한 v2 전환·백업/복원·회전 검증 전에는 Infisical 완전 포팅이나 제품 완료를 선언하지 않는다.

## 1. 선언 문법과 진단 코드

### 1.1 선언 문법 (5개 작성 형식 단일 정규화)

```aip
resource Secret {
  fields {
    id: Uuid
    project: Project
    value: Text(1..10000)? encrypted(scope: project)
    tag: Text(1..255)? encrypted(scope: project, lookup: eq)
    serviceToken: Text(1..500) encrypted(scope: service)
  }
}
```

호스트 형식도 `"Text(1..10000)? encrypted(scope: project)"` 타입 문자열을 쓴다. 기본 범위를 암묵적으로 공유하지 않는다. 옵션 없는 `encrypted`는 `DEF.ENCRYPTED_MISSING_SCOPE`, 알 수 없는 옵션·중복 옵션은 거부한다. `default database`와 함께 사용할 수 없다.

### 1.2 키 범위와 봉투 암호화

모든 범위는 KEK로 감싼 독립 DEK를 쓴다. env/file도 직접 데이터 암호화 키 모드와 혼합하지 않는다.

| 범위 | DEK 분리 단위 | 조건 |
|---|---|---|
| service | 배포 schema | 명시적 `scope: service` |
| ref field | schema + 대상 resource + 참조 ID | 같은 행의 필수 비암호화 참조. 모든 전이·patch·효과에서 변경 금지 |
| owner | actor 자신의 id 또는 단 하나의 필수 actor 참조 | 0개/여러 참조는 모호성 진단. 첫 수직 구현 뒤 추가 |

암호화 리소스에는 명시적 id가 필요하다. 공유 기본키 참조도 기존 identity 규칙과 AAD ID 정규형을 보존해야 하며 별도 회귀 없이 임의 숫자 id로 대체하지 않는다.

### 1.3 타입·DDL

정의 facts는 Text/Email/Url/Decimal과 owner 범위를 정규화한다. 실제 저장의 첫 수직 구현은 PostgreSQL+Text다. Email/Url/Decimal과 owner의 실제 암복호화·동치·scope 동작은 별도의 런타임 회귀 뒤에 활성화한다. 정의 수용을 저장 지원으로 세거나 전체 8.18 완료 기준에서 이 타입들을 조용히 제외하지 않는다.

- id/Ref/Json/Int/Bool/Enum/Date/Time의 암호화는 이 설계 범위 밖이며 명시적으로 거부한다.
- 물리 데이터 열은 text, 평문 범위/형식은 암호화 전에 검사한다. 프리픽스 CHECK는 형식 힌트일 뿐 인증 태그 검증을 대신하지 못한다.
- `lookup: eq`는 `<col>__bidx`와 nullable `<col>__bidx_next`를 사용한다. 실제 유일성은 §3.2의 단일 기준 열 인덱스로 보장한다.
- plaintext↔encrypted, scope/필드/resource 이름 변경은 기본 Blocked다. 일반 DDL에 평문 backfill 상수를 넣지 않는다.

### 1.4 실행 facts

```json
{"ty":"Text?","range":[1,10000],"encrypted":{"scope":{"kind":"ref","field":"project","target":"Project"},"lookup":"none"}}
```

메타데이터는 execution digest와 migration fence에 들어간다. 기존 암호화 비지원 바이너리와 같은 배포 digest로 서빙하지 않는다. 공개 SDK는 평문 scalar 타입을 유지하며 키·암호문·scope 내부 열을 공개하지 않는다.

### 1.5 진단

정의 진단은 `codes.rs`에 등록하고 생성 spec을 재생성했다. wire 진단은 제품 런타임 연결 때 `wire_codes.rs`에 등록한다. 정의 facts 생성에는 runtime-disabled advisory를 내고, `DEF.ENCRYPTION_RUNTIME_UNAVAILABLE`는 제품 서비스 실행 경계에서 거부한다. 실제 암호화 저장 지원이 생기기 전 이 경계를 해제하지 않는다.

| 정의 진단 | 거부 조건 |
|---|---|
| ENCRYPTED_ID_FORBIDDEN / REQUIRES_ID | 암호화된 id / id 없는 resource |
| ENCRYPTED_TYPE_UNSUPPORTED / DEFAULT_DB_FORBIDDEN | 타입 또는 DB 기본값 조합 |
| ENCRYPTED_MISSING_SCOPE / BAD_OPTION / REDUNDANT_OPTION | scope 누락·알 수 없는 옵션·중복 |
| ENCRYPTED_BAD_SCOPE / MUTABLE_SCOPE / AMBIGUOUS_OWNER | 잘못되거나 바뀌는 범위 |
| ENCRYPTED_FILTER_FORBIDDEN / SORT_FORBIDDEN / AGGREGATE_FORBIDDEN | 지원하지 않는 질의 |
| ENCRYPTED_UNIQUE_REQUIRES_LOOKUP | unique인데 lookup 없음 |
| ENCRYPTED_POLICY_EXPR_FORBIDDEN / TRANSITION_VALUE_FORBIDDEN | 정책·상수 대입·expect·repeat·파생 효과·notify의 민감 필드 참조 |
| ENCRYPTED_LOCATION_FORBIDDEN | extension/operation/predicate 입력·출력 등 resource 필드 밖 선언 |

접두어는 `DEF.`다. 별도 advisory는 `DEF.ADVISORY_WEAK_BLIND_INDEX`. 계획 wire는 `WIRE.ENCRYPTION_UNAVAILABLE`, `WIRE.ENCRYPTION_CORRUPTED`, `WIRE.ENCRYPTED_FIELD_NOT_QUERYABLE`이며 값·암호문·키 상세를 반환하지 않는다.

## 2. 키 공급·저장 포맷·Rotation/Rekey

### 2.1 키 공급과 DEK 메타데이터

서비스 설정에는 환경 변수 이름·파일 경로·provider 식별자만 둔다. 키 값은 저장소·생성 SDK·설정 JSON에 기록하지 않는다.

- env/file KEK provider와 `KekProvider` 인터페이스를 먼저 구현한다. 파일은 소유자·일반 파일·심볼릭 링크·크기 제한을 확인하고 0400/0600만 허용한다. 오류/Debug에 키를 출력하지 않는다.
- `Zeroizing` 메모리와 TTL+entry 제한 캐시는 메모리 잔류를 줄이지만 프로세스 침해 방어 보장은 아니다. key identity를 포함하지 않은 scope-only 캐시 키는 금지한다.
- `<infra>.aip_encryption_deks`에는 scope_kind/id, **불변 dek_id**, 증가하는 version, state, 현재 wrapping kek_id, wrapped_dek가 있다. scope당 ACTIVE는 하나다. `provision`이 DDL을 소유하고 runtime은 필요한 DML만 가진다.
- 최초 DEK 동시 생성은 scope 잠금/unique 및 동일 트랜잭션 안의 재조회로 승자를 확정한다. ON CONFLICT 뒤 패자의 임시 DEK를 데이터 암호화에 사용하지 않는다.
- 외부 KMS·키 hot reload는 인터페이스/미지원 상태를 표시한다. 아직 없는 SIGHUP 처리를 운영 기능으로 쓰지 않는다.

### 2.2 v2 포맷과 AAD

Rev.1의 데이터 헤더 `kek_id`는 KEK-only rewrap 때 데이터 행을 바꾸지 않는다는 계약과 충돌했다. **v2 구현 전 제안 포맷을 불변 DEK 참조로 교정한다.**

```text
v2:<scope_tag>:<dek_id>:<dek_ver>:<base64_nonce_12B>:<base64_ciphertext_and_16B_tag>
```

kek_id는 DEK metadata의 wrapping 정보다. KEK rewrap 후에도 데이터 헤더·DEK identity는 그대로이고 unwrap은 현재 metadata를 사용한다. v2 셀과 wrapping 라이브러리 및 KEK-only rewrap 회귀는 구현했으나 제품 metadata/DB 경로는 아직 연결하지 않았다. PoC v1 포맷을 바꾸지 않는다.

- 검증 라이브러리의 AES-256-GCM AEAD와 CSPRNG nonce를 사용한다. nonce 재사용 금지·키별 사용량/회전 상한을 구현 전에 고정한다. 직접 암호 프리미티브를 구현하지 않는다.
- AAD는 `domain`과 각 요소를 UTF-8 **u32 big-endian 길이 + 바이트**로 인코딩한다. 구분자 문자열 연결을 쓰지 않는다.
- 데이터 AAD 요소: `aip.enc.v2`, schema, resource, field, 정규 row_id, scope_kind, scope_id, dek_id, dek_version, scope_tag.
- wrapping AAD 요소: `aip.dek.v2`, schema, scope_kind, scope_id, dek_id, dek_version, wrapping kek_id.
- bigint ID는 정규 십진, UUID는 기존 소문자 하이픈 정규형이다. create는 UUID 선생성 또는 같은 tx의 nextval 선확정 후 AAD를 만든다. 권한 거부 시 행은 남지 않으며 sequence gap은 허용한다. MySQL의 안전한 ID 선확정은 별도 게이트 전 미지원이다.
- v1은 schema/tenant 결속이 없으므로 **명시적인 오프라인 import 도구에서만** 이전 PoC 키+v1 AAD로 열고 v2로 다시 저장한다. 제품 read에서 v2 검증 실패 후 v1/평문으로 자동 fallback하지 않는다.
- frame 크기·요소 길이·version·base64·nonce/tag·ID를 먼저 제한하고, 변조/알 수 없는 키는 일반 오류로 처리한다.

기본 API/nonce 요구는 [RustCrypto AES-GCM 문서](https://docs.rs/aes-gcm/latest/aes_gcm/), AEAD의 nonce/AAD 인터페이스는 [RFC 5116](https://datatracker.ietf.org/doc/html/rfc5116), subkey의 info/domain 분리는 [RFC 5869](https://datatracker.ietf.org/doc/html/rfc5869)를 따른다. 이 자료가 AIP의 회전 상태 기계나 구현 전체를 검증한 것은 아니다.

### 2.3 durable rekey와 writer fencing

**P1-2 해결 계약:** SKIP LOCKED와 증가 커서 한 번만으로 완료를 선언하지 않는다.

1. 작업은 `QUEUED → RUNNING_SWEEP → RUNNING_FINAL_VERIFY → SUCCEEDED|FAILED|CANCELLED`. cursor_id/pass_no/lease_owner/epoch/기한/대상 버전을 영속화한다.
2. 매 배치 첫 SQL이 작업 행을 `FOR UPDATE`로 잠그고 상태·owner·epoch·DB clock 기한을 검사한다. 권한을 잃은 worker는 데이터도 커서도 쓰지 못한다. 배치 UPDATE·cursor·건수·lease 갱신은 같은 tx로 commit한다.
3. 최대 500행 keyset+SKIP LOCKED로 처리한다. 패스 끝 cursor=NULL로 재스캔한다. pass 상한 뒤에는 짧은 lock_timeout을 둔 **SKIP LOCKED 없는** 잔여 처리로 넘어간다. 잠금을 못 얻으면 완료 대신 재시도/실패한다.
4. 쓰기는 같은 tx에서 scope의 write epoch/ACTIVE DEK를 확인하고 공유 잠금을 commit까지 유지한다. 활성 버전 cache만 보고 쓰지 않는다. 승격·최종 RETIRED 전환은 그 scope의 배타 잠금을 잡아 이미 시작한 구버전 쓰기가 끝날 때까지 기다린다.
5. 최종 배타 fence 아래에서 모든 대상 resource/field의 미전환 행 수=0을 확인한 뒤에만 이전 키를 RETIRED로 바꾼다. 삭제/키 파기는 백업·보존 정책까지 별도 확인한다.
6. scope/field/version 조건을 SQL에 모두 넣고 실제 scope를 무시한 프리픽스 LIKE만으로 완료 판정하지 않는다. trigger를 끄지 않는다. 업무 행마다 이전 암호문을 감사 로그에 남기지 않는다.

구현 증분(2026-10-19): `DekStore::active_for_write_in`은 caller transaction에서 scope/ACTIVE DEK를 공유 잠금으로 읽고 현재 epoch·불변 key identity를 반환하는 metadata primitive다. 누락·READABLE·RETIRED는 새 write key로 반환하지 않는다. 두 writer의 동시 공유, 직접 epoch UPDATE와 배타 승격 차단, 마지막 writer 종료 뒤 새 epoch/key 조회, 다른 scope 독립성을 `encryption_fence` 실DB 검사로 연결한다. 로컬은 compile-only clippy 단계이며 새 CI 결과 전까지 실제 fence 성공으로 세지 않는다. resource 쓰기·nonce 사용량 상한·durable rekey lease는 미연결이고 제품 암호화 실행 gate는 유지한다.

## 3. 조회·인가·유일성·평문 잔존 차단

### 3.1 기능 범위

| 경로 | 계약 |
|---|---|
| read/select/traverse | 행/필드 정책을 SQL에서 먼저 평가하고 허용된 non-null 셀만 복호화 |
| eq/in/target.where | lookup 선언+검색 키+바인딩된 scope가 있을 때만. 질의 범위·post-filter는 §3.2 |
| sort/cursor/부분 문자열/수치 aggregate | 암호화 값에는 금지. 평문 id 페이징과 값 없는 count(*)는 가능 |
| create/patch | 평문 scalar/범위 검증→인가 경계→암호화. 생성·묶음·compose·worker 효과 경로를 모두 포함 |
| 정책/from/check/invariant/expect/파생 effect/notify/RAG | 민감 필드 접근 차단. 앱이 비민감 상태/참조로 정책을 표현 |
| extension/operation 입출력의 encrypted 선언 | 1.0 거부. 무표식 일반 Text를 자동 민감값으로 간주하거나 안전하다고 보증하지 않음 |

### 3.2 blind index 동치·회전·unique (P1-1)

- 별도 HMAC 키링을 쓰고 AEAD/idem/audit와 HKDF domain을 분리한다. 입력은 길이 접두 schema/resource/field/scope_kind/scope_id/canonical scalar bytes다.
- Text/Email/Url은 기존 validation 뒤 **원문 바이트 동치**다. NFC·소문자화를 몰래 추가하지 않는다. Decimal은 `1`, `1.0`, `-0` 등의 SQL numeric 동치·scale를 맞춘 별도 canonicalizer와 양 DB oracle 전에는 lookup을 열지 않는다. `scalar::parse` 자체를 canonicalizer로 부르지 않는다.
- 읽기는 유효 키 태그들의 ANY로 후보를 좁히고 AEAD 복호화 후 평문 일치를 다시 확인한다. post-filter를 LIMIT 뒤에 단순 적용해 페이지·total·쓰기 대상을 누락시키지 않는다. 추가 후보 scan의 rows/cost 상한을 소모하며 끝까지 못 확인하면 성공·부분 total 대신 비용 오류를 낸다.
- ref/owner scope 검색에는 질의/인가에서 단일 scope를 먼저 결속해야 한다. 미결속 다중 scope 검색은 무제한 키/tenant 열거 대신 명시적 미지원으로 둔다. 이 제한의 공개 계약은 §8에서 검토한다.
- unique의 기준 열은 회전 중에도 하나의 current key 태그만 쓴다. `IDLE(k1) → PREPARING(current=k1,next=k2) → BACKFILL → CUTOVER → IDLE(k2)`.
- PREPARING 이후 모든 쓰기는 두 열을 함께 채운다. 기존 current UNIQUE를 계속 유지하고 next의 nullable 백필을 끝낸 뒤 UNIQUE INDEX CONCURRENTLY 성공·valid 상태를 확인한다. 오류/중단 시 구 current를 유지한다.
- CUTOVER는 배포/범위 배타 fence와 완료 검사를 거쳐 열·인덱스·metadata를 원자 전환한다. 오래된 plan/cache/writer는 epoch 차이로 거부·재계획한다. 인덱스를 먼저 drop해 제약 공백을 만들지 않는다.
- lookup은 동일 scope의 동일 값 빈도와 unique 충돌의 존재 오라클을 만든다. 행 정책·rate limit으로 줄일 수 있지만 없애지 못한다. 짧거나 저엔트로피 필드에는 advisory를 낸다.

### 3.3 인가·숨은 metadata

SQL의 rowRead와 `CASE WHEN field-policy THEN ciphertext END`를 먼저 평가한다. null/가려진 셀에는 키 조회·복호화를 하지 않는다. 내부 id/scope metadata는 별도 내부 투영으로 읽고 응답/SDK/오류/trace에서 제거하며 사용자 필드와 충돌하지 않게 한다. root·각 traverse 계층과 shared ID를 따로 시험한다.

### 3.4 감사·멱등·작업 저널 (P1-3 / P1-4)

평문·원본 암호문을 저장하지 말아야 하는 경로를 전수 분모로 둔다.

| 저장/노출 경로 | 처리·음성 검사 |
|---|---|
| 업무 데이터 | AEAD frame만 저장. 명시적 민감값 sentinel의 평문 없음 |
| aip_audit | 원문/암호문 대신 changed+version 요약. 지문을 켜면 별도 audit domain의 제한된 HMAC. null/변경 여부 누출은 명시 |
| aip_idem.request | create/apply/atomic[*]/compose의 values·expect·target(s).where를 경로별로 방문해 지문화 |
| aip_idem.result 및 /status | 일반 write 결과의 ID/상태만 허용. 결과·오류에 민감 입력 echo 금지 |
| aip_jobs input/output/grant | 1.0 encrypted 파라미터 선언 거부. resource encrypted 값을 반환/저장하는 operation 경로를 자동으로 안전하다고 세지 않음 |
| aip_effects input/result | worker ctx.data.apply 요청에도 동일 visitor/HMAC 적용. 평문 요청 저널 우회 금지 |
| event/notify/webhook/RAG | encrypted 파생 참조 정적 차단. 비민감 ID/상태만 전달 |
| 로그/OTLP/진단/생성 SDK | 값·암호문·키·바인드 파라미터를 기록하지 않음 |

멱등 marker는 `idem_v2:<fp_key_id>:<64hex>`, audit는 `audit_v2:<fp_key_id>:<32hex>`. HMAC 입력은 길이 접두 domain/schema/principal/request key/resource/field/정규 JSON scalar다. caller가 보내는 marker 모양 문자열도 일반 입력으로 재지문화하며 신뢰하지 않는다.

비교 시 **저장된 marker의 key_id**로 들어온 입력을 다시 지문화하고 라이브러리 상수시간 verify를 쓴다. 새 활성 키만으로 재계산하지 않는다. 이전 지문 키는 멱등 보존기간/불확정 효과가 끝날 때까지 유지한다. 필요한 키가 없으면 불일치로 단정하거나 원문 저장으로 fallback하지 않고 fail closed한다.

1.x의 job 입력 암호화는 별도의 job/operation/input-field AAD와 종결·보존 정책 scrub까지 필요하며 현재 resource 셀 AAD를 재사용하지 않는다.

## 4. PoC 재사용·마이그레이션·롤백

### 4.1 재사용 경계

AEAD 라이브러리 호출·v1 테스트 벡터는 재사용한다. PoC의 전역 키/UUID 고정 SQL/trigger disable/한 번의 keyset rekey/원문 요청 저장은 제품으로 복사하지 않는다. v1 decoder는 명시적 import에 격리한다.

### 4.2 변경·채택·복원 (P1-5)

- nullable encrypted 추가도 runtime·metadata·키가 준비돼야 한다. 필수 필드 추가는 빈 테이블 또는 nullable→행별 이전→검증→필수화 순서다.
- 외부 adopt는 기존 스키마를 몰래 변경하지 않는다. prefix **100행 샘플은 금지**하고 전체 non-null 행의 frame 형태/AEAD/AAD/scope/key 가용성을 상한 있는 배치로 전수 확인한다. 프리픽스만 붙은 평문도 거부한다. 스캔과 최종 활성화 사이에는 동일 deployment fence로 변경을 막거나 재검증한다.
- 암호화 미지원 구 바이너리로 v2 schema에 롤백하는 것은 `WIRE.SCHEMA_MISMATCH`로 차단해야 한다. **그 구 바이너리에도 신규 feature를 거부하는 capability fence가 실제 있는지 확인 전에는 차단을 보장하지 않는다.** 없으면 구 바이너리+변경 전 DB snapshot을 함께 복원하는 경로만 허용한다.
- 포맷 변경은 reader 준비→호환 쓰기→backfill→구 reader 제거의 expand/contract다. 같은 포맷 키 회전은 전 인스턴스 새 키 사전 배포→활성 승격→rekey→보존/복원 확인 후 구 키 제거다.
- KEK-only rewrap은 데이터 frame을 바꾸지 않고 wrapped_dek/kek_id를 같은 metadata tx에서 바꾼다. 이전 key cache가 새 metadata를 잘못 해석하지 않도록 identity/version으로 캐시한다.

## 5. 위협 모델과 한계

- DB dump만 유출됐을 때 본문은 AEAD로 보호한다. 동일 값/길이/행 수·접근 패턴은 별도 누출이다.
- 앱 호스트가 침해되면 현재 요청 평문·키 cache가 노출될 수 있다. worker env allowlist와 zeroization이 호스트 침해를 해결한다고 주장하지 않는다.
- live DEK 행 삭제는 **과거 백업의 wrapped_dek를 지우지 않는다**. 과거 백업까지 폐기하려면 해당 KEK 영구 파기 정책·다른 tenant 영향·백업 보존 규칙을 함께 다뤄야 한다. 복원 시 삭제 tombstone을 트래픽 전에 재적용한다. service 범위로 tenant별 삭제권을 보장하지 않는다.
- byte equality·unique는 정책에 따라 존재 확인 오라클을 만든다. 사용자 데이터를 무단 수집해 이 설계를 시험하지 않고 전용 fixture만 쓴다.
- 암호 테스트 통과는 보안 증명이 아니다. 크기/사용량 상한·키 절차·장애/복원 검증을 함께 출시 게이트로 둔다.

## 6. 대안·권고·구현 단계

### 6.1 대안 비교

| 축 | A: PoC 단일 키 이식 | B: 명시 scope 봉투 암호화(권고) | C: 클라이언트 E2EE |
|---|---|---|---|
| 서버 작성량 | 작음 | 엔진 공통 경계+운영 도구 | 키 교환·다중 기기·서버 통합 별도 |
| 권한/업무 규칙 | 서버 유지, tenant 키 분리 부족 | 서버가 인가 후 복호화 | 서버가 평문 검증 못함 |
| 키 회전/격리 | 전체 데이터 재암호화 | KEK rewrap과 DEK rekey 분리 | client별 복잡한 이전 |
| 구현/디버깅 | 쉬움, 운영 제약 큼 | 상태 기계·장애 검증 필요 | client/운영 전반 복잡 |
| 확장·AI 작성 | 작은 surface, 범위 누락 위험 | 선언 한곳·구체 진단/실행 gate | 앱별 키 흐름 작성량 큼 |
| 보안/원칙 | 프로젝트 격리 부족 | 서버 최종 경계 유지, lookup 누출 명시 | zero-knowledge에는 적합, 현재 서버 확장 목표와 다름 |

### 6.2 권고

B를 기술 구현 후보로 둔다. 이는 창시자 확정으로 바꾸는 선언이 아니다. 첫 단계에서는 최소 안전 경계를 먼저 증명하고 이후 기능을 덧붙인다.

### 6.3 단계

1. PostgreSQL+Text+명시 service/ref scope, read/create/patch, 감사/멱등/효과 저널 평문 차단.
2. 동치·scope-bound lookup/unique·회전, owner/추가 scalar, Infisical 전환.
3. 원격 KMS/MySQL 연동/E2EE/job 입력 암호화는 별도 작업이며 구현되지 않았다고 표시한다.

## 7. 구현 WBS와 실행 gate

| WBS | 소유 범위 | 실행 완료 조건 |
|---|---|---|
| 8.18.1 | crates/aip-crypto | 라이브러리 AEAD KAT, nonce/frame 상한, 모든 AAD 요소 변조·v1 explicit import, fp 회전/경계 충돌, provider 파일 권한·symlink·cache TTL/용량 |
| 8.18.2 | v1 parser/sema/spec | 5형식 동등 digest, 위치/타입/scope 불변/식·lookup 제약 진단 및 생성 spec |
| 8.18.3 | migrate/provision/adopt | DDL·변경 Blocked·전수 adopt(100번째 뒤 변조 포함)·동시 활성화·구 바이너리/snapshot 복원 |
| 8.18.4 | v2/v3/v6 | root/traverse field 인가·ID 선확정·8경로 평문0·atomic/compose/worker·지문키 회전 후 같은 요청 replay/변조 거부 |
| 8.18.5 | service rekey | 잠긴 낮은 ID 재스캔·cursor 원자성·stale worker와 stale writer 차단·최종0 확인·dual-column unique·실행 중단/재시작 |
| 8.18.6 | Infisical | 원본 의미·키 경계·권한·실DB/HTTP negative·저널 전수 검사와 실제 재현 runbook |

P1 반례를 먼저 실패로 재현한다: 회전 중 중복 입력, 낮은 ID 잠금 누락, 원문 sentinel 잔존, fp 키 교체 후 replay 실패, 구 바이너리 rollback. 추가로 KEK-only rewrap 후 모든 이전 데이터 복호화와 구 epoch writer의 승격 후 commit 차단을 검사한다. 문서 토큰 검색만으로 이 실행 gate를 통과시키지 않는다.

## 8. 미정 정책과 다음 확인

기술 기본안은 명시 scope·단일 modifier·표준 AEAD API다. 공개 정책으로 확정하지 않은 것은 lookup/unique 오라클 허용 범위, scope 미결속 검색 제한, 감사 지문 공개/보존, 키 파기와 백업 정책이다. 값/키/보안 책임을 바꾸는 선택을 에이전트 합의로 창시자 승인 처리하지 않는다. 이 항목의 확인이 없어도 parser/crypto의 격리된 음성 테스트와 다른 WBS 작업은 계속할 수 있다.

변경 이력: 2026-10-18 Rev.1 설계 초안. 2026-10-19 Rev.3에서 실제 미반영 Rev.2 보고를 대조하고 P1 5건·DEK 식별/rewrap·writer epoch·Decimal 동치·전수 adopt·과거 백업 한계를 정본에 반영. 구현/실DB 검증 완료 주장은 없음.
