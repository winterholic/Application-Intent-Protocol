# Core · Product Service 의미 지원 매트릭스

- 기준 revision: 86c8d381474ae4bff01025a851f39b4a0a5ddaac (origin/main, 2026-11-17)
- 검토 경로: Core .aipoc → syntax → sema → Core IR → plan → PG; Service .aip → checked service facts → V6 HTTP → V3 write / V2 read → PostgreSQL
- 목적: 공통 의미가 다르게 구현되는 회귀를 막고, 범위가 다른 기능은 unsupported로 고정한다. 이 문서는 설계 문서만으로 실행 지원을 주장하지 않는다.
- 표의 “동일”은 여기 적은 계약의 현재 의미 비교다. 경로·DSL·출력 계약이 같다는 뜻은 아니다.

## 지원 매트릭스

| 항목 | Core 지원 | Product Service 지원 | 의미 동일? | 차이 의도 | 기존 테스트 근거 (파일:줄) |
|---|---|---|---|---|---|
| Missing / Null / 값 존재 | 지원. 미선택 필드는 Core output에서 빠지고 nullable 값은 JSON null로 반환한다. | 지원. HTTP 응답에서 null/값/미선택 필드와 patch 생략/null을 구분한다. | 조회의 공통 계약은 동일. 쓰기는 Service patch만 공통 expected 범위 밖에서 비교한다. | 아니오(조회). 각 DSL의 field shape가 공개 응답으로 매핑된다. | Core `crates/aip-cli/tests/core_semantic_fixture.rs:99-115`; Service `crates/aip-cli/tests/service_flow.rs:502-507,528-544` |
| 집계와 빈 결과 | 지원: count/sum은 빈 입력에서 0, min/max는 null이다. | 부분 지원: 이 fixture의 Int 집계는 지원하며 avg와 Decimal aggregate는 unsupported로 표시한다. 집계별 row visibility 경계는 별도 계약이다. | 공통 fixture에서 빈 count/sum/min/max를 양쪽 runtime으로 실행 비교한다. | 부분적으로 예. Service avg/Decimal aggregate 미지원은 fixture에 명시한다. | Core `crates/aip-cli/tests/core_semantic_fixture.rs:127-142`; Service `crates/aip-cli/tests/service_flow.rs:509-513,807-815`; 집계 visibility `spikes/spike-v2-read/src/plan.rs:116-119` |
| 조건식의 SQL NULL | 지원. SQL NULL equality를 Core runtime query로 검사한다. | 지원. 필터 결과에서 NULL 행 제외 및 값 일치를 HTTP 응답으로 검사한다. | fixture 범위에서 SQL predicate 결과는 동일. Core의 nullable Bool 정적 타입 규칙은 별도다. | 아니오. predicate 계약은 같고 표현 계층은 다르다. | Core `crates/aip-cli/tests/core_semantic_fixture.rs:117-125`; Service `crates/aip-cli/tests/service_flow.rs:514-527`; nullable Bool 검사 `crates/aip-sema/tests/aggregate_boolean_nullability_review.rs:9-37` |
| 관계 선택 · 정렬 | 지원. 관계 집계와 명시적 정렬이 runtime 출력으로 내려간다. | 지원. traverse 관계, 지정 정렬, 안정된 동점 순서를 HTTP에서 검사한다. | fixture에 명시된 정렬에 대해 두 경로가 같은 순서를 낸다. 무선언 traversal 동등성은 보장하지 않는다. | DSL 표현과 기본 허용 경계가 다르다. | Core `crates/aip-cli/tests/core_semantic_fixture.rs:25-30,99-115`; Service `crates/aip-cli/tests/service_flow.rs:499-504,528` |
| latest / first 및 fallback | 지원: Core AST/IR/PG 계획이 latest, first, the를 표현한다. fallback은 coalesce 같은 일반 식으로 구성한다. | 부분 지원: Service .aip의 update effect 대상/값에서 latest ... otherwise ...를 지원한다. 일반 read projection에서 Core first/latest 표현은 unsupported다. | effect 경로의 선택 순서와 nullable fallback만 제한적으로 비교 가능. 쿼리 projection 전체의 의미 동등성은 미지원이다. | 예. Service latest는 SQL write effect의 제한 기능이고, Core는 일반 query/expression 기능이다. | Core crates/aip-syntax/src/parser.rs:2536-2551, crates/aip-sema/src/check.rs:426-440, crates/aip-pg/src/sqlexpr.rs:700-716; Service spikes/spike-v1-fixture/tests/latest_effect_match.rs:24-39, spikes/spike-v3-write/tests/latest_effect_values.rs:42-90 |
| 부분 수정 · 값 보존 | Core의 선언형 command 실행은 지원하지만 Service patch wire 요청과 같은 부분 patch contract는 fixture 지원 목록에 없다. | 지원. HTTP /apply에서 생략한 필드는 보존하고 explicit null은 nullable 필드를 비운다. | 아니오. API별 의미다. | 예. Service는 transition patch 입력, Core는 선언형 effect 실행이다. | Core transaction 경계 `crates/aip-runtime/src/engine.rs:392-395,444-458`; Service `crates/aip-cli/tests/service_flow.rs:530-544` |
| 타입 검증 · 출력 계약 | 지원. runtime에서 Decimal(10,2) 경계 값을 정확한 문자열로 반환한다. | 지원. HTTP wire와 저장소에서 경계 문자열을 유지한다. | 이 fixture의 Decimal 경계 값은 동일하다. 다른 precision/scale 전체 호환성은 범위 밖이다. | 부분적으로 예. Decimal aggregate는 Service unsupported이며 공통 필드 wire 비교에서 제외한다. | Core `crates/aip-cli/tests/core_semantic_fixture.rs:106-110`; Service `crates/aip-cli/tests/service_flow.rs:505-508,539-544` |
| 정책 평가 | Core query/command allow는 지원하지만 field 단위 read redaction과 이 fixture의 `policyNull`/`policyVisible` 출력 의미는 지원하지 않는다. | 지원. fieldRead predicate가 UNKNOWN일 때 null redaction, TRUE일 때 값을 반환한다. | 이 두 expected 키는 Core에서 unsupported로 표시한다. 이를 row visibility로 바꾸면 행 자체가 사라져 field redaction과 다른 결과다. | 예. 제품 DSL의 fieldRead와 Core query allow/row visibility는 다른 정책 계약이다. | Core fixture 경로 선언과 unsupported 이유 `conformance/semantic-v1.json:22-41`, 경로 검증 `crates/aip-sema/tests/shared_semantic_fixture.rs:35-51`; Service `crates/aip-cli/tests/service_flow.rs:91,514-515` |
| 트랜잭션 원자성 · 실패 | 지원. Core command는 SQL transaction 경계에서 효과를 수행하고 실패하면 rollback한다. | Service owned SQL write는 transaction을 사용하며 확정/불확실한 commit 실패를 구분한다. | 단일 DB transaction의 all-or-nothing 의미는 같다. 재시도/외부 효과 경계는 별도 계약이다. | 예. 소유권·재실행 안전성에 따라 retry 범위가 다르다. | Core `crates/aip-cli/tests/ariari_e2e.rs:499-510`, `crates/aip-runtime/src/engine.rs:392-395,444-458`; Service `spikes/spike-v3-write/tests/commit_classification.rs:133-160`, `spikes/spike-v6-transport/src/lib.rs:246-277` |

## `semantic-v1.json` expected 키별 실행 커버리지

`supportedBy`는 각 expected 키의 경로 판정이다. 지원 경로에는 아래 runtime 단언 위치가 있고, unsupported 경로에는 fixture `unsupported`의 이유가 있다. `crates/aip-sema/tests/shared_semantic_fixture.rs`는 expected 키 집합과 지원/미지원 집합이 경로별로 정확히 맞물리는지 검사한다.

| expected 키 | Core | Service | 차이 / 판정 | 현재 근거 (파일:줄) |
|---|---|---|---|---|
| `missingFields` | runtime 응답에서 미선택 필드 부재 단언 | HTTP 응답에서 미선택 필드 부재 단언 | 의미 동일 | Core `crates/aip-cli/tests/core_semantic_fixture.rs:111-115`; Service `crates/aip-cli/tests/service_flow.rs:528-529` |
| `null` | nullable note가 JSON null인지 runtime 단언 | 저장 nullable note가 HTTP JSON null인지 단언 | 의미 동일 | Core `crates/aip-cli/tests/core_semantic_fixture.rs:106`; Service `crates/aip-cli/tests/service_flow.rs:505-506` |
| `value` | note 값 runtime 단언 | note 값 HTTP 응답 단언 | 의미 동일 | Core `crates/aip-cli/tests/core_semantic_fixture.rs:107`; Service `crates/aip-cli/tests/service_flow.rs:507` |
| `policyNull` | unsupported: Core field-level read/redaction 표현 없음 | `note = "kept"`가 UNKNOWN인 row의 phase redaction 결과 단언 | 제품 fieldRead와 Core row visibility는 다른 결과 | fixture `conformance/semantic-v1.json:26-27,40`; Service `crates/aip-cli/tests/service_flow.rs:91,514` |
| `policyVisible` | unsupported: Core field-level read/redaction 표현 없음 | predicate TRUE row의 phase 값 단언 | 제품 fieldRead와 Core row visibility는 다른 결과 | fixture `conformance/semantic-v1.json:27,41`; Service `crates/aip-cli/tests/service_flow.rs:91,515` |
| `emptyCount` | 빈 aggregate와 relation count runtime 단언 | HTTP relation count 단언 | fixture 사례에서 의미 동일 | Core `crates/aip-cli/tests/core_semantic_fixture.rs:109-110,139`; Service `crates/aip-cli/tests/service_flow.rs:512-513` |
| `emptySum` | 빈 aggregate runtime 단언 | HTTP aggregate 값 단언 | 의미 동일 | Core `crates/aip-cli/tests/core_semantic_fixture.rs:140`; Service `crates/aip-cli/tests/service_flow.rs:509` |
| `emptyMin` | 빈 aggregate runtime 단언 | HTTP aggregate 값 단언 | 의미 동일 | Core `crates/aip-cli/tests/core_semantic_fixture.rs:141`; Service `crates/aip-cli/tests/service_flow.rs:510` |
| `emptyMax` | 빈 aggregate runtime 단언 | HTTP aggregate 값 단언 | 의미 동일 | Core `crates/aip-cli/tests/core_semantic_fixture.rs:142`; Service `crates/aip-cli/tests/service_flow.rs:511` |
| `decimalExact` | Decimal 경계 wire 값 runtime 단언 | Decimal 경계 HTTP 값 단언 | fixture 사례에서 의미 동일 | Core `crates/aip-cli/tests/core_semantic_fixture.rs:108`; Service `crates/aip-cli/tests/service_flow.rs:508` |
| `nullEqualityMatches` | NULL note가 매치 결과에 없는지 runtime 단언 | HTTP filter 결과에 NULL note 행이 없는지 단언 | 의미 동일 | Core `crates/aip-cli/tests/core_semantic_fixture.rs:117-125`; Service `crates/aip-cli/tests/service_flow.rs:516-527` |
| `nonNullEqualityMatches` | 일치하는 값 행이 결과에 있는지 runtime 단언 | HTTP filter 결과에 note 값 행이 있는지 단언 | 의미 동일 | Core `crates/aip-cli/tests/core_semantic_fixture.rs:117-125`; Service `crates/aip-cli/tests/service_flow.rs:516-527` |
| `sortIds` | runtime 결과 순서 단언 | HTTP 결과 순서 단언 | 명시 정렬 계약에서 의미 동일 | Core `crates/aip-cli/tests/core_semantic_fixture.rs:99-105`; Service `crates/aip-cli/tests/service_flow.rs:499-504` |

- 키 집합은 fixture의 `expected`에서 13개이며 이 표에 각각 한 번씩 나타난다. 실행 지원/미지원 정합성은 위 정적 fixture 테스트로 별도 검사한다.
- 두 경로가 다른 것은 `policyNull`과 `policyVisible`의 Core field-level redaction 미지원이다. fixture에서 이를 명시적 unsupported로 유지한다. 재현 가능한 제품 결함 차이는 발견되지 않았다.

- 기존 테스트 위치는 해당 의미가 구현/회귀로 고정된 근거다. spike-v* 테스트는 Service가 쓰는 엔진 모듈 테스트이며 Service HTTP 증거로 세지 않는다.
- service_flow는 HTTP와 PostgreSQL을 함께 쓰지만 generated TS/Python SDK 전체 왕복, 모든 정책 조합, 외부 효과 rollback, deadlock 무교착을 보증하지 않는다.
- Core shared fixture 테스트는 .aipoc을 syntax→sema→Core IR로 검사한다. Service fixture 테스트는 실제 .aip/HTTP/PG 경로에서 데이터를 읽고 쓴다. 이 분리는 fixture 자산을 공유하되 Core의 정적 검증을 Service 실행 증거로 오인하지 않기 위한 것이다.

### 확인 경계

## 첫 공통 conformance fixture

정본: conformance/semantic-v1.json.

- 공통 입력 세 행은 note = null, note = "kept", nullable Decimal의 null/값, 빈 vote 집합과 비어 있지 않은 집합, 동률 정렬을 포함한다.
- 공통 expected는 `supportedBy`로 expected 키별 Core/Service 지원 경로를 표시한다. Core field-level read redaction 미지원은 `unsupported`에 사유를 둔다. SQL NULL equality는 두 runtime 응답/결과에서 각각 실행 비교한다.
- Core crates/aip-sema/tests/shared_semantic_fixture.rs는 fixture 정합성과 nullable output shape, empty count shape를 정적으로 검사한다. 이름과 주석으로 이 경계를 밝히며, runtime 값 비교는 aip-cli의 core_semantic_fixture 테스트에서 수행한다.
- Service crates/aip-cli/tests/service_flow.rs는 같은 파일의 입력 행으로 PG를 채우고 HTTP read/apply 결과와 DB 상태를 기대 결과에 대조한다. Service 검증은 GitHub Actions Linux PostgreSQL 17에서 별도 실행한다.
- Core crates/aip-cli/tests/core_semantic_fixture.rs는 fixture 행을 Core runtime Engine으로 PostgreSQL에 적재하고 Core query 결과를 fixture expected와 비교한다. 두 경로는 각자의 실제 실행 결과를 같은 expected에 대조한다.
- 공통 expected에 Int 빈 count/sum은 숫자 0, 빈 min/max는 null을 추가했다. Service에서 지원하지 않는 avg와 Decimal aggregate는 fixture `unsupported`에 기록한다. 동점 score의 정렬 결과와 Decimal(10,2) 필드 경계값도 fixture에서 검증한다.

## 발견한 차이와 처리

1. Core와 Service의 빈 count/sum은 0, 빈 min/max는 null이라는 공통 계약을 fixture 실행 비교로 고정한다. Service avg는 fixture에서 unsupported로 구분한다.
2. Service count는 rowRead를 자동 적용하지 않고, sum/min/max는 source rowRead를 FILTER로 적용한다. 이 차이는 공개 count와 값 기반 집계의 권한 경계를 반영한다. 공통 fixture는 사용자 행 정책이 모두 허용되는 범위만 검사하며 이 경계를 강제로 합치지 않는다.
3. 의도되지 않은 회귀는 이번 정적 fixture 또는 Linux PG17 실행에서 확인되는 즉시 재현 assertion으로 남긴다. 현재 소스 검토에서 고칠 근거가 확정된 런타임 의미 결함은 발견하지 못해 production 로직을 변경하지 않았다.

## 통합 IR 필요성·비용·위험·단계

- **필요성:** IR 하나면 개념적 중복과 parity 위험을 줄일 수 있지만, 현재 Core는 .aipoc 의미 분석·IR·계획기이고 Service는 authored .aip facts와 정책/HTTP/엔진 경로다. 교집합이 불명확한 상태에서 IR을 합치면 존재하지 않는 동일 의미를 강제할 수 있다.
- **비용/위험:** effect/policy/nullable/output-shape 및 서비스 facts를 IR로 왕복하는 변환기, 호환성·마이그레이션·생성 SDK 영향까지 필요하다. 기존 제품 동작, 권한 필터, transaction 재시도 의미가 바뀔 위험이 높다.
- **단계:** (1) 이 JSON fixture와 9항목 matrix를 유지하고 차등 결과를 축적한다. (2) 교집합을 좁혀 SQL NULL, 빈 집계, 타입/output shape처럼 안정된 의미부터 버전 있는 semantic case로 확장한다. (3) 별도 prototype adapter로 .aipoc/.aip 양쪽에서 읽기 전용 정규화 IR을 만들어 차등 비교한다. (4) 충분한 적합성·성능·호환성 증거 후에만 특정 공통 도메인의 공유 IR을 제안한다. 이번 작업은 단계 (1)이다.
