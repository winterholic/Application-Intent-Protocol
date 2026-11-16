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
