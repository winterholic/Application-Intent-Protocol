# Track F1 결과: CI 동일 SHA 검증과 단일 진입점 (2026-11-22)

기준: `origin/main` b292a880 위에서 시작, 검증 중 다른 Track 병합분(`b818613d` 독립 소비자 연결 등)에 rebase했다. 실행 환경은 이 Mac Studio(Darwin arm64, PostgreSQL 17.11 Homebrew, 포트 55426, `CARGO_TARGET_DIR=/tmp/aip-stab-f1-target`)와 GitHub Actions다. 태그, GitHub Release, npm, PyPI 게시는 하지 않았고 게시 단계가 있는 `release.yml`은 건드리지도 실행하지도 않았다.

## 1. 변경 요약

| 파일 | 목적 | 커밋 |
|---|---|---|
| `.github/workflows/*.yml` 10개 | main push의 경로 필터 제거, push 트리거 추가(F-03) | `5855da8f`(rebase 전 `92127557`) |
| `.github/workflows/*.yml` 16개 | push·dispatch·call의 concurrency를 SHA 단위로(다른 SHA 실행을 취소·대체하지 않음) | 같음 |
| `.github/workflows/release-candidate.yml` | `required`가 `scripts/release-candidate-required.py`로 판정 | 같음 |
| `scripts/release-candidate-required.py` | success만 통과, 선언한 workflow 전부 집계, 모든 job이 성공이고 후보 SHA | 같음 |
| `product/tests/test_release_candidate_required.py` | 위 스크립트, workflow 구조, 연결 목록, 진입점 시험(29개) | `5855da8f`, `37d7dad8`, `a522b6ac` |
| `.github/workflows/concurrency-contracts.yml`(신규) | 안정 probe 12개를 필수 경로로(B-06) | `37d7dad8` |
| `.github/workflows/concurrency-exploration.yml`(신규) | 탐색 probe 5개를 수동·주간으로 분리(B-06) | `37d7dad8` |
| `scripts/run-concurrency-probes.sh`(신규) | probe를 프로세스 하나씩 실행, 안정/탐색 목록의 단일 출처 | `37d7dad8` |
| `.github/workflows/macos-platform-tests.yml` | macOS 전용 v6 시험 5곳 추가(B-05) | `37d7dad8` |
| `tools/verify.sh` | 이름이 어디에도 없던 시험 연결(F-05), 새 시험 연결 | `5855da8f`, `37d7dad8` |
| `scripts/verify-head.sh`(신규) | 최신 HEAD 필수 검증 단일 진입점(F-02) | `37d7dad8` |
| `.github/workflows/verify.yml` | `head-fast` job: CI에서 진입점의 빠른 단계 실행 | `a522b6ac` |
| `plan-docs/stabilization-20261122/baseline.md` | F1·B-05·B-06 행과 5.4절 트리거 열 갱신 | 이 문서와 같은 커밋 |

## 2. 발견: 이 변경 전에는 SHA가 검증 없이 지나갔다

- 최근 Verify 200회 중 success 68, failure 63, **cancelled 67**(`gh run list -w Verify --limit 200`). cancelled 대부분이 main 연속 push에서 새 push가 앞선 SHA의 대기 run을 대체한 것이다. 예: `b292a880`, `b818613d`, `c47cc44f`, `3aa98afa`의 Verify가 모두 cancelled(2026-11-22 03:42~03:51Z). 그 SHA들은 Verify 판정이 없다.
- `4f44ca87`부터 `74ba5b6b`까지는 경로 필터 workflow도 안 돌았다.

## 3. F-03: workflow별 변경

"전"은 `b292a880`, "후"는 이 Track 병합 뒤다. PR 트리거의 경로 필터는 그대로 둔다(빠른 피드백용). main push만 바꿨다.

| workflow | main push 전 | main push 후 | concurrency |
|---|---|---|---|
| `verify.yml` | 모든 push | 모든 push | 그룹에 SHA, 취소 안 함 |
| `aip-release-gate.yml` | 경로 필터 | **모든 push** | 그룹에 SHA, PR만 취소 |
| `app-packages.yml` | 모든 push | 모든 push | 같음 |
| `brokers.yml` | 모든 push | 모든 push | 같음 |
| `cgroup.yml` | 모든 push | 모든 push | 같음 |
| `object-storage.yml` | 모든 push | 모든 push | 같음 |
| `database-contracts.yml` | 경로 필터 | **모든 push** | 같음 |
| `deploy-checks.yml` | 경로 필터 | **모든 push** | 같음 |
| `dependencies.yml` | 경로 필터 | **모든 push**(PR 경로·월요일 schedule 유지) | 같음 |
| `flaskbb-parity.yml` | 경로 필터 | **모든 push**(개별 브랜치 목록 유지) | 같음 |
| `flaskbb-post-order.yml` | 경로 필터 | **모든 push**(개별 브랜치 목록 유지) | 같음 |
| `macos-platform-tests.yml` | 경로 필터 | **모든 push** | 같음 |
| `worker-jobs-linux.yml` | 경로 필터 | **모든 push** | 같음 |
| `sdk-semantic-e2e.yml` | push 트리거 없음 | **`push: main` 추가** | 같음 |
| `semantic-conformance.yml` | push 트리거 없음 | **`push: main` 추가** | 같음 |
| `concurrency-contracts.yml`(신규) | - | 모든 push, 필수 | 같음 |
| `concurrency-exploration.yml`(신규) | - | 없음(수동·주간, 필수 아님) | SHA |
| `release-candidate.yml` | - | 없음(dispatch, `release/**`, `v*`) | 그룹이 SHA |
| `independent-consumer-rc.yml` | 브랜치 push만 | **변경 안 함**(Track C 소유). 지금은 push 트리거 자체가 없고 RC·dispatch에서만 돈다 | - |

그룹 식: `<이름>-${{ github.workflow }}-${{ github.event_name == 'pull_request' && github.ref || github.sha }}`, `cancel-in-progress: ${{ github.event_name == 'pull_request' }}`. PR은 전처럼 같은 ref의 이전 실행을 취소하고, push·dispatch·call은 SHA마다 독립이라 앞선 SHA의 판정이 사라지지 않는다. `github.workflow`를 넣어 같은 SHA의 직접 실행과 RC가 부른 실행이 서로를 기다리지 않게 했다.

비용: main push 하나가 필수 workflow 16개를 모두 돌린다(macOS 포함). 개인 저장소 단일 main이라 수용했다. 줄이고 싶으면 경로 필터를 되살리는 대신 `release/**`에서만 전체를 돌리는 선택이 있고, 그건 F-03의 취지(최신 HEAD 검증)와 충돌한다.

**RED→GREEN**: 시험을 먼저 쓰고 workflow를 고치기 전에 돌렸다.

```
$ python3 -m unittest product.tests.test_release_candidate_required      # workflow 수정 전
Ran 20 tests ... FAILED (failures=27)
  FAIL ...runs_on_every_main_push_without_a_path_filter  (file=aip-release-gate.yml, database-contracts.yml, deploy-checks.yml, dependencies.yml,
       flaskbb-parity.yml, flaskbb-post-order.yml, macos-platform-tests.yml, worker-jobs-linux.yml, sdk-semantic-e2e.yml, semantic-conformance.yml)
  FAIL ...concurrency_never_cancels_another_sha  (16개 workflow + release-candidate.yml)
  FAIL ...required_uses_the_gate_script_with_the_run_sha_and_the_api_job_list
$ python3 -m unittest product.tests.test_release_candidate_required      # 수정 후
Ran 20 tests ... OK
```

(처음 시험은 `re.M` 누락으로 경로 필터 10개 중 8개를 못 잡았다. 그 결함을 고친 뒤의 27건이 위 값이다. 출력 원본 `red-workflow-structure.txt`는 /tmp/aip-f1-evidence에 있고 저장소에는 넣지 않았다.)

## 4. 집계 게이트: 같은 SHA와 skipped·cancelled

`required` job은 인라인 jq 대신 `scripts/release-candidate-required.py`를 쓴다. 판정:

1. `needs` 결과가 전부 정확히 `success`다. skipped, cancelled, failure, 없음은 실패.
2. `release-candidate.yml`이 선언한 job 전부가 `needs`에 있고, 선언 안 한 need는 없다. 새 job을 선언만 하고 집계에서 빼면 실패한다.
3. `GET /repos/{repo}/actions/runs/{run}/attempts/{n}/jobs`로 이 실행의 모든 하위 job을 받아, 모든 job이 `success`이고 `head_sha`가 `GITHUB_SHA`와 같다. 호출된 workflow 안에서 skipped인 job(호출 결과는 success)도 잡는다. 허용은 `worker-jobs-linux / crypto-compile-repair`(그 workflow의 `crypto_repair_only` 전용 job) 하나뿐이다.
4. 선언한 workflow마다 job이 하나 이상 관측돼야 한다.

구조 시험이 추가로 고정하는 것: RC가 부르는 workflow 어디에도 게시·태그 단계가 없다(`npm publish`, `twine upload`, `gh release create`, `pypi-publish`, `git push`, `git tag`, `cargo publish`, `id-token: write`), `release.yml`은 호출 안 한다, 체크아웃 `ref`는 후보 SHA뿐이다(독립 소비자는 `TARGET_SHA`가 `inputs.core_sha || github.sha`이고 RC가 `core_sha`를 안 넘기는 경우만).

**RED→GREEN**:

```
# 스크립트에 skipped·cancelled를 통과로 보고 SHA 비교를 끈 결함을 심음
$ python3 -m unittest product.tests.test_release_candidate_required.GateScriptTest
FAIL: test_a_cancelled_workflow_is_not_a_pass
FAIL: test_a_job_that_ran_for_another_sha_fails_even_when_its_workflow_is_green
FAIL: test_a_skipped_workflow_is_not_a_pass
Ran 12 tests ... FAILED (failures=3)
# 원래 코드로 복구
$ python3 -m unittest product.tests.test_release_candidate_required
Ran 20 tests ... OK
```

기존 lint gate 시험 `product/tests/test_rust_lint_gate.py`(verify.sh 구조)도 같이 돌렸다: `Ran 13 tests ... OK`. 새 시험은 `tools/verify.sh`의 unittest 줄에 연결해 Verify가 매번 돌린다.

## 5. F-02: 단일 진입점 `scripts/verify-head.sh`

```
scripts/verify-head.sh [--stage fast|heavy|all] [--dry-run] [--only <step>] [--out <새 디렉터리>] [--allow-dirty] [--pg-port N] [--with-release-gate] [--list]
```

| 단계 | step | 필요한 것 |
|---|---|---|
| fast | `scripts-syntax`, `workflow-structure`(위 시험 + lint gate 시험), `tooling-tests`, `versions`, `rust-format`, `sema-quick` | cargo, node, python3. PG 불필요 |
| heavy | `core-verify`(`tools/verify.sh`), `expr-apps`, `stable-probes`, `worker-isolation`, macOS면 `macos-only-v6`, `semantic-conformance`, `sdk-semantic-e2e`, 선택 `release-gate` | PostgreSQL 17(없으면 임시로 기동), mypy 또는 uv, tsc |

- 실행 SHA: `git rev-parse HEAD`를 `summary.txt`와 `result.json`에 적는다. `origin/main`과의 관계(같음, 앞섬, 뒤처짐, 갈라짐)도 적는다. CI에서 `GITHUB_SHA`와 HEAD가 다르면 종료 2.
- 더러운 작업 트리에서 heavy는 거부(종료 2). `--allow-dirty`를 주면 `dirty=true`로 기록하고 판정이 `PASSED ON A DIRTY TREE`가 되어 증명으로 쓰지 않는다.
- 판정은 모든 step이 PASS일 때만 `PROVEN`. 실행하지 않은 step은 `NOT RUN`이고 통과가 아니다.
- `--dry-run`: 계획과 SHA만 출력하고 아무것도 실행하지 않는다. `result.json`에 `proven:false`. 진입점 어디에도 태그·게시 명령이 없고 시험이 이를 확인한다. `--with-release-gate`의 `release-gate.sh`는 임시 디렉터리에서 빌드·smoke만 하고 올리지 않는다.
- PG17: `AIP_TEST_DATABASE_URL`·`AIP_TEST_ADMIN_URL`이 있으면 그대로 쓰고(`server_version_num`이 17이 아니면 `NOT RUN`), 없으면 `pg_ctl`로 `--out/pgdata`에 포트 `--pg-port`(기본 55432)로 띄웠다 끝에 내린다. PG* 변수는 비운다.
- CI: Verify의 `head-fast` job이 `bash scripts/verify-head.sh --stage fast`를 돌리고 `verify-head-fast-<sha>` artifact에 기록을 올린다. heavy는 CI에서 `core`, 필수 workflow들이 같은 명령을 각자 돌리므로 진입점은 로컬 재현용이다(둘이 갈라지지 않게 `--list`의 파일 경로가 존재하는지 시험이 확인한다).

**로컬 실행 증거**(클린 트리, 이 Mac):

```
$ bash scripts/verify-head.sh --stage fast --out /tmp/aip-f1-fast1
verify-head: sha=653c23458a45a03d3990160b104c22e597d44ade dirty=0 relation="diverged from origin/main" stage=fast dry_run=0
PASS  [fast] workflow-structure (8s)  ...   PASS  [fast] rust-format (2s)  ...   PASS  [fast] sema-quick (3s)
VERDICT: PROVEN for 653c23458a45a03d3990160b104c22e597d44ade (fast; 6 steps passed)
$ bash scripts/verify-head.sh --stage all --dry-run      -> exit 0, VERDICT: NOT RUN, 로그 0개
$ (더러운 트리) bash scripts/verify-head.sh --stage heavy  -> "working tree is dirty ...", exit 2
```

시험(`EntryPointTest`, 6개)과 RED: 실패한 step을 PASS로 보고하게 바꾸면 `test_a_failing_step_fails_the_run_and_is_not_proven`이 실패했다(`Ran 6 tests ... FAILED (failures=1)`), 복구 후 `Ran 29 tests ... OK`.

heavy 단계 전체(`tools/verify.sh` 등)는 이번에 로컬에서 돌리지 않았다(`미실행`: 수십 분, mypy/tsc 준비 필요). 대신 heavy의 구성 요소 중 B-05·B-06 대상은 로컬에서 직접 돌렸다(6절).

## 6. B-05, B-06, F-05: 연결 목록과 제외 사유

### 연결

| 대상 | 연결 위치 | 로컬 증거 |
|---|---|---|
| `spikes/spike-v6-transport/tests/extension_lifetime.rs`, `extension_unix.rs`, `write_extension_lifetime.rs`, `write_session_outcome.rs`, `durable.rs::messages_share_atomic_job_effects_and_reject_foreign_acknowledgements`(macOS 전용 5곳) | `macos-platform-tests.yml` 새 step | macOS에서 5곳 모두 `ok`(`extension_timeout_and_server_shutdown_reap_real_worker_children`, `mac_net_deny_blocks_a_live_unix_socket_for_node_and_python`, `write_slots_deadlines_and_shutdown_release_children_and_uncommitted_effects`, `committed_write_is_reported_even_when_its_admitting_session_expires_during_execution`, `messages_share_atomic_job_effects...`) |
| `concurrency_probe_http.rs` 안정 12개: `f1`, `i1`~`i5`, `n1`, `e1`~`e5` | `concurrency-contracts.yml`(필수, RC 포함) → `scripts/run-concurrency-probes.sh stable` | 12개 모두 `rc=0`, 합계 약 20초(`/tmp/aip-f1-probes.log`: 예 `e1 ... 3s test result: ok. 1 passed`) |
| `spikes/spike-v6-transport/client/uuid.test.ts`, `extension-rejection.test.ts` | `tools/verify.sh` node 단계 | `ℹ tests 3 ℹ pass 3 ℹ fail 0` |
| `tools/test_*.py`(55개), `scripts/test_finish_storage_evidence.py`(7개) | `tools/verify.sh` unittest 단계 | `Ran 55 tests ... OK`, `Ran 7 tests ... OK`(`-W error::ResourceWarning`) |

macOS 전용 시험이 앞으로 빠지는 일은 구조 시험이 막는다: 파일 머리가 `#![cfg(target_os = "macos")]`인 시험 파일(과 `extension_lifetime`, `durable`)은 macOS workflow에 `--test <파일>`이 있어야 한다.

### 분리(필수 아님)

| 대상 | 위치 | 이유 |
|---|---|---|
| `a1_opposite_order_two_product_bundles` | `concurrency-exploration.yml`(수동, 주간 일요일 04:41Z) | 열린 교착(LIMITS E9)을 일부러 재현하고 라운드 수를 키우면 오래 걸린다. 결과가 합격이 아니라 측정값이다 |
| `a2`, `a3`, `a4` | 같음 | 영구 회귀 시험(`bundle_lock_order_*.rs`, `expr_bundle_lock_order.rs`)이 이미 필수 경로에 있다. probe는 반복·규모 탐색용 |
| `a5` | 같음 | 쓰기 롤백 회귀가 다른 시험으로 보호된다. 탐색 목록에 둠 |

probe 함수는 17개다(기준선 6.1의 "18개"는 오기. 시험이 `#[ignore]` 함수 수를 세어 17로 고정했고, 안정+탐색 목록이 그 17개와 정확히 일치하는지 확인한다).

### 연결하지 않은 것

| 대상 | 이유 |
|---|---|
| `prototype/tests/read-decoder.test.mjs`, `cache-completion.test.mjs` | 연결하지 않음: 레거시 prototype이고 **현재 실패**(`read-decoder` 1 fail "읽기 값이 생성 계약과 다름", `cache-completion` 4건 ✖ "WRITE completion cannot re-cache an earlier read"). 시험을 skip하거나 완화하지 않았다. 결함 여부는 prototype 소유자 판단. 아래 9절에 보고 |
| `prototype/tests/{extension-sdk,http-recovery,write-extension-sdk}.test.mjs` | 통과(9, 12, 8)하지만 prototype 소비자가 없는 레거시라 필수에는 안 넣음 |
| `prototype/tests/test_bundle.py` | 실행 안 해봄(레거시) |
| `spikes/spike-0-ts/test/*.test.ts` | 현재 실패(PoC, 전부 ✖). 이 패키지는 CI에서 `tsc` 제공용이다 |
| `crates/aip-execution/tests/encryption_rekey.rs:530`, `crates/aip-migrate/tests/large_backfill.rs`, GCS 실자격 시험, `slice13b_host.rs` | 의도된 ignore(알려진 공백, 수동 측정, 자격 증명, 범위 밖) |
| `porting/flaskbb/tests/report-bulk.test.mjs`, `test_last_post_source_oracle.py`, `product/tests/fixtures/facereview-mysql/reference/test_writes_runner_config.py` | FlaskBB·FaceReview(MySQL), 필수 아님 |
| `AIP_TEST_BWRAP_DIAGNOSTICS=1` 진단 시험, `FACEREVIEW_SOURCE_PATH` oracle | 진단 전용, 외부 원본 필요 |

## 7. F-04, F-06, F-08

**F-04 (verify.sh 이중 실행)**: 의도된 중복이라 유지했다. 근거: `release-gate.sh`는 `git clone --no-hardlinks`한 `--ref` 커밋에서 `npm ci`부터 다시 하는 증거 실행이고(`release-gate.sh` 머리 주석 "It never reads the working tree"), `release.yml`의 게시 조건은 같은 SHA의 `aip-release-gate` 성공만 본다(기준선 2.7절). 게이트에서 `full-regression`을 빼면 그 증거가 약해진다. RC에서 Verify와 게이트는 병렬 job이라 벽시계 시간은 늘지 않고 runner 시간만 쓴다. 줄인 것: 없음. 줄이려면 `release.yml`이 RC `required` 성공을 요구하도록 먼저 바꿔야 하며, 게시 workflow라 이번 범위에서 건드리지 않았다(9절 권고).

**F-06 (Verify 간헐 실패)**: 기준선 2.1절 P0-4의 귀속이 틀렸다.
- 기록된 실패 3건(`bfa12e50`, `f0b1b31b`, `cb37868b`, Run `44902640861`, `44903513877`, `44904438206`)의 실패 step은 모두 "rust engine clippy: spike-v6-transport"(`Verification failed: ... (exit 101)`)이고 원인은 `tests/concurrency_probe_http.rs:616 error: manual RangeInclusive::contains implementation`(`-D clippy::manual-range-contains`)다. 결정적 실패이며 간헐이 아니다. `596ab5dd`가 고쳤다.
- 기준선이 지목한 `isolation.rs:1296` panic(`leader ownership observation missing`)은 같은 로그에서 Verify step "probe leader ownership and early-wait mutation control"이 **일부러** 일으키는 실패(mutation을 주고 실패와 `grep`을 기대)의 출력이다. 실패 원인이 아니다.
- 감사 P0-4의 다른 실패(Run `44117223558`, `723fb474`, 2026-11-17): 실패 step은 "Verify real HTTP to Node and Python workers to MySQL"이고 로그에 `FaceReview suites: 10 passed; 1 failed/blocked`와 `server: {"code":"WIRE.DB_UNAVAILABLE","event":"mysql_job_claim_failed"}`가 있다. 원인 후보: MySQL 잡 claim이 DB_UNAVAILABLE을 낸 시점의 브라우저 journey 시간 초과. **확인 필요**(해당 run은 이후 재현되지 않았고 MySQL은 1.0 범위 밖이라 더 파지 않았다).
- 같은 기간 Verify 200회 중 cancelled가 67회라서 "간헐 실패"로 보이던 것의 상당수는 cancelled(2절)다. SHA 단위 concurrency로 줄인다.

**F-08 (`continue-on-error` 14개)**: 가림 없음. 14개 step(`fixture-tools`, `setup-upstream-python`, `upstream-venv`, `upstream-installer`, `upstream-requirements`, `upstream-install`, `upstream-import`, `oracle-schema`, `oracle-regression`, `negative-date`, `negative-topic-visibility`, `oracle-schema-cleanup`, `topic-move-http`, `unhide-http`)의 outcome이 모두 마지막 step "Preserve ORM and fixture failures as a nonzero workflow result"(`if: always()`)의 `keys`에 있고, 하나라도 `success`가 아니면 `sys.exit(1)`이다. `record_command.py`는 종료 코드를 전달한다(`return 0 if matched else exit_code`). `continue-on-error`는 이 workflow에만 있다. 다른 workflow의 `set +e`(cgroup, flaskbb-post-order, object-storage)와 `|| true`(deploy-checks 정리 한 줄, release.yml 로그 출력)도 읽었고 상태를 모아 끝에서 `exit`하거나(`cgroup`, `finish-storage-evidence.py`) 판정과 무관하다. 고칠 것은 없었고, 새 `continue-on-error` step이 `id`나 최종 재확인 없이 추가되면 `test_every_continue_on_error_step_is_aggregated_into_a_failing_final_step`이 실패한다.

## 8. CI 결과

(아래 9절의 표를 push 후 채웠다.)

## 9. 처리 못한 항목, 요청, 권고

CI 실행 결과는 이 절 아래 "8. CI 결과"에 둔다.

## 10. 2026-11-22 후속: provenance fast 경로와 정책 거부 계약

기존 F1의 워크플로·판정기·단일 진입점 변경은 현재 `origin/main`에 포함되어 있었다. 시작 SHA `c162837e`는 최신 main `0e3f0c80bc3b24a3b35b31e0655048fc13c30365`의 조상이었다. 기존 문서의 전체 증거는 유지하고 이 절과 baseline의 F1 후속 메모만 덧붙였다.

### Provenance 검사

- `verification/1.0-consumers/test_provenance.py`의 17개 DB 없는 단위 검사를 `scripts/verify-head.sh --stage fast`에 `provenance-unit`으로 추가했다. SHA, dirty tree, 엔진 빌드 출처·바이너리·버전, SDK 이름·버전·내용의 일치와 그 거부 계약을 테스트한다.
- 독립 소비자 workflow는 이미 같은 테스트를 실행한다. `release-candidate.yml`이 Verify 재사용 호출에 `provenance_unit_runs_in_independent_consumers: true`를 주면 fast 진입점만 해당 step을 생략하고, 필수 독립 소비자 job은 그대로 1회 실행한다. 로컬 및 다른 Verify 호출의 기본 fast 계획에는 검사가 포함된다. 독립 소비자 workflow 자체는 수정하지 않았다.

### 정책 회귀

- `policy_hardening`과 `advisories`는 기존 `sema-quick` fast step에서 이미 실행되므로 연결을 추가하지 않았다.
- `product/tests/policy-residual-paths.test.mjs`에는 보장된 거부 계약 두 건과 별도의 owner-decision 잔여 probe가 함께 있다. `tools/verify.sh`의 빌드된 CLI·PostgreSQL 경로에 앞의 두 테스트만 Node test-name filter로 연결했다. 잔여 probe는 연결하지 않았고 실행하지 않았다. 기존 요구 게이트를 제거하거나 약화하지 않았다.

### 실행 근거

```text
$ python3 -m unittest product.tests.test_release_candidate_required.EntryPointTest.test_provenance_contract_runs_once_in_the_fast_path
FAIL: ... verification/1.0-consumers/test_provenance.py not found in fast plan
$ python3 -m unittest product.tests.test_release_candidate_required.EntryPointTest product.tests.test_rust_lint_gate.RustLintGateTest.test_success_runs_both_sdk_packages_with_the_current_shared_target_binary product.tests.test_rust_lint_gate.RustLintGateTest.test_node_step_runs_the_external_create_order_regression product.tests.test_rust_lint_gate.RustLintGateTest.test_node_step_runs_the_upgrade_drill_unit_tests
Ran 10 tests ... OK
$ python3 verification/1.0-consumers/test_provenance.py
Ran 17 tests ... OK
```

```text
$ bash scripts/verify-head.sh --stage fast --out /private/tmp/aip-f1-fast-final-check
PASS [fast] scripts-syntax; workflow-structure; tooling-tests; versions; rust-format; provenance-unit; sema-quick
VERDICT: PASSED ON A DIRTY TREE for 0e3f0c80bc3b24a3b35b31e0655048fc13c30365
```

이 실행은 전 단계 통과했지만 문서·코드 변경이 있는 dirty tree여서 SHA 증명으로 취급하지 않는다. PostgreSQL을 시작하지 않았다. 정책 거부 테스트는 `tools/verify.sh`에 연결만 했고 이 실행에서 수행하지 않았다.

현재 main SHA의 GitHub Actions 조회(2026-11-22 04:28 UTC): `gh run list -R winterholic/Application-Intent-Protocol --commit 0e3f0c80bc3b24a3b35b31e0655048fc13c30365 --limit 100` 결과 16건 — success 6, failure 1(`Object storage`), in progress 1(`SDK semantic end-to-end`), queued 8. 이것은 변경 전 현재 SHA의 상태이며 새 최종 SHA의 CI 증거가 아니다.

남은 검증: 변경을 커밋한 최종 SHA에서 fast 단계를 clean tree로 다시 실행하고, GitHub Actions의 전체 최종 workflow는 리드 후속으로 확인한다. 이 문서 작성 시점에는 final SHA를 push하지 않았다.
