# Track B 결과: 동시성·트랜잭션·멱등 (2026-11-22)

작업 브랜치 `stab/b-20261122`(worktree `aip-stab-b-20261122`), 기준 `origin/main` 4f44ca87. 공통 규칙은 `aip-phase1-common.md`, 항목 번호(B-01~B-08)는 [baseline.md](baseline.md) 3절. 공개 의미에 걸린 결정은 [decisions/b.md](decisions/b.md).

## 0. 병합 커밋

(병합 뒤 채운다)

## 1. 변경 요약

| 파일 | 목적 | 항목 |
|---|---|---|
| `spikes/spike-v3-write/src/lib.rs` | `lock_bundle_targets`: bundle이 이름 붙인 행이 둘 이상이면 `(resource, id)` 순서로 시작 시 선잠금. 연산 실행·판정 순서는 그대로 | B-01 |
| `spikes/spike-v3-write/tests/bundle_lock_order_opposite_targets.rs` | 반대 순서 교착 회귀와 의미 보존(오류 순서, 안 보이는 행) 시험 | B-01 |
| `spikes/spike-v6-transport/tests/concurrency_probe_http.rs` | a1 탐색 시험을 동시 요청 수·대상 행 수·순서 변형·지연 백분위·교착 수·롤백 검증을 내는 측정 하네스로 확장(기본값은 이전 동작) | B-01 |
| `plan-docs/stabilization-20261122/track-b-bench/` | 측정 격자 스크립트와 원시 결과 | B-01 |
| `product/LIMITS.md` E9, `CHANGELOG.md` | 선잠금 반영, 남은 제약과 지침 | B-01 |

## 2. B1: bundle 잠금 순서(E9) 측정

### 2.1 재현

환경(Homebrew PostgreSQL 17.11, 이 Mac, 포트 55422. `deadlock_timeout=1s`는 `concurrency.md`와 같은 값, `timezone=UTC`는 CI와 같은 값):

```
export PATH=/opt/homebrew/opt/postgresql@17/bin:$PATH LC_ALL=en_US.UTF-8
initdb -D /tmp/aip-stab-b-pg17 -U runner --auth=trust -E UTF8 --locale=C
pg_ctl -D /tmp/aip-stab-b-pg17 -o "-p 55422 -c listen_addresses=127.0.0.1 -c unix_socket_directories=/tmp -c deadlock_timeout=1s -c max_connections=300 -c timezone=UTC" -l /tmp/aip-stab-b-pg.log -w start
psql -X -h 127.0.0.1 -p 55422 -U runner -d postgres -c "create database aip_test_ci"
unset PGHOST PGPORT
export AIP_TEST_DATABASE_URL='host=127.0.0.1 port=55422 dbname=aip_test_ci user=runner'
export AIP_TEST_ADMIN_URL='host=127.0.0.1 port=55422 dbname=postgres user=runner'
export CARGO_TARGET_DIR=/tmp/aip-stab-b-target
```

한 칸(한 설정)을 재는 명령(프로세스당 한 칸. `sqlgen::set_schema`가 프로세스 전역이다):

```
PROBE_N=<동시 요청> PROBE_ROWS=<대상 행> PROBE_ORDER=opposite|shuffle|sorted PROBE_ROUNDS=<라운드> \
  cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --features test-db --test concurrency_probe_http \
  -- --ignored --nocapture --exact a1_opposite_order_two_product_bundles
```

기본값(`PROBE_N=6`, `PROBE_ROWS=2`, `opposite`)은 이전 a1과 같다. 전체 격자는 `bash plan-docs/stabilization-20261122/track-b-bench/run-matrix.sh <label> 10`(라벨별 결과가 `track-b-bench/<label>.txt`에 쌓인다. 이 문서의 표는 `baseline`, `baseline-deadlock-timeout-50ms`, `after-prelock`).

- 한 라운드는 모든 요청을 동시에 보낸다. 호출자 i가 `take(id, 1)`을 대상 행 수만큼 한 묶음으로 보낸다. `opposite`: 짝수 호출자는 id 오름차순, 홀수는 내림차순. `shuffle`: 호출자마다 다른 순열. `sorted`: 모두 오름차순(호출자가 순서를 정규화한 대조군).
- 열 뜻. 성공률: 전체 호출 중 `OK`. **교착 희생자**: `pg_stat_database.deadlocks` 증가분이다. 희생자마다 서버가 최대 3회 다시 실행하므로 **재시도 수의 상한**이다(재시도 3회를 소진하거나 기한이 지나면 재시도 없이 실패). 서버가 재시도 횟수를 따로 노출하지 않아 정확한 값은 못 잰다. p50/p95/p99: 전체 호출 한 건의 소요(ms, 실패 포함, 기한 초과는 약 2000ms). **기한 초과율**: `WIRE.DEADLINE_EXCEEDED` 비율. **롤백 위반**: 라운드마다 모든 대상 행의 재고 감소가 성공 건수와 같고 멱등 기록 수가 성공 건수와 같은지를 단언한다(어긋나면 부분 커밋). 모든 칸에서 0이다. 라운드 10회, 호출 수는 동시 요청 × 10.
- 서버는 HTTP로 실제 기동하고(`listen_with_prototype_deployment_launch`), 풀 크기는 기본 16이다.

### 2.2 방식별 측정

방식: (A) **현재 방식 + 서버 재시도**(수정 전 코드, `deadlock_timeout=1s`). (B) **현재 방식, `deadlock_timeout=50ms`**(선택지 D: 교착 감지 단축). (C) **`(resource,id)` 선잠금**(이번 구현, 같은 격자, `deadlock_timeout=1s`). (D) **잠금 순서 정규화**는 서버가 연산을 재정렬하는 방안인데 의미가 바뀌어 구현하지 않았다. 잠금 효과는 호출자가 미리 정렬해 보내는 `sorted` 행으로 상한을 측정했다(A와 C 표의 `sorted` 행).

#### A. 현재 방식 + 재시도 (수정 전)

| 동시 요청 | 대상 행 | 순서 | 성공률 | 교착 희생자 | p50 / p95 / p99 (ms, 전체 호출) | 기한 초과율 | 롤백 위반 |
|---:|---:|---|---:|---:|---|---:|---:|
| 2 | 2 | opposite | 100% (20/20) | 9 | 1157 / 1181 / 1183 | 0% | 0 |
| 4 | 2 | opposite | 28% (11/40) | 15 | 2012 / 2016 / 2016 | 72% | 0 |
| 8 | 2 | opposite | 1% (1/80) | 31 | 2017 / 2029 / 2032 | 99% | 0 |
| 12 | 2 | opposite | 0% (0/120) | 17 | 2020 / 2034 / 2040 | 100% | 0 |
| 2 | 4 | opposite | 100% (20/20) | 10 | 1164 / 1190 / 1190 | 0% | 0 |
| 4 | 4 | opposite | 0% (0/40) | 18 | 2012 / 2016 / 2016 | 100% | 0 |
| 8 | 4 | opposite | 1% (1/80) | 21 | 2015 / 2021 / 2023 | 99% | 0 |
| 12 | 4 | opposite | 0% (0/120) | 33 | 2018 / 2060 / 2072 | 100% | 0 |
| 4 | 8 | opposite | 0% (0/40) | 19 | 2014 / 2018 / 2019 | 100% | 0 |
| 8 | 8 | opposite | 0% (0/80) | 19 | 2016 / 2021 / 2025 | 100% | 0 |
| 12 | 8 | opposite | 0% (0/120) | 20 | 2015 / 2020 / 2022 | 100% | 0 |
| 8 | 4 | shuffle | 0% (0/80) | 19 | 2016 / 2037 / 2043 | 100% | 0 |
| 8 | 8 | shuffle | 0% (0/80) | 21 | 2014 / 2027 / 2033 | 100% | 0 |
| 8 | 4 | sorted | 100% (80/80) | 0 | 12 / 20 / 31 | 0% | 0 |
| 12 | 8 | sorted | 100% (120/120) | 0 | 27 / 46 / 54 | 0% | 0 |

#### B. 현재 방식, `deadlock_timeout=50ms`

| 동시 요청 | 대상 행 | 순서 | 성공률 | 교착 희생자 | p50 / p95 / p99 (ms, 전체 호출) | 기한 초과율 | 롤백 위반 |
|---:|---:|---|---:|---:|---|---:|---:|
| 2 | 2 | opposite | 100% (20/20) | 5 | 205 / 231 / 232 | 0% | 0 |
| 4 | 2 | opposite | 100% (40/40) | 38 | 559 / 1205 / 1288 | 0% | 0 |
| 8 | 2 | opposite | 25% (20/80) | 146 | 2009 / 2014 / 2015 | 70% | 0 |
| 12 | 2 | opposite | 10% (12/120) | 221 | 2017 / 2035 / 2039 | 83% | 0 |
| 4 | 4 | opposite | 100% (40/40) | 42 | 823 / 1254 / 1372 | 0% | 0 |
| 8 | 4 | opposite | 5% (4/80) | 120 | 2014 / 2024 / 2027 | 95% | 0 |
| 12 | 4 | opposite | 2% (2/120) | 117 | 2016 / 2023 / 2028 | 98% | 0 |
| 8 | 8 | opposite | 5% (4/80) | 128 | 2014 / 2020 / 2024 | 95% | 0 |
| 12 | 8 | opposite | 0% (0/120) | 120 | 2016 / 2024 / 2027 | 100% | 0 |

#### C. `(resource, id)` 선잠금 (구현 후)

| 동시 요청 | 대상 행 | 순서 | 성공률 | 교착 희생자 | p50 / p95 / p99 (ms, 전체 호출) | 기한 초과율 | 롤백 위반 |
|---:|---:|---|---:|---:|---|---:|---:|
| 2 | 2 | opposite | 100% (20/20) | 0 | 7 / 10 / 12 | 0% | 0 |
| 4 | 2 | opposite | 100% (40/40) | 0 | 9 / 12 / 15 | 0% | 0 |
| 8 | 2 | opposite | 100% (80/80) | 0 | 12 / 21 / 29 | 0% | 0 |
| 12 | 2 | opposite | 100% (120/120) | 0 | 18 / 28 / 31 | 0% | 0 |
| 2 | 4 | opposite | 100% (20/20) | 0 | 7 / 10 / 13 | 0% | 0 |
| 4 | 4 | opposite | 100% (40/40) | 0 | 17 / 31 / 43 | 0% | 0 |
| 8 | 4 | opposite | 100% (80/80) | 0 | 16 / 26 / 36 | 0% | 0 |
| 12 | 4 | opposite | 100% (120/120) | 0 | 23 / 43 / 50 | 0% | 0 |
| 4 | 8 | opposite | 100% (40/40) | 0 | 16 / 23 / 31 | 0% | 0 |
| 8 | 8 | opposite | 100% (80/80) | 0 | 22 / 33 / 47 | 0% | 0 |
| 12 | 8 | opposite | 100% (120/120) | 0 | 30 / 48 / 61 | 0% | 0 |
| 8 | 4 | shuffle | 100% (80/80) | 0 | 14 / 24 / 33 | 0% | 0 |
| 8 | 8 | shuffle | 100% (80/80) | 0 | 23 / 40 / 51 | 0% | 0 |
| 8 | 4 | sorted | 100% (80/80) | 0 | 17 / 27 / 33 | 0% | 0 |
| 12 | 8 | sorted | 100% (120/120) | 0 | 41 / 76 / 87 | 0% | 0 |

### 2.3 해석

- A: 2건만 겹쳐도 교착 한 번에 1.2초가 들고(`concurrency.md` 3.1과 일치), 4건 이상이면 대부분 기한(2초) 안에 못 끝난다. 롤백 위반은 없다: 기한을 넘긴 묶음은 아무것도 남기지 않는다.
- B: 4건까지는 모두 성공하지만 8건 이상에서 성공률이 25% 이하로 떨어진다. 감지 비용은 줄어도 교착이 반복되어 재시도 3회가 소진된다. 권한이 필요한 설정이라 단독 해법으로 쓰지 않는다.
- C: 격자 전체에서 교착 0, 성공률 100%, p99 ≤ 61ms. 호출자가 정렬한 대조군(`sorted`)과 같은 수준이다. 연산 순서가 같으니 응답 의미는 같다([decisions/b.md](decisions/b.md) B-D1 표).
- 전후 비교 (검증 명령 그대로, `PROBE_N=2 PROBE_ROUNDS=3`): 이전 `round=1 elapsed=1179ms`, `round=2 elapsed=1180ms`(교착 1회+재시도) → 이후 `round=0 elapsed=14ms`, `round=1 elapsed=9ms`, `round=2 elapsed=9ms`, `deadlock_victims=0`.

### 2.4 시험(RED → GREEN)

`spikes/spike-v3-write/tests/bundle_lock_order_opposite_targets.rs`

- `bundles_taking_the_same_products_in_opposite_orders_do_not_deadlock`: 8건 동시(1-2-3-4, 4-3-2-1, 중간 쌍). 선잠금 호출을 끄면(RED) `round 0: ["WIRE.DEADLINE_EXCEEDED ... @2001ms", ...]`로 실패, 켜면(GREEN) 통과.
- `the_first_failing_operation_still_decides_the_error_and_nothing_is_kept`: 선잠금 뒤에도 먼저 실패하는 연산의 코드가 보고되고 앞 연산이 롤백된다(의미 보존, 변경 전후 모두 통과).
- `rows_the_caller_cannot_read_are_neither_locked_nor_waited_for`: 다른 트랜잭션이 잡고 있는 안 보이는 행을 기다리지 않는다. 선잠금 문장에서 `rows read` 조건을 빼면(RED) 실패함을 확인했다.


## 3. B2: durable·transport 정산 불변식

여섯 불변식의 코드는 `spikes/spike-v6-transport/src/{durable,lib}.rs` 및 기존 B 책임 영역에 반영했다. 기존 여섯 회귀는 앞선 작업에서 컴파일까지만 했고 PG17 실행은 Verify에 남아 있다. 이번 정산 후속 표적 실행 결과는 아래에 별도로 기록한다.

| # | 불변식 | 회귀 | 검증 |
|---:|---|---|---|
| 1 | 멱등 키는 principal별로 격리되고, replay도 해당 소유자 결과만 반환한다. 같은 principal의 다른 요청은 mismatch로 거부된다. | `the_same_key_from_two_principals_runs_twice_and_replays_only_to_its_owner` | 새 통합 시험, PG17 CI 실행 대상 |
| 2 | deployment catalog 조회 오류는 drift로 오판하지 않고 `WIRE.DB_UNAVAILABLE`; 실제 drift는 `WIRE.DEPLOYMENT_CHANGED`를 유지한다. | `a_failed_catalog_read_is_unavailable_not_a_changed_deployment` | 새 fault-proxy 시험, PG17 CI 실행 대상 |
| 3 | 쓰기 확장의 `ctx.data.read`에서 `40001`·`40P01`은 `WIRE.CONFLICT`이고 전체 요청은 롤백된다. 다른 SQL 오류 분류는 유지한다. | `a_serialization_failure_or_deadlock_in_ctx_data_read_is_a_retryable_conflict` | v2 read 오류 매핑 및 새 통합 시험, PG17 CI 실행 대상 |
| 4 | 취소 확인 행 조회 연결 오류를 “취소 없음”으로 취급하지 않고 정산을 재시도한다. | `cancel_is_confirmed_after_the_job_row_lookup_loses_its_connection` | durable fault-proxy 시험, PG17 CI 실행 대상 |
| 5 | 취소 UPDATE 뒤 COMMIT이 DB에 도달하지 않은 경우 정산을 재시도한다. | `cancel_is_confirmed_after_the_commit_never_reaches_postgres` | durable fault-proxy 시험, PG17 CI 실행 대상 |
| 6 | COMMIT은 반영됐으나 ACK만 유실되면 저장 상태를 확인하고 취소 결과를 보존한다. | `cancel_stays_cancelled_when_the_commit_ack_is_lost` | durable fault-proxy 시험, PG17 CI 실행 대상 |

정산 후속 회귀(2026-11-22): `durable_cancel_settlement.rs`에 취소/일반 결과 UPDATE 0행, 일반 완료 COMMIT 미전달 재시도, COMMIT ACK 뒤 동일한 현재 시도 결과의 확인과 다른 저장 결과의 거부를 추가했다. 기존 유한 재시도 경계 안에서 행 수·epoch·attempt·state/code/output을 확인하며, 작업 실행 횟수는 1로 유지한다. 로컬에서 이미 실행 중인 PostgreSQL 17(55422)에 연결해 표적 파일을 실행했고 DB를 시작하거나 재기동하지 않았다.

```
env -u PGHOST -u PGPORT -u PGUSER -u PGDATABASE -u PGPASSWORD -u PGSERVICE -u PGSERVICEFILE \
  AIP_TEST_DATABASE_URL='host=127.0.0.1 port=55422 dbname=aip_test_ci user=runner' \
  AIP_TEST_ADMIN_URL='host=127.0.0.1 port=55422 dbname=postgres user=runner' \
  cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --features test-db \
    --target-dir target --test durable_cancel_settlement
test result: ok. 8 passed; 0 failed; 0 ignored
```

구현 전 RED에서는 취소/일반 결과 UPDATE 0행의 확정 메트릭, DB 미도달 COMMIT 뒤 성공 오보고, 다른 저장 결과를 현재 시도 성공으로 확정하는 네 회귀가 실패했고 기존 취소 오류·ACK 회귀 3개는 통과했다. 구현 후 정상 결과와 손실 ACK의 일치/불일치도 포함해 같은 명령에서 8개 모두 통과했다.

비DB 검사 실행 결과:

```
cargo check --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --features test-db
Finished `dev` profile [unoptimized + debuginfo]
cargo check --manifest-path spikes/spike-v3-write/Cargo.toml --locked
cargo check --manifest-path spikes/spike-v2-read/Cargo.toml --locked
Finished `dev` profile [unoptimized + debuginfo]
cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --features test-db --no-run --test durable_cancel_settlement --test apply_idem_principal_scope --test deployment_fence_catalog_error --test write_extension_read_conflict
Executable tests/apply_idem_principal_scope.rs
Executable tests/deployment_fence_catalog_error.rs
Executable tests/durable_cancel_settlement.rs
Executable tests/write_extension_read_conflict.rs
cargo fmt --manifest-path spikes/spike-v6-transport/Cargo.toml -- --check
cargo fmt --manifest-path spikes/spike-v3-write/Cargo.toml -- --check
cargo fmt --manifest-path spikes/spike-v2-read/Cargo.toml -- --check
```

test-support가 `PGHOST`를 감지해 처음 실행을 거부했다. 환경 변수 값은 조회하지 않고 알려진 `PG*` 변수를 unset한 뒤 명시 loopback DSN을 주어 테스트를 **컴파일만** 했다. DB 테스트나 PG17 측정은 실행하지 않았다. 배경에 기록된 별도 Codex 샌드박스의 PG17 `initdb` shmget 거부를 반복하지 않고 CI에 맡긴다.

## 4. Asia/Seoul 실패 대조

대조 대상은 cron 시험이 아니라 `spikes/spike-v3-write/tests/latest_effect_values.rs`와 `latest_nullable_projection.rs`다. 두 테스트는 `timestamptz::text`의 `+00` 문자열을 비교하고 있어 서울 DB 세션에서는 값의 instant가 같아도 출력 표기 때문에 실패한다. 비교값을 UTC instant 기준 문자열로 정규화했다. v3 제품 코드는 수정하지 않았다. 기존 `Verify`의 `track_b_pg17` 수동 입력은 PostgreSQL 17에서 DB 기본 TimeZone을 각각 `UTC`와 `Asia/Seoul`로 설정하고 `SHOW TimeZone` 세션 확인 뒤 두 테스트를 모두 실행한다.

## 5. B1 기준·CI 대조와 인계

bundle 측정은 `cf429b31966c6ae9e481ca1d20e7f53ae4e81fe8` 기록을 재사용한다: PG17 격자에서 성공률 100%, 교착 희생자 0, p99 최대 61ms, 기한 초과 0%, 롤백 위반 0. 재측정은 하지 않았다.

- `cf429b31`의 선잠금 구현과 `bundle_lock_order_opposite_targets.rs` 회귀는 최신 main에 포함된다. v3 `lib.rs` 추가 수정은 필요하지 않다. 기본 Rust 엔진 테스트 루프에서 실행되며, 이번 로컬 PG 실행은 하지 않아 CI에서 확인해야 한다.
- 새 여섯 DB 회귀도 기본 Verify Rust 엔진 테스트 루프 대상이다. 이번에는 `--no-run` 컴파일까지만 확인했다.
- ignored F1 탐색 시험은 기본 엔진 루프에서 제외된다. Track F1에 요청할 연결 명령(전용 PG17 DSN 사용, `PG*` unset):

  `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --features test-db --test concurrency_probe_http -- --ignored --exact f1_failing_bundles_leave_nothing_behind_under_concurrency`

- v3는 기준 구현·측정 근거를 인계할 준비가 됐고 PG17 CI 실행이 남았다. v6 변경은 소스 검사·cargo check·4개 테스트 파일 컴파일·fmt까지 끝났으며 여섯 DB 불변식의 PG17 CI 실행이 남았다.
- 기존 `tools/verify.sh`의 엔진 루프는 유지하고 `Verify` workflow에 Track B 전용 수동 입력만 추가했다. v2 read 변경은 쓰기 확장 read-conflict 분류를 위한 기존 B 책임 변경이다.
