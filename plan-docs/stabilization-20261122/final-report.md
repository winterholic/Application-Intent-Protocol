# AIP 안정화 최종 보고서 (원 요청 10절 형식)

- 작성일: 2026-11-22. 상태: **최종 보고**.
- 안정화 코드 기준: Core `d3acc39376cdd5277df8a7ea241cb3bfb23f05f6`(이전 통합 후보 `720b9b25`에 두 교정을 반영). 이 SHA의 K4 개별 PG17 검증과 Verify 결과는 아래 CI 기록에 연결한다.
- RC 출시 후보: 문서 커밋 `f7b0c225bb0405c77d174e621c3a6753049a07a6`에서 의존성 감사가 실패했고, Cargo.lock만 교정한 `1eda239c28a8399d62f88360ebd654dbbeee6f53`에서 전체 RC가 성공했다. 문서 커밋은 RC 검증 SHA가 아니다.
- 근거: `plan-docs/stabilization-20261122/`의 Track A~F 기록·결정 문서, 해당 커밋 이력, 리드가 확인한 CI·Portal·Starter 결과. 이 문서 편집에서는 제품 테스트나 전체 RC를 실행하지 않고 문서 검사만 수행한다.
- 이 문서는 1.0 출시 승인이 아니다. 태그, GitHub Release, npm, PyPI 게시는 하지 않았다.

## 한눈에

| 항목 | 상태 |
|---|---|
| 해결 가능한 결함의 수정과 회귀 시험 | Track A~E에서 수행. 1절 |
| 안정화 Core SHA 필수 CI | 리드 확인: Verify 45014410289, SDK semantic 45014412421 성공. 기준 SHA 모두 `d3acc393…` |
| 안정화 Core SHA 독립 소비자 job | 리드 확인: 45014414753 `consumers` 성공, 기준 SHA `d3acc393…` |
| Portal·Starter Core 호환 확인 | Portal `fd69229a`, Starter `0d292333`; 양쪽 결과 및 Starter CI 성공 확인 |
| 소유자 결정 | 5개 영역의 결정은 확정되지 않음. 선택지와 권고는 5절 |
| sema.rs 대기 todo 3건 | 남음(C1 budget, C2 산술 범위, C3 id 정렬) |

CI 이력과 리드 확인 결과:

| Run | SHA | 결과 | 설명 |
|---|---|---|---|
| macOS Actions 44985825145 | `c86d6902` | success | 이전 SHA의 기록이며 최종 SHA 검증이 아니다 |
| SDK 45002420780 | `b6af9d44` | success | bwrap worker 시작 문제를 수정한 별도 SHA의 결과 |
| SDK 45003272337 | `eba0ac35` | failure | SQL 식별자 인용으로 migration column lookup이 어긋남. 후속 수정 `720b9b25` |
| Verify 45002432400 | `eba0ac35` | failure | `durable.rs` bare `QUEUED` 리터럴. 후속 수정 `720b9b25` |
| Verify 45006760676 | `720b9b25` | failure | 전체 식별자 인용으로 DDL digest가 바뀌고 Python JobState 확인 실패. `5c19468e`에서 예약어 101개만 인용하고 SQL 경계를 교정 |
| Verify [45014410289](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45014410289) | `d3acc39376cdd5277df8a7ea241cb3bfb23f05f6` | success | 리드 확인: `core`와 `head-fast` |
| SDK semantic [45014412421](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45014412421) | `d3acc39376cdd5277df8a7ea241cb3bfb23f05f6` | success | 리드 확인: `sdk-postgres-17` |
| independent-consumer-rc [45014414753](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45014414753) | `d3acc39376cdd5277df8a7ea241cb3bfb23f05f6` | success | 리드 확인: `consumers` |
| Verify [45019169141](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45019169141) | `f7b0c225bb0405c77d174e621c3a6753049a07a6` | success | `head-fast`, `core` 성공 |
| Release candidate [45019286972](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45019286972) | `f7b0c225bb0405c77d174e621c3a6753049a07a6` | failure | `dependencies / audit`: `RUSTSEC-2026-0592` (`url 2.5.8`) |
| Release candidate [45024949144](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45024949144) | `1eda239c28a8399d62f88360ebd654dbbeee6f53` | success | 24 jobs 성공, workflow가 허용한 `crypto-compile-repair` skip 1개 |

위 세 안정화 SHA 결과는 리드가 같은 SHA에서 확인한 실행이며, 이 문서 편집 세션에서 해당 Run 로그를 재조회하지 않았다. 이후 문서 SHA와 의존성 교정 SHA의 Verify·RC 결과는 별도 행에 구분했다. 이전 실패는 교정 커밋의 이력을 보이는 것이며 실패를 성공으로 바꿔 적지 않았다.

---

## 1. 실제로 개선된 것

완료한 일만 적는다. 제안은 6절이다. 안정화 코드의 기준 SHA는 `d3acc39376cdd5277df8a7ea241cb3bfb23f05f6`이다. Track에서 리드가 통합한 관련 커밋을 기준으로 요약했다.

### Track A: 권한·보안·테넌트

| 항목 | 커밋과 코드 위치 | 변경 전 문제 | 변경 후 보장 |
|---|---|---|---|
| 공개 count 입력 경고 | `b292a880`. `sema.rs:2412` `DEF.ADVISORY_PUBLIC_COUNT_INPUT` | `where`가 호출자의 `input`을 쓰는 count가 숨은 행의 값을 이분 탐색으로 노출해도 `check`가 침묵 | 해당 정의에 경고. 응답, facts, 계약 지문은 그대로. 저장소 정의 71개에서 새 오류 0, 새 경고 0 |
| PG17 count 반례 시험 | `c47cc44f`. `crates/aip-cli/tests/service_flow.rs` | 공개 count가 읽을 수 없는 행과 다른 테넌트 행을 센다는 사실이 시험으로 고정돼 있지 않았다 | 현재 값(예: 호출자가 읽는 행 2개인데 count 4)을 시험이 지킨다. 의미는 바꾸지 않았다 |
| 외부 create 판정 순서(K4) | `74ba5b6b`. `spike-v3-write/src/lib.rs:1243` `create_row` | allow가 `default database` 필드를 읽는 외부 create에서 권한 없는 호출자가 `FORBIDDEN` 대신 `ALREADY_EXISTS`를 받아 숨은 행 존재가 드러남 | 그 필드를 읽지 않는 최상위 AND 항은 INSERT 전에 판정하고 전체 allow는 INSERT 뒤 재판정. 권한 없는 호출자는 `WIRE.FORBIDDEN`. 권한 있는 호출자의 진단 코드는 유지. 확장 `ctx.data.create`도 같은 함수라 함께 적용 |
| 테넌트 경고 | `80e6f21b`. `policy_advice.rs:17,168` `DEF.ADVISORY_TENANT_UNSCOPED_COUNT` | 원본에 테넌트가 있는 단독 count의 `where`가 테넌트를 묶지 않아도 경고 없음(미탐) | 경고 추가(오류 아님). 5개 상황 반례표 시험이 고정 |
| 테넌트 오탐 | `80e6f21b`. `policy_advice.rs:248` | `not (org != actor.org)`를 테넌트 조건으로 읽지 못해 `TENANT_UNSCOPED_READ` 오탐 | `not (a != b)`는 `a = b`, `not not x`는 `x`로 정규화해 오탐 제거 |

### Track B: 동시성·트랜잭션·멱등

| 항목 | 커밋과 코드 위치 | 변경 전 문제 | 변경 후 보장 |
|---|---|---|---|
| bundle 잠금 순서 | `cf429b31`. `spike-v3-write/src/lib.rs:2136` `lock_bundle_targets`, 호출 `:2185` | 같은 행을 반대 순서로 수정하는 bundle 둘이 동시에 오면 교착. 동시 2건이면 교착 1회에 약 1.2초, 4건 이상이면 대부분 2초 기한 초과(수정 전 격자 15칸 중 12칸에서 성공률 25% 미만) | bundle이 이름 붙인 행이 둘 이상이면 시작 시 `(resource, id)` 순으로 선잠금. 연산 실행 순서, 판정, 오류 코드, 롤백은 그대로. 읽을 수 없는 행은 잠그지도 기다리지도 않음 |
| 취소·완료 정산 | `afaac8e2`, `d2c2b269`. `spike-v6-transport/src/durable.rs:1257` `OutcomeExpectation`, `settle_attempt` | 취소 확인 조회의 `if let Ok(Some(row))`가 DB 오류를 조용히 버림. UPDATE 0행이나 COMMIT 미도달 뒤에도 성공을 보고할 수 있었음 | 저장된 상태, epoch, attempt, state/code/output이 기대와 같을 때만 확정. COMMIT 미도달은 정산 재시도, ACK 유실은 저장 상태를 확인해 결과 보존. 작업 실행 횟수는 1 |
| 배포 fence 오판 | `afaac8e2`. `spike-v6-transport/src/lib.rs` `check_deployment_fence`(`:198` 부근) | catalog 조회 실패가 `WIRE.DEPLOYMENT_CHANGED`로 보고돼 운영자가 재시작해야 하는 변경으로 오해 | 조회 실패와 시간 초과는 `WIRE.DB_UNAVAILABLE`(재시도 가능). 실제 drift만 `DEPLOYMENT_CHANGED` |
| 확장 읽기의 직렬화 실패 | `afaac8e2`. `spike-v2-read/src/lib.rs:271` 부근 | 쓰기 확장의 `ctx.data.read`에서 `40001`, `40P01`이 `WIRE.INTERNAL`로 분류됨 | `WIRE.CONFLICT`(재시도 가능), 요청 전체 롤백 |
| 멱등 키의 주체 격리 | `afaac8e2`. `tests/apply_idem_principal_scope.rs` | 같은 키를 다른 주체가 보낼 때의 동작이 시험으로 고정돼 있지 않았다 | 두 주체가 각자 한 번씩 실행되고 결과는 소유자에게만 재생된다는 시험. 이 커밋의 `lib.rs` 변경은 위 fence 5줄뿐이라 **제품 코드 변경 없이 시험만 추가**된 항목이다 |
| 큐 상태 상수 | `720b9b25`. `crates/aip-execution/src/jobs.rs` `JOB_STATE_QUEUED`, `durable.rs:1268` | `durable.rs`가 bare `"QUEUED"` 리터럴을 써서 `wire_codes` 검사가 실패(Verify 45002432400) | 상수로 교체. 동작은 같다 |
| Asia/Seoul 시험 | `afaac8e2`. `latest_effect_values.rs`, `latest_nullable_projection.rs` | 세션 시간대가 서울이면 같은 instant인데 문자열 비교가 실패 | UTC instant 기준으로 비교. 제품 코드 변경 없음 |

### Track C: 최신 Core 기준 독립 소비자 E2E

| 항목 | 커밋과 코드 위치 | 변경 전 문제 | 변경 후 보장 |
|---|---|---|---|
| 대상 SHA·엔진·SDK 출처 검증 | `d627c4d7`. `verification/1.0-consumers/provenance.py`, `run.py`, `test_provenance.py` | 소비자 검증이 과거 Core `4e132564`에 고정돼 최신 코드를 검증하지 못함 | 대상 SHA와 checkout HEAD가 다르거나 트리가 더러우면 실패. 엔진 바이너리 sha256, `BUILD.json`, SDK tarball·wheel 내용을 대조. 결과 JSON 기록. 불일치 반례 17개 시험 |
| required 연결 | `b818613d`. `independent-consumer-rc.yml`, `release-candidate.yml` | release-candidate의 required에 소비자 검증이 없음 | `workflow_call` 입력 `core_sha`(기본 `github.sha`)로 같은 SHA 실행, `required.needs`에 등록 |

### Track D: Parser → Sema → Runtime → SDK 정합

| 항목 | 커밋과 코드 위치 | 변경 전 문제 | 변경 후 보장 |
|---|---|---|---|
| SDK 이중 실행 방지 | `2718cfaa`. `transport.ts:65` `NOT_EXECUTED`, `product/python/aip_sdk/__init__.py:52,444` | 응답 유실 뒤 재시도가 `RATE_LIMITED`, `SERVER_BUSY` 등 실행 전 거절을 받으면 키를 지우고 최종 실패로 보고. 호출자가 새 키로 재전송해 이중 실행 가능. Python은 키 없이 재호출하면 새 UUID를 만듦 | 앞선 시도가 있었던 경우 실행 전 거절 5개 코드는 미확정으로 유지. Python도 미확정 키를 재사용 |
| bundle 결과 id 검증 | `32110c31`. `transport.ts:25` `validWriteResponse` | Uuid 자원의 compose를 포함한 bundle이 커밋됐는데도 형식 오류로 보고되어 영구 미확정 | 서버의 `ops[i].applied` 구간으로 행을 맞춤 |
| TS 타입 거부 | `3f68ea76`. `spikes/spike-v5-sdk/sdk/generic.ts:102` `CreateOpWithRefs` 외 | 참조 아닌 키를 refs로 줌, 같은 필드를 select에 두 번, refs로 채운 필드를 values에도 줌이 컴파일됨 | 컴파일 오류. 잘못된 요청 57개가 각각 한 줄씩 오류를 낸다. 타입은 런타임 권한을 보장하지 않는다 |
| 예약어 필드 | `5c831619`(DDL·읽기·쓰기 SQL 인용), `33510442`(v3 쓰기 컬럼), `a514c941`. `sqlgen.rs:85,110`, `spike-v3-write/src/lib.rs` 앞부분 `column()` | `grant`, `order` 등 12개 필드명은 `service check`를 통과하고 `service init`이 DB 구문 오류로 실패(C-D1) | 컬럼 식별자를 인용. v3 쓰기 SQL 단위 시험이 RED→GREEN(`grant` 기대 `"grant"`, 실제 `grant`였다) |
| 확장 Json 입력 | `a514c941`. `spike-v2-read/src/scalar.rs:7`, `client/typed.ts:169` `validJson` | 확장 입력의 `Json`이 어떤 값도 받지 못함. 문서는 `Json`으로 받으라고 안내(C-D2) | 배열, 객체, 문자열, 숫자, 불리언, null 허용. JSON에 담을 수 없는 JS 값은 전송 전 거부 |
| check와 init 정합 | `d5f3a0fd`. `crates/aip-service/src/lib.rs:50,999` `--for-postgres-init` | `default database` 필드는 `check`를 통과하고 `init`이 `EXTERNAL_DEFAULT_REQUIRED`로 거절(C-D3) | DB 연결 없이 init 적합성을 검사하는 옵션. 기본 `check`는 외부 채택 정의를 계속 허용(sema 전역 거부는 하지 않음) |
| 마이그레이션 열 조회 키 | `720b9b25`. `crates/aip-migrate/src/lib.rs:707` `table_defs` | 식별자 인용 일관화 뒤 생성 DDL의 열 이름이 `"nickname"`으로 나와, 이름으로 조회하는 마이그레이션이 열을 못 찾음(SDK 45003272337 failure) | 인용 부호를 벗긴 키로 조회. 재현 시험 `generated_quoted_nullable_column_matches_migration_lookup` 추가 |

### Track E: 진단·확장·구조

| 항목 | 커밋과 코드 위치 | 변경 전 문제 | 변경 후 보장 |
|---|---|---|---|
| 진단 기계 필드 | `3aa98afa`. `spike-v1-fixture/src/diag.rs:62` 외 | 위치 없는 진단이 `0:0`이라 구분 불가, 범위·앵커를 문구에서 파싱 | `located`, `anchor`, `scopePath`, 경고의 `line`·`column`·`explain`, 비정의 실패의 `reason`을 가산. 기존 필드 125개 비교에서 차이 0, 종료 코드 15개 동일 |
| 좌표 | `06e5f2d8` | 위치를 아는 진단도 `Span::default`(0:0) | 잘못된 리터럴, 크기 초과, 다중 블록, 잘못된 binding은 실제 좌표. 위치를 모르는 것만 0:0과 `located:false` |
| 접근 대상 미공개 코드 | `4fbe131b`. `sema.rs:2757,2866` `DEF.ACCESS_TARGET_NOT_EXPOSED` | 쓰기 확장·job의 비공개 전이 접근이 `DEF.UNKNOWN_SYMBOL`이라 소비자가 `message` 부분 문자열로 구분 | 전용 코드. 저장소 정의 195개 390회 비교에서 바뀐 것은 fixture 1개의 `code`, `explain`, `help`뿐 |
| 읽기 확장 선언 | `753be26f`. `sema.rs` 읽기 확장 access 분기, `spike-v4-worker/src/lib.rs:165` `reads` | 읽기 확장의 `ctx.data.read`가 선언 없이 어느 resource든 읽음 | opt-in `access read.<Resource>`. 선언한 확장만 목록 밖 읽기를 DB 계획 전 `WIRE.ACCESS_NOT_DECLARED`로 거부. 선언 안 한 확장은 그대로 |
| 진단 출력 모듈 | `6b1e05a3`. `crates/aip-service/src/diagnostics.rs` | 진단 출력이 `lib.rs`(2790줄)에 섞여 있음 | 출력과 종료 코드를 분리. 모듈 분리 전후 출력이 바이트 동일 |
| PG 없는 진단 시험 | `3aa98afa`. `crates/aip-test-support/build.rs` | 진단 시험도 PG 환경이 필요 | `AIP_TEST_NO_DB=1`로 실행 |

### Track F: CI 구조

| 항목 | 커밋 | 변경 전 문제 | 변경 후 보장 |
|---|---|---|---|
| main push 필터·SHA 단위 concurrency·required 판정 | `9e78caea` (`scripts/release-candidate-required.py:38` `evaluate`) | 최근 Verify 200회 중 67회가 cancelled. 어떤 main SHA는 판정이 없었고 `4f44ca87`~`74ba5b6b`는 경로 필터 workflow도 안 돎 | main push는 필터 없이 실행, concurrency 그룹에 SHA. required는 success만 통과, 선언한 workflow 전부 집계, 모든 job이 후보 SHA |
| 단일 진입점, 안정 probe, macOS 전용 시험 | `071694e6`, `c162837e`. `scripts/verify-head.sh` | macOS 전용 시험 5곳과 안정 동시성 probe 12개가 어떤 workflow에서도 안 돎. 최신 HEAD 필수 검증을 재현할 한 명령이 없음 | 위 시험을 필수 경로에 연결. `verify-head.sh`는 SHA를 기록하고 더러운 트리의 통과를 증명으로 쓰지 않음 |
| provenance·정책 거부 연결 | `5df0e878` | provenance 단위 시험과 정책 거부 계약이 빠른 경로에 없음 | fast 단계와 `tools/verify.sh`에 연결 |
| D 의미 시험의 PG17 job 연결 | `50571faf`, `d3edb84c`, `c5898ed0`, `9f85a150` | D의 PG17 시험 5개가 필수 경로 밖. worker 격리 런타임이 없어 SDK job의 worker 시작 실패 | `sdk-semantic-e2e.yml`이 D 시험 5개를 실행. 격리 런타임 준비. Verify에 Track B PG17 수동 입력 |
| macOS 지원 문서 | `eba0ac35` | 문서가 macOS Seatbelt를 CI 없음으로 기록 | Run 44985825145(`c86d6902`)를 근거로 정정. 후보 SHA나 최종 SHA의 검증은 아니라고 명시 |

---

## 2. 검증한 것

종류, 환경, 결과, SHA를 문서와 커밋에 기록된 실행 근거로만 적었다. SHA는 그 실행이 돈 코드 기준이며, 통합 이후 재실행하지 않은 것은 그렇게 표시했다.

### 2.1 로컬 실행 기록(Track 문서에 명령과 출력이 있는 것)

| Track | 종류 | 환경 | 결과(문서 인용) | 기준 SHA |
|---|---|---|---|---|
| A | PG17 count 반례 매트릭스 | 로컬 PG17.11 | `test result: ok. 2 passed; 0 failed` | Track A 브랜치, 통합 후 재실행 기록 없음 |
| A | 외부 create 순서(RED→GREEN) | 로컬 PG17.11 | RED `actual: 'WIRE.ALREADY_EXISTS', expected: 'WIRE.FORBIDDEN'`, GREEN `ℹ pass 5 / ℹ fail 0` | 같음 |
| A | 경고·테넌트 시험 | 무DB | `advisories` 10 passed, `tenant_advice_matrix` 5 passed, `policy-residual-paths` 3 pass | 같음 |
| A | 저장소 정의 회귀 | 이전·새 바이너리 비교 | `71 files; changed: 0` (A3 구현 뒤에도 같음) | 같음 |
| B | bundle 격자(수정 전후) | 로컬 PG17.11, `deadlock_timeout=1s` | 수정 후 격자 15칸 전부 성공률 100%, 교착 0, p99 최대 61ms, 롤백 위반 0. 수정 전 최악 `0% (0/120)`, p99 2072ms | `cf429b31` |
| B | 취소·완료 정산 | 로컬 PG17(55422) | `durable_cancel_settlement`: `test result: ok. 8 passed; 0 failed; 0 ignored`. 구현 전 RED는 4개 회귀 실패 | `d2c2b269` 기록 시점 |
| B | 위 외 DB 회귀 3개 파일 | - | 컴파일만(`--no-run`): `apply_idem_principal_scope`, `deployment_fence_catalog_error`, `write_extension_read_conflict` | `afaac8e2` 기록 시점 |
| C | 독립 소비자 E2E | docker `postgres:17`, macOS | `PASS two consumer paths on 659e58440890…`, 결과 JSON `checks` 30개 모두 `ok` | `659e5844` |
| C | expr-apps | 로컬 PG17.11 | `합계: PASS 87 / FAIL 0 / SKIP 0` (이전 기준선 64) | `659e5844` |
| C | provenance 단위·RED | 무DB | RED `FAILED (failures=4)`, 원복 후 `Ran 17 tests … OK` | `d627c4d7` |
| C | 정의 변이 | 로컬 PG17.11 | teamdocs 6개 변이와 storefront 6개 변이를 시나리오가 잡음. `take`의 `from stock >= 0` 변이는 **잡지 못했고** 동등 변이로 판단(오류 코드만 다름) | `659e5844` |
| D | 무DB 표적 | 로컬 | `sdk-type-rejection` 1 passed, `unsettled-codes`+`bundle-result-ids` 10 passed, `python_sdk_contract_test` 5 OK, `operations.test.ts` 6 passed, `cargo test -p aip-cli --test service` 11 passed | `a514c941` 이후 재개 시점 |
| D | 예약어 RED→GREEN | 무DB | RED `grant` 기대 `"grant"` 실제 `grant`, GREEN `1 passed` | `33510442` 계열 |
| D | PG17 의미 시험(경로 일치, 정의→DB→SDK, 정의 매트릭스 43개) | 로컬 PG17.11 | 정의 매트릭스 3회 일치, 숨은 필드·재고·원자 생성 RED 확인 | `da8c338f` 기준 마지막 실행. **재개 뒤 로컬 재실행 안 함** |
| E | 진단 필드 | 무DB | `diagnostic_fields` `19 passed`, 이전·이후 `old fields compared: 125 differences: 0` | Track E 통합 `753be26f` |
| E | 읽기 확장 선언 | 무DB, 실제 Node·Python IPC | `read_extension_declared_reads` `2 passed` | `753be26f` |
| F1 | 단일 진입점 fast | 로컬 | `VERDICT: PROVEN`(구현 커밋 `5df0e878`의 clean tree, 7단계) | `5df0e878` |
| F1 | 구조 시험 RED→GREEN | 무DB | 수정 전 `FAILED (failures=27)`, 수정 후 `Ran 20 tests ... OK`. 진입점 시험 포함 후 29개 OK | F1 브랜치 시점 |

RED를 확인한 변이와 확인하지 못한 것은 각 Track 문서에 있다. A-05 시험 3개는 결함이 없는 경로의 회귀 시험이라 처음부터 통과하고, 코드를 잠깐 바꿔 시험이 판정함을 보였다.

### 2.2 GitHub Actions

위 "한눈에"의 CI 표가 전부다. 요약하면 다음과 같다.

- 통과: macOS Run 44985825145(`c86d6902`), SDK Run 45002420780(`b6af9d44`).
- 이전 후보 실패: SDK 45003272337(마이그레이션 열 조회 키), Verify 45002432400(`QUEUED` wire code), Verify 45006760676(전 식별자 인용으로 DDL digest와 Python JobState 검사 실패). 첫 두 원인은 `720b9b25`에서, 세 번째는 `5c19468e`의 예약어 101개 한정 인용·SQL 경계 교정에서 처리됐다. 안정화 SHA `d3acc393`의 세 CI 성공은 위 표에 별도로 적었다.
- 안정화 코드 SHA `d3acc39376cdd5277df8a7ea241cb3bfb23f05f6`의 Verify 45014410289, SDK semantic 45014412421, independent-consumer-rc 45014414753는 리드 확인 결과 모두 success다. Verify는 [45014410289](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45014410289)이다. 상세는 상단 CI 표와 Track F1 §8 참조.
- 문서 SHA `f7b0c225bb0405c77d174e621c3a6753049a07a6`의 Verify [45019169141](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45019169141)은 `head-fast`, `core`가 성공했지만, 같은 SHA의 RC [45019286972](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45019286972)는 `RUSTSEC-2026-0592`로 의존성 감사가 실패했다.
- `1eda239c28a8399d62f88360ebd654dbbeee6f53`은 `f7b0c225`의 직계 자식이며 Cargo.lock 9개만 바꿔 `url 2.5.9`와 resolver 동반 `form_urlencoded 1.2.3`을 반영했다. 전체 RC [45024949144](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45024949144)는 이 정확한 SHA에서 completed/success, 24 jobs 성공과 허용된 조건부 skip 1개를 리드가 확인했다.

SDK Run 45002420780이 실행하는 시험에는 D의 PG17 시험 5개(`sdk-semantic-e2e.yml`, `50571faf`)가 들어 있다. 따라서 이 success는 로컬에서 재실행하지 않은 D 의미 시험의 CI 근거가 된다. 단 그 기준은 `b6af9d44`이고 이후 B·마이그레이션 수정은 포함하지 않는다. 이 추론은 워크플로 파일 diff를 읽은 것이며 Run 로그는 확인하지 않았다.

### 2.3 미실행

- 이 문서 편집 세션에서는 제품 코드 테스트와 `verify-head.sh --stage heavy`를 실행하지 않았다. 전체 RC도 이 세션이 직접 dispatch하지 않았지만, 위에 기록한 RC Run 45024949144는 리드가 `1eda239c…`에서 성공을 확인해 전달했다.
- Portal: main `fd69229a`; 기준 Core `d3acc393`; drift baseline 일치 `75/75`, `pnpm verify`의 docs `92`, `check_plan_docs` 오류 `0`(리드 확인).
- Starter: main `0d292333`, core pin `d3acc393`; `verify-core --core main`과 pinned 모두 ALL PASS, `pnpm test` 16 pass(리드 확인). CI Run `45018181264`도 `0d292333`에서 success. 이번 세션에서 `gh -R winterholic/Application-Intent-Protocol-Starter run view 45018181264 --json databaseId,headSha,status,conclusion,workflowName,url`로 1회 조회해 completed/success와 head SHA를 확인했다.
- B의 정산 회귀 중 정산 파일 외 3개 DB 회귀는 이 세션에서 실행하지 않았고, 최종 CI 전달 내용에는 파일별 실행 결과가 없다.
- Linux 전용 단계(`bwrap`, AppArmor)는 로컬 Mac에서 실행하지 못했다. SDK Run 45002420780이 `bwrap` 문제의 해소를 보였다(리드 보고).
- `actionlint`, storefront·teamdocs의 Python SDK 시나리오, 비동기 job·외부 효과·파일·실시간은 이 보고서 작업에서 실행하지 않았다.
- Track D의 예약어 SQL 종단 실행은 이번 로컬 문서 작업에서 재실행하지 않았다. 최종 SDK semantic CI는 성공했지만 이 세션에서 job별 로그나 개별 예약어 시험 출력은 확인하지 않았다.
- 이번 보고서 작업에서는 제품 테스트를 실행하지 않았다.

---

## 3. 실사용 능력

기준은 Track C의 표현력 분류(`659e5844`)다. A 선언만, B 선언 가능하나 복잡, C 공식 확장 필요, D 여러 요청으로 나뉘어 원자성이 깨짐, E 구현 불가.

| 분류 | 항목(PG17 실행 근거가 있는 것) |
|---|---|
| A 선언만 | 낙관적 잠금 patch, 상태 전이와 역할 제한, 주문+줄 원자 생성(합계 제외), 가변 수량 재고 차감·증가, 재고 부족 시 전체 롤백, 결제 확정 콜백(금액 일치, 중복 방지), 결제 원장과 전이를 한 번에, 공유 권한, 필드 단위 읽기 제한, 관계 조회, count·Int sum, 멱등 재전송과 장애 후 복구 |
| B 선언 가능하나 복잡 | 다중 조직 소속(조직별 역할, `exists` 술어 반복), Decimal sum(`check`만 확인), 효과 값 산술(`check`만 확인), 확장 호출 SDK 타입 |
| C 공식 확장 필요 | 주문 합계 `Σ 수량×단가`의 서버 보장, 취소 시 줄 수량만큼 재고 반환, 확장에 여러 줄 입력 |
| E 구현 불가 | 관계 필드로 필터·정렬, `Σ 수량×단가` 집계, cursor와 nullable 정렬 |
| 해당 없음·미실행 | 비동기 job, 외부 효과, 파일, 실시간(지원 표 밖) |

세 앱 이상에서 반복된 후속 후보는 줄 합계 롤업, 줄 수량 기반 역반영, 확장 입력 배열, 확장 안 중첩 전이 거절 사유 전달 네 가지뿐이다. 무제한 표현식, 루프, 함수는 후보로 두지 않았다.

주의: "확장에 여러 줄 입력"(C-D2)은 이후 `a514c941`에서 Json 입력을 열었지만, storefront의 여러 줄 입력 PG17 여정 결과는 이 보고서 근거에서 확인하지 못했다. 최종 consumer job 성공을 그 개별 시나리오 결과로 확대하지 않는다.

소비자 앱 E2E에는 로컬 `659e5844` 결과와 안정화 Core SHA에서 리드가 확인한 `independent-consumer-rc` Run 45014414753 성공이 있다. 이 Run은 해당 workflow의 `consumers` job 결과이며 release-candidate 전체 실행이나 모든 실사용 분류의 재검증으로 해석하지 않는다.

---

## 4. 남은 중요 문제 (심각도순, 해결된 항목 반복 없음)

1. **공개 count의 정보 노출은 설계 결정 대기다.** PG17 반례에서 호출자가 읽을 수 있는 행은 2개지만 공개 count는 4를 반환했고, `where`에서 `input`을 쓰면 다른 테넌트 값 900을 단계적으로 복원할 수 있었다. 현재 조치는 경고이며 공개 의미는 바뀌지 않았다. 소유자 결정 5.1.
2. **Track D의 `sema.rs` todo 세 건이 남았다.** C1은 `budget cost`가 읽기 비용보다 낮아 런타임에서 항상 `WIRE.COST_EXCEEDED`가 나도 `check`가 통과한다. C2는 `to f = f + this.g`에서 항 필드 하한이 0 이하를 허용해 런타임 `WIRE.BAD_VALUE`가 날 수 있다. C3은 실행 가능한 공유 기본키 `id: Member`의 `sort id`를 `DEF.OP_TYPE_MISMATCH`로 거부하며 의도 확인이 필요하다. 재현은 `semantic-definition-matrix.test.mjs`의 `todo`; 이번 작업에서는 실행하지 않았다. C5(bundle 상한 경고)와 C6(Int 안전 정수 경계)도 결정·수정 대기다.
3. **`server_start_error`의 `NotFound` 문구가 원인을 오도한다.** bwrap이 없는 상황을 node/python3 실행 파일이 없는 것처럼 안내한 사례가 보고됐다. 오류 문구·원인 분류는 수정되지 않았다.
4. **독립 검증에는 비차단 확인 한계가 남는다.** worker `a552c924`는 두 교정 커밋에 단언 완화나 skip이 없고, 예약어 목록이 PG17 kwlist의 R·T 101개와 일치하며, digest `1c7684a2` 유지와 변이 RED 4종을 확인했다고 보고했다. 다만 전체 HTTP 경로와 서버 예약어 목록 대조를 이 보고서 작업에서 로컬로 다시 실행하지 않았다. 검증자는 구현자와 같은 Claude 계열이어서 모델 계열 독립성도 없다. 이를 최종 성공 CI와 혼동하지 않는다.
5. **남은 정보 추론·계약 결정이 있다.** K1 숨은 unique 값, K2 숨은 범위 필드 증감, K3 숨은 자식으로 인한 참조 오류, K4 잔여 경로는 각 Track에 기재된 현행 결과와 소유자 선택이 남았다. 개별 세부사항과 권고는 5.2절.
6. **낮은 우선순위의 검증·DX 공백이 남았다.** 전체 식별자 인용 변경의 DDL 재독자 전수 확인은 기록되지 않았다. `prototype/tests/read-decoder.test.mjs`, `cache-completion.test.mjs`, `spikes/spike-0-ts/test/*`는 현재 실패해 필수 경로에 연결하지 않은 상태이며 결함 여부는 별도 판단이 필요하다. 확장 안쪽 전이의 거절 사유는 `WIRE.EXTENSION_ERROR`로 뭉쳐 보이고, 단독 집계 기한은 고정 2초다.
7. **Track B-D2의 500ms budget은 근거 미확인이다.** 상수와 E6 측정 하네스는 있지만, 500ms를 선정한 측정값·결정 근거를 찾지 못했다. Track B §5와 결정 문서에 표시했다.
8. **추적 기록의 옛 SHA 표기는 일부 최종 저장소에서 찾을 수 없다.** 초안이 짝지은 후보는 커밋 메시지 기반 추정이므로, 최종 코드의 근거 SHA처럼 인용하지 않는다. 안정화 코드 기준은 `d3acc393`; 출시 후보와 전체 RC 검증 기준은 의존성 교정 후 `1eda239c`다.

### 확인 완료 (2026-11-22)

- K4 개별 PG17 검증: [Verify 45014410289](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45014410289), SHA `d3acc39376cdd5277df8a7ea241cb3bfb23f05f6`의 `core` 로그 8433–8435에서 외부 create의 `FORBIDDEN` 두 건과 잔여 경로 현행 계약 고정 한 건이 모두 통과했다. 이 개별 회귀 통과는 K4 잔여 지원 여부를 결정하지 않으며, 그 선택지는 §5.2에 남긴다.
- 출시 후보 전체 검증: 문서 SHA `f7b0c225bb0405c77d174e621c3a6753049a07a6`에서는 Verify [45019169141](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45019169141)의 `head-fast`·`core`가 성공했으나 RC [45019286972](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45019286972)는 `RUSTSEC-2026-0592` (`url 2.5.8`)로 실패했다. 직계 자식 `1eda239c28a8399d62f88360ebd654dbbeee6f53`에서 Cargo.lock 9개만 교정한 뒤 전체 RC [45024949144](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/45024949144)가 성공했다(24 jobs 성공, workflow 허용 조건부 skip 1개). 새 문서 커밋은 이 RC 검증 SHA가 아니다.

---

## 5. 소유자 결정

모두 확정된 것이 없다. 코드는 호환 유지형 가산 변경까지만 들어갔다. 선택지와 추천은 Track 결정 문서를 따랐고, 5.5의 선택지는 문서에 한 곳에 정리돼 있지 않아 근거 문서를 바탕으로 작성자가 묶었다.

### 5.1 공개 count 최종 의미 (D-A1, D-A2)

| | 1안 현행 유지, 공개 의도 표기 요구 | 2안 count에 `rows read` 기본 상속 | 3안 공개와 호출자 가시 집계를 표기로 구분 |
|---|---|---|---|
| 내용 | 응답 값 유지. `release count public` 같은 표기. 표기 없으면 지금은 경고, 나중에 오류 | `measure`가 count에도 가시 행 필터를 적용(sum과 같은 경로). 공개 총계는 표현 불가 | `release count visible`(상속)을 추가. 기존 `release count`는 공개 의미 유지 |
| 저장소 영향 | 경고 단계 0. 오류 단계는 14개 정의(9파일, 소비자 앱 3) 수정 필요 | 14개 정의의 응답 값이 바뀔 수 있음. 계약 지문은 그대로라 구 클라이언트가 조용히 다른 값을 받음 | 0 |
| 보안성 | 표기 강제 전까지 그대로 | 가장 높음 | 새 표기를 쓰는 정의만 높음. 기본이 공개라 모르고 쓰면 노출(경고가 알림) |
| 코드 범위 | `sema.rs` aggregate, 문법 카드, 문서 | `plan.rs:119 measure`, `plan_aggregate`, 문서 | `sema.rs`, `plan.rs`, 문법 카드, SDK 생성, 문서 |

- **추천: 3안을 1.0.x 가산 변경으로 도입**, 구현한 경고 두 종은 유지. 1안의 오류 단계와 2안의 기본 전환은 1.0에 넣지 않는다. 이유는 3안만 기존 정의와 응답 값 영향이 0이라서다.
- 따로 정할 것: ① 1.0에 `visible` 표기를 넣을지, ② 1.1에서 기본을 상속으로 바꾼다고 약속할지(지문 변경으로 구 클라이언트를 끊을지 포함).
- D-A2(`where`에 `input`이 있는 count의 거부 여부): (a)(b)는 새 거부라 `recruitment.aip`의 `approvedCount`류를 깨므로 **경고 유지(c)**를 추천한다. 거부는 위 결정 뒤에 다시 본다.

### 5.2 남은 정보 노출 패턴의 지원 여부 (D-A3~D-A7)

| 패턴 | 선택지 | 추천 |
|---|---|---|
| K1 `unique`에 숨은 필드 | (a) 정의 거부 (b) 현행 유지와 문서화 (c) 경고 | (c)를 다음 단계로. 권한 없는 호출자는 이미 `FORBIDDEN`이고 권한 있는 호출자의 `ALREADY_EXISTS`는 진단이라 코드 통일은 권하지 않음 |
| K2 숨은 범위 필드 증감 | (a) range 있는 숨은 필드의 `f ± 리터럴` 거부 (b) 허용과 문서화 | (b). 호출 횟수가 범위 크기에 비례하는 비용이고 값이 파괴됨. (a)는 숨은 카운터 패턴을 깸 |
| K3 숨은 자식 때문에 부모 삭제 실패 | (a) 현행 유지와 문서화 (b) 거부 사유를 일반 코드로 합침 | (a). 진단 훼손 없이 닫는 정의 단계 방법이 없음. (b)는 `REFERENCE_CONFLICT`에 의존하는 SDK 오류 처리를 깸 |
| K4 잔여(allow가 기본값 필드를 읽는 외부 create) | (a) 정의 오류로 거부 (b) 유일 위반을 `FORBIDDEN`으로 합침 (c) 현행 유지 | (a). 저장소 안 영향은 MySQL fixture 1개(`facereview-comments`)뿐. 저장소 밖 정의는 확인할 수 없어 CHANGELOG에 호환 변경으로 기록 |
| 테넌트 경고의 구조적 한계(D-A7) | 관계 경로 테넌트, `exists`, 근거 없는 정의 | 현행 유지. 근거 없이 추측해 경고하지 않는 것이 불변 조건 |

지원 범위에 대한 권고: 권한 있는 호출자의 진단(`ALREADY_EXISTS`, `REFERENCE_CONFLICT`, `BAD_VALUE`)은 유지하고, 권한 없는 호출자 경로는 판정 순서로 닫는다는 현재 방침을 1.0의 "지원하는 노출 한계"로 문서화한다.

### 5.3 bundle 잠금 전략 (B-D1)

구현된 것은 선택지 B′(호환 유지 선잠금)다. 소유자가 확인할 것은 "락을 쥐는 시점과 기간이 바뀐다"를 계약으로 받아들일지다. 이름 붙은 행을 묶음 끝까지 쥐므로 다른 호출자의 같은 행 갱신이 이전보다 일찍부터 기다리고, 앞 연산이 일찍 실패하는 묶음도 뒤 연산 대상 행의 락을 기다릴 수 있다. 응답 코드와 값은 바뀌지 않는다.

| 선택지 | 의미 변화 | 판단 |
|---|---|---|
| A. 지침만(LIMITS E9에 순서 맞추기) | 없음 | 채택(문서 반영 완료). 해당 경로는 교착 가능 |
| B′. `target.ids`와 compose 대상 선잠금 | 시간 특성만 | 구현됨. **추천: 계약으로 수용**. 교착 후 1초 지연이 사라지는 이득이 더 큼(격자 15칸에서 교착 0, p99 61ms 이하) |
| B″. `where` 대상도 시작 시 선잠금 | 있음(`where`는 앞 연산이 바꾼 상태를 보는 의미) | 반대 |
| C. 서버가 연산을 `(resource, id)`로 재정렬 | 있음(`from` 판정, `refs`, 실패 순서가 바뀜) | 반대 |
| D. `deadlock_timeout` 단축 | 없음 | 반대. 서버 설정 50ms에서도 8건 이상이면 성공률 25% 이하(`5% (4/80)` 등), 권한 필요 |
| E. 재시도 횟수·기한 상향 | 계약 값 변경 | 반대. 교착을 숨기고 지연만 늘림 |

`SATURATED_REPLAY_BUDGET`(500ms) 등 계약 값은 바꾸지 않았다(B-D2). bundle 잠금 격자는 확인했으나 500ms 선정 근거·해당 budget 측정 결과는 확인되지 않아 `근거 미확인`으로 수정했다(Track B §5, decisions/b.md, baseline B-07).

### 5.4 진단 JSON 장기 호환성 (D-E1, D-E2, D-E5, D-E6)

| 선택지 | 내용 | 장단점 |
|---|---|---|
| A. 최소 계약 | 종료 코드, 스트림, `ok`, `code`, `severity`, `line`·`column`(위치 없음은 0:0과 `located`), `message`의 존재와 타입만 보장. `message`, `help`, `scope`, `explain`, `scopePath`, `anchor`, `reason`은 변경 가능 | 문구 개선과 필드 추가가 자유로움. 편집기·에이전트가 `scopePath`, `anchor`에 의존하면 약속 밖 |
| B. 현재 출력의 모든 키 보장 | | 문구까지 고정돼 개선과 번역을 막음. 선택 필드 부재 케이스가 많아 비용 큼 |
| C. 약속 없음(현상) | | 비용 없음. 소비자가 안전한 필드를 모름 |

- **추천: A.** `anchor`, `scopePath`, `reason`은 새 필드라 쓰는 소비자가 생기면 A에 편입.
- D-E2(`schema` 버전 필드): A면 1.0 전에 `"schema":"aip-diagnostics/1"`을 세 출력 모양에 추가(문서의 추정 약 10줄). 닫힌 스키마로 검증하는 소비자는 깨지나 저장소 안에는 없다. 필드를 두는 순간 "major에서만 깨진다"는 약속이 되므로 D-E1과 함께 결정.
- D-E3(위치 없는 진단): 현행 유지(0:0과 `located:false`). 1:1로 바꾸는 안은 0:0을 위치 없음으로 읽던 소비자를 깨므로 권하지 않음.
- D-E5(비정의 실패 코드 62종): 잠정 유지, 후속으로 영역 접두 등록. D-E6(`BrokenPipe` 종료 0): 문서화만.
- `COMPATIBILITY.md` 진단 JSON 절 수정(G7)은 이 결정 뒤에 소유자 승인으로 진행한다.

### 5.5 확장 ABI 안정화 범위 (D-E7, D-E8, OI-X2, D-DEC-6)

현재 확장 ABI는 어떤 문서도 안정 계약으로 확정하지 않았다(`product/SUPPORT.md`, `e2-extension-access.md`: OI-X2). 아래 선택지는 근거 문서를 바탕으로 작성자가 묶은 것이다.

| 선택지 | 범위 | 장단점 |
|---|---|---|
| A. 1.0에서 ABI 전체를 비확정으로 유지 | 선언 문법 `access`, `ctx` 호출 형태, 응답 모양, 오류 전달 모두 변경 가능 | 부담 없음. 소비자(FaceReview 등)가 변경에 노출됨 |
| B. 권한 선언 규칙과 오류 계약만 "조건부 안정" | `access` 키 규칙(쓰기 확장의 `R.t`, `create.R`, `read.R`, 읽기 확장의 opt-in `read.R`), `WIRE.ACCESS_NOT_DECLARED`, `WIRE.EXTENSION_ERROR`/`EXTENSION_REJECTED` 의미. ctx 호출 형태와 응답 모양은 비확정 | 보안 관련 규칙만 고정. 안쪽 거절이 `EXTENSION_ERROR` 하나로만 보이는 한계(C-D4)를 계약에 넣게 됨 |
| C. ABI 전체 안정화 | | 불확실한 공개 API를 안정 계약에 편입하는 것이라 원 요청 0절 금지와 충돌 |

- **추천: A를 기본으로 하고, B의 선언 규칙만 별도 문서로 "현재 동작 기록"에 남긴다.** C는 하지 않는다.
- 관련 하위 결정: D-E7 읽기 확장 무선언 읽기는 **(a) opt-in을 유지**(구현됨). 필수화는 FaceReview 4건이 `WIRE.ACCESS_NOT_DECLARED`로 깨져 반대. D-E8 미사용 grant 경고는 실행 중 관측으로 낮은 우선순위. D-DEC-6 안쪽 거절 노출은 현행 유지(구분이 새면 존재 오라클).

### 5.6 그 밖에 D 문서에 남은 결정(참고)

D-DEC-1(`Int` 범위): 문서화하고 무범위 `Int`에 advisory를 검토, 장기는 큰 정수 타입. D-DEC-2(쓰기 확장 `/apply` 헤더 없음 코드): 메시지만 먼저 고치고 코드 정렬은 1.1. D-DEC-3: 1.0 한도로 문서화. D-DEC-4: 기존 호출을 깨는 TS 타입 엄격화는 하지 않음. D-DEC-7: 감사 `via` 구분은 1.1 검토.

---

## 6. 다음 작업

비용은 문서가 줄 수나 범위를 적은 것은 인용하고, 나머지는 작성자 추정의 상대 크기(소, 중, 대)다. 기대 효과는 근거 문서에 적힌 문제의 해소를 뜻하며 수치로 약속하지 않는다.

### 6.1 완료한 작업

1절의 항목 전부(A~F). 결정 문서와 반례 시험은 함께 들어갔다. 안정화 SHA의 Core CI 세 결과(Verify, SDK semantic, independent consumers), Portal·Starter Core 호환 확인, `1eda239c`의 전체 RC 성공, Track B/F1 기록 보완, 본 최종 보고서 추가도 완료했다.

### 6.2 제안 작업 (아직 하지 않음)

| 순서 | 작업 | 비용 | 기대 효과 |
|---|---|---|---|
| 1 | `sema.rs` 대기 3건 구현: C1 budget cost 검사, C2 this-field 산술 하한 검사, C3 id 정렬 | 중. 재현 시험이 이미 있음(`todo`). `COMPATIBILITY.md` 항목 필요(기존 정의가 새로 거부될 수 있음). C3는 의도 확인 선행 | check 통과인데 런타임이 항상 또는 값에 따라 거부되는 정의 2건 제거 |
| 2 | 서버 시작 오류 메시지가 bwrap 부재를 구분하게 수정 | 소(추정) | 이번처럼 CI 원인 진단이 오도되는 것을 방지 |
| 3 | 인용 변경 회귀 범위 점검: DDL 문자열을 다시 읽는 코드 전수 확인 | 소~중 | 같은 종류의 숨은 회귀 사전 차단 |
| 4 | 소유자 결정 5.1~5.5 확정 | 소유자 시간 | 이후 구현의 방향 고정 |
| 5 | 5.1 확정 후 count `visible` 표기 구현(3안) | 대. `sema.rs`, `plan.rs`, 문법 카드, SDK 생성, 문서 | 공개 총계와 호출자 가시 집계의 명시적 구분. 기존 정의 영향 0 |
| 6 | 5.4 확정 후 `schema` 필드 추가와 `COMPATIBILITY.md` 진단 JSON 절 수정 | 소(코드 약 10줄 + 문서) | 저장된 진단 JSON의 버전 식별, 호환 약속 명문화 |
| 7 | K1 경고 추가, K4 잔여 정의 거부 | 소(K1: `policy_advice.rs` 한 항), 중(K4: 호환 영향 기록 포함) | 권한 있는 호출자가 보는 숨은 값 존재 노출 감소 |
| 8 | 읽기 확장 `read.<R>`를 문법 카드와 공용 문서에 반영, FaceReview 정의에 선언 추가 | 소~중(문법 카드를 바꾸면 생성 문서 골든 갱신) | 읽기 확장이 무엇을 읽는지 정의에 드러남 |
| 9 | `aip --version --verbose`에 빌드 커밋 노출 | 소(Core 가산 변경) | 소비자 E2E의 "엔진 SHA" 확인이 간접 추정에서 직접 확인으로 |
| 10 | 줄 합계 롤업, 줄 단위 효과 등 반복 패턴 후보 설계 | 대(공개 의미 추가, 소유자 결정 선행) | 표현력 C 등급 항목을 확장 없이 선언으로 처리 |
| 11 | 큰 모듈 점진 분리(`sema.rs` 숨은 필드 검사, 정책 확장 한도, bundle·compose 실행, `serve_request` 라우팅) | 대 | 유지보수 비용 감소. 결함 수정과 겹칠 때만 진행하는 것을 권함 |

우선순위는 남은 C1·C2와 `server_start_error` 문구 수정 순이다. K4 개별 PG17 결과와 전체 RC 검증은 완료됐으며, K4 잔여 지원 여부 및 공개 의미·호환성 선택은 §5의 소유자 결정으로 남긴다.
