# 1.0 릴리스 게이트 증거 (WBS 8.7)

이 문서는 1.0 출시 판정에 쓸 증거 묶음이다. **승인이 아니다.** 실행과 작성은 같은 Claude Run이며, 교차 공급자 독립 검증은 별도 세션이 한다. 독립 검증 전에는 "증거 수집 완료, 승인 대기"로만 읽는다.

## 게이트가 하는 일

`scripts/release-gate.sh --baseline <이전 릴리스 ref> [--ref <ref>] [--out <새 디렉터리>]`는 작업 트리를 읽지 않고 커밋된 ref의 새 clone에서 아래를 순서대로 실행하며, 첫 실패에서 멈춘다. 단계마다 로그 파일이 남고 `summary.txt`에 한 줄이 쌓인다. DB는 전용 테스트 PostgreSQL(`AIP_TEST_DATABASE_URL`·`AIP_TEST_ADMIN_URL`)만 쓴다.

| 단계 | 명령 | 증명하는 것 |
| --- | --- | --- |
| clone·checkout | `git clone --no-hardlinks`, `git checkout --detach <sha>` | 커밋된 파일만으로 빌드된다. clone이 깨끗한지(`git status --porcelain`) 확인한다 |
| npm-ci | `npm ci --prefix spikes/spike-0-ts --ignore-scripts` | 검증 안내서의 설치 절차가 새 checkout에서 통한다 |
| full-regression | `bash tools/verify.sh` | fmt, clippy `-D warnings`, workspace·엔진 전체 Rust 시험, SDK·패키지·Python 시험 |
| release-build·package | `cargo build --release -p aip-cli --locked`, `node product/tools/package.mjs --profile release` | release 실행 파일과 SDK tarball이 한 묶음으로 만들어진다 |
| artifact-checksums·binary·smoke | `shasum -a 256 -c SHA256SUMS`, 묶음의 `bin/aip`, `product/tools/smoke.mjs --bin … --sdk-package … --workers python` | 묶음의 실행 파일과 SDK가 실제 서버·JWT·PostgreSQL·두 wire·Python worker로 동작한다 |
| baseline-build·upgrade-drill | `product/tools/upgrade-drill.mjs --old-bin <이전 릴리스> --new-bin <묶음의 bin/aip>` | 이전 릴리스가 만든 schema의 업그레이드와 두 가지 롤백 |

## 실행 기록 (2026-10-18)

- 후보: `4e78ddbdacd14e2d5a89f999d0d0d15089a9acc8`. 기준 릴리스(업그레이드 출발점): `4c27040`(2026-10-15, 8.x 작업 이전).
- 환경: macOS arm64(Darwin 27.0.0), rustc 1.98.1, node 26.9.0, python 3.13.15, PostgreSQL 17.11(전용 컨테이너, `127.0.0.1:32801`). 로그와 `summary.txt`는 실행한 Mac Studio의 `/tmp/aip-claude-gate-run1/`에 있다(저장소에는 넣지 않았다).

| 단계 | 결과 | 시간 |
| --- | --- | --- |
| clone·checkout(후보, 기준) | 통과, clone 깨끗함 | 0s |
| npm-ci | 통과(19 packages) | 1s |
| full-regression | 통과. Rust `test result` 325개 묶음 합계 1179 passed·0 failed·9 ignored, Node 152 pass·0 fail, Python 26 OK | 665s |
| release-build | 통과 | 47s |
| package | 통과, 파일 8개 + `SHA256SUMS` | 1s |
| artifact-checksums | 통과 | 0s |
| artifact-binary | 통과 | 0s |
| artifact-smoke | 통과. decimal·safe 두 wire 모두 첫 읽기 1행, 캐시 적중 1, 쓰기 1, 쓰기 뒤 캐시 무효화 1, 멱등 재생 1, 폐기 토큰 거부 1, Python worker 확장 | 5s |
| baseline-build | 통과 | 18s |
| upgrade-drill | 통과(아래) | 2s |

묶음 파일의 SHA-256(`SHA256SUMS`): `bin/aip` `261bbd38…ae11b`, `sdk/aip-sdk-0.1.0.tgz` `a73c9434…d0219`. 전체 값은 `summary.txt`에 있다. 이 해시는 파일 무결성 확인용이며 빌드 서명이나 외부 provenance가 아니다.

### 후보가 main이 아닌 이유 (반드시 읽을 것)

후보 `4e78ddb`는 `99dd719`(main의 한 시점)에 이 트랙의 수정 두 개(`b8da233`, `f896f72`)를 cherry-pick한 **로컬 커밋**이다. 같은 날 main에 들어온 MySQL 7.2.1 트랙의 진행 중 커밋 때문에 main(`f896f72` 시점)에서는 게이트의 앞 단계가 실패한다. 이 트랙의 파일이 아니어서 고치지 않았다.

- `cargo fmt --all --check`: `spikes/spike-v6-transport/src/lib.rs`, `crates/aip-migrate/src/lib.rs`(mod 순서), `mysql_driver.rs`, `crates/aip-service/src/lib.rs`에서 diff.
- `cargo clippy --workspace --lib --bins -- -D warnings`: `aip-service`의 `Gate::MysqlRuntime` variant가 기본 빌드에서 생성되지 않아 `dead_code` 오류.

따라서 **main의 최종 커밋에 대해 게이트를 다시 실행해야 한다.** 위 표는 "main에서 MySQL 트랙의 미완 커밋을 뺀 트리"의 결과다.

### 업그레이드·롤백 드릴 결과

이전 릴리스(`4c27040`)의 실제 binary가 `service init`한 schema에 쓰기·job을 만든 뒤 새 binary로 올렸다(workers=python, durable job 포함).

| 확인 | 결과 |
| --- | --- |
| 적용 전 새 binary `serve` | `WIRE.SCHEMA_MISMATCH`로 시작 거부. 메시지가 `aip service migrate`와 `--apply`를 안내한다 |
| `service migrate` 계획 | `blocked:false`, `requiresReview:true`, 변경 1개(durable job `trace_id` 컬럼 추가), DROP·TRUNCATE·DELETE 단계 없음 |
| 계획 적용 | 저널 1행에서 2행. `service provision` 성공 |
| 업그레이드 뒤 | `/ready` 200. 이전 binary가 쓴 행·멱등 기록이 보이고 같은 key 재전송은 `replayed:true`, 이전 binary가 끝낸 job의 상태·결과가 읽히며 새 job도 `SUCCEEDED`. 계약 지문 변함 없음 |
| 롤백 A: 업그레이드된 schema에 이전 binary | `WIRE.SCHEMA_MISMATCH`로 시작 거부(fail closed). 역방향 migration은 없다 |
| 롤백 B: 업그레이드 전 `pg_dump`를 복원 | 이전 binary가 정상 시작. 행·멱등 기록 복원, 복원으로 사라진 쓰기는 새 쓰기로 다시 실행된다 |

운영 절차는 [릴리스 업그레이드와 롤백](OPERATIONS.md#릴리스-업그레이드와-롤백)에 적었다.

## 게이트가 찾아 고친 결함

| 결함 | 증거 | 수정 |
| --- | --- | --- |
| **실제 구버전 schema를 거부**: trace_id 업그레이드가 컬럼을 쉼표까지만 지워 어떤 배포도 저장한 적 없는 DDL 문자열을 만들었다. 시험은 같은 함수로 구버전 marker를 만들어 버그와 일치했다. 드릴 첫 실행에서 새 binary가 `WIRE.SCHEMA_MISMATCH`였다 | 수정 전 드릴 실패, 수정 뒤 통과 | `b1b7d1b`: 공백까지 지우고 이전 릴리스가 저장한 문장 끝을 시험에 고정 |
| 업그레이드가 필요한 schema에서 `serve`가 원인 없는 일반 메시지를 냄 | 드릴이 안내 문구를 단언 | `f896f72` |
| 코드 배출 지점 snapshot 시험이 checkout 경로에 `/src/`가 있으면 실패 | 게이트 첫 실행 실패 | `b8da233` |
| 패키징이 항상 debug 실행 파일을 묶음 | 코드 확인 | `--profile release` 추가(`cc5aa71`) |
| 운영·역할 문서 예제의 schema `my_app`이 `BAD_SCHEMA`로 거부됨 | 시작 가이드 실행 중 발견 | `4ee67e7`: `aip_my_app` |

## 게이트가 증명하지 않는 것

- main 최종 커밋(위 설명). 독립 검증자는 main에서 다시 돌려야 한다.
- Linux 격리(Bubblewrap)·cgroup 실행, 외부 IdP 실계정, 실제 AWS S3·GCS 계정, 대형 부하·장기 운영, Windows. 일부는 CI와 이전 회차 기록([제품 검증](VERIFICATION.md))에 있으나 이 게이트가 다시 실행한 것은 아니다.
- 의존성 취약점·라이선스 점검(`tools/dependency-check.sh`, cargo-audit·cargo-deny 설치됨): 미실행: 이번 회차에 시간을 쓰지 않았다. 독립 검증 때 실행한다.
- `python3 tools/check_plan_docs.py`: 12개 ERROR. 모두 `plan-docs/alignment/D-development-experience.md`가 저장소 밖의 `../../../../ariari/...` 경로를 링크한 것이다. 게이트에 넣지 않았다.
- 기준 릴리스는 `4c27040` 하나다. 그 사이 다른 시점에서의 업그레이드, 정의가 바뀐 migration과의 조합은 실행하지 않았다.
- 같은 Run의 구현·실행이다. 독립 검증 전이다.
