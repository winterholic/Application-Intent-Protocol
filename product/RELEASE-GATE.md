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

### main 커밋에서의 실행 (후보 `4e78ddb`와 별개)

위 후보 결과는 main에서의 성공으로 바꿔 읽지 않는다. 다른 트랙이 fmt·clippy 실패를 정리한 뒤, main의 커밋 `a97f1b9c2d44175e52030da30c6da03a7a6cfd40`을 고정해 같은 명령을 다시 실행했다.

`scripts/release-gate.sh --baseline 4c27040 --ref a97f1b9 --out /tmp/aip-claude-gate-main1` (같은 환경, 같은 전용 PostgreSQL 17)

| 단계 | 결과 | 시간 |
| --- | --- | --- |
| clone·checkout(후보, 기준)·npm-ci | 통과, clone 깨끗함 | 1s 이내 |
| full-regression | 통과. Rust 1212 passed·0 failed·9 ignored, Node 154 pass·0 fail, Python 32 OK(이 중 `fmt --check`와 `clippy -D warnings` 포함) | 606s |
| release-build·package·artifact-checksums·artifact-binary | 통과 | 32s·1s·0s·1s |
| artifact-smoke(두 wire, Python worker) | 통과 | 3s |
| baseline-build·upgrade-drill | 통과(`"ok": true`) | 12s·2s |

이 묶음의 `bin/aip` SHA-256은 `ce8ae6f0…a056a20`, SDK tarball은 `a73c9434…d0219`다. 묶음은 후보 때와 달리 8.10 수정(nbf 선택 허용, Python 3.9/3.10 코드젠, job span 부모)과 retention·S3 ledger를 모두 포함한다. 원시 로그와 `summary.txt`는 실행한 Mac Studio의 `/tmp/aip-claude-gate-main1/`에 있다(저장소에는 넣지 않았다).

이 실행이 증명하지 않는 것은 위 "게이트가 증명하지 않는 것"과 같다. 추가로 이 게이트에는 실제 Keycloak smoke(`scripts/smoke-keycloak.sh`)·IdP/TLS smoke·MinIO live 시험이 들어 있지 않다. 그 결과는 [제품 검증](VERIFICATION.md)의 WBS 8.5·8.10 절에 따로 있다. 같은 Run의 구현·실행이므로 독립 검증은 별도 8.x 검증 세션이 한다. 이 커밋 이후의 main 변경은 이 결과에 포함되지 않는다.

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

- main 최종 커밋: 후보 결과만으로는 아니다. `a97f1b9`에서는 위 절에서 따로 통과했고, 그 뒤의 커밋은 포함하지 않는다. 독립 검증자는 자기가 고정한 SHA에서 다시 돌린다.
- Linux 격리(Bubblewrap)·cgroup 실행, 외부 IdP 실계정, 실제 AWS S3·GCS 계정, 대형 부하·장기 운영, Windows. 일부는 CI와 이전 회차 기록([제품 검증](VERIFICATION.md))에 있으나 이 게이트가 다시 실행한 것은 아니다.
- 의존성 점검: `tools/dependency-check.sh`는 게이트 단계에 없다. 별도 실행 결과는 아래 "후속 증분"을 본다.
- `python3 tools/check_plan_docs.py`: 12개 ERROR. 모두 `plan-docs/alignment/D-development-experience.md`가 저장소 밖의 `../../../../ariari/...` 경로를 링크한 것이다. 게이트에 넣지 않았다.
- 기준 릴리스는 `4c27040` 하나다. 그 사이 다른 시점에서의 업그레이드, 정의가 바뀐 migration과의 조합은 실행하지 않았다.
- 같은 Run의 구현·실행이다. 독립 검증 전이다.

## S3 owner quota의 1.0 보장 범위

owner별 S3 quota는 **서명한 크기와 저장소에서 확인된 크기의 합계**에 대한 상한이다. 소유자가 bucket에 남길 수 있는 바이트의 하드 상한이 아니다. 아래 표가 보장하는 경로와 보장하지 못하는 경로의 확정본이다. "측정"은 전용 PostgreSQL 17과 TLS MinIO에서 실제로 실행한 시험이 있다는 뜻이고, "정적"은 코드 추론만 있다는 뜻이다.

**시험이 재현하는 것의 범위:** live 시험은 `make_all_due`로 장부의 만료 시각을 당긴 뒤 유효한 서명 URL로 실제 MinIO에 PUT하는 **상태 전이** 재현이다. 서명 후 65분 또는 25시간 동안 실제로 지속된 전송을 측정한 것이 아니다.

**미확정 예약을 푸는 기준(2026-10-18 변경):** 서명 PUT은 65분째 `HEAD`에서 객체가 없어도 **풀지 않는다.** 한 번의 `HEAD` 부재는 진행 중인 전송이 없다는 증거가 아니다(URL은 전송 시작 때만 유효하면 된다). 바이트를 계속 세고 24시간 뒤 한 번 더 확인해 그때도 없을 때만 푼다. 그 사이 도착한 객체는 실제 크기로 확정된다. 그래서 다른 업로드에 재할당되는 일은 두 번의 확인 안에서는 없다. 두 번째 확인 뒤에도 끝나지 않은 전송은 여전히 세지 못한다. 대가로 쓰지 않은 서명 예약이 약 25시간 바이트를 차지한다(같은 id 재서명은 대체).

| 경로 | 보장 | 근거 |
|---|---|---|
| 서명 시점의 owner 합계(바이트·개수) 초과 | 보장(`WIRE.QUOTA_EXCEEDED`, URL 없음) | 측정: `object_ledger` 22개, HTTP 경유 5개 |
| 같은 owner의 동시 서명 | 보장(owner 단위 advisory lock) | 측정: `concurrent_reservations_cannot_jointly_exceed_the_quota`. 락을 없앤 변이는 이 시험이 잡는다. HTTP 시험은 서버가 서명을 충분히 직렬화해 락 변이를 못 잡으므로 경쟁 검출기가 아니다 |
| owner 사이 격리 | 보장 | 측정 |
| multipart part 크기 누적, 유휴 upload의 abort와 예약 해제 | 보장 | 측정: 실제 MinIO |
| 사용하지 않은 PUT 예약 | 약 25시간(65분 + 24시간) 뒤 해제. 그 전에는 바이트를 계속 센다 | 측정: `live_expired_signed_put_is_checked_against_the_store`, 장부 시험 |
| 올라간 객체의 실제 크기 확정 | 보장 | 측정: 실제 MinIO |
| 첫 확인(65분) 뒤에 끝나는 PUT | **보장**: 보류 중인 예약이 확정된다. 그 사이 다른 업로드가 그 바이트를 받지 못한다 | 측정: `live_an_upload_finishing_after_the_first_check_is_still_counted`(상태 전이 재현) |
| 두 번째 확인(약 25시간) 뒤에 끝나는 PUT | **보장하지 못함**: 예약이 풀린 뒤 완료되면 객체는 bucket에 있고 장부는 0바이트 | 측정: `live_known_limit_an_upload_finishing_after_the_hold_ended_is_not_counted` |
| 장부가 생기기 전에 올라간 객체, 서버를 거치지 않고 provider에 직접 쓴 객체 | **보장하지 못함**: 행이 없어 세지 않고, 그 위에 quota 전체를 더 서명할 수 있다 | 측정: `live_known_limit_an_object_that_predates_the_ledger_is_not_counted`(행이 없는 상태를 강제). 직접 쓰기는 같은 상태이므로 별도 시험은 없다 |
| 서명 DELETE | 저장소가 객체 없음을 확인할 때까지 quota 유지 | 측정: 실제 MinIO |
| 서명한 크기보다 큰 PUT | 보장(SigV4가 Content-Length를 서명) | 정적. 선언보다 작은 업로드는 장부가 서명 크기로 세므로 보수적이다 |
| provider 비용·bucket 전체 용량 | **범위 밖** | 운영자가 bucket 수명 주기·용량 제한을 설정해야 한다 |
| AWS S3·GCS 실계정 동작(HEAD·abort 응답 차이 포함) | **미실행** | MinIO로만 검증했다 |

- **영향:** 악의적 소유자는 두 번의 확인(약 25시간)이 끝난 뒤에 완료되도록 전송을 극단적으로 느리게 끌면 quota를 우회해 저장 공간을 늘릴 수 있다. 이전 설계(65분)보다 훨씬 어려워졌지만 하드 상한은 아니다. 우회 크기는 동시에 열어 둘 수 있는 전송 수와 객체당 5GiB 상한에 묶인다. 객체는 owner 접두어 아래 있으므로 소유자 격리와 다운로드 권한에는 영향이 없다.
- **1.0에서의 표기:** 이 quota를 "owner당 저장 용량을 보장한다"고 광고하지 않는다. 광고할 수 있는 것은 "서명 시점에 owner 합계가 설정값을 넘는 요청을 거절하고, 서명 후 약 25시간 안에 끝나는 업로드는 계속 센다"까지다. 운영자는 bucket 수명 주기 규칙과 provider 쪽 용량·비용 제한을 함께 설정해야 한다([서버 인프라](INFRASTRUCTURE.md)).
- **이 설계가 증명하지 못하는 것:** 보류 시간을 늘려도 전송 지속 시간의 상한은 provider가 보장하지 않는다. 증명은 provider 쪽 신호가 있어야 닫힌다: (1) 객체 생성 알림(S3 Event Notifications·Pub/Sub)을 받아 장부와 대조하거나, (2) owner 접두어를 `ListObjectsV2`로 훑어 장부에 없는 객체를 확정하는 것이다. 둘 다 provider 설정과 owner 해시 ↔ 원문 owner 매핑 설계가 필요해 이번 릴리스에 넣지 않았다. 객체를 자동 삭제해 상한을 지키는 방식은 데이터 손실이므로 택하지 않는다.

2026-10-19 증분: `POST /objects/confirm`과 TS `client.objects.confirm({id})`를 추가했다. 인증된 owner 접두어의 provider HEAD로 기존 객체를 장부에 계상하고, 기존 PENDING/DELETING 예약·타이머는 유지하며 활성 multipart는 거부한다. quota 초과의 실제 객체도 관측값을 기록하고 다음 서명을 막는다. 실패를 0으로 해석하지 않는다. 위 두 미계상 경로는 **명시적 confirm을 호출하면 복구 가능**해졌지만 자동 발견은 여전히 미구현이다. 이 증분의 Rust/TS 검사와 실제 MinIO 실행 결과는 각각 구분해서 기록하며, 이전 표의 측정 근거를 새 API의 실행 증거로 재사용하지 않는다.

## 인계 (2026-10-18)

- **현재 변경(main, 모두 push됨):** 8.11 `c542d90`, 8.5 `0088a88`·`dc4129d`, 8.7 `b1b7d1b`·`cc5aa71`·`b8da233`·`f896f72`·`4ee67e7`·`b7f592e`, 8.10 후속 6건 `a8b0d0b`·`5b7f23c`·`791367b`·`99e1052`·`3b0925f`·`dc95dc9`·`a97f1b9`. 모두 같은 Claude Run의 구현이며 독립 검증 전이다.
- **검증 위치:** 항목별 명령·결과는 [제품 검증](VERIFICATION.md)의 WBS 8.11·8.5·8.10 절, 게이트 결과는 이 문서다.
- **미완료·열린 것:** 위 "증명하지 않는 것", 중첩 job의 span 부모 시험 없음, Python 3.11·3.12 미실행, Auth0·Entra 미실행, S3 quota 우회 경계(위). spike-v6 clippy 실패와 의존성 점검은 아래 "후속 증분"에서 처리했다.
- **독립 검증이 필요한 설계:** S3 장부(서명 크기 기준, 65분 유예, 임대·`version` 경쟁), retention 쿼리 비용, nbf 선택 허용, `trace_id` 컬럼의 `<trace>-<span>` 의미, Python 코드젠 호환 방식, 구버전 schema 인식 수정.

## 후속 증분 (2026-10-18, 독립 검증 결과 반영, Claude 구현·실행)

**전체 게이트는 이 증분 뒤의 SHA에서 다시 돌리지 않았다.** 마지막 전체 게이트 성공은 위 `a97f1b9` 하나다. `364ad32`·`cd915f5`에서 시작한 두 번의 게이트 실행은 세션이 끝나며 중단되어 결과가 없다(`npm-ci`까지만 통과). 독립 검증자는 자기가 고정한 SHA에서 다시 돌려야 한다.

전용 PostgreSQL 17(`aip-claude-final-pg`, 127.0.0.1:32811)과 TLS MinIO(`aip-claude-final-minio`, 127.0.0.1:32812)에서 실행했다. 공유 5432에는 접속하지 않았다. 검증자 샌드박스의 `initdb ... shmget Operation not permitted`는 그 샌드박스의 제약이며 제품이나 호스트 DB의 불가가 아니다. 호스트에서는 같은 이미지의 컨테이너가 정상 기동했다.

| 항목 | 명령(고정 clone `364ad32`) | 결과 |
|---|---|---|
| 8.5 장부 | `cargo test -p aip-execution --test object_ledger` | 17 passed(이 증분에서 22로 늘어남, 아래) |
| 8.5 retention | `cargo test -p aip-execution --test events`, `cargo test --lib retention_tests`(spike-v6), `--test outbox` | 13, 4, 2 passed |
| 8.5 S3 HTTP·실 MinIO | `cargo test --test object_ledger_s3`, 같은 명령 `-- --ignored --test-threads=1` | 4 passed; live 4 passed(이 증분에서 live 6) |
| 8.11 COMMIT_UNKNOWN | `commit_unknown_wire` 13, `durable_commit_classification` 4, `write_extension_commit_classification` 12, `durable_job_commit_loss` 3, v3 `commit_classification` 18, v4 10 | 모두 passed(실제 PG) |
| 8.11 변이(실제 PG) | M1 요청 COMMIT의 Unknown을 DB_UNAVAILABLE로, M2 효과 COMMIT, M3 쓰기 경로를 COMMIT 전 단절로 | 3개 모두 잡힘(`durable_commit_classification` 3 실패, `durable_job_commit_loss` 2 실패, `commit_unknown_wire` 6 실패). 파일은 복원했다 |
| 의존성 | `bash tools/dependency-check.sh` | 처음엔 rc=1: prototype과 spike v2·v3·v4·v5·v7·v11의 `rustls 0.23.45`가 RUSTSEC-2026-0346~0349(2026-10-14 공시) 4건. root와 v1·v6는 통과. `cargo update -p rustls --precise 0.23.46`을 7개 lock에 적용(`4df44b2`) 뒤 rc=0, `npm audit` 0 |

### spike clippy
`items after a test module`(spike-v6 `src/server.rs`)를 재현(cd915f5에서 `cargo clippy --all-targets -- -D warnings` 실패)하고 테스트 모듈을 파일 끝으로 옮겼다. 원인: `tools/verify.sh`는 루트 workspace와 일부 crate만 lint하고 workspace 밖 engine crate(`spikes/*`)는 lint하지 않아 게이트가 못 잡았다. 같은 검사를 돌리자 spike-v1(2건), spike-v2(1건 lib, 2건 테스트 `unused mut`)도 실패해 고쳤고, `tools/verify.sh`가 6개 engine crate를 `--all-targets -D warnings`로 lint하도록 했다. 6개 모두 통과, `cargo fmt --all --check` 통과, 기본·mysql·poc 루트 clippy 4개 통과, spike-v6 전체 228 passed.

### S3 quota: 늦은 PUT 보수화
- **재현한 결함:** 65분째 `HEAD`에서 객체가 없으면 예약을 풀어, 진행 중이던 전송이 나중에 끝나도 장부에 없었다. 앞선 live 시험은 이 누락을 "있어야 통과"로 고정하고 있었다.
- **변경:** `Outcome::NotYetStored`. 서명 PUT의 첫 부재는 예약(바이트·개수)을 유지하고 `settling`으로 표시한 뒤 24시간 뒤 한 번 더 확인한다. 두 번째에도 없을 때만 푼다. 늦게 도착하면 실제 크기로 확정한다. 다시 서명하면 새 예약(`settling=false`)이다. 서명 DELETE의 부재와 multipart abort는 그대로 즉시 해제다. 스키마는 `ADD COLUMN IF NOT EXISTS settling boolean NOT NULL DEFAULT false`만 더한다(파괴적 변경 없음, 옛 테이블 업그레이드 시험 있음).
- **시험:** 장부 22(신규: 첫 부재 유지·둘째 부재 해제, 보류 중 도착 확정, 재서명 뒤 stale 보류 무시, DELETE는 보류하지 않음, 옛 테이블 DDL 업그레이드). live MinIO 6 passed(`live_an_upload_finishing_after_the_first_check_is_still_counted`가 보장 시험이고, 옛 누락 시험은 `..._after_the_hold_ended_is_not_counted`로 한계를 두 번째 확인 뒤로 옮겨 고정). 변이 확인: 첫 부재를 즉시 해제하게 하면 live 3개 실패, 두 번째에도 계속 보류하게 하면 장부 1개와 live 2개 실패.
- **회귀:** `aip-execution` 129 passed, spike-v6 228 passed(0 failed), spike-v6·aip-execution `--all-targets` clippy 통과.
- **남은 것:** 두 번의 확인 뒤에 끝나는 전송, 장부 이전 객체, provider 직접 쓰기는 세지 못한다(표 참고). 증명하려면 provider 알림 또는 목록 대조가 필요하다. AWS·GCS 실계정 미실행. Linux 미실행.
- **독립 검증이 필요한 설계:** 보류 24시간 상수, 쓰지 않은 예약이 약 25시간 바이트를 차지하는 트레이드오프, `settling` 컬럼 추가.
