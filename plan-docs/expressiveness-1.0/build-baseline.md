# 빌드·시험 비용 기준선

측정: 2026-11-21, revision `4e132564`, macOS arm64 18코어, rustc·cargo 1.98.1, `[profile.dev] debug = "line-tables-only"`, PostgreSQL 17.11 컨테이너. 값은 `/usr/bin/time -p`의 `real`(초)이고 한 번씩 쟀다. 다른 세션의 빌드와 동시에 돌았을 수 있어 ±50%로 읽는다.

## 측정값

| 작업 | 명령 | real |
| --- | --- | --- |
| aip 바이너리 콜드 빌드(새 target) | `CARGO_TARGET_DIR=/tmp/aip-expr-target cargo build -p aip-cli --bin aip` | 20.54 (user 128.7) |
| spike-v3 PG 시험 첫 빌드(의존성 일부 공유) | `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test create_ref_visibility` | 4.67 (+ 앞선 실패 시도 1.84) |
| spike-v6 PG 시험 첫 빌드 | `... spike-v6-transport ... --test bundle_http` | 12.76 (user 45.9) |
| spike-v4 PG 시험 첫 빌드 | `... spike-v4-worker ... --test write_wire` | 9.10 (user 29.2) |
| v6 bundle_http 재실행(변경 없음) | 같은 명령 | 0.26 (시험 0.12) |
| sema.rs 한 줄 변경 → v1 check | `cargo check --manifest-path spikes/spike-v1-fixture/Cargo.toml` | 0.15 |
| sema.rs 한 줄 변경 → v1 시험 | `... --test general_update` | 0.65 |
| sema.rs 한 줄 변경 → aip 바이너리 | `cargo build -p aip-cli --bin aip` (spike-v1~v6, aip-service, aip-cli 재컴파일 11개) | 2.50 |
| v3 lib.rs 한 줄 변경 → aip 바이너리 | 같은 명령 | 2.69 |
| sema.rs 한 줄 변경 → v6 PG 시험 | `... --test bundle_http` | 2.50 |
| sema.rs 한 줄 변경 → v4 PG 시험 | `... --test write_wire` | 1.96 |
| sema.rs 변경 → `cargo check -p aip-service` | 처음 check 프로필이라 의존성 check 포함 | 15.63 |
| 대표 PG 시험 실행 시간 | v3 `general_update` 0.29, `counters` 0.13, v2 `keyset_cursor` 0.48, v6 `bundle_http` 0.22, v4 `write_wire` 0.34 | 시험 자체는 1초 미만 |

`touch`만으로는 재컴파일이 일어나지 않았다(cargo가 mtime 대신 내용 변화로 판단한 것으로 보인다, 확인 필요). 측정은 주석·상수 한 줄을 넣었다 되돌리는 방식으로 했고 작업 트리는 원상태다(`git status` 깨끗함).

target 크기: 위 작업 전부 뒤 `/tmp/aip-expr-target` 4.9G.

## 해석

- 증분 비용은 작다. 수정 중 반복은 "spike 하나의 좁은 시험"이 1~3초, aip 바이너리 재링크가 약 2.5초다. 비용의 대부분은 **콜드 target**(첫 빌드 10~20초 + 크기 수 GB)이다.
- spike들은 워크스페이스에서 제외돼(`Cargo.toml` `exclude`) 각자 `Cargo.lock`이 있다. `CARGO_TARGET_DIR`을 주지 않으면 spike마다 `spikes/<spike>/target`이 따로 생긴다. 같은 `CARGO_TARGET_DIR`을 주면 같은 버전의 의존성 산출물을 공유한다(위 v3·v4·v6 첫 빌드가 aip 콜드 빌드보다 짧은 이유).
- 테스트 DSN이 `env!`로 컴파일 시 박힌다(`aip-test-support`, build.rs `rerun-if-env-changed`). DSN·포트를 바꾸면 그 crate와 의존 시험이 다시 컴파일된다. 작업자마다 DSN을 고정한다.

## 공유 target 현황

- 원래 checkout `../aip/target`은 `/Volumes/Backup/projects/aip/aip-target`(19G) 심볼릭 링크다. 다른 브랜치(release/aip-1.0-resume-20261031)와 세션들이 쓴다.
- 다른 worktree들은 각자 `target/`을 가진다(공유하지 않음).
- 이번 작업은 공유 target을 쓰지 않고 `/tmp/aip-expr-target`을 새로 만들었다. cargo clean은 하지 않았다.

## 병렬 작업자 권장안

| 항목 | 권장 |
| --- | --- |
| target | 작업자(브랜치)마다 하나: `CARGO_TARGET_DIR=/tmp/aip-target-<묶음>`(예: `/tmp/aip-target-wp1`, `/tmp/aip-target-wp3`). 같은 target을 두 작업자가 쓰면 cargo 빌드 잠금 때문에 서로 기다리고, 다른 worktree 경로의 같은 crate는 산출물이 따로 쌓여 공유 이득도 적다 |
| 한 작업자 안 | 그 작업자의 모든 spike·crate 명령에 같은 `CARGO_TARGET_DIR`을 준다(의존성 공유) |
| 공유 target 금지 | `/Volumes/Backup/projects/aip/aip-target`는 원래 checkout 전용으로 둔다 |
| DB | 작업자마다 전용 PG 17 컨테이너와 고유 고포트. 기존 로컬 PG(예: 55961에 이미 다른 PostgreSQL이 떠 있었다)를 재사용하지 않는다 |
| 셸 | `unset PGHOST PGPORT`를 먼저(로컬 셸에 설정돼 있음) |
| 디스크 | 작업자당 약 5G. 묶음이 끝나면 그 `/tmp/aip-target-<묶음>`만 지운다(공유 target이나 다른 작업자 것은 건드리지 않는다) |
| 반복 | 수정 중에는 묶음의 좁은 시험만. 전체 워크스페이스 시험 반복 금지 |
