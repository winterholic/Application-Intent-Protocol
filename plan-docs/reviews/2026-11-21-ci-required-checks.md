# 필수 CI 재실행, 의존성 점검, worker 격리 검토 (2026-11-21)

범위: 감사 P0-3, P1-1, P1-6, P1-10, P1-13과 worker 격리 검토. 대상 SHA는 main `723fb474ca7891180c87edfb08ed7a689e1b2001`(이 문서 작성 시점의 origin/main). 실행 결과의 run 링크와 판정은 PR 본문과 §1 표를 따른다.

## 1. main 723fb474 workflow_dispatch 결과

판정 기준: 성공 = run conclusion success, 실패 = failure(원인은 로그로 확정), 환경 차단 = 러너·권한·서비스 오류로 시험이 시작되지 못함. 이번 실행에서 환경 차단은 없었다.

| workflow | ref | run | 판정 | 메모 |
|---|---|---|---|---|
| Database contracts | main | [44763951372](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44763951372) | 성공 | postgres-runtime-role, mysql-and-mariadb-fences |
| Linux worker jobs | main | [44763956990](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44763956990) | 성공 | `isolation::tests` 21 passed (§5) |
| Deployment checks | main | [44763964359](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44763964359) | 성공 | |
| Dependencies | main | [44763971217](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44763971217) | **실패** | chrono, chrono-tz unmaintained 권고(§4) |
| Dependencies (수정 후) | 브랜치 | [44766846751](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44766846751) | 성공 | `Dependency check passed`, `No known vulnerabilities found`(pip-audit). main에서는 이 PR 병합 뒤 재실행 필요 |
| AIP 1.0 release gate | main | [44763977523](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44763977523) | 성공 | `ALL STEPS PASSED for 723fb474ca7891180c87edfb08ed7a689e1b2001` |
| FlaskBB remaining contracts | 브랜치 `ci/1.0-required-checks-20261121` | [44764251211](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44764251211) | 성공 | main에는 `workflow_dispatch`가 없어 이 PR에서 추가했고, 추가한 브랜치 ref에서 실행했다. 브랜치 트리는 main 723fb474에 CI·문서 변경만 얹은 것이다 |
| FlaskBB PostOrder | 브랜치 `ci/1.0-required-checks-20261121` | [44764257315](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44764257315) | 성공 | 같은 이유. joserfc pin 변경 뒤 `eeac61e8`에서 [44766850008](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44766850008)로 다시 실행해 성공 |

main에서 직접 돌리지 못한 두 개는 이 PR이 병합된 뒤 main에서 한 번 더 dispatch해야 "main에서" 확인된 것이 된다.

## 2. 필수 검사가 경로 필터와 무관하게 도는 구조 (P0-3)

원인: 경로 필터 workflow는 해당 경로를 건드리지 않은 push에서 돌지 않는다(`a0fa2729`, `723fb474`). PR #11 병합 커밋 `d327b264`는 push 실행이 0건이었다(원인은 GitHub 쪽이며 이 저장소에서 재현하지 못했다. `확인 필요`).

변경:

- 14개 workflow에 `workflow_call`을 추가했다(`verify`, `aip-release-gate`, `database-contracts`, `worker-jobs-linux`, `cgroup`, `flaskbb-parity`, `flaskbb-post-order`, `deploy-checks`, `dependencies`, `app-packages`, `brokers`, `object-storage`, `sdk-semantic-e2e`, `semantic-conformance`). 호출된 workflow에는 경로 필터가 적용되지 않는다. `flaskbb-*` 두 개에는 `workflow_dispatch`도 추가했다.
- `.github/workflows/release-candidate.yml`: `release/**` 브랜치 push, `v*` 태그 push, 수동 실행에서 위 14개를 모두 호출한다. 마지막 `required` job은 14개의 결과가 전부 `success`일 때만 통과한다. 건너뜀·취소는 실패다. 로그에 SHA와 workflow별 결과를 남긴다.
- 기존 push/PR 트리거는 건드리지 않았다. 일상 PR에서 경로 필터로 CI 비용을 줄이는 동작은 그대로다.

한계: 새 workflow 파일은 기본 브랜치에 병합되기 전에는 `gh workflow run`으로 실행할 수 없다(`HTTP 404: workflow release-candidate.yml not found on the default branch`). 병합 전 확인은 `release/**` 브랜치 push로 했다(§1 아래 RC 실행). 병합 후에는 `gh workflow run release-candidate.yml --ref <SHA의 브랜치>`로 쓴다. 태그를 만들 때는 같은 SHA에서 `required` job이 성공했는지 확인한다([CONTRIBUTING](../../CONTRIBUTING.md#릴리스)). `workflow_call` 대상 중 `Verify`는 MySQL FaceReview 단계를 포함하므로, 그 단계가 간헐 실패하면 RC도 실패한다(감사 P0-4, 다른 작업자 담당). 이를 숨기려고 단계를 빼지 않았다.

## 3. P1-1: 엔진 목록과 spike-v7-migrate

- `tools/verify.sh`의 clippy와 test 두 루프에 `spike-v11-dev-checks`를 추가했다. 근거: `crates/aip-service/Cargo.toml`이 이 crate를 path 의존으로 두고 `load` 경로에서 호출한다.
- `spike-v7-migrate`는 목록 밖으로 격리한다. 근거: `crates/`, `prototype/`, `product/tools/`, `scripts/` 어디에도 v7 참조가 없고(`grep -rn "spike-v7"` 결과 없음), 제품 migration은 `crates/aip-migrate`의 별개 구현이다. 시험이 `spike_v2_read::connect`로 DB가 필요한 독립 실험이다. lockfile·advisory 점검은 `tools/dependency-check.sh`가 계속 맡는다. 결정은 `product/VALIDATION.md`와 `tools/verify.sh` 주석에도 적었다. 제품 경로에 연결되면 목록에 넣는다.

## 4. P1-13: 의존성 권고

dispatch `44763971217`(main 723fb474) 로그로 확정한 실패 원인:

| 권고 | 대상 | 내용 | 처리 |
|---|---|---|---|
| RUSTSEC-2026-0543 | chrono 0.4.45 | unmaintained(정보성), 모든 버전 해당, 수정 버전 없음. advisory-db 커밋 2026-11-17T10:14Z에 번호 부여. 마지막 성공 run(`6cfafe41`, 11-17)과 lock은 같고 DB만 바뀌었다 | **예외**: `deny.toml`에 사유·재검토 조건과 함께 `ignore` 추가. `cargo audit`에도 같은 id 전달 |
| RUSTSEC-2026-0555 | chrono-tz 0.10.4 | 같음 | 위와 같음 |
| yanked | async-executor 1.14.0 (root, spike-v6 lock) | `cargo audit` 경고 | **수정**: 두 lock을 1.14.1로 갱신(`cargo update -p async-executor`) |
| CVE-2026-119284 | joserfc 1.7.5 (`porting/flaskbb/ui/requirements.txt`) | `pip-audit` 신규 점검에서 발견. 1.7.6에서 수정 | **수정**: 1.7.6으로 올림 |

예외 근거: chrono·chrono-tz는 취약점이 아니라 유지보수 중단 표시이고 대체 버전이 없다. chrono는 `tokio-postgres`(`with-chrono-0_4`), `aip-runtime`(RFC 3339 파싱, 토큰 만료 시계), `spike-v2-read`, `spike-v6-transport`에 들어 있다. jiff나 time으로 바꾸면 여러 엔진의 시각·시간대 처리를 건드리므로 1.0 의존성 갱신이 아니라 1.1 작업으로 둔다. 재검토: 권고 철회, `tokio-postgres`의 후계 도입, 또는 2027-03-01 중 이른 쪽.

점검 범위 보강(`tools/dependency-check.sh`, `dependencies.yml`):

- `npm audit`에 `spikes/spike-v6-transport/tests/socketio-client`(lockfile이 하나 더 있었다)를 추가했다. 결과 0 vulnerabilities.
- `pip-audit`을 `porting/flaskbb/ui/requirements.txt`에 추가했다. 이 파일이 바뀌면 `Dependencies`가 돈다.
- 점검하지 않는 것(의도): `porting/flaskbb/upstream/requirements*.txt`는 upstream FlaskBB 원본의 오래된 pin(werkzeug 0.16.0, urllib3 1.25.6 등, `pip-audit` 로컬 실행에서 다수 권고)이고, parity 기준선을 그 버전에 맞춰 기록했으며 CI 시험용 venv에서만 설치하고 배포하지 않는다. `product/python`은 `dependencies = []`다. upstream pin을 올리면 오라클이 바뀌어 parity 기준이 무너지므로 올리지 않는다.

로컬 재현(변경 전후): 변경 전 같은 lock에서 CI가 `advisories FAILED`, 변경 후 `bash tools/dependency-check.sh` → `Dependency check passed.` joserfc pin을 되돌리면 `Found 1 known vulnerability in 1 package`로 실패하는 것도 확인했다.

## 5. Worker 격리 검토 (P1-10, `spikes/spike-v4-worker/src/{isolation,reap}.rs`)

판정: **코드와 실행 결과에서 수정이 필요한 결함을 확인하지 못했다.** 따라서 코드를 바꾸지 않았고 회귀 시험도 추가하지 않았다. `isolation.rs`는 마지막 성공 SHA `5ad33628` 이후 변경이 없다(`git diff 5ad3362 723fb47 -- spikes/spike-v4-worker`는 `write.rs`와 시험만).

실행 증거: Linux worker jobs `44763956990`의 `Prove Linux worker sandbox boundaries with Bubblewrap`(`AIP_TEST_BWRAP=1 cargo test ... isolation::tests`)가 `21 passed; 0 failed`. 포함: `runtime_probe_deadline_kills_a_hung_runtime`, `runtime_probe_timeout_leaves_no_descendant_process`, `..._after_the_runtime_itself_exited`, `runtime_probe_timeout_keeps_leader_identity_owned_until_cleanup`, `runtime_probe_accepts_version_output_later_than_two_seconds`. 보조 근거: cgroup run `44762070325`(PR 브랜치, 11-20)의 probe 진단에서 timeout 이벤트 16건은 모두 의도된 음성 대조(`hung-runtime-negative-control` 4, setsid fixture 12)이고 전부 `residual_pid_present=false`다. 격리된 Node 버전 probe는 6건 모두 timeout 없이 끝났고 최대 1922ms였다.

항목별 판정:

| 위험 | 판정 | 근거 |
|---|---|---|
| 프로세스 누수(timeout 경로) | 없음 | leader를 `child.wait()` 전에 소유한 채 `kill_tree(leader_pid)`를 호출한다. 파이프를 다 읽은 뒤에만 wait하므로 deadline 시점에 leader는 미회수 상태이고, `ps` 부모 링크로 setsid 후손까지 SIGSTOP 후 SIGKILL, 마지막에 `killpg`. 위 시험이 leader 종료 후 후손과 setsid 후손 모두 남지 않음을 확인 |
| 프로세스 누수(성공 경로) | 설계상 남을 수 있음, 결함 아님 | 성공한 probe가 파이프를 닫은 뒤 detach한 후손은 정리하지 않는다. leader를 회수한 뒤 PGID로 `killpg`하면 PID 재사용 시 무관한 그룹을 죽일 수 있어 일부러 하지 않는다. Bubblewrap은 PID namespace와 `--die-with-parent`로, cgroup은 `cgroup.kill`로 덮고, `Isolation::None`은 `isolation.rs`와 `reap.rs` 주석에 한계로 적혀 있다 |
| PID 재사용으로 잘못 종료 | 이론상 창만 있음, 관측 없음 | `kill_tree`는 `root <= 1`을 거부하고 leader PID는 미회수(zombie 포함) 동안 점유된다. 후손 PID는 `ps` 스냅샷과 `kill` 사이 수 ms 동안 종료·재사용될 수 있으나 PID가 순환 할당이라 재사용에는 wrap-around가 필요하다. 실제 재현 조건을 찾지 못했다(`미실행`: 재사용 유도 시험) |
| 비정상 timeout | 확인 못함 | 5초 단일 예산에 대해 격리 Node 최대 1922ms로 여유 약 3초. 과거 실패(Run `44048428391`의 2002ms)는 2초 예산 때의 일이고 #12에서 5초로 바뀐 뒤 main/PR의 Linux 실행에서 재발이 없다. 느린 러너에서는 여전히 이론상 가능하며, 그 경우 서비스가 시작을 거부하고 진단 문자열을 낸다 |
| cgroup 정리 | 문제 없음 | `CgroupGuard`가 `Drop`에서 kill·rmdir하므로 `spawn()` 실패 경로에서도 남지 않는다 |

관찰(수정 대상 아님, 1.1 후보):

- `kill_tree`는 동기 함수이고 `std::process::Command("ps")`를 최대 10 라운드 × 3회 호출한다. `Worker::drop`, `kill_all`, probe timeout 경로가 async 문맥에서 이를 직접 호출해 해당 tokio worker thread를 `ps` 시간만큼 막는다. 누수나 오종료는 아니고 지연이다.
- timeout 후 `cgroup.terminate()`가 실패하면 반환 오류가 `TimedOut`이 아니라 cgroup 오류로 바뀐다.
- macOS Seatbelt는 이 저장소 CI에 없다(`product/VALIDATION.md`). 수동 검증 지원 수준으로 표기한다(감사 P1-10). 이번 실행도 Linux만 확인했다.
- Verify의 early-wait mutation 실패는 의도된 결함 주입이라 결함으로 보지 않았다.
