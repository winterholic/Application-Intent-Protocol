# 저부하 안정화 기록 (2026-11-28)

기준: origin/main `22fde027`. 작업은 별도 worktree에서 직렬로 했다(cargo `-j 1 --locked`, `--test-threads=1`, `AIP_TEST_NO_DB=1`, 기존 target 재사용). 시작 시 load average 약 3.9, 하람앱 빌드 프로세스 없음.

## 작업 1. `server_start_error` NotFound 문구 (P2) — 수정 `bafe76be`

- 문제: `Isolation::validate_available`은 bwrap(Linux)·sandbox-exec(macOS)가 없으면 `NotFound`를 내는데(`spikes/spike-v4-worker/src/isolation.rs:77`), `server_start_error`는 이를 "node 또는 python3가 없다"로만 안내해 원인을 오도했다.
- 수정: `crates/aip-service/src/diagnostics.rs`의 NotFound 문구에 격리 실행기(bwrap, sandbox-exec)를 포함. 값은 싣지 않아 기존 비밀 비노출 시험 조건 유지. 코드와 JSON 형태 불변.
- 시험: `a_missing_isolation_launcher_is_not_reported_as_a_missing_language_runtime` (aip-service lib).
- RED: `panicked ... worker 런타임 실행 파일을 찾지 못함(workers.language의 node 또는 python3가 PATH에 있어야 함)`
- GREEN: `cargo test -j 1 --locked -p aip-service --lib -- listener_that_did_not_start missing_isolation` → `2 passed; 0 failed`
- 영향: 오류 메시지 문자열만. 문구를 매칭하는 도구는 코드(`SERVER_CONFIG`)를 쓸 것.

## 작업 2. C2 전이 `this.g` 항 하한 (P1 계약 정합성) — 수정 `343f12e5`

- 문제: `to f = f + this.g`에서 `g: Int(-5..9)`처럼 선언 범위가 0 이하를 허용해도 `check`가 통과하고, 값이 0 이하이면 런타임이 `WIRE.BAD_VALUE`로 거부(`spikes/spike-v3-write/src/lib.rs:629` 가드). `input` 항은 같은 경우 `DEF.BAD_RANGE`로 정적 거부된다.
- 수정: `spikes/spike-v1-fixture/src/sema.rs` `arith`의 `this.` 항 분기에서, 전이 `to`(효과 아님)이고 Int 항의 선언 범위 하한이 1 미만이면 `DEF.BAD_RANGE`. 범위 없는 `Int`는 기존 정의 보호를 위해 허용. 효과 `update`는 런타임 가드가 없고 기존 PG 시험 정의(`cost: Int(0..100)`)가 있어 제외. `COMPATIBILITY.md` 표에 행 추가, 매트릭스 골든과 `todo` 해제.
- 시험: `a_transition_term_from_a_ranged_int_field_needs_a_positive_range` (spikes/spike-v1-fixture/tests/expr_arith.rs).
- RED: `accepted:` 패닉(수정 전 `Int(0..9)` 정의가 통과).
- GREEN: `cargo test -j 1 --locked -p spike-v1-fixture -- --test-threads=1` → crate 전체 모든 `test result: ok`, 실패 0.
- 영향: 기존 정의가 `service check`에서 새로 거부될 수 있음(위 COMPATIBILITY 행). 저장소 내 `this.`항 사용처 확인: 전이 `to`의 범위 있는 항 없음(`bonus: Int` 범위 없음).
- 수행하지 못한 검증: `product/tests/semantic-definition-matrix.test.mjs`는 aip 바이너리 빌드와 PG17이 필요해 미실행. 골든(`arith-this-field-zero-able`)과 `todo` 해제는 손으로 고쳤고 미확인. spike-v3-write PG 시험도 미실행.
- 남은 위험: 범위 없는 `Int` 항(`reserved: Int`)과 Decimal 항은 여전히 런타임 `WIRE.BAD_VALUE` 가능. 효과 `update`의 `this.g` 항도 마찬가지.

## 작업 3. JIT 프로비저닝 COMMIT 오류 무시 (P0 실패 처리) — 수정 `28ca00cf`

- 문제: `crates/aip-auth/src/lib.rs` 프로비저닝 경로가 `tokio::time::timeout(..., COMMIT).await.is_err()`만 검사했다. 이는 타임아웃만 잡고, 타임아웃 안에 DB가 COMMIT을 오류로 거절하면(`Ok(Err(_))`) 성공으로 처리해 롤백된 `actor_id`로 세션을 발급한다.
- 수정: 판정을 `commit_succeeded`(Ok(Ok(()))만 성공)로 바꿈. 실패 시 기존 ROLLBACK과 `AuthError::Unavailable` 경로 사용.
- 시험: `a_commit_the_database_rejected_is_not_a_success` (aip-auth lib, PG 불필요한 판정 단위 시험).
- RED(판정을 `is_ok()`로 되돌림): `FAILED ... panicked at crates/aip-auth/src/lib.rs:1042`
- GREEN: `cargo test -j 1 --locked -p aip-auth --lib -- --test-threads=1` → `26 passed; 0 failed`
- 영향: 프로비저닝 COMMIT이 DB 오류로 실패한 요청이 인증 성공 대신 `Unavailable`. 정상 경로 불변.
- 수행하지 못한 검증: 실제 PG에서 COMMIT 실패를 유도하는 통합 시험은 PG를 띄우지 않아 미실행. 시험은 판정 함수 수준이다.
- 같은 파일 기존 행 조회 경로(`let _ = db.batch_execute("COMMIT")`, 읽기 전용 트랜잭션)는 영향이 작아 건드리지 않음.

## 검토하고 수정하지 않은 항목

- **C1 (budget cost)**: 확인 결과 todo 시험의 전제("어떤 요청에도 항상 COST_EXCEEDED")가 맞지 않는다. 비용은 `limit*(1+관계+집계+필드정책)+offset`(`spikes/spike-v2-read/src/plan.rs:91,853`)이고 기본 `limit`는 `budget rows`다. `rows 5; cost 1`이어도 호출자가 `limit: 1`을 주면 비용 1로 통과한다. 정적 거부는 요청 값에 따라 성립하는 정의를 거부할 수 있어 소유자 판단이 필요하다. 권장: todo 시험의 주장을 "기본 limit 읽기가 항상 거부"로 좁히고 경고(advisory)로 낼지 결정. 재현: `semantic-definition-matrix.test.mjs`의 `cost-too-low`에서 `limit:1` 요청.
- **K1 경고**: final-report 5.2의 소유자 결정 항목(D-A3)이라 구현하지 않음.

## 이어갈 후보

1. C1 주장 정정과 경고 여부 결정(위).
2. 범위 없는 `Int`·Decimal `this.g` 항의 정적 경고(런타임 BAD_VALUE 가능).
3. 인용 변경 회귀 전수 점검(6.2 #3).
4. `crates/aip-auth` 읽기 경로의 `let _ = COMMIT` 오류 처리 정리.
5. `aip --version --verbose`에 빌드 커밋 노출(6.2 #9).
