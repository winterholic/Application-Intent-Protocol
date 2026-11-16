# A 영역 감사: 데이터·조건 의미 일관성

- 날짜: 2026-11-16
- 기준: 새 worktree의 `origin/main` (`f1c6696`)
- 범위: 작성 DSL → 의미 검사/IR → 검증·계획 → SQL/Runtime → SDK 결과 중 데이터/조건 의미
- 판단 원칙: 문서·포팅 기록은 사례 색인으로만 사용하고 구현 판정은 코드와 실행 결과로 제한한다. PostgreSQL 실통합 검증이 불가능한 항목은 그 사실을 표시한다.

## 요약 판정

| 우선순위 | 발견 | 경로 판정 | 영향·조치 |
|---|---|---|---|
| 1 | `min/max/avg` 빈 집합 NULL 가능성이 Core 의미 검사와 grouped output shape에 반영되지 않음 | **확인된 Core 경로 결함**, `aip service` 경로 결함으로는 미확인 | 필수 필드 대입을 거부하고 grouped shape를 nullable로 표시해야 함. 기존 nullable 타입/shape primitive로 수정 가능. 대규모 재설계 불필요. |
| 2 | 선언 DSL/ Core IR/PG compiler와 `aip service` 실행 경로 사이에 end-to-end 연결이 없음 | **검증 공백/추적성 문제** | Core 컴파일러 결함은 현재 서비스에서 실행된다고 말할 수 없다. 서비스가 쓰는 facts JSON·spike 엔진과 Core 간 의미 동등성 검증이 필요. |
| 3 | 관계 선택과 집계/정렬의 제품 통합 테스트는 코드상 존재하지만 이번 실행에서 DB 구동 불가 | **테스트 공백(이번 실행)** | 아래 표에 테스트 파일과 미실행 이유를 기록. |

## 1. 제품 경로와 PoC 경로

| 구간 | 제품 경로 (`aip service`) | PoC/Core 경로 | 판정 근거 |
|---|---|---|---|
| CLI 분기 | `aip-cli`의 `service` 명령이 서비스 구현으로 진입 | `aip-cli`의 `poc-runtime` feature는 별도 컴파일 경로 | `crates/aip-cli/src/main.rs:429-435`는 기본 PoC Runtime 비활성 및 제품 경로 안내. `:514-540`은 `.aip`를 제품 정의로, `.aipoc`를 PoC로 구분. |
| 입력 해석 | `aip-service`가 `spike-v1-fixture` 및 `spike-v11-dev-checks`로 authored fixture/definition을 로드하고 읽기·쓰기 동작은 facts JSON으로 전달 | `aip-syntax` → `aip-sema` → `aip-ir` → `aip-plan` → `aip-pg` | `crates/aip-service/Cargo.toml:25-30`, `crates/aip-service/src/lib.rs:566-581`; Core 경로는 `crates/aip-cli/src/main.rs:475-507`. |
| 실행기·SDK | 서비스는 직접 의존하는 `spike-v2-read`, `spike-v4-worker`, `spike-v5-sdk`, `spike-v6-transport`를 사용 | `aip-pg` SQL compiler와 optional PoC runtime | `crates/aip-service/Cargo.toml:25-30`; 서비스와 Core 경로를 연결하는 호출은 확인하지 못함. 이는 검색한 해당 CLI/service 진입·의존성 두 단계 범위의 정적 관찰이다. |
| 통합 검증 | `crates/aip-service/tests/mysql_read_http.rs`, `mysql_write_http.rs`; FaceReview 서비스 시나리오는 `crates/aip-cli/tests/mysql_facereview_*.rs` | `spikes/*/tests` 단독 테스트, `aip-sema`/`aip-pg` 테스트 | `tools/verify.sh`는 workspace 검사와 별도 제외된 spike engine 검사를 분리한다. Spike 단독 결과는 서비스 통합 증거가 아니다. |

제품 경로에는 `spike-*` 명칭의 crate가 직접 연결되어 있다. 여기서 “spike”라는 이름만으로 그 구현을 PoC 전용이라고 분류하지 않았다. 반대로 해당 crate의 단독 테스트만으로 서비스 통합을 통과했다고 보지도 않았다.

## 2. 항목별 판정

| 영역 | 코드 경로(file:line) | 기존 테스트/증거 | 상태와 판정 |
|---|---|---|---|
| Missing·Null·값 존재 구분 | Core: `crates/aip-sema/src/to_core.rs:2276-2284` (`the/latest/first` nullable), grouped shape `:2086-2098`; 서비스: `crates/aip-service/tests/mysql_read_http.rs:219-249,265-269` | 서비스 HTTP 테스트는 숨김 필드/nullable scalar/reference의 Null, 빈 관계 list를 단언. `spikes/spike-v2-read/tests/hidden_field_null.rs`는 spike engine 단독 테스트 | **테스트 공백** — 서비스가 소비하는 테스트 소스는 있으나 DB 통합 실행은 미실행. Core grouped aggregate nullable 결함은 아래 RED로 확인. |
| 조건부 효과와 평가 시점 | 서비스 MySQL write: `crates/aip-service/tests/mysql_write_http.rs:745-778`; 전이/계획 경로 `crates/aip-service/src/lib.rs` 및 imported fixture 실행 코드 | `mysql_write_http`는 partial patch 및 transaction/effects를 호출하는 서비스 harness; spike-only PG test `spikes/spike-v3-write/tests/conditional_update_effect.rs`는 제품 통합 증거 아님 | **테스트 공백** — 이 실행에서는 서비스 write DB 테스트를 못 돌림. 트랜잭션/동시성 이슈는 다른 담당 범위라 별도 감사하지 않음. |
| 정렬 기준·동률 | 서비스 테스트 DSL `crates/aip-service/tests/mysql_read_http.rs:25-31,235-305`; Core PG list sort `crates/aip-pg/src/plan.rs:1493-1520` | 서비스 테스트는 tie-break 및 cursor pagination을 실행하도록 작성됨. Core compiler는 별도 경로 | **테스트 공백** — 코드상 서비스 시험이 있지만 이번 DB 실행에서는 미확인. Core PG tie-break 구현을 서비스 동작으로 일반화하지 않음. |
| latest/first/last | 문법·Core AST는 `crates/aip-syntax/src/ast.rs:739-741`; Core SQL은 `crates/aip-pg/src/sqlexpr.rs:695-716`; 서비스 계열 SQL 엔진 테스트 `spikes/spike-v2-read/tests/latest_effect_value.rs:25-61` | `latest_effect_value`는 spike read SQL generation이며 Core `first`/service full path 검증은 아님. AST에 `Latest`, `First`는 있고 `Last`는 없음 | **의미 불명확** — `latest`/`first`는 표현되나 `last`는 Core DSL에 primitive가 없다. 동률·NULL 선택 계약은 경로별 테스트가 분리되어 있고 서비스 DB 확인이 안 됨. |
| 관계 참조·대상 선택 | 서비스 read DSL `crates/aip-service/tests/mysql_read_http.rs:25-31,225-269`; Core `crates/aip-sema/src/to_core.rs:2260-2284`; service read engine은 `spikes/spike-v2-read` | HTTP test는 owner/club reference 및 nested comments를 설정하고 결과를 단언. Spike 단독 `ref_select_visibility.rs` 별도 | **테스트 공백** — 서비스 test 본문은 존재, 이번 실행은 DB 연결 전 중단. 대상 행의 visibility/null 정책은 서비스 test에서 확인할 명시 단언이 있음. |
| 집계·빈 결과 | Core: `crates/aip-sema/src/check.rs:397-417`; grouped shape `crates/aip-sema/src/to_core.rs:2086-2098,2276-2284`; Core SQL `crates/aip-pg/src/sqlexpr.rs:718-751`. FaceReview 서비스 fixture: `crates/aip-cli/tests/mysql_facereview_trending_job.rs:1274-1277` | FaceReview product service test는 빈 `avg_processing_minutes = null`을 단언하도록 작성됨. Core 재현은 아래 RED 2건. spike read empty aggregate test는 단독 테스트 | **결함 (Core path)** — Core checker는 Avg를 Decimal로, Min/Max를 입력형으로만 반환하며 nullability는 `Ty`에 없다. grouped output shape는 `ty_shape(..., false)`로 무조건 non-null 처리. 서비스 경로에는 직접 전파된다고 입증되지 않음. |
| 부분 수정·필드 삭제 | `crates/aip-service/tests/mysql_write_http.rs:33-35,745-778`; 서비스 요청 patch 처리 | 서비스 test는 omitted `content` 유지, `amount` 포함 patch를 단언하도록 작성. 명시적 null/삭제 기대도 같은 suite의 source에 별도 단언 | **테스트 공백** — 테스트 source 확인, DB 실행 미확인. 삭제가 nullable null 대입과 구별되는 전용 DSL 의미인지 전수 입증되지 않아 “검증됨”으로 승격하지 않음. |
| Decimal/Money 정밀도 | 서비스 `mysql_read_http.rs:25,235-249`, `mysql_write_http.rs:33,78,745-778`; FaceReview·SDK `crates/aip-cli/tests/mysql_facereview_trending_job.rs`; contract `crates/aip-contract/src/ty.rs` | 서비스 SQL fixture가 Decimal string을 사용하며 write test는 `123.45` roundtrip을 단언하도록 작성. contract unit test는 JSON wire 타입을 다루지만 SDK-실DB 경로와 다름 | **테스트 공백** — 코드상 문자열 wire 표현/정밀도 단언이 있으나 서비스 DB suite 미실행. Money의 제품 경로 정밀도는 이번 검사에서 독립적인 실DB 증거로 확인하지 못함. |
| 입력·출력 타입 일치 | Core checker `crates/aip-sema/src/check.rs:397-417`; Core serialization shape `crates/aip-sema/src/to_core.rs:2086-2100`; service SDK response parsing path via `spike-v5-sdk` | FaceReview service tests contain concrete SDK `/read`, `/extension` calls; Core RED proves output nullable flag mismatch. No one test spans authoring DSL→Core IR→service SDK | **의미 불명확** — Core output shape와 서비스 facts/SDK output contract 간 공통 적합성 시험이 연결되지 않음. Core grouped Min shape는 결함으로 별도 판정. |

### 집계 결함 상세와 최소 재현

- 재현 조건: Core DSL에서 `min`을 빈 집합이 가능한 후보 집합에 적용하고 결과를 required Decimal 필드에 대입하거나, grouped query에서 그룹 키와 추가 predicate가 일치하는 후보를 0건으로 만들어 결과를 반환한다. 빈 집합에서 PostgreSQL `min`은 NULL이다.
- 근본 원인 (코드로 확인, 런타임 원인은 SQL 의미에 대한 추론): `check.rs:404-415`는 `Ty`만 돌려주어 Min/Max/Avg의 빈 집합 nullability를 표현하지 않는다. `to_core.rs:2086-2098`은 grouped expression을 `nullable=false`로 만든다. Core SQL은 `sqlexpr.rs:718-751`에서 Min/Max/Avg에 NULL fallback을 만든다.
- 실제 RED: `crates/aip-sema/tests/aggregate_empty_nullability.rs`의 `min_of_empty_set_is_not_assignable_to_a_required_field`, `grouped_min_output_shape_is_nullable_for_empty_input`.
- 영향: Core pipeline 사용자는 검증 단계에서 불가능한 NULL을 required output으로 선언할 수 있고, 반환 shape가 실제 NULL과 모순될 수 있다.
- 해결: 기존 nullability/shape primitive에 aggregate cardinality 의미를 전달하고 할당 호환성을 검사한다. 새로운 DSL primitive 불필요. 의미 표준화는 `sum` 빈 집합=0, `count`=0, `min/max/avg`=null을 Core·facts engine·SDK 계약에 공통으로 명시하고 회귀 고정해야 한다.
- 같은 유형 경로: 서비스 FaceReview 빈 Avg가 null임을 단언하는 test source가 존재하므로 해당 output type과 SDK serialization의 일치 여부도 연결 시험 대상이다. 이번에 DB를 실행하지 못해 서비스에서 mismatch가 재현됐다고 주장하지 않는다.
- 범위: 이것은 Core compiler 결함이다. `aip service`는 위 표의 dependency trace 상 Core checker/compiler에 도달하지 않으므로, 서비스 장애/실서비스 오동작으로 계산하지 않는다. 제품 경로의 동등한 결함은 아직 미확인이다.

## 3. 포팅 자산에서 확인한 실패·우회 유형

포팅 자산은 과거 사례와 남은 parity 공백을 찾는 색인이다. 아래 분류는 사례의 원인 유형을 요약한 것이며 현재 제품 결함의 실행 증거는 아니다.

| 자산·사례 | 분류 | A 영역과의 관계 | 현재 감사에서의 취급 |
|---|---|---|---|
| FaceReview: aggregate SDK 호출 및 UUID job 입력이 없어 실제 console에서 드러난 SDK 공백 (`porting/facereview/ui-overlay/README.md:42`) | **Abstraction / Implementation** | SDK 추상화가 실제 DSL 결과/입력형을 노출하지 못한 구현 공백 | 해당 기록은 fix 완료라고 되어 있음. 현재 코드의 서비스 시험 연결 및 DB 통과 여부는 이 작업에서 확인 못함. |
| FaceReview: 닫힌 요청 재거부 시험이 `EFFECT_TARGET_MISSING`을 기대했으나 서버는 `INVALID_STATE` (`README.md:46`) | **Verification** | 유사한 상태 전이에서 기대 오류 의미가 서버 의미와 불일치 | 기록상 expectation 조정 후 통과. 데이터 감사 결함으로 분류하지 않음. |
| FlaskBB G-3: `exists R where a or b`에서 전체 `a or b`가 행 조건이 되어 행 집합이 비면 상수 분기까지 거짓 (`porting/flaskbb/PRODUCT-GAPS.md:11` 주변 G-3) | **Semantic** | Missing/empty set와 boolean 조건 범위의 결합 사례 | 앱에서 괄호로 감싼 우회. 기존 `exists` semantics와 문법은 일치한다는 기록이며 checker advisory는 제안 상태. |
| FlaskBB G-6: 정수 post order, timestamp, UUID 기준 불일치와 동률 규칙 차이 (`PRODUCT-GAPS.md` G-6, `:27` 후속) | **Abstraction / Semantic** | 정렬 기준·관계 대상 선택이 원본 의미와 달라질 수 있는 사례 | 기록상 parity open. 이번 감사에서는 별도 실행·서비스 재현 안 함. |
| FlaskBB G-7: create 필드 누락 + nullable DB 값이 NULL로 남아 조건에서 제외, 명시 `false`를 넣어 우회 (`PRODUCT-GAPS.md` G-7) | **Semantic / Implementation** | Missing와 explicit null/default 구분 | 선언 기본값 확장안 기록. 현재 제품 일반 결함으로 승격하지 않음. |
| FlaskBB G-10: 서버 선택 참조를 전이 값으로 대입하거나 patch 값을 조건화하기 어려움 (`PRODUCT-GAPS.md` G-10) | **Expressiveness** | 관계 대상 선택과 조건부 값 갱신 | 관리자 전용 경로로 범위를 줄인 포팅 우회. 권한 의미 문제는 이번 범위에서 제외. |
| Infisical G-16: grace period 시간 산술이 없음 (`plan-docs/porting/INFISICAL-GAPS.md`, G-16) | **Expressiveness** | 조건 의미에 필요한 날짜/시간 계산 | 포팅 자산의 gap 기록. 현재 실행 증거 아님. |
| Infisical: `forceDelete`가 하위 폴더/override 관계 정리의 한계를 드러냄 (`porting/infisical/DB-RUNBOOK.md:79-80`, `app.aip:228-248`; 자세한 상태는 gaps/status) | **Expressiveness / Abstraction** | 여러 관계 대상의 선택과 집계/삭제 범위 | 관계 cascade 정책 관련. 동시성·권한 측면은 타 작업 범위여서 이 감사에서 판정하지 않음. |

## 4. 실행 증거 및 제한

### 실행한 Core baseline

명령:

```text
cargo test -p aip-syntax -p aip-sema -p aip-ir -p aip-plan -p aip-pg -p aip-contract --no-fail-fast
```

결과: exit 0. `aip-sema` 29 tests, `aip-ir` 29 tests, `aip-contract` 2 tests 및 syntax/plan/pg 대상 테스트 통과. 이는 Core 단위 기준선이지 서비스 경로 기준선이 아니다.

### RED 재현

명령:

```text
cargo test -p aip-sema --test aggregate_empty_nullability --no-fail-fast -- --nocapture
```

출력 (exit 101, 0 passed / 2 failed):

```text
min over an empty set is SQL NULL and must not be assigned to required Decimal; diagnostics: []
grouped min can be NULL when a group's input is empty, output shape must mark it nullable: Value { ty: Decimal { precision: None, scale: None }, nullable: false }
```

이 재현은 Core sema의 실제 parser/checker/shape 경로를 거친다. Core PostgreSQL 실제 실행이나 `aip service` 실행은 포함하지 않는다. 이 테스트 파일은 의도적으로 RED 상태로 남겼으며 금지된 결함 수정은 하지 않았다.

### 제품 경로 baseline / PostgreSQL

요청된 제품 기준선 명령을 시도했다.

```text
cargo test --workspace --no-fail-fast
cargo test -p aip-service -p aip-cli --no-fail-fast
```

두 명령 모두 exit 101, `aip-test-support/build.rs:13`에서 중단:

```text
AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults
```

로컬에서 `pg_isready -h 127.0.0.1 -p 52142`는 accepting connections를 반환했으나, `psql -Atqc 'select…'` 연결은 암호가 없어 실패했다:

```text
fe_sendauth: no password supplied
```

따라서 제품 경로 PostgreSQL 시험과 새 제품 RED 재현은 실행하지 못했다. DSN/암호를 찾거나 보호 캐시·설정을 우회하지 않았다. 위 오류는 이 실행 환경의 관찰이며 PostgreSQL 호스트 전체의 부재를 뜻하지 않는다.

## 5. 보류/미실행 항목

- `aip service`의 서비스 통합 suite: DB DSN 누락으로 미실행.
- 서비스 HTTP SDK까지 이어지는 Core/facts 공통 의미 적합성 시험: 현재 연결이 없고 DB 통합 실행도 못해 미실행.
- 결함 수정 및 GREEN: 사용자 범위가 감사와 RED 증명으로 제한되어 하지 않음.
- `last` DSL 연산 도입: 기능 추가 금지에 따라 하지 않음. Core AST에는 `Latest`·`First`만 확인.
- 동시성·권한: 담당 범위가 아니므로 조사·수정하지 않음.

## 6. 제안 수정 난이도

| 항목 | 예상 난이도 | 대규모 재설계 |
|---|---|---|
| Core aggregate nullability | 낮음~중간. expression type에 nullable 전달, assignment 검사와 grouped shape 연결, 기존/신규 의미 테스트 필요 | 아니오 |
| 제품 서비스와 Core 의미의 end-to-end parity | 중간~높음. 두 입력/실행 계약을 비교하는 시험 경로 또는 공통 의미 계약 필요 | 현 증거로는 판단 불가. Core와 facts engine이 별도 구현인 한 교차 검증 작업은 필요 |
| latest/first/last 표준 선택 계약 | 먼저 의미 결정을 표준화해야 함. `last`를 새로 지원하는 것은 별도 기능으로 현재 범위 밖 | 이번 감사에서 결론 불가 |

## 7. 확인 명령 이력

- `git -C /Users/winterholic/development/projects/aip/aip fetch origin && git -C /Users/winterholic/development/projects/aip/aip worktree add -b audit/semantic-a-data-20261116 /tmp/aip-audit-a origin/main` — 성공, 새 worktree HEAD `f1c6696`.
- `cargo test -p aip-syntax -p aip-sema -p aip-ir -p aip-plan -p aip-pg -p aip-contract --no-fail-fast` — 성공.
- `cargo test -p aip-sema --test aggregate_empty_nullability --no-fail-fast -- --nocapture` — 의도된 RED, 2 failed.
- `cargo test --workspace --no-fail-fast` — PostgreSQL DSN guard에서 중단.
- `cargo test -p aip-service -p aip-cli --no-fail-fast` — 같은 DSN guard에서 중단.
- `pg_isready -h 127.0.0.1 -p 52142` — accepting connections.
- `psql -Atqc 'select…'` — `fe_sendauth: no password supplied`.

## 8. 전용 PG 보완 실행 (2026-11-16, 후속)

### 전용 클러스터 시도와 차단

작업자 소유 예정 경로 `/tmp/aip-audit-pg17-a/data`, 포트 `55471`, 데이터베이스 `aip_test_a_semantic_20261116`, OS 소유자 `winterholic`으로 새 PostgreSQL 17.11 클러스터를 초기화하려 했다. 기존 포트 52142와 다른 DB는 확인하거나 변경하지 않았다. `AIP_TEST_DATABASE_URL` / `AIP_TEST_ADMIN_URL`은 이 endpoint 및 `aip_test_` DB를 명시하도록 구성했고, `PGHOST`, `PGPORT`, `PGDATABASE`, `PGUSER`, `PGPASSWORD`, `PGSERVICE`, `PGSERVICEFILE`은 명령 환경에서 unset했다.

실행 명령:

```text
/opt/homebrew/opt/postgresql@17/bin/initdb -D /tmp/aip-audit-pg17-a/data -U winterholic --auth-local=trust --auth-host=trust --no-instructions
```

환경·단계·오류: Codex command tool의 이 작업자 실행 환경에서 initdb bootstrap 단계가 OS named SysV shared memory 생성에서 실패했다. 반환 코드 1. 출력:

```text
selecting dynamic shared memory implementation ... sysv
FATAL: could not create shared memory segment: Operation not permitted
DETAIL: Failed system call: shmget(key=476717691, size=56, 03600)
initdb: removing contents of data directory "/tmp/aip-audit-pg17-a/data"
```

권한 거부 후 다른 실행 도구/초기화 방식으로 같은 클러스터를 재시도하지 않았다. 초기화 뒤의 `pg_ctl start`는 `&&` 체인상 실행되지 않았다. 따라서 해당 경로는 작업자 전용으로 예약·기록했으나 PG cluster 초기화가 완료되지 않았고, 서버는 시작되지 않았으며 `SELECT 1`과 DB 생성은 실행하지 못했다. 데이터 경로는 initdb 실패 후 빈 경로로 남았다. 재실행 시에는 실행 환경이 SysV shared memory를 허용하는지 먼저 해결한 뒤 위 initdb 명령부터 수행하고, `55471`에서 서버 시작 → `aip_test_a_semantic_20261116` 생성 → explicit DSN으로 SELECT 1 → 제품 통합 테스트 순으로 진행한다. DB를 실제로 만들지 못했으므로 중지·삭제할 서버는 없으며, 보존된 작업 경로와 재실행 조건을 기록한다.

### 실행한 DB 비의존 제품 검사

명령:

```text
env -u PGHOST -u PGPORT -u PGDATABASE -u PGUSER -u PGPASSWORD -u PGSERVICE -u PGSERVICEFILE AIP_TEST_DATABASE_URL='postgresql://winterholic@127.0.0.1:55471/aip_test_a_semantic_20261116?sslmode=disable' AIP_TEST_ADMIN_URL='postgresql://winterholic@127.0.0.1:55471/postgres?sslmode=disable' cargo test -p aip-cli --test service -- --nocapture
```

결과: exit 0, **4 passed**. 실제로 실행된 것은 제품 CLI의 definition check, serve 옵션 거부, Python SDK 생성/import 검사 2개다. service DB를 호출하지 않아 A 영역 DB 의미 검증 증거는 아니다.

명령:

```text
env -u PGHOST -u PGPORT -u PGDATABASE -u PGUSER -u PGPASSWORD -u PGSERVICE -u PGSERVICEFILE AIP_TEST_DATABASE_URL='postgresql://winterholic@127.0.0.1:55471/aip_test_a_semantic_20261116?sslmode=disable' AIP_TEST_ADMIN_URL='postgresql://winterholic@127.0.0.1:55471/postgres?sslmode=disable' cargo test -p aip-service --lib -- --nocapture
```

결과: exit 0, **47 passed**. 이는 service crate의 unit tests이며 HTTP/DB data semantics matrix를 호출하지 않는다.

명령:

```text
env -u PGHOST -u PGPORT -u PGDATABASE -u PGUSER -u PGPASSWORD -u PGSERVICE -u PGSERVICEFILE AIP_TEST_DATABASE_URL='postgresql://winterholic@127.0.0.1:55471/aip_test_a_semantic_20261116?sslmode=disable' AIP_TEST_ADMIN_URL='postgresql://winterholic@127.0.0.1:55471/postgres?sslmode=disable' cargo test -p aip-cli --test service_flow --no-run
```

결과: exit 0, `tests/service_flow.rs` test executable을 컴파일했다. `service_flow` assertion은 **실행 전**이며, 이 harness가 DB migration/init, HTTP `/read`, `/apply`, restart/replay을 포함함을 확인했다. 해당 파일의 시나리오는 identity/event/restart 중심으로, Missing/Null·aggregate·partial patch·Decimal matrix를 대신하지 않는다. DB가 시작되지 않아 실행 명령은 제출하지 않았다.

### 제품 테스트 추적과 A 영역 실행 판정

기존 테스트 중 `crates/aip-service/tests/mysql_read_http.rs` 및 `mysql_write_http.rs`는 MySQL harness다. PG 테스트로 오인하지 않았다. `crates/aip-cli/tests/service_flow.rs`는 `aip service`를 실행하는 PostgreSQL 통합 harness지만 해당 시나리오의 assertions는 principal identity, event read/apply, migration/restart, replay에 집중되어 있다. `crates/aip-cli/tests/service.rs`는 DB 비의존 CLI/SDK 생성 검사다. 따라서 이번에 실제 실행한 service test assertions는 작성·검증·SDK 생성 기준이고, A 영역 9항목 중 DB 결과를 실제 실행으로 확정한 것은 없다.

| 영역 | 실제 실행 assertion | 후속 판정 |
|---|---|---|
| Missing·Null·값 존재 | 없음. `service_flow` 실행 전 중단 | 미검증 |
| 조건부 효과·평가 시점 | 없음 | 미검증 (동시성/트랜잭션 평가는 타 범위) |
| 정렬·동률 | 없음 | 미검증 |
| latest/first/last | 없음 | 의미 불명확 유지 |
| 관계 참조·대상 선택 | 없음 | 미검증 |
| 집계·빈 결과 | 없음 | 제품 경로 결함 미확인. 기존 Core 결함은 별도 유지 |
| 부분 수정·필드 삭제 | 없음 | 미검증 |
| Decimal/Money 경계 | 없음 | 미검증 |
| 입력·출력 타입 일치 | service CLI check 및 SDK codegen assertions는 통과했으나 데이터 응답 roundtrip은 없음 | DB/wire 결과 미검증 |

### Core aggregate nullability 두 재현의 관계

`min_of_empty_set_is_not_assignable_to_a_required_field`와 `grouped_min_output_shape_is_nullable_for_empty_input`은 **하나의 설계 누락(aggregate nullability를 Core 의미 타입에 표현하지 않음)의 두 downstream 증상**이다. 첫 증상은 `check.rs`의 할당 호환성 경로에서 `Ty`만 사용해 NULL 가능성을 판정하지 못한다. 두 번째는 grouped shape 경로가 expression별 `may_be_null` 계산을 건너뛰고 `ty_shape(..., false)`를 사용한다. 즉 원인 계층은 공유하지만 검사·shape 생성은 독립 지점이며 각각 회귀 단언이 필요하다. 이는 Core 소스/기존 RED 결과로 한 코드 수준 결론이다. 전용 PG가 시작되지 않아 Core SQL의 실제 NULL row와 Core output contract를 DB로 대조하지 못했고, `aip service` 사실/SDK 경로의 동등한 영향도 계속 미확인이다.

제품 경로 결함 RED는 추가하지 않았다. DB 연동 assertion을 실행하지 못했고, 정적 경로 추적만으로 새 재현을 만드는 것은 실제 제품 결함을 증명하지 않기 때문이다. Core RED 2개는 이전 commit `db98c9e`에 그대로 유지한다.

### 추가 확인 출력

```text
cargo test -p aip-cli --test service -- --nocapture
4 passed; 0 failed; 0 ignored

cargo test -p aip-service --lib -- --nocapture
47 passed; 0 failed; 0 ignored

cargo test -p aip-cli --test service_flow --no-run
Executable tests/service_flow.rs (...)
```

## 9. CI용 제품 데이터 의미 matrix 준비 (2026-11-16 후속)

테스트 전용 commit `4f82d2fc335221c0391f81d811dc127609759297`에서 `crates/aip-cli/tests/service_flow.rs`에 `service_data_semantics_hold_across_http_and_database_state`를 추가했다. 기존 product service harness의 `service init` → JWT principal bind → HTTP `/read`·`/apply` → 전용 schema DB query 흐름을 재사용한다. fixture는 선언 DSL의 nullable scalar, Decimal(10,2), relation select, count aggregate, transition patch만 쓴다.

| 의미 | CI assertion 준비 내용 | 실행 상태 |
|---|---|---|
| Missing / Null | 선택하지 않은 응답 필드의 부재, 선택된 nullable 필드의 JSON null, patch 생략 시 저장값 보존, 명시적 null patch 시 DB NULL 저장 | compile-only; DB assertion 미실행 |
| 정렬 동률·관계 선택 | score 동률 행은 id 순서로 반환, 선택된 member relation의 id/name 포함 | compile-only; DB assertion 미실행 |
| 빈 집계 | Vote가 없는 Event의 count 결과 0, Vote가 있는 Event는 1 | compile-only; DB assertion 미실행 |
| 부분 수정 | title만 patch한 뒤 note/amount/checked를 HTTP 밖 DB query로 보존 확인 | compile-only; DB assertion 미실행 |
| Decimal 경계 | Decimal(10,2) 최대 예시 `99999999.99`를 HTTP 문자열과 DB text cast로 고정 | compile-only; DB assertion 미실행 |
| 순서 불변식 | 동일 row set을 반대 삽입 순서로 재적재하고 정렬된 HTTP rows 전체 동등성 확인 | compile-only; DB assertion 미실행 |

새 함수에는 `cfg`·`ignore`·이름 필터가 없으며 `service_flow` test target의 일반 테스트다. Linux에서도 harness가 실행할 대상으로 컴파일되며, Ubuntu CI에서 실제 실행했다는 증거는 아직 없다. 이는 부모가 C의 격리 PostgreSQL 검증 workflow 통합 뒤 실행 결과로 확정해야 한다.

이 fixture와 현재 확인한 제품 DSL 선언은 `count` 집계까지만 대상으로 삼았다. `min`/`max`/`avg` 빈 집합 의미를 이 제품 harness의 현재 선언 지원으로 입증할 근거는 없어 시험을 억지로 확장하지 않았다. 해당 의미는 기존 Core RED 2개와 제품 서비스 경로 검증 공백을 분리해 유지한다. `create` 입력에서 빠진 nullable 필드와 명시적 null을 구분하는 계약도 이 matrix에 포함하지 않았다.

### 로컬 검사와 제한

```text
env -u PGHOST -u PGPORT AIP_TEST_DATABASE_URL='postgresql://test:unused@127.0.0.1:55439/aip_test_semantic_compile' AIP_TEST_ADMIN_URL='postgresql://test:unused@127.0.0.1:55439/postgres' cargo test -p aip-cli --test service_flow --no-run --locked
```

결과: exit 0, `Executable tests/service_flow.rs` 생성. DSN은 build guard 통과에만 쓰는 명시값이며 DB 연결은 하지 않았다. `cargo fmt --all --check`와 `git diff --check`도 exit 0. 따라서 SQL/schema/HTTP assertion 자체는 모두 미실행이다. 로컬 PostgreSQL 권한 차단에 대한 initdb 재시도나 대체 경로 접속은 하지 않았다.

## 10. 기존 `latest` 실패와 제품 서비스 wrapper 도달성 (2026-11-16 후속)

### 실패 증거와 공통 경계

`/tmp/aip-baseline-engines.log`에서 확인한 결과는 다음과 같다.

- `latest_effect_values`: `latest_effect_reads_latest_row_and_uses_fallback_in_postgres` 실패. 로그가 가리키는 `tests/latest_effect_values.rs:77`은 선택된 요약 행의 `created::text` 기대값 비교다. 직전 76행의 같은 선택 경로 `label` 단언은 통과했다.
- `latest_nullable_projection`: `five_projections_preserve_one_nullable_candidate_and_fallback_only_for_absence`가 `apply, order 101, fallback=true`에서 실패. 테스트 루프상 이 지점 전에 `fallback=true, order 100`의 nullable selected row assertion은 통과했다. 125행은 이후 후보가 없는 order 101의 `actual` projection tuple과 기대 tuple을 비교한다. 이는 해당 실행에서 “선택된 NULL을 fallback으로 바꾸지 않기” 분기는 통과하고 “후보 부재 때 fallback” 분기에서 실패했음을 좁혀 준다.

로그에는 두 실패 모두 `assertion left == right failed`만 있고 실제 left/right 값은 없다. 따라서 첫 실패의 값이 NULL, 다른 후보, 또는 timestamp text 차이인지, 두 번째 tuple에서 어느 projection이 달랐는지 확정할 수 없다.

두 케이스는 공통 `latest` 표현식 lowering 경계를 쓰지만 검증 분기는 다르다. 첫 실패는 후보가 있는 행의 `created` projection 결과이며, 두 번째는 후보가 전혀 없을 때 `otherwise` fallback을 고르는 결과다. 따라서 **같은 primitive 주변의 nullable/선택 문제라는 가설은 가능하지만 단일 root cause로 확정할 증거는 없다.** 후보 선택 또는 nullable projection SQL 생성의 공통 문제와 fallback 부재 분기를 각각 후보로 둔다.

코드상 Form A sema는 `latest`를 서버 update 효과의 값/대상 조건으로만 허용하고, `otherwise`의 타입 호환을 검사한다. 계약 설명은 fallback을 행 부재에만 적용하고, 선택한 행 필드의 NULL은 보존하며, 정렬 NULL 마지막·내림차순과 동률 id 내림차순을 명시한다 (`spikes/spike-v1-fixture/src/sema.rs:642-717`, `syntax_card.rs:206-207`). `spike-v2-read/src/sqlgen.rs:497-521`은 선택 subquery와 독립된 `EXISTS` presence 분기로 이 계약을 SQL로 내린다. 이는 소스 기반 설명이며 실패한 SQL의 실제 반환값은 아니다.

### 현재 `aip service` 경로와 계약

`latest`는 이 service에서 호출 불가능한 단순 spike 전용 기능이 아니다. 정적 호출 경로는 다음과 같다.

1. `crates/aip-service/src/lib.rs::serve`는 `.aip` definition에서 `output.execution` facts를 만들고, `require_supported_runtime`을 통과시킨다. 현재 gate는 encrypted fields를 막을 뿐 `latest`를 거부하지 않는다.
2. `serve`는 같은 facts를 PostgreSQL `spike-v6-transport::server::listen_with_infrastructure`에 전달한다. 서버는 활성 facts와 wire로 contract fingerprint를 계산하고 `/apply` 요청에 계약 헤더를 요구한다.
3. `/apply` handler는 동일 facts로 `handle_apply` → `run_write` → `spike_v3_write::apply_in_with_wire`를 호출한다. 그래서 기존 엔진 테스트와 제품 wrapper는 같은 v3 write primitive·v2 SQL expression compiler를 공유한다. wrapper는 별도로 인증된 caller, idempotency, wire 계약을 적용한다.
4. `spike-v1-fixture/src/isolation.rs`는 `latest`가 포함된 transition facts에 `serializable` marker를 넣는다. wrapper의 `write_needs_serializable` → `apply_attempt`가 이 marker를 읽어 실제 `/apply` transaction isolation을 고른다.

따라서 기존 실패는 AIP service에서도 실행될 수 있는 DSL/primitive의 문제 후보를 다룬다. 다만 두 원래 테스트는 `spike_v3_write::apply`와 직접 PostgreSQL 연결을 사용하므로 HTTP 인증·active fingerprint·service transaction wrapper를 통과하지 않는다. 이번 A `service_flow` matrix definition에도 `latest`는 들어 있지 않다. 기존 latest projection 조건을 그 matrix에 다시 복제하지 않았다.

### 남은 확인 범위

- 현재 자료로 확인한 것은 call graph와 active DSL 계약까지다. 서비스 HTTP wrapper를 통과한 `latest` DB 결과는 아직 assertion 실행 증거가 없다.
- 첫 실패의 timestamp 실제값, 두 번째 실패 tuple의 실제값, 두 실패가 같은 SQL 결함인지 여부는 미확인이다.
- service wrapper의 fingerprint/auth/idem 경계가 이 두 데이터 결과를 바꾸는지, product deployment에서 같은 현상이 발생하는지는 미확인이다.
- 로컬 DB를 재시도하지 않았다. C에 전달된 격리 CI full-output 결과가 오면 left/right와 wrapper 미검증 범위를 반영한다.

이 후속 정적 조사는 테스트 추가나 기능 변경을 하지 않았다. 다음 A matrix 변경은 이미 있는 low-level `latest` 조건을 반복하기보다 service wrapper에서만 관측 가능한 경계를 대상으로 해야 한다. 현재 자료로는 wrapper DB 결과를 확인했다고 주장하지 않는다.
