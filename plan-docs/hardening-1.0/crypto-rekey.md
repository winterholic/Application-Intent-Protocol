# rekey 대상 바인딩 반례 (codex/crypto-target-binding-red-20261107)

[old-branches.md](old-branches.md)에서 보류한 브랜치의 반례 시험을 main(`06cc9c66`) 위에서 실행해 세 반례의 원인을 확정하고, 결함은 고치고 나머지는 시험 전제를 고치거나 알려진 공백으로 남겼다. 실행 환경은 PostgreSQL 17 컨테이너 하나(`postgres:17`, 시험 DB `aip_test_rekey`)와 소스에서 만든 debug 빌드다.

## 결론

| 반례 | 원인 위치 | 판정 | 조치 |
| --- | --- | --- | --- |
| 스키마 그림자 `decoy_frame_changed` | `rekey.rs`의 `rekey_target_column`이 대상 테이블을 `quote_ident(&target.table)`로만 써서 세션 `search_path`로 해석 | **실제 결함**. 시험의 `search_path` 앞 decoy 스키마는 인위적이지만, 운영 연결에는 `search_path` 고정이 없어 `public`의 같은 이름 테이블이 같은 방식으로 잡힌다 | 테이블을 배포 스키마로 한정(1줄). 시험 2개 추가, 수정 전 실패·수정 후 통과 |
| ref 범위 `report includes rows outside Team 42` | `RekeyTarget`에 ref 열이 없어 SELECT·CAS가 ref로 거르지 못함(`rekey.rs`의 `select_sql`·`update_sql`) | **API 공백**이고 테넌트 경계 누출은 아니다. 다른 ref 행의 바이트는 바뀌지 않았다. 시험 전제도 둘 틀렸다(아래) | 공개 API 변경은 하지 않음. 전제를 고친 시험 2개를 추가. 선택지는 아래 |
| CAS 600초 초과 | 교착이 아니라 **시험 훅 설계 오류**. 훅이 모든 비어 있지 않은 페이지 뒤에 불리는데 시험은 장벽을 두 번만 맞춘다 | 시험 결함. 제품 교착 아님 | 훅 없이 행 잠금으로 경쟁을 만들도록 다시 씀. 그러자 실제 공백이 드러나 `#[ignore]`로 남김 |

## 1. 시험 이식과 최초 결과

브랜치 커밋 `7d085aa2`를 main에 cherry-pick한 뒤 `std::sync::Barrier`를 `tokio::sync::Barrier`로 바꾸자 컴파일됐다(old-branches.md의 E0277 6건과 일치).

```
test rekey_schema_shadow_changes_only_the_intended_schema ... FAILED
  schema shadow counterexample: ["intended frame not on active DEK", "decoy frame bytes changed"]; intended_frame_changed=false; decoy_frame_changed=true
test rekey_ref_scope_ignores_other_refs_and_preserves_their_frame_bytes ... FAILED
  mixed-ref counterexample: ["report includes rows outside Team 42"]; scanned=12 failed_count=1 unrelated_frame_preserved=true
test rekey_ref_change_after_select_is_rejected_by_cas_scope ... (90초 알람까지 끝나지 않음)
```

## 2. 스키마 그림자

- 원인: `rekey_target_column`의 SELECT·CAS UPDATE가 `FROM "<table>"`로 스키마 없이 나간다. 락 테이블 등 인프라 SQL은 모두 `{infra_schema}.`로 한정돼 있어 대상 테이블만 `search_path`에 의존했다.
- 운영 경로의 `search_path`:
  - 서비스와 rekey CLI가 쓰는 연결은 `spike_v2_read::connect_owned_with_url`(`spikes/spike-v2-read/src/lib.rs:155`)이다. `Config::from_str(url)`로 URL을 그대로 쓰고 `search_path`·`options`를 더하지 않는다. `crates/aip-service`와 `spikes`의 제품 코드(시험 제외)에서 `search_path`를 설정하는 곳은 `pg_catalog` 고정(adopt·verify·migrate 검사용)뿐이다.
  - 그래서 운영 세션은 서버·역할 기본값을 쓴다. 시험 DB에서 확인: `show search_path` → `"$user", public`, 역할 설정 없음.
  - `aip service serve`의 읽기·쓰기 SQL은 `sqlgen`이 `schema.table`로 한정한다(`spikes/spike-v2-read/src/sqlgen.rs:47`, `:120`). 한정하지 않는 쪽은 rekey뿐이었다.
  - `execute_rekey`는 대상 테이블을 `"schema.table"`에서 스키마를 떼어 `RekeyTarget::new(table, …)`에 넘긴다(`encryption_runtime.rs:274`, `:313`). 스키마가 배포 스키마와 같은지만 검사하고 이름은 버렸다.
- 운영에서 재현되는 형태: 기본 `search_path`에서 `public`에 배포 테이블과 같은 이름의 테이블이 있으면, rekey는 배포 스키마가 아니라 `public` 쪽을 다시 암호화하고 배포 테이블은 그대로 둔다. 없으면 PostgreSQL 이름 해석상 `relation does not exist`로 실패한다(이 경우는 실행으로 확인하지 않았다). 두 경우 모두 시험 이식 전에는 시험이 없었다. 새 시험 `rekey_ignores_a_public_table_with_the_same_name_under_the_default_search_path`가 이 경우다(서버 기본 `search_path`를 단언하고 `public` 테이블을 만든 뒤 지운다).
- 현재 도달성: 운영 rekey는 게이트가 닫혀 있다. `aip service rekey`는 DB 연결 전에 `REKEY_OPERATIONAL_GATE_CLOSED`로 끝나고(`aip-service/src/lib.rs`의 `rekey_cli`), 공개 `execute_rekey`도 `OperationalGateClosed`를 낸다(`rekey_operational_inventory_verified`는 항상 `false`, `encryption_runtime.rs:156`). `aip service serve`는 rekey를 호출하지 않는다. 즉 지금 이 결함으로 데이터가 바뀌는 운영 경로는 없고, `aip_execution::rekey` 라이브러리 API와 게이트가 열린 뒤의 경로가 영향권이다.
- 수정: `let table = format!("{}.{}", quote_ident(ctx.store.deployment_schema()), quote_ident(&target.table));`. 공개 시그니처는 그대로다. `DekStore::new`가 이미 `aip_` 접두와 `[a-z0-9_]`만 허용하므로(`infrastructure_schema`) 한정자가 인용 규칙과 어긋날 수 없다.
- 호환 영향: `RekeyTarget.table`은 계속 비한정 이름이다. 이전에 배포 스키마 밖의 테이블을 `search_path`로 가리켜 rekey하던 호출자는 이제 배포 스키마에서만 찾는다. 이 저장소의 호출자는 `execute_rekey`(스키마가 이미 같음)와 시험뿐이다.
- 범위 밖에서 본 것: PoC 런타임(`aip run`·`aip rekey`, `poc-runtime` 기능, `crates/aip-runtime`)은 모든 SQL이 비한정 이름이고 `aip_runtime::pool`이 `search_path`를 건드리지 않는다. 한 스키마를 기본값으로 쓰는 설계라 rekey만의 결함이 아니며 손대지 않았다.

수정 전후(`rekey.rs`의 한정 한 줄만 `git show origin/main:…`로 되돌려 확인):

```
수정 전  rekey_schema_shadow_changes_only_the_intended_schema ... FAILED  decoy_frame_changed=true
수정 전  rekey_ignores_a_public_table_with_the_same_name_under_the_default_search_path ... FAILED  the deployment table was not rekeyed
수정 후  두 시험 모두 ok
```

## 3. ref 범위

- 원인: 서비스는 ref 선언의 열(`ScopeSpec::Ref { column }`)을 알고 검증까지 하지만(`verify_rekey_inventory`) `RekeyTarget`에 담지 않는다. `rekey_target_column`은 테이블 전체를 훑으므로 다른 ref의 행도 스캔하고, 파싱이 안 되는 frame을 실패로 센다. `scanned=12`는 3패스 × (sweep 2행 + verify 2행)이다.
- 테넌트 경계: 누출 아님. 다른 ref 행이 정상 frame이면 헤더의 `dek_id`가 달라 `is_old`·`is_active`가 모두 거짓이라 건너뛴다. 열리지도 바뀌지도 않는다. 시험도 `unrelated_frame_preserved=true`였다. 영향은 두 가지다. 파싱 불가 frame은 어느 ref 것인지 가릴 수 없어 실패로 세어 그 ref의 구 DEK 폐기를 막는다(fail-closed). `failed_ids`에 다른 ref 행 id가 실린다(rekey를 돌리는 운영자 출력).
- 시험 전제 점검:
  1. 다른 ref 행을 `malformed-frame-preserve-bytes`로 둔다. 암호화 열은 seal 경로만 쓰고 adopt는 전체 non-null 행을 검증해 활성화하므로, 서비스 운영으로는 만들어지지 않는다. DBA 직접 수정이나 손상일 때만 생긴다. 이 경우 fail-closed가 맞다.
  2. `scanned == 1`을 기대한다. ref 열로 완벽히 거른 구현에서도 sweep과 verify가 같은 행을 한 번씩 세어 `scanned=2`다(아래 실험).
- 조치: 전제를 고친 시험 `rekey_ref_scope_leaves_validly_sealed_rows_of_other_refs_untouched`는 다른 ref 행을 그 ref의 DEK로 정상 seal하고, Team 42는 깨끗하게 rekey되며(`failed_count == 0`, `remaining_old == 0`) 다른 ref의 바이트·NULL이 그대로이고 그 ref 키로 여전히 열리는지 단언한다. `rekey_ref_scope_malformed_frame_of_another_ref_fails_closed_without_changing_bytes`는 원래 데이터로 현재 동작(보고하고 폐기를 막되 바이트 불변)을 못박는다. ref 열 바인딩이 들어가면 이 시험은 `failed_count == 0`을 기대하는 시험으로 바꿔야 한다.
- RED: `is_old`를 항상 참으로 바꾸면 `rekey_ref_scope_leaves_validly_sealed_rows_of_other_refs_untouched`가 `["Team 42 rekey was not clean"] … failed_ids: ["2","2","2"]`로 실패하고, 원래대로 돌리면 통과한다.

## 4. CAS 시험이 오래 걸린 원인

- 교착이 아니다. 시험 훅 `rekey_target_column_with_page_hook`은 비어 있지 않은 페이지마다 불린다. rekey 루프는 sweep 페이지 뒤에 verify 페이지를 다시 읽으므로(`rekey.rs`의 `phase == "verify"`) 훅이 두 번 불리는데, 시험은 장벽을 `wait` 두 번(SELECT 직후, CAS 직전)만 맞춘다. 두 번째 호출의 첫 `wait`는 짝이 없어 영원히 기다린다.
- 확인: 훅에 `eprintln!`을 넣고 `--nocapture`로 실행하니 `HOOK enter → after first wait → after second wait → HOOK enter`에서 멈췄다. 90초 알람 시점에 `pg_stat_activity`에 시험 DB 세션은 조회 세션 하나뿐이어서 PG 쪽 잠금 대기도 없었다. 옛 메모의 "PG에서 rekey 연결이 클라이언트 대기 상태"는 이 상태와 맞는다.
- 훅을 첫 페이지에서만 멈추게 고치자 0.1초 안에 끝나며 진짜 반례가 나왔다: `["CAS changed the frame after its selected ref changed", "stale selected row was not reported as raced"]; ref=99 frame_changed=true raced=0`.
- 반영한 형태: 제품 코드에 훅을 두지 않기 위해 `rekey_target_column_with_page_hook`(공개 `#[doc(hidden)]` 추가)은 가져오지 않았다. 대신 시험이 ref 변경 UPDATE를 커밋하지 않은 채 행 잠금을 쥐고 rekey를 시작한다. 페이지 SELECT는 아직 Team 42로 보고, CAS UPDATE가 잠금 뒤에 줄을 선다. `pg_blocking_pids`로 줄 선 것을 확인한 뒤 COMMIT한다. 10초 기한이 있어 어긋나면 멈추지 않고 실패한다.
- 현실성: ref 필드는 전이·patch·효과에서 변경할 수 없다(`docs/design/13-field-encryption.md`의 "ref field" 행). 서비스로는 만들 수 없고 DBA의 직접 SQL이어야 한다. 그때도 행은 이미 Team 99 키로 열리지 않는 frame이라 rekey가 Team 42의 새 DEK로 다시 써도 가독성은 달라지지 않는다. 평문 노출은 없고, 범위 밖 행을 쓰는 불변식 위반이다.
- 조치: 시험은 `#[ignore = "known gap: RekeyTarget has no ref column…"]`로 남기고 `--ignored`로 반례를 볼 수 있다.

## 5. ref 열 바인딩 실험과 선택지

공개 API를 바꾸지 않는다는 조건이라 구현하지 않았다. 효과를 보이려고 `rekey.rs`에 `team_id` 열을 하드코딩한 임시 수정(SELECT와 CAS에 `AND team_id::text = '<scope id>'`)을 만들어 실행한 뒤 되돌렸다. 커밋에 없다.

```
rekey_ref_change_after_select_is_rejected_by_cas_scope ... ok     (임시 수정 적용, --ignored)
브랜치 원본 ref 범위 시험 ... scanned=2 failed_count=0   (임시 수정 적용 시 실패 사유가 `scanned != 1` 하나만 남음)
```

| 선택지 | 내용 | 호환 영향 |
| --- | --- | --- |
| A. `RekeyTarget`에 `ref_column: Option<String>`와 `with_ref_column(self, col)` 추가, SELECT·CAS에 `AND <col>::text = <scope id>`, `target_key`를 v3로 올려 열을 포함. ref 범위인데 열이 없으면 `InvalidInput`으로 거부 | 두 반례를 모두 닫는다. 공개 필드가 늘어 구조체 리터럴로 만드는 호출자는 컴파일이 깨진다. `new`는 그대로. 이 저장소의 호출자는 `encryption_runtime.rs:313` 하나이고 열은 `ScopeSpec::Ref`에 이미 있다. `target_key`가 바뀌어 v2 체크포인트는 이어받지 못하고 처음부터 훑는다. 운영 rekey 게이트가 닫혀 있어 운영 체크포인트는 없다 |
| B. 새 함수 `rekey_target_column_bound(…, &RefBinding)` 추가, 기존 함수는 ref 범위에서 `InvalidInput` | 기존 service 범위 호출자는 영향 없음. ref 범위 호출자는 런타임에서 깨진다(조용히 범위 없이 도는 것보다 낫다). API가 둘이 된다 |
| C. 현 상태 유지(본 작업) | 파싱 불가 frame은 fail-closed로 폐기를 막고, CAS의 ref 변경 공백은 직접 SQL 전제로 남는다 | 변경 없음 |

권고는 A다. 크레이트가 `publish = false`이고 호출자가 하나라 호환 비용이 가장 작고, 게이트를 열기 전에 닫을 수 있다. A를 하면 `rekey_ref_change_after_select_is_rejected_by_cas_scope`의 `#[ignore]`를 풀고 `…malformed_frame_of_another_ref_fails_closed…`를 `failed_count == 0` 기대로 바꾼다.

## 6. 그 밖의 관찰

- `rekey_checkpoint_identity_does_not_bind_deployment_schema`(브랜치 시험)는 통과한다. `target_key`에 배포 스키마가 없는 것은 사실이지만, 락·체크포인트 표가 배포 스키마의 해시로 만든 인프라 스키마(`aip_infra_<해시>`)에 있어 배포마다 따로다. 충돌하지 않으므로 결함으로 보지 않았다.
- 실패한 행 id가 패스마다 `failed_ids`에 다시 들어간다(`["2","2","2"]`). 보고 중복일 뿐이며 이번 범위 밖이라 고치지 않았다.

## 검증

- `cargo test -p aip-execution --test encryption_rekey`: 10 passed, 1 ignored.
- 관련 암호화 시험은 같은 DB로 모두 통과했다(`encryption_cells`·`deks`·`fence`·`journal`·`sealer_opener`, `aip-migrate`의 `encryption_gate`·`encryption_activation_gate`, `aip-service`의 `encryption_runtime_test`, spike-v6의 `encryption_e2e`·`encryption_gate`, spike-v3의 `encrypted_opened`·`encrypted_sealed`, `aip-cli --features poc-runtime`의 `encrypt_e2e`). 개수와 출력은 작업 보고에 있다.
