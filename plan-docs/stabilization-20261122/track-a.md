# Track A 결과: 권한·보안·테넌트 (2026-11-22)

브랜치 `stab/a-20261122`(origin/main에 직접 병합, PR 없음). 환경은 Homebrew `postgresql@17`(17.11), 포트 55421, 데이터 `/tmp/aip-stab-a-pg17`, `deadlock_timeout=1s`, **`timezone=UTC`**(아래 "환경 주의"), `CARGO_TARGET_DIR=/tmp/aip-stab-a-target`, `PGHOST`·`PGPORT` 해제. 소유자 결정이 필요한 항목은 [decisions/a.md](decisions/a.md)에 있다.

**`sema.rs` 변경은 `b292a880` 한 번이고 main에 병합됐다.** 이후 A의 변경은 `sema.rs`를 건드리지 않았다(E·D가 이어서 `sema.rs`를 수정해도 된다). `spikes/spike-v1-fixture/src/codes.rs`에는 레지스트리 항목 2개를 추가만 했다.

## 1. 변경

| 커밋 | 파일 | 목적 |
|---|---|---|
| `b292a880` | `sema.rs`, `codes.rs`, `spike-v11-dev-checks/src/lib.rs`, `tests/advisories.rs`, `spec/definition-diagnostics.md`(생성), `CHANGELOG.md`, `COMPATIBILITY.md` | A-01(F1): `where`가 호출자의 `input`을 쓰는 count에 경고 `DEF.ADVISORY_PUBLIC_COUNT_INPUT`. 응답·facts·계약 지문 불변. 골든은 `AIP_UPDATE_GOLDEN=1 … --test diagnostics`로 새 레지스트리 항목만 갱신 |
| `c47cc44f` | `crates/aip-cli/tests/service_flow.rs` | A1: PG17 count 반례 매트릭스 2개(값, 정의 단계). 공개 count 의미는 안 바꾸고 현재 값을 고정 |
| `74ba5b6b` | `spikes/spike-v3-write/src/lib.rs`(`create_row`), `product/tests/external-create-order.test.mjs`, `policy-probes/results-after.txt` | K4 좁힘: 외부 create의 `allow`에서 기본값 필드를 읽지 않는 AND 항을 INSERT 전에 판정. 확장 `ctx.data.create` on 외부 저장소를 처음 실행해 확인 |
| `80e6f21b` | `spikes/spike-v1-fixture/src/policy_advice.rs`, `codes.rs`, `spike-v11-dev-checks/{src/lib.rs,tests/policy_advice.rs}`, `tests/tenant_advice_matrix.rs`, `spec/definition-diagnostics.md`(생성), `CHANGELOG.md`, `COMPATIBILITY.md` | A3: 경고 `DEF.ADVISORY_TENANT_UNSCOPED_COUNT`, 부정으로 쓴 테넌트 조건의 오탐 수정, 5개 상황 반례 표 시험 |
| (이 문서와 같은 병합) | `product/tests/policy-residual-paths.test.mjs`, `decisions/a.md`, `track-a.md`, `baseline.md`, `policy.md` | A-05 판정 순서 시험, K1~K3 현재 값 고정, 결정 항목, 기록 |

## 2. A1: 공개 count 반례 매트릭스와 설계 비교

- 자동 시험: `crates/aip-cli/tests/service_flow.rs`의 `service_policy_public_count_counterexample_matrix_http_postgres`(PG17 HTTP), `service_check_public_count_definition_matrix`(정의 단계). 실행:

  ```
  cargo test -p aip-cli --locked --test service_flow -- public_count
  test service_check_public_count_definition_matrix ... ok
  test service_policy_public_count_counterexample_matrix_http_postgres ... ok
  test result: ok. 2 passed; 0 failed
  ```
- 값 표와 3안 비교(보안성·호환성·작성 부담·런타임 복잡도, 저장소 영향 개수)는 [decisions/a.md D-A1](decisions/a.md#d-a1-공개-count의-기본-의미)이다. 추천은 3안(추가형 구분)이고 공개 count 기본 의미는 바꾸지 않았다.
- RED 확인(고친 쪽 되돌리기): `measure()`가 count에도 `FILTER (WHERE visible)`를 걸게 바꾸면 매트릭스가 `count는 읽을 수 없는 행과 다른 테넌트 행을 모두 센다` 줄에서 실패(`left: Number(2) right: Number(4)`). 경고를 끄면 정의 매트릭스가 새 경고 두 개가 없다고 실패. 둘 다 되돌려 통과.
- 저장소 영향 측정(decisions/a.md): count 28개 가운데 `where`가 정책을 함의하는 14개는 어느 안에서도 불변, 함의하지 않는 14개(`where` 없음 8, 상수 5, `input` 1)가 값이 바뀔 수 있는 대상(9파일, 독립 소비자 앱 3).

## 3. A-01 구현: `DEF.ADVISORY_PUBLIC_COUNT_INPUT`

- 규칙: count이고 `where`가 `input`을 쓰고, 원본에 정책이 없거나(전부 숨은 행) 정책이 `true`가 아니며 `where`가 정책을 함의하지 않고, sourceAccess guard가 `where`가 쓰는 `input`을 모두 받지는 않을 때. sum·min·max는 `rows read`를 다시 걸므로 대상이 아니다.
- RED→GREEN: `spikes/spike-v1-fixture/tests/advisories.rs`의 새 시험 2개. RED 출력 `left: [] right: ["DEF.ADVISORY_PUBLIC_COUNT_INPUT"]`(경고 없음), 구현 뒤 `10 passed`.
- 회귀(저장소의 기존 정의): 이전 바이너리(`/tmp/aip-old-a`, 병합 전 소스)와 새 바이너리가 저장소의 `.aip`·`.e/.h.ts·py` 71개에 `aip service check`를 돌려 종료 코드, 진단 코드, 경고를 비교했다: `71 files; changed: 0`. A3 구현 뒤에도 같은 비교로 `71 files; changed: 0`.
- 거부(오류)로 올리지 않은 이유: 새 거부는 외부 정의를 깬다. `recruitment.aip`의 `approvedCount`는 (a)안에서 거부돼 시험이 깨진 전례가 있다(`policy.md` F1). D-A2에 선택지로 남겼다.

## 4. A2: K1~K4, F1, A-05 PG17 재검증

실행 환경: PG17.11, `AIP_BIN=/tmp/aip-stab-a-target/debug/aip`. `policy-probes.mjs`를 26개 시나리오 전부 다시 돌렸다(`results-after.txt`와 비교).

| 항목 | 재검증 결과(수정 후 HEAD) | 처리 |
|---|---|---|
| F1 | 매트릭스 시험이 값으로 고정(이분 탐색이 다른 테넌트 행 900 복원) | 경고만(A-01). 거부는 D-A2 |
| K1 `unique`+숨은 필드 | code=4242 `ALREADY_EXISTS`, code=1 `ok`. 다른 테넌트 행과 충돌해도 같음 | 시험으로 고정, D-A3 |
| K2 숨은 범위 증감 | 37번 성공 뒤 `BAD_VALUE`(숨은 값 63 복원) | 시험으로 고정, D-A4 |
| K3 숨은 자식 FK | 자식 있는 부모 `REFERENCE_CONFLICT`, 없는 부모 `ok` | 시험으로 고정, D-A5 |
| K4 외부 create allow가 기본값 필드 읽음 | 수정 전: `forbidden+existing=ALREADY_EXISTS`, `forbidden+fresh=FORBIDDEN`(노출). **수정 후: 둘 다 `WIRE.FORBIDDEN`**. 남은 경우만 노출 | **수정(판정 순서)**, 잔여는 D-A6 |
| A-05 전이 unique | 권한 없는 호출자: 충돌하는 값 300도 새 값 999도 `FORBIDDEN`. 권한 있는 호출자: `ALREADY_EXISTS`/`ok` | 결함 없음, 회귀 시험 추가 |
| A-05 전이 check | 권한 없는 호출자 `FORBIDDEN`(qty 99, 5 모두). 권한 있는 호출자 `CHECK_FAILED` | 결함 없음 |
| A-05 전이 `to code = code + 1` 충돌, 삭제의 FK(숨은 자식) | 권한 없는 호출자 모두 `FORBIDDEN` | 결함 없음 |
| A-05 limit·invariant, create check | 권한 없는 호출자의 create 모두 `FORBIDDEN`(정원 찬 값, 안 찬 값, check 위반 동일 본문). 권한 있는 호출자 `ALREADY_EXISTS`·`CHECK_FAILED` 유지 | 결함 없음 |
| A-05 외부 저장소 전이(unique, check, FK) | 모두 `FORBIDDEN`(권한 없는 호출자). 권한 있는 호출자 `ALREADY_EXISTS`, `BAD_VALUE`(DB check) | 결함 없음(탐색 스크립트, 시험은 아래 K4 시험이 create만) |
| A-05 확장 `ctx.data.create` on 외부 저장소 | 수정 전: plain allow는 `EXTENSION_ERROR` 동일(노출 없음). allow가 기본값 필드를 읽는 변형은 `ALREADY_EXISTS` vs `EXTENSION_ERROR`(K4와 같은 결함). **수정 후 둘 다 동일** | 수정 |

발견한 결함은 K4 한 가지(공개 create와 확장 create가 같은 `create_row`를 공유)다. RED→GREEN:

```
RED (수정 전 바이너리) node --test product/tests/external-create-order.test.mjs
  ✖ an external create whose allow also reads a database default refuses the caller who fails the other terms before the INSERT
    actual: 'WIRE.ALREADY_EXISTS', expected: 'WIRE.FORBIDDEN'
  ✖ an extension ctx.data.create on external storage ... (allow that also reads a database default)
    actual: code 'WIRE.ALREADY_EXISTS', expected: 'WIRE.EXTENSION_ERROR'
GREEN (수정 후) ℹ tests 5 / ℹ pass 5 / ℹ fail 0
```

해결 방식: 권한 검사 순서. 그 필드를 읽지 않는 최상위 AND 항은 INSERT 전에 판정하고 전체 `allow`는 INSERT 뒤에 다시 판정한다. 권한 있는 호출자(그 항을 채움)의 진단 코드는 그대로다(시험이 `ALREADY_EXISTS`와 `ok`를 단언). 응답 코드를 일괄 통일하지 않았다.

A-05 시험(`product/tests/policy-residual-paths.test.mjs`)의 RED 확인(코드를 잠깐 바꿈): 전이 `allow`를 항상 참으로 바꾸면 첫 시험이 실패, `create_row`의 `allow`를 항상 참으로 바꾸면 둘째 시험이 실패. 되돌려 `ℹ pass 3`. 이 시험들은 현재 코드에 결함이 없음을 지키는 회귀 시험이라 처음부터 통과한다(결함을 찾지 못한 경로에 대해서는 RED 근거가 없고, 위 변이로 시험이 실제로 판정함을 보인 것이다).

## 5. A3: 테넌트 조언의 오탐·미탐 표

시험: `spikes/spike-v1-fixture/tests/tenant_advice_matrix.rs`(정의 전부 이 파일에 있음). "수정 전"은 `80e6f21b` 이전 바이너리, "현재"는 그 뒤. 판정: 정확 / 오탐(FP) / 미탐(FN) / 침묵(설계). `Org`의 `rows read`가 `id = actor.org`면 생성되는 참조가 읽을 수 있어야 한다는 규칙 때문에 `org = actor.org`가 암묵적으로 보장돼 create allow에 쓰지 않아도 경고가 없다(정확). 이 보장이 없는 경우를 `'` 표기로 따로 시험한다(`Org rows read when true`).

| 상황 | 반례(id) | 수정 전 | 현재 | 판정 |
|---|---|---|---|---|
| 1 read엔 있고 create allow엔 없음 | 1a: allow에 테넌트 없음, Org가 `id = actor.org` | 경고 없음 | 경고 없음 | 정확(참조 가시성으로 보장) |
| | 1a': 같은 정의, Org가 `true` | `WRITE_WIDER_THAN_READ` | 같음 | 정확 |
| | 1b', 1d': allow가 직접·술어로 테넌트 | 없음 | 없음 | 정확 |
| | 1e': allow가 `owner.org = actor.org`(`org` 필드 자체는 안 묶임) | `WRITE_WIDER` | 같음 | 정확 |
| | 1c: create가 `org`를 안 받음(nullable) | 없음 | 없음 | 정확 |
| | 1f': read에 `open` 항, allow는 테넌트만 | `WRITE_WIDER` | 같음 | 정확 |
| 2 쓰기가 읽기보다 넓음 | 2a: 전이 `patch org`, Org가 한정됨 | 없음 | 없음 | 정확 |
| | 2b: 같은 전이, Org가 `true` | `WRITE_WIDER` | 같음 | 정확 |
| | 2c: `internal` 전이의 patch | 없음 | 없음 | 정확(공개 API 아님) |
| | 2d: `allow true` 전이(`rows read`가 대상을 거름) | 없음 | 없음 | 정확 |
| | 2e: create allow가 read의 `open` 항을 안 요구 | `WRITE_WIDER` | 같음 | 정확 |
| 3 relation·aggregate·extension 간접 | 3a: traverse 대상 `Comment`가 `rows read when true` | `PUBLIC_READ_TENANT_FIELD`(Comment) | 같음 | 정확(자원 단위로 판정) |
| | **3b: count 집계, 원본에 테넌트, 상수 `where`(단독 호출 가능)** | **경고 없음** | **`TENANT_UNSCOPED_COUNT`** | **미탐 → 수정** |
| | 3c: `where`가 테넌트를 묶음 / 3h: `where owner = actor` / 3d: sum | 없음 | 없음 | 정확 |
| | 3i: `expose aggregate` 안 한 count | 없음 | 없음 | 정확(단독 총계 없음) |
| | 3g: `where` 없는 count | `PUBLIC_COUNT` | 같음(중복 경고 없음) | 정확 |
| | 3e/3e': 쓰기 확장의 `create.Note`(allow 공유) | 없음 / `WRITE_WIDER` | 같음 | 정확(자원의 create allow로 판정) |
| | 3f: `refIds`(대상 raw id 공개, opt-in) | 없음 | 없음 | 침묵(설계, 테넌트 조언 대상 아님) |
| 4 같은 의미, 다른 모양 | 4a 술어, 4b 인자 없는 술어, 4c 반대 순서, 4e `and true`, 4f/4g OR 가지, 4i, 4j(`org.id`) | 없음 | 없음 | 정확 |
| | **4d: `not (org != actor.org)`** | **`TENANT_UNSCOPED_READ`** | 없음 | **오탐 → 수정**(4d2 `not not`도 같은 정규화로 통과) |
| | 4h: OR 가지 하나가 테넌트·owner 모두 아님, 4h2: `not (org = actor.org)` | `UNSCOPED_READ` | 같음 | 정확 |
| | 4k: `owner.org = actor.org` | 없음 | 없음 | 미탐 가능(관계 경로를 테넌트로 인정, D-A7) |
| | 4l: `exists` 정책 | 없음 | 없음 | 침묵(설계) |
| 5 모든 경로 누락 | 5a: 테넌트 근거 없는 정의 | 없음 | 없음 | 침묵(설계: 근거 없이 추측 금지) |
| | 5b: Member 정책이 근거, Note는 read `true`·create `true` | `PUBLIC_READ_TENANT_FIELD` | 같음 | 정확 |
| | 5b2: read `body != ""`, create `true` | `UNSCOPED_READ`+`WRITE_WIDER` | 같음 | 정확 |
| | 5c: `rows read` 생략(기본 거부) | 없음 | 없음 | 정확(읽기 불가는 노출 아님) |
| | 5d: read가 `owner = actor`, create allow가 그 항을 안 요구 | `WRITE_WIDER` | 같음 | 정확(타인 소유 행을 만들 수 있음) |

수정한 두 가지(둘 다 확실한 판단, 정의가 깨지지 않음):
- 3b 미탐: `DEF.ADVISORY_TENANT_UNSCOPED_COUNT`(경고, 오류 아님). 테넌트 근거(정책이 묶는 같은 이름·대상의 참조)가 있는 원본, 그 원본의 `rows read`는 테넌트를 묶음, 집계가 `expose aggregate`로 단독 호출 가능, `where`(`input` 없음)가 테넌트를 `actor`와 묶지 않을 때만. `input` where는 `…COUNT_INPUT`, where 없는 count는 `…PUBLIC_COUNT`가 맡아 중복하지 않는다.
- 4d 오탐: 정책 정규화가 `not (a != b)`→`a = b`, `not not x`→`x`를 읽는다.

RED→GREEN: 위 두 기능을 끄면 `tenant_advice_matrix`가 `case 3b`, `case 4d`에서 실패(`2 failed`), 되돌려 `5 passed`. 저장소 기존 정의 회귀: 이전 바이너리와 새 바이너리의 `service check` 비교 `71 files; changed: 0`(종료 코드·진단·경고 동일).

## 6. 측정값

- 저장소 `.aip` 67개 중 59개가 `Form::A`로 로드됨(8개는 의도적 오류 반례·조각). count 28개: 정책 함의 14, `where` 없음 8, 상수 `where` 5, `input` `where` 1.
- 경고 신규 발생(저장소 정의): `ADVISORY_PUBLIC_COUNT_INPUT` 0, `TENANT_UNSCOPED_COUNT` 0. 오류 신규 0.
- `default database` 필드가 있는 정의 5개 중 create allow가 생략한 기본값 필드를 읽는 resource는 1개(MySQL fixture `facereview-comments`). D-A6.
- `policy-probes.mjs` 26개 시나리오: 수정 전후 차이는 K4 한 줄뿐(`forbidden+existing`이 `ALREADY_EXISTS`에서 `FORBIDDEN`으로).

## 7. 환경 주의

PG17을 기본 설정으로 띄우면 이 Mac의 시간대(`Asia/Seoul`)가 세션 시간대가 돼 `spike-v3-write`의 `latest_effect_values`·`latest_nullable_projection`이 `"2025-02-01 09:00:00+09"`와 `"…00:00:00+00"`을 비교해 실패한다(A의 변경과 무관, 변경 전 소스에서 같은 실패를 확인). CI의 `postgres:17` 컨테이너는 UTC라 통과한다. 로컬에서는 `ALTER SYSTEM SET timezone='UTC'`로 맞췄다. 후속 작업자는 PG17 기동 절차(baseline 5.1)에 `-c timezone=UTC`를 넣으면 된다.

## 8. 처리하지 못한 것과 이유

- 공개 count 기본 의미, K1~K4 정의 거부, F1 거부(D-A1~D-A6): 공개 의미·호환성을 바꾸므로 확정하지 않았다(지시).
- 읽기 확장 R1(`ctx.data.read` 무선언): A의 범위가 아니라 E(E-02)다.
- 전이(transition)·compose 단계의 효과 `create`(권한 있는 호출자 기준의 유일 충돌, K1 계열): 시험하지 않았다. 호출자가 전이 권한으로 만드는 행이라 K1 계열의 같은 노출이다.
- 외부 저장소 전이의 A-05 결과는 탐색 스크립트로만 확인했다(시험은 외부 create와 확장 create).
- `crates/aip-cli/tests/service_flow.rs` 전체와 `tools/verify.sh` 전체는 돌리지 않았다(Phase2).

## 9. F1(CI)에 연결 요청할 테스트

- `product/tests/policy-residual-paths.test.mjs`: `AIP_BIN`, PG17 필요. `tools/verify.sh`의 node 목록에 `product/tests/external-create-order.test.mjs`와 같은 방식으로 추가(`AIP_BIN=… node --test product/tests/policy-residual-paths.test.mjs`, 3개 시험, 약 1초). 같은 파일이 `external-create-order.test.mjs`에도 K4 시험 4개를 더했으므로 이미 연결된 그 명령은 그대로 새 시험을 실행한다.
- 나머지는 엔진·크레이트 시험이라 `tools/verify.sh`의 cargo 루프가 자동 실행한다: `spikes/spike-v1-fixture/tests/{advisories,tenant_advice_matrix}.rs`, `spikes/spike-v11-dev-checks/tests/policy_advice.rs`, `crates/aip-cli/tests/service_flow.rs`의 `public_count` 시험 2개.
