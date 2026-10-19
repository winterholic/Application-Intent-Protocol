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
- 개인 재정의(`type = personal`): 값과 우선 병합은 G2 대기, 연결 행과 연쇄 삭제는 [G15](#g15)로 선언했다.
- 삭제 권한은 프로젝트 쓰기 권한(admin·member)이다. 원본은 환경·경로·이름·태그 조건이 붙은 `Delete` 액션이다(G7).
- 폴더 커밋(PIT)·버전 행: 값과 버전이 없다(G2). 원본 `deleteMany`는 `secrets_v2`만 지운다(`:355-374`에 버전 표 접근이 없음, 판독).
- 삭제 시 리마인더 제거는 `remindedAt`이 같은 행에 있어 자연히 따라 없어진다. 원본의 별도 리마인더 표 모델은 가져오지 않았다. 삭제된 시크릿에 대한 `secretReminder` job은 enqueue 단계의 `allow`가 거부한다(`exists Secret`).
- 시크릿을 지운 뒤 동기화(`syncSecrets`)·웹훅은 미선언(G13).

<a id="g11"></a>
### G11. create가 같은 트랜잭션에서 관련 행을 만들지 못한다

`expose create`에는 `transition`의 `create` 효과가 없다. 조직을 만든 사용자가 그 조직의 첫 admin이 되려면 별도 요청이 필요하다. 이전 초안은 이 단계가 없어 만든 조직을 만든 사람이 읽을 수도 없는 고아 조직이 됐다(**재현**).

현재 포팅 처리: `Organization.createdBy`를 `actor`로 강제하고, 조직의 생성자만 구성원이 0명인 조직에서 자기 자신을 admin으로 한 번 등록할 수 있게 `OrgMembership.create`의 `allow`를 구성했다(**재현**: `test_organization_creator_becomes_admin_exactly_once`). 두 요청 사이 짧은 구간이 남는다. 2026-10-20 환경 표 도입 뒤 같은 방식으로 환경의 첫 루트 폴더도 별도 요청이며, 이 구간의 경주 상태는 [G16](#g16)에 적었다.

### G12. create 결과 id를 다음 연산에 넘기는 일반 조합이 없다 (시크릿 이름·세대 분리로는 해당 없음)

이전 판은 `SecretKey`를 만든 id를 `Secret` create에 쓰는 두 요청 구조를 이 갭의 예로 들었다. 2026-10-20에 두 resource를 하나로 합쳐 시크릿 생성은 한 요청이다. 갭 자체(`product/SDK.md` "연산 사이로 값을 넘기는 일반 조합은 없다")는 제품에 남아 있으나 이 포팅에서 당장 쓰는 곳은 없다.

<a id="g15"></a>
### G15. 개인 재정의 시크릿: 연결·가시성·연쇄 삭제는 동등, 값 병합은 G2 대기 (2026-10-20)

원본 동작(`infisical-upstream` 고정 47246bec, MIT 파일만 읽음):

| 사실 | 근거 |
|---|---|
| 개인 재정의는 `secrets_v2`의 같은 표 행이다. `type`(기본 `shared`)과 nullable `userId`(users FK, ON DELETE CASCADE) | `backend/src/db/migrations/20240730181850_secret-v2.ts:13,21-22` |
| 개인 재정의는 같은 폴더에 같은 키의 공유 시크릿이 있어야 만들 수 있다. 사용자(USER)만 만든다 | `secret-v2-bridge-service.ts:391-392,406-410` |
| 만들 때 `userId`는 호출자다 | `:499` (`userId: type === Personal ? actorId : null`) |
| 개인 재정의는 읽기·describe 수준 권한이면 만들고 관리할 수 있다(쓰기 권한 불요). 권한 판정 본체는 EE라 읽지 않았다 | 주석과 호출 `:420-424`, `:634-640`, `:966-977` |
| 이름 변경은 거부된다 | `:621-623` |
| 개인 재정의의 수정·삭제는 `(key, folder, type=personal, userId=호출자)`로 찾는다. 남의 행은 대상이 아니다 | `:642-650`, `:954-958`, `secret-v2-bridge-dal.ts:359-363` |
| 목록은 `userId`가 null이거나 호출자인 행만 읽는다 | `secret-v2-bridge-dal.ts:381-413` (`:396-397`) |
| 목록 우선순위 `Priority`: 같은 키의 개인 값이 공유 값을 대체한다. `NeverInclude`는 공유만, `IncludeAll`은 모두 | `secret-v2-bridge-service.ts:1513-1536` |
| 공유 시크릿을 지우면 모든 사용자의 같은 키 개인 행이 같은 `deleteMany`에서 지워진다 | `secret-v2-bridge-dal.ts:364-370`, `service.ts:1053-1055` |
| 유일 인덱스는 `type = 'shared'`에만 걸려 있다. 개인 행의 중복을 막는 인덱스는 없고 생성 때 중복 검사도 하지 않는다 | `20260623144551_secret-folder-unique-index.ts`(`WHERE "type" = 'shared'`), `service.ts:395-410`(중복 검사는 공유만) |

현재 포팅(`app.aip`): `PersonalSecret{secret, user}`, `unique secret, user`. 행은 소유자만 읽는다(`rows read when user = actor and canReadProject(...)`), 소유자만 만들 수 있고(`user = actor`) 프로젝트 읽기 권한이면 충분하다. 소유자만 `PersonalSecret.delete`로 지운다. `Secret.delete`는 `delete many PersonalSecret ... where secret = this.id` 효과로 같은 트랜잭션에서 모든 사용자의 재정의를 지운다. **재현**(실DB, 임시 PostgreSQL 17, `tests/test_live_db.py`): 없는 시크릿·남의 이름 `WIRE.MISSING_TARGET`·`WIRE.FORBIDDEN`, 읽기 전용 사용자는 만들 수 있고 권한 없음·다른 조직은 `WIRE.MISSING_TARGET`, 같은 사용자 두 번째는 `WIRE.ALREADY_EXISTS`, 소유자 외에는 조직 admin도 보지 못하고 지우지 못함, 공유 시크릿 일괄 삭제가 두 시크릿의 모든 재정의를 지우고 다른 시크릿의 것은 남김, 효과 감사 행 `delete:delete`(전이 `Secret.delete`의 효과, before `user` 3, after null), 재정의 1001개는 `WIRE.EFFECT_TARGET_MISSING`으로 통째 거부·1000개는 통과(수정 전 정의에서 신규 9건 모두 실패 후 통과).

원본과 다른 점:
- **값과 우선 병합**: 개인 값, 그것이 공유 값을 덮는 목록 병합(`Priority`)은 값 열이 있어야 선언할 수 있다. 값이 없는 지금은 "재정의가 있다"는 사실만 소유자가 읽는다(G2 대기).
- **유일성이 더 엄격**: 원본은 개인 행 중복을 막지 않는다(위 표). 포팅은 `(secret, user)`를 DB 유일로 둔다. 원본에서 같은 사용자의 중복 행이 실제로 생기는지는 실행하지 않아 확인 필요.
- **폴더 `forceDelete`와 재정의**: 원본은 폴더 삭제의 DB 연쇄가 개인 행도 지운다. 포팅의 `Folder.forceDelete`는 시크릿을 지우는 효과가 그 시크릿의 재정의까지 지우지 못한다(두 단계 일치 `where secret.folder = this.id`는 `service check`가 `DEF.PARSE_EXPECTED`로 거부, G14와 같은 제약). 외래 키 위반으로 미확정 실패를 내는 대신 재정의가 남은 시크릿이 있는 폴더의 `forceDelete`를 `WIRE.INVALID_STATE`로 거부한다. 재정의를 먼저 지우거나 시크릿을 `Secret.delete`로 지우면 된다.
- **재정의 1000개 상한**: 공유 시크릿 하나에 재정의가 1000개를 넘으면 삭제가 통째로 거부된다(원본은 무제한).
- **사용자 삭제 연쇄**: 원본은 users FK 연쇄로 그 사용자의 개인 행이 지워진다. 포팅은 `User` 삭제 전이를 선언하지 않았다.
- 개인 재정의의 `remind`·태그·메타데이터는 대상 아님(값이 없음).

### G14. 폴더 트리 삭제: 한 단계 폴더와 시크릿은 원본과 동등, 하위 트리 연쇄 삭제·이름 문자 검증은 미구현 (2026-10-20)

이전 포팅의 `Folder`는 `path` 문자열 하나(`unique project, path, env`)였고 삭제 전이가 없었다. 제품 술어에는 문자열 접두 비교가 없어 경로 문자열로는 "하위 폴더가 있는가"를 선언할 수 없다(`startsWith(...)`는 `DEF.UNKNOWN_SYMBOL`, `path.prefix(...)`는 `DEF.PARSE_EXPECTED`, 2026-10-20 `aip service check`로 재현). 그래서 원본처럼 부모 참조 트리(`parent`, `name`)로 바꾸고 삭제를 선언했다.

원본 동작(`infisical-upstream` 고정 47246bec에서 직접 읽음):

| 사실 | 근거 |
|---|---|
| 폴더는 `name`, `parentId`(자기 참조, ON DELETE CASCADE), `envId`를 가진다. 환경 루트는 `parentId` null, 이름 `root` | `backend/src/db/migrations/20231218092441_secret-folder.ts:9-16`, `services/project-env/project-env-service.ts:124,131`, `secret-folder-dal.ts:30` |
| 같은 부모 아래 이름 중복은 서비스가 거부한다(DB는 `(parentId, name)` 비유일 인덱스) | `secret-folder-service.ts:207`, `20250319134021_recursive-folder-index.ts` |
| `forceDelete`가 아니면 폴더에 시크릿·동적 시크릿·하위 폴더가 있을 때 삭제를 거부한다. 루트는 부모 아래에서 조회되지 않아 삭제 대상이 되지 않는다 | `secret-folder-service.ts:741-870`(거부 `:809-822`), 라우트 기본값 `forceDelete: false`는 `server/routes/v2/secret-folder-router.ts:318` |
| 폴더 행 삭제 하나로 하위 폴더와 시크릿이 DB 연쇄로 사라진다 | 같은 마이그레이션 `:14-16`, `20240730181850_secret-v2.ts:24`(`secrets_v2.folderId` ON DELETE CASCADE) |
| 삭제 때 폴더 버전 기록과 폴더 커밋 "Folder deleted"를 남기고 동기화를 큐에 넣는다 | `secret-folder-service.ts:824-870` |
| 폴더 이름 문자는 영숫자·하이픈·밑줄만 허용 | `server/routes/v2/secret-folder-router.ts:36-42`, `lib/validator/validate-folder-name.ts:4-8` |
| 생성 때 경로의 없는 중간 폴더를 함께 만든다 | `secret-folder-service.ts:215-238` |

현재 포팅(`app.aip`): `Folder{env, parent?, name}`(`env`는 [G16](#g16)의 `Environment` 참조로, 2026-10-20에 `project`와 문자열 `env`를 대체했다), `unique parent, name`. 사용자는 루트가 아닌 폴더만 만들 수 있고 부모와 같은 환경이어야 한다(환경의 첫 루트는 G16). `Folder.delete`는 시크릿과 하위 폴더가 없을 때만, `Folder.forceDelete`는 하위 폴더가 없을 때 같은 트랜잭션에서 폴더와 그 시크릿(최대 1000행)을 지운다. 루트는 둘 다 거부한다. **재현**(실DB, 임시 PostgreSQL 17, `tests/test_live_db.py`): 29건 중 10건이 수정 전 정의에서 실패했고(RED, 폴더 호출을 트리 모양으로 바꾼 기존 3건 포함. 루트 삭제 거부 테스트는 전이가 없어도 통과하므로 변이 검사로 보강), 새 정의에서 29건이 통과했다. 거부 사유 코드 단언은 첫 통과 뒤 실제 응답을 확인하고 추가했다. 거부 사유 코드를 고정했다: 내용·하위 폴더·루트 `WIRE.INVALID_STATE`, 읽기 전용·부모 불일치·루트 생성 `WIRE.FORBIDDEN`, 읽을 수 없는 부모 `WIRE.MISSING_TARGET`, 1001행 `WIRE.EFFECT_TARGET_MISSING`(1000행은 통과). 감사에는 전이 `forceDelete`와 효과 행 `forceDelete:delete`가 남는다.

폴더 이름·설명 변경(2026-10-20 추가): 원본 `updateFolder`(`secret-folder-service.ts:487-620`)는 루트가 아닌 폴더(부모 아래에서 조회, `:525`)의 `name`과 `description`을 바꾼다. 이름이 바뀔 때만 같은 부모 아래 중복을 검사하고(`:531-546`) `{ name, description }`을 그대로 update에 넘긴다(`:560`, 라우트의 `description`은 `optional().nullable()`, `server/routes/v2/secret-folder-router.ts:144`). 설명 열은 `string`(255) nullable이다(`20250303213350_add-folder-description.ts`). 포팅은 `description: Text(0..255)?`와 `Folder.update`(`patch name, description`, `from parent != null`)를 선언했다. **재현**(실DB): 쓰기 권한자만 변경(읽기 전용 `WIRE.FORBIDDEN`), 루트 `WIRE.INVALID_STATE`, 형제 이름 충돌 `WIRE.ALREADY_EXISTS`, 같은 이름 유지·다른 부모 아래 같은 이름 허용, 빈 이름·256자 이름 `WIRE.BAD_VALUE`, 다른 조직 `WIRE.MISSING_TARGET`, 설명 생략은 기존 값 유지·`null`은 삭제(원본의 undefined/null과 같음), 이름 변경 뒤에도 시크릿과 하위 폴더가 붙어 있음, `parent`·`env`·`project`를 값으로 넘기면 거부되어 이동 불가, 감사에 전이 `update`와 before/after(`name`, `description`)가 남음(`tests/test_live_db.py`, 수정 전 정의에서 신규 8건 모두 실패 후 통과).

원본과 다른 점(미구현, 이 트랙은 core를 수정하지 않았다):
- **하위 트리 연쇄 삭제**: 원본은 `forceDelete`에서 임의 깊이의 하위 폴더와 그 시크릿을 한 번에 지운다. 하위 폴더의 시크릿을 지우려면 두 단계 일치(`where folder.parent = this.id`)가 필요한데 `service check`가 `DEF.PARSE_EXPECTED`("`=` 필요, `.` 발견")로 거부한다(재현). `delete many Folder ... where parent = this.id`는 통과하지만 한 단계뿐이고 그 폴더의 시크릿이 외래 키에 막힌다. 그래서 하위 폴더가 있는 폴더의 `forceDelete`는 부분 삭제 대신 거부한다. 아래에서 위로 지우면 원본과 같은 결과에 도달한다(`test_force_delete_refuses_a_folder_that_still_has_subfolders`). 설계안: 효과의 일치 조건에 한 단계 참조 경로를 허용하거나 재귀 효과(깊이 상한)를 두는 것. 결정은 제품 담당 몫이다.
- **효과 상한**: 폴더당 시크릿이 1000개를 넘으면 `forceDelete`는 통째로 거부된다(원본은 무제한).
- **폴더 이름 문자 검증**: 문자열 형식 제약이 `Text`에 없다(`Text(1..255) pattern "..."`과 `matches "..."` 모두 `DEF.PARSE_EXPECTED`, 재현). 이름 길이만 제한한다. 이전 포팅의 `path`도 같은 제약이 없었다.
- **중간 폴더 자동 생성·경로 입력**: 생성은 부모 id를 받는다. 원본의 경로 문자열 해석과 누락 구간 생성은 클라이언트 몫이다.
- 폴더 이동(`/move`), 폴더 버전·커밋(PIT), 삭제 뒤 동기화 큐, 동적 시크릿 검사(EE)는 미구현이다. 이동은 대상이 자기 하위 폴더인지 확인하려면 임의 깊이 조상 검사가 필요한데 술어에 재귀가 없다.
- 원본 삭제는 열린 변경 요청과 시크릿 승인 정책이 걸린 폴더도 거부한다(`secret-folder-service.ts` `$checkFolderPolicy`, 판정 함수는 `@app/ee/services/secret-approval-*`를 import, `:13-16`). 승인 정책·변경 요청은 EE 모듈이라 이 포팅에 없고 읽지 않았다.
- 삭제 권한은 프로젝트 쓰기 권한(admin·member)이다. 원본의 `SecretFolders` `Delete` 권한(환경·경로 조건)은 EE 권한 평가에 있어 읽지 않았다.

<a id="g16"></a>
### G16. 환경 표: 생성·수정·소프트/하드 삭제·복원은 원본과 동등, 위치 이동·유예 기간·루트 동시 생성은 미구현 (2026-10-20)

이전 포팅의 `Folder.env`는 `Text(1..64)` 문자열이었고 환경이라는 행이 없었다(환경 이름만 폴더에 복사). 원본은 환경이 `project_environments` 행이고 폴더가 `envId`로 참조한다. 그래서 `Environment` resource를 두고 `Folder.env`를 그 참조로 바꿨고, `Folder.project`는 없애고 `env.project`로 얻는다.

원본 동작(`infisical-upstream` 고정 47246bec에서 직접 읽음, MIT 파일만):

| 원본 동작 | 근거 |
|---|---|
| 환경 열: `id`, `name`, `slug`, `position`, `projectId`(프로젝트 삭제 시 연쇄), `deleteAfter`, `softDeletedAt`, `deletedByUserId`, `deletedByIdentityId` | `db/schemas/project-environments.ts`, `db/migrations/20231212110939_project.ts:21-36`, `20260527120000_add-delete-after-and-soft-deleted-at-to-project-environments.ts` |
| `(projectId, position)`은 유일(지연 제약). 소프트 삭제된 행도 같은 표에 있어 위치를 차지한다 | `20231212110939_project.ts:29-33` |
| `(projectId, slug)` 유일 제약은 찾지 못했다(`TableName.Environment`를 쓰는 마이그레이션의 unique 선언 검색 결과는 정책-환경 표뿐). 서비스가 slug 중복을 거부하고, 소프트 삭제된 행이 쥔 slug도 거부한다 | `project-env-service.ts:87-102`(생성), `:190-205`(수정), `project-env-dal.ts`의 `findBySlugIncludingExpired` |
| 생성: 위치가 있으면 그 자리 이후를 한 칸씩 밀고, 없으면 마지막 뒤에 붙인다. 같은 트랜잭션에서 루트 폴더(이름 `root`, `parentId` null)를 만든다 | `project-env-service.ts:113-134`, 루트는 `:124,131` |
| 수정: `name`, `slug`, `position`. 소프트 삭제된 행은 조회되지 않아 수정할 수 없다. 위치가 이미 있으면 다른 환경들을 밀어 자리를 만든다(거부하지 않는다) | `project-env-service.ts:148-241`(위치 `:223`), `project-env-dal.ts`의 `findOne`(`whereNull("deleteAfter")`) |
| 삭제(기본): 행을 지우지 않고 `softDeletedAt`, `deleteAfter`(14일 뒤), 삭제한 사용자·아이덴티티를 기록한다. 읽기 경로(`findById`, `findOne`, `find`, `findBySlugs`)는 `deleteAfter`가 있는 행을 숨긴다. 위치 틈은 메운다 | `project-env-service.ts:312-346`, `project-env-queue.ts:12-13`(`SOFT_DELETE_GRACE_DAYS = 14`), `project-env-dal.ts` |
| 삭제(`hardDelete`): 행을 바로 지우고 DB 연쇄가 폴더와 시크릿을 지운다. 위치 틈을 메운다 | `project-env-service.ts:298-309`, 폴더의 `envId`는 `ON DELETE CASCADE`(`20231218092441_secret-folder.ts:14`) |
| 복원: 소프트 삭제된 행만, 유예 기간이 끝나기 전에만 되돌린다. 마지막 위치 뒤에 붙인다 | `project-env-service.ts:361-443`(유예 검사 `:402-407`) |
| 유예가 끝난 행은 cron이 비운다(시크릿 버전·참조를 먼저 지운 뒤 행) | `project-env-queue.ts`, 포팅에는 미선언([INFISICAL-JOBS.md](INFISICAL-JOBS.md)의 `project-env-hard-delete`) |
| `slug`는 소문자·숫자·하이픈 64자 이하, `name`은 1~255자 | `server/routes/v1/project-env-router.ts:139-141`, `server/lib/schemas.ts:15-26` |
| 환경 연산은 `Environments` 권한을 요구하고 삭제는 승인 정책이 걸린 환경을 거부한다 | `project-env-service.ts` 각 함수 앞부분, 삭제 거부는 `:277-297`. 권한·승인 정책 본체는 EE 모듈이라 읽지 않았다 |

현재 포팅(`app.aip`): `Environment{project, slug, name, position, softDeletedAt?, deletedBy?}`, `unique project, slug`, `unique project, position`. 프로젝트 쓰기 권한자가 `create`, `update`(`slug`, `name`, `position`), `softDelete`, `restore`, `hardDelete`를 한다. 소프트 삭제된 행은 읽기 전용 권한자에게 보이지 않고(`rows read`) 쓰기 권한자에게만 보여 복원·삭제 대상이 된다. 그 환경의 폴더와 시크릿은 모두에게 숨겨지고 새로 만들 수 없다. `hardDelete`는 환경 행과 그 환경의 모든 폴더(최대 1000행)를 같은 트랜잭션에서 지운다(폴더마다 `env`를 가지므로 한 단계 일치 `where env = this.id`로 선언된다). 환경의 첫 폴더(루트)는 쓰기 권한자가 두 번째 요청으로 한 번 만든다(이름이 `root`이고 그 환경에 루트가 없을 때만).

**재현**(실DB, 임시 PostgreSQL 17, `tests/test_live_db.py`): 쓰기 권한자만 생성·수정·삭제·복원(읽기 전용 `WIRE.FORBIDDEN`, 접근 없음·다른 조직 `WIRE.MISSING_TARGET`), slug·위치 중복 `WIRE.ALREADY_EXISTS`, 같은 slug·위치는 다른 프로젝트에서 허용, 길이 한계(slug 64, name 255)와 위치 0 `WIRE.BAD_VALUE`, 소프트 삭제 뒤에도 slug·위치 점유 유지와 수정·재삭제 `WIRE.INVALID_STATE`, 활성 환경 복원 `WIRE.INVALID_STATE`, 복원 뒤 내용물 그대로, 루트는 환경마다 한 번이고 이름이 `root`일 때만, 시크릿이 하나라도 있으면 `hardDelete` `WIRE.INVALID_STATE`(소프트 삭제로 숨겨진 시크릿도 막는다), 폴더 1001개는 `WIRE.EFFECT_TARGET_MISSING`(1000개는 통과), 감사에 환경 `update`·`hardDelete`와 폴더 효과 행 `hardDelete:delete`가 남음. 수정 전 정의에서 신규 15건이 모두 실패했고, 폴더 쪽 기존 테스트도 `env` 참조 모양으로 바꾸면서 같이 실패했다(RED). 새 정의에서 통과했다.

**원본과 다른 점과 이유**(거부는 모두 2026-10-20 `aip service check`로 재현):

- **위치 밀기·틈 메우기·자동 끝 붙이기**: 한 요청에서 프로젝트의 다른 환경들 위치를 바꿔야 한다. 전이의 `update many`에 `position = position - 1`(또는 `+ 1`)을 쓰면 `DEF.PARSE_EXPECTED`("이름 필요, `-` 발견"/"`+` 발견"), 같은 프로젝트 전체를 고르는 `where project = this.project`는 `DEF.UNSUPPORTED_EFFECT`("many update에는 non-null 부모 참조 = this.id 조건 필요")로 거부된다. 포팅은 호출자가 위치를 주고, 이미 있는 위치는 거부하며(`unique project, position`), 틈을 메우지 않는다. 소프트 삭제된 환경은 위치를 그대로 쥔다(원본은 맨 뒤로 옮긴다).
- **14일 유예와 영구 삭제**: `softDeletedAt`만 기록한다. 시각 산술(`to deleteAfter = now + 20160m`)은 `DEF.PARSE_EXPECTED`("식 필요, `1209600000ms` 발견")로 거부되고, 시간 단위는 `ms`, `s`, `m`뿐이며(`d`, `h`, `days`는 `DEF.LEX_BAD_DURATION`) 지속 시간 리터럴은 식에 쓸 수 없다. 그래서 `deleteAfter` 열도, "유예가 지났으면 복원 거부"도, 유예 뒤 영구 삭제 cron도 없다. 소프트 삭제된 환경은 `hardDelete`를 직접 부를 때까지 남는다.
- **루트 폴더 동시 생성**: `expose create`는 다른 행을 함께 만들 수 없다([G11](#g11)). 환경과 루트 사이에 요청이 하나 더 있다. 두 요청이 동시에 오면 둘 다 "루트 없음"을 통과해 루트가 둘 생길 수 있다(`unique parent, name`은 `parent`가 null이면 막지 못한다). 이 경주 상태는 테스트하지 않았다.
- **시크릿이 있는 환경의 `hardDelete`**: 시크릿은 환경에서 두 단계 떨어져 있다(시크릿 → 폴더 → 환경). 효과 `delete many Secret ... where folder.env = this.id`는 `DEF.PARSE_EXPECTED`("`=` 필요, `.` 발견", G14와 같은 제약)로 거부된다. 포팅은 환경 안에 시크릿이 하나라도 있으면 `hardDelete`를 거부한다. `Secret.delete`로 시크릿을 먼저 지우면 같은 결과에 도달한다.
- **slug 문자 검증**: `Text`에 문자열 형식 제약이 없다(G14). 길이만 제한한다.
- **환경 수 상한**(`plan.environmentLimit`, `project-env-service.ts:104`)은 라이선스 서비스에서 온다. EE 모듈이라 읽지 않았고 선언하지 않았다.
- **`deletedByIdentityId`**: 포팅의 행위자는 `User`뿐이라 `deletedBy`만 둔다(머신 아이덴티티 행위자는 I02 미구현).
- 삭제의 승인 정책 거부와 `Environments` 권한 조건은 EE 범위라 이 포팅에 없다.
- 환경이 소프트 삭제된 동안 폴더와 시크릿이 숨겨지는 방식은 읽기 정책(`env.softDeletedAt = null`)이다. 원본에서는 환경 조회가 실패해서 생기는 결과다. `PersonalSecret`은 가리키는 시크릿이 숨겨지는 것으로 간접히 막힌다(직접 검증하지 않았다).

설계안(결정은 제품 담당 몫): 효과의 일치 조건에 한 단계 참조 경로를 허용하면 G14(하위 트리)와 이 갭(시크릿이 있는 환경 삭제)이 함께 풀린다. 위치 밀기는 같은 부모 아래 행의 순서 값을 다시 매기는 효과가 필요하고, 유예는 `Time` 값에 지속 시간을 더하는 식과 날짜 단위가 필요하다.

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
