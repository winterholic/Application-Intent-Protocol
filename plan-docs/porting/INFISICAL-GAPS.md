# Infisical 포팅이 드러낸 AIP 제품 갭 (WBS 7.3, 2026-10-18)

이 목록은 설계 문서(`INFISICAL-DESIGN.md`)에 있던 "AIP 제품 갭 목록"을 대체한다. 갭은 AIP **제품 코드**에 없는 것을 뜻한다. 이 포팅 트랙은 core를 수정하지 않고 갭을 기록한다. 우회로 메우지 않는다. 각 항목은 확인 방법과 상태를 구분한다.

상태 표기: **재현**(이 세션이 명령을 실행해 확인), **판독**(소스를 읽어 확인, 실행 안 함), **분류 변경**(이전에 갭이라 적었으나 갭이 아님).

## 정식 제품 갭

### G2. `encrypted` 필드 수정자와 서버 키 공급이 제품에 없다 (중요)

원본 Infisical의 기본 경로는 서버측 KMS다([INFISICAL-KEYMODEL.md](INFISICAL-KEYMODEL.md)). 이것을 AIP로 옮기려면 서버가 키를 쥐고 필드를 암복호하는 기능이 필요한데 제품 경로에는 없다.

| 사실 | 확인 |
|---|---|
| 제품 서비스 방언은 `value: Text encrypted`를 거부한다. 출력: `DEF.PARSE_EXPECTED`, "`:` 필요, `}` 발견", scope `resource User > fields` | **재현**: `aip service check`를 `fields { ...; secret: Text encrypted }` 정의로 실행 |
| `AIP_ENCRYPTION_KEYS`는 PoC 런타임 `crates/aip-runtime`(`crypto.rs`, `rekey.rs`, `engine.rs`)에만 있다. `aip-service`와 `spikes/*`의 Cargo 의존성에 `aip-runtime`이 없다 | **재현**: `grep -rln AIP_ENCRYPTION_KEYS crates spikes product`와 Cargo.toml 대조 |
| PoC 런타임은 `poc-runtime` feature로만 빌드되고 기본 빌드에서 `aip run/migrate/rekey`는 `RUNTIME_FEATURE_DISABLED`다 | **판독**: `crates/aip-cli/Cargo.toml`, `product/OPERATIONS.md` |

영향: 시크릿 값, 값 버전, 회전, blind index 조회(G4)를 선언할 수 없다. 비밀 관리 앱과 개인정보 앱을 AIP 제품 방언으로 만들 수 없다. 임시 평문 Text 열은 모든 읽기 가능자에게 평문이 노출되므로 우회로 판단해 쓰지 않았다.

설계 소유: 별도 세션의 `docs/design/13-field-encryption.md`(상태 Proposed, 창시자 승인 대기). 설계 13(`a257b8d`)은 GPT 독립 정적 리뷰에서 "현 초안 그대로 구현에 넘기지 않는다"는 판정을 받았다. P1은 blind index 회전과 unique, SKIP LOCKED 재개 누락, idempotency·job 평문 경로, HMAC 회전 뒤 재시도, 이전 binary rollback 호환성이다. 이 리뷰는 정적 검토이며 테스트·빌드·DB를 실행하지 않았으므로 실행상 고장의 증명은 아니다. 사용자 승인도 없다. 따라서 이 초안은 승인된 설계도 제품이 보증하는 기능도 아니고, 암호화에 의존하는 Infisical 포팅 항목(값·버전·회전·blind index·동기화 내용)은 **미구현·설계 미확정**으로 취급한다. 이 트랙은 그 문서를 수정하지 않는다. 문서가 인용한 `porting/infisical/app.aip:134` 등 줄 번호는 이 재작업 이후 달라졌다(`valueCiphertext`는 정의에서 제거).

현재 포팅 처리: `app.aip`는 값을 선언하지 않고 머리 주석과 `docs { summary }`로 갭을 표시한다. 값 없는 Secret은 폴더, 이름, 소유자, 리마인더 시각만 가진다.

### G10. 삭제 의미와 이름 유일 제약: 원본과 동등하게 해소 (2026-10-20)

이전 판은 "제품 방언에 물리 delete와 부분 유일 제약이 없다"고 보고 `SecretKey`(영구 이름)와 `Secret`(세대, `deleted` 플래그)로 나누어 소프트 삭제와 `limit atMostOneLiveSecret`으로 근사했다. 2026-10-19에 `transition ... { delete; ... }`가 있음이 확인됐고(FlaskBB `User.purge` 등), 원본을 다시 읽으니 그 근사는 원본과 달랐다.

원본 동작(`infisical-upstream` 고정 47246bec에서 직접 읽음):

| 사실 | 근거 |
|---|---|
| 시크릿 삭제는 `secrets_v2` 행을 실제로 지운다(`.delete().returning("*")`) | `backend/src/services/secret-v2-bridge/secret-v2-bridge-dal.ts:348-379` `deleteMany` |
| 공유 시크릿을 지우면 같은 키의 개인(personal) 재정의 행도 함께 지운다 | 같은 함수 `:364-370` |
| 삭제 때 해당 시크릿의 리마인더를 제거하고 폴더 커밋 "Secret Deleted"를 만든다 | `secret-v2-bridge-fns.ts:426-490` `fnSecretBulkDelete`(`removeSecretReminder` `:452`, `createCommit` `:472`) |
| 단건 삭제는 `Delete` 액션 권한(환경·경로·이름·태그 조건 포함)을 요구한다 | `secret-v2-bridge-service.ts:923-1000` `deleteSecret`(`:985`) |
| 같은 폴더의 공유 시크릿 이름은 유일하다. 부분 유일 인덱스 `uq_secrets_v2_key_folder_id_shared ON (key, folderId) WHERE type = 'shared'` | `backend/src/db/migrations/20260623144551_secret-folder-unique-index.ts` |
| 키 길이는 500 | `20240730181850_secret-v2.ts:14` `t.string("key", 500)` |

현재 포팅(`app.aip`): `Secret` 하나(`folder`, `key`, `owner`, `remindedAt`)에 `unique folder, key`와 `delete` 전이를 선언했다. 공유 시크릿만 다루므로 원본의 부분 인덱스와 같은 집합에 대한 유일 제약이고, 삭제는 행이 실제로 사라진다. `SecretKey`, `deleted` 필드, `limit`, `invariant`는 제거했다. **재현**(실DB, 임시 PostgreSQL 17): `tests/test_live_db.py`의 `test_secret_name_is_unique_in_a_folder_including_the_root`(중복 `WIRE.ALREADY_EXISTS`), `test_delete_removes_the_row_and_the_name_can_be_recreated_twice`(삭제 뒤 SQL로 행 0개, 3번 재생성하면 id가 모두 다름), `test_bulk_delete_removes_every_named_secret_and_no_other`, `test_delete_leaves_a_before_image_in_the_audit_log`(감사 행 1건, `before.key`, `after` null). 새 테스트는 수정 전 정의에서 12건이 실패했고(RED, 5 FAIL + 7 ERROR) 새 정의에서 20건 통과했다.

아직 원본과 다른 점(미구현, 갭 또는 범위 밖):
- 개인 재정의(`type = personal`) 행과 그 연쇄 삭제: 모델에 `type`이 없다.
- 삭제 권한은 프로젝트 쓰기 권한(admin·member)이다. 원본은 환경·경로·이름·태그 조건이 붙은 `Delete` 액션이다(G7).
- 폴더 커밋(PIT)·버전 행: 값과 버전이 없다(G2). 원본 `deleteMany`는 `secrets_v2`만 지운다(`:355-374`에 버전 표 접근이 없음, 판독).
- 삭제 시 리마인더 제거는 `remindedAt`이 같은 행에 있어 자연히 따라 없어진다. 원본의 별도 리마인더 표 모델은 가져오지 않았다. 삭제된 시크릿에 대한 `secretReminder` job은 enqueue 단계의 `allow`가 거부한다(`exists Secret`).
- 시크릿을 지운 뒤 동기화(`syncSecrets`)·웹훅은 미선언(G13).

### G11. create가 같은 트랜잭션에서 관련 행을 만들지 못한다

`expose create`에는 `transition`의 `create` 효과가 없다. 조직을 만든 사용자가 그 조직의 첫 admin이 되려면 별도 요청이 필요하다. 이전 초안은 이 단계가 없어 만든 조직을 만든 사람이 읽을 수도 없는 고아 조직이 됐다(**재현**).

현재 포팅 처리: `Organization.createdBy`를 `actor`로 강제하고, 조직의 생성자만 구성원이 0명인 조직에서 자기 자신을 admin으로 한 번 등록할 수 있게 `OrgMembership.create`의 `allow`를 구성했다(**재현**: `test_organization_creator_becomes_admin_exactly_once`). 두 요청 사이 짧은 구간이 남는다.

### G12. create 결과 id를 다음 연산에 넘기는 일반 조합이 없다 (시크릿 이름·세대 분리로는 해당 없음)

이전 판은 `SecretKey`를 만든 id를 `Secret` create에 쓰는 두 요청 구조를 이 갭의 예로 들었다. 2026-10-20에 두 resource를 하나로 합쳐 시크릿 생성은 한 요청이다. 갭 자체(`product/SDK.md` "연산 사이로 값을 넘기는 일반 조합은 없다")는 제품에 남아 있으나 이 포팅에서 당장 쓰는 곳은 없다.

## 이전에 갭으로 적었으나 분류를 바꾼 것

| 이전 ID | 이전 서술 | 정정 |
|---|---|---|
| G3 | SRP 패스워드 프로토콜을 AIP auth가 지원하지 않는 갭 | **분류 변경**. SRP는 원본에서도 레거시 경로이고 로그인과 비밀번호는 AIP가 아니라 외부 IdP의 몫이다([INFISICAL-AUTHN.md](INFISICAL-AUTHN.md)). 제품 갭이 아니다 |
| G2(이전) | "E2E/클라이언트 보유 키 모델이 AIP의 서버 키와 충돌" | **철회**. 원본의 기본 경로가 서버측 KMS라 충돌이 없다. 위 새 G2가 대체 |
| G5 | TTL/lease 동적 시크릿 의미 | 동적 시크릿은 `ee/services/dynamic-secret`에 있다. EE 범위이므로 MIT 분모의 갭이 아니다(EE 경계는 [INFISICAL-EE-BOUNDARY.md](INFISICAL-EE-BOUNDARY.md)) |

## 미확정 갭

| ID | 내용 | 상태 |
|---|---|---|
| G1 | 순위 기반 전문검색 선언 불가 | FlaskBB 7.5가 제품 방언 `search` 파서 거부를 확인했다(다른 세션의 기록). 이 세션은 재확인하지 않았다 |
| G4 | blind index(암호문 동등 검색) | G2에 종속. 설계 13의 `lookup: eq`가 대응안이지만 이 안의 blind index 회전과 unique가 리뷰 P1이라 미확정. 원본은 서버가 HKDF+HMAC으로 계산한다 |
| G6 | X.509/EST/SCEP/ACME 발급 의미 | 확장 job으로 감쌀 수 있으나 선언 의미는 없음. 미착수, 판독 수준 |
| G7 | CASL 조건식 → predicate | 이 세션에서 역할 기반 권한(admin/member/viewer/no-access, 조직 admin 암묵 권한)은 predicate로 표현하고 실DB로 확인했다. 원본의 조건식(환경, 경로, 태그 조건)과 권한 평가 핵심의 EE 소재는 미확인이며 EE 경계 문서에서 다룬다 |
| G8 | provider capability 등록 절차 | 90종 연동의 등록·검증·버전 운영 절차 미정. 미착수 |
| G13 | 서버 job 확장이 큐별 의미(동기화, 리마인더 전달, 웹훅)를 실제로 수행하려면 필요한 capability(외부 HTTP, SMTP, 큐 재시도 정책)와 resource가 아직 선언되지 않았다 | 큐 33개 중 1개만 구현([INFISICAL-JOBS.md](INFISICAL-JOBS.md)) |
