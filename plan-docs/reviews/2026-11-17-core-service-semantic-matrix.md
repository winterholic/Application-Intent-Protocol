# Core · Product Service 의미 지원 매트릭스

- 기준 revision: e48ee7aea4f3ec7036798439ca193b31581346af (origin/main, 2026-11-17)
- 검토 경로: Core .aipoc → syntax → sema → Core IR → plan → PG; Service .aip → checked service facts → V6 HTTP → V3 write / V2 read → PostgreSQL
- 목적: 공통 의미가 다르게 구현되는 회귀를 막고, 범위가 다른 기능은 unsupported로 고정한다. 이 문서는 설계 문서만으로 실행 지원을 주장하지 않는다.
- 표의 “동일”은 여기 적은 계약의 현재 의미 비교다. 경로·DSL·출력 계약이 같다는 뜻은 아니다.

## 지원 매트릭스

| 항목 | Core 지원 | Product Service 지원 | 의미 동일? | 차이 의도 | 기존 테스트 근거 (파일:줄) |
|---|---|---|---|---|---|
| Missing / Null / 값 존재 | 지원. 미선택 필드는 Core output shape에서 빠지고 선택적 필드는 nullable로 표시한다. | 지원. 응답의 미선택/미선언 필드는 생략, 저장된 nullable 값은 JSON null, 값은 wire 값으로 반환한다. patch 생략과 명시적 null도 구분한다. | 조회의 생략/null/값 계약은 동일. 쓰기도 생략은 보존, null은 nullable 필드 삭제로 동일하다. | 아니오. 각 DSL의 field shape가 같은 공개 계약으로 매핑된다. | Core crates/aip-sema/tests/shared_semantic_fixture.rs:43-65, crates/aip-sema/tests/aggregate_empty_nullability.rs:57-73; Service crates/aip-cli/tests/service_flow.rs:350-380 |
| 집계와 빈 결과 | 지원: count와 sum은 빈 입력에서 0, min/max/avg는 nullable 결과다. | 부분 지원: count, sum, min, max 지원. avg는 .aip에서 DEF.UNSUPPORTED다. count는 source rowRead를 상속하지 않고, sum/min/max는 그 정책을 FILTER로 적용한다. | 공통 지원 집계의 빈 결과 계약은 동일(0 또는 null). 접근 정책 상속은 다르다. | 부분적으로 예. avg 범위 차이는 명시적 제한. row visibility 상속은 Service가 공개 aggregate와 값 노출 집계를 나누어 적용하는 별도 계약이며, 자동으로 동일시하면 안 된다. | Core crates/aip-sema/tests/aggregate_empty_nullability.rs:100-141; Service crates/aip-cli/tests/service_flow.rs:56-63, 330-339, 구현 spikes/spike-v2-read/src/plan.rs:116-145; avg 거부 spikes/spike-v1-fixture/src/sema.rs:1660-1708 |
| 조건식의 SQL NULL | 지원. nullable 비교 결과를 nullable Bool로 전파하고 null-safe 연산 및 필수 Bool 대입 검사를 한다. | 지원. SQL predicate 결과 UNKNOWN은 WHERE/policy가 허용하지 않는 것으로 처리한다. | 조건의 fail-closed 결과는 동일. Core는 Bool nullability를 IR·정적 검사에도 표현한다. | 아니오. 표현 계층은 다르지만 SQL predicate 계약은 같다. | Core crates/aip-sema/tests/aggregate_boolean_nullability_review.rs:9-61; Service 회귀 crates/aip-cli/tests/service_flow.rs:50-53, 326-341 |
| 관계 선택 · 정렬 | 지원. 관계 필드 및 중첩 선택은 Core selection과 PG 계획으로 내려간다. Core query의 explicit sort는 DSL에서 지정한다. | 지원. .aip의 traverse로 관계를 허용 목록에 선언하고 정렬 키를 지정한다. 동률의 결과는 id tie-break로 안정화한다. | 선언된 관계와 지정 정렬에 대해서 동일한 결과를 만들 수 있다. 무선언 traversal은 양쪽에서 보장되지 않는다. | API 표현과 기본 허용 경계가 다른 것은 의도. tie-break가 명시되지 않은 Core query는 Service와 동일하다고 간주하지 않는다. | Core crates/aip-sema/tests/core_ir.rs:395-420, crates/aip-pg/tests/golden/ariari.plan.json:11090-11125; Service crates/aip-cli/tests/service_flow.rs:50-61, 326-341, 365-386 |
| latest / first 및 fallback | 지원: Core AST/IR/PG 계획이 latest, first, the를 표현한다. fallback은 coalesce 같은 일반 식으로 구성한다. | 부분 지원: Service .aip의 update effect 대상/값에서 latest ... otherwise ...를 지원한다. 일반 read projection에서 Core first/latest 표현은 unsupported다. | effect 경로의 선택 순서와 nullable fallback만 제한적으로 비교 가능. 쿼리 projection 전체의 의미 동등성은 미지원이다. | 예. Service latest는 SQL write effect의 제한 기능이고, Core는 일반 query/expression 기능이다. | Core crates/aip-syntax/src/parser.rs:2536-2551, crates/aip-sema/src/check.rs:426-440, crates/aip-pg/src/sqlexpr.rs:700-716; Service spikes/spike-v1-fixture/tests/latest_effect_match.rs:24-39, spikes/spike-v3-write/tests/latest_effect_values.rs:42-90 |
| 부분 수정 · 값 보존 | 첫-class 부분 patch 입력은 unsupported. Core 명령은 선언한 effect/set을 수행할 수 있지만 Service patch 목록과 같은 omitted-field patch wire 계약은 없다. | 지원. HTTP /apply에서 보내지 않은 nullable·일반 필드는 보존하고 명시적 null은 nullable 필드를 비운다. | 아니오. Core에서 여러 필드 command를 직접 설계하면 유사 결과를 낼 수 있으나 API 의미는 다르다. | 예. Service는 authored transition patch 계약을 제공하고 Core는 선언형 effect 실행을 제공한다. | Core 직접 patch 테스트 없음(명령 실행 경계: crates/aip-runtime/src/engine.rs:392-458); Service crates/aip-cli/tests/service_flow.rs:364-380 |
| 타입 검증 · 출력 계약 | 지원. sema에서 타입 오류를 거부하고 IR output shape에 타입/nullability를 고정한다. | 지원. .aip 로딩·검증 후 HTTP wire 계약과 저장 타입을 적용한다. Decimal은 정확한 문자열 wire를 유지한다. | 선언 가능한 겹치는 타입의 타입/nullable 계약은 같다. 서비스 전용 scalar 및 일부 SDK wire 구성은 겹치는 범위 밖이다. | 부분적으로 예. 차등 언어·wire 인코딩은 의도된 표현 차이다. | Core crates/aip-sema/tests/conformance.rs:1-84, crates/aip-sema/tests/core_ir.rs:405-420; Service crates/aip-cli/tests/service_flow.rs:49-63, 326-356, loader crates/aip-service/src/lib.rs:560-590 |
| 정책 평가 | 지원. Core allow, row visibility, command guard 등을 sema/IR/PG 계획에 표현한다. | 지원. facts의 rowRead/fieldRead/transition policy를 HTTP read/write에 적용한다. 기본 거부, 필드 redaction, 요청 간 정책 변경을 검사한다. | 선언 표현과 평가 엔진은 서로 독립이다. 명시적으로 동일한 predicate를 번역한 경우만 의미 대응으로 볼 수 있다. 자동 동등성은 미지원. | 예. 별도 DSL과 별도 정책 경로가 각자 계약이다. | Core crates/aip-sema/tests/core_ir.rs:45-66, crates/aip-pg/tests/golden/ariari.plan.json:11000-11125; Service crates/aip-cli/tests/service_flow.rs:396-493 |
| 트랜잭션 원자성 · 실패 | 지원. Core runtime은 한 명령의 SQL 효과를 transaction 경계 안에서 수행하고 실패 시 rollback한다. | 지원. Service V3/V6가 owned SQL write를 transaction으로 묶는다. bounded deadlock retry는 owned SQL 경계에서만 재실행하며 caller-owned bundle이나 외부 효과까지 재실행하지 않는다. | 단일 DB transaction의 all-or-nothing 의미는 같다. 재시도와 외부 효과 경계는 계약이 다르다. | 예. 소유권·재실행 안전성 차이로 retry 범위가 다르다. | Core crates/aip-cli/tests/ariari_e2e.rs:499-505, crates/aip-runtime/src/engine.rs:392-458; Service engine tests spikes/spike-v3-write/tests/latest_effect_match.rs:28-85, spikes/spike-v3-write/tests/commit_classification.rs:133-226, V6 spikes/spike-v6-transport/src/lib.rs:246-290 |

### 확인 경계

- 기존 테스트 위치는 해당 의미가 구현/회귀로 고정된 근거다. spike-v* 테스트는 Service가 쓰는 엔진 모듈 테스트이며 Service HTTP 증거로 세지 않는다.
- service_flow는 HTTP와 PostgreSQL을 함께 쓰지만 generated TS/Python SDK 전체 왕복, 모든 정책 조합, 외부 효과 rollback, deadlock 무교착을 보증하지 않는다.
- Core shared fixture 테스트는 .aipoc을 syntax→sema→Core IR로 검사한다. Service fixture 테스트는 실제 .aip/HTTP/PG 경로에서 데이터를 읽고 쓴다. 이 분리는 fixture 자산을 공유하되 Core의 정적 검증을 Service 실행 증거로 오인하지 않기 위한 것이다.

## 첫 공통 conformance fixture

정본: conformance/semantic-v1.json.

- 공통 입력 세 행은 note = null, note = "kept", nullable Decimal의 null/값, 빈 vote 집합과 비어 있지 않은 집합, 동률 정렬을 포함한다.
- 공통 기대 결과는 미선택 필드 생략, null과 값 구별, 빈 count=0, SQL NULL = 'kept'가 매치하지 않음, IS NULL 의미, 정렬된 id를 기록한다.
- Core crates/aip-sema/tests/shared_semantic_fixture.rs는 같은 파일을 읽어 입력/기대의 일관성과 nullable output shape, empty count shape, 조건식이 IR까지 내려가는 것을 검사한다.
- Service crates/aip-cli/tests/service_flow.rs는 같은 파일의 입력 행으로 PG를 채우고 HTTP read/apply 결과와 DB 상태를 기대 결과에 대조한다. Service 검증은 GitHub Actions Linux PostgreSQL 17에서 별도 실행한다.
- 이번 fixture에는 Core PG runtime 실행은 포함하지 않는다. Core 쪽 결과는 compile/IR 계약 테스트이며 서비스 쪽 DB 성공과 구분한다.

## 발견한 차이와 처리

1. Core sum(empty)는 0으로 계약되어 있다. Service sum도 PostgreSQL 구현에서 coalesce(sum(...), 0)을 사용한다. fixture는 count=0을 고정하고, avg만 Service unsupported로 기록한다.
2. Service count는 rowRead를 자동 적용하지 않고, sum/min/max는 source rowRead를 FILTER로 적용한다. 이 차이는 공개 count와 값 기반 집계의 권한 경계를 반영한다. 공통 fixture는 사용자 행 정책이 모두 허용되는 범위만 검사하며 이 경계를 강제로 합치지 않는다.
3. 의도되지 않은 회귀는 이번 정적 fixture 또는 Linux PG17 실행에서 확인되는 즉시 재현 assertion으로 남긴다. 현재 소스 검토에서 고칠 근거가 확정된 런타임 의미 결함은 발견하지 못해 production 로직을 변경하지 않았다.

## 통합 IR 필요성·비용·위험·단계

- **필요성:** IR 하나면 개념적 중복과 parity 위험을 줄일 수 있지만, 현재 Core는 .aipoc 의미 분석·IR·계획기이고 Service는 authored .aip facts와 정책/HTTP/엔진 경로다. 교집합이 불명확한 상태에서 IR을 합치면 존재하지 않는 동일 의미를 강제할 수 있다.
- **비용/위험:** effect/policy/nullable/output-shape 및 서비스 facts를 IR로 왕복하는 변환기, 호환성·마이그레이션·생성 SDK 영향까지 필요하다. 기존 제품 동작, 권한 필터, transaction 재시도 의미가 바뀔 위험이 높다.
- **단계:** (1) 이 JSON fixture와 9항목 matrix를 유지하고 차등 결과를 축적한다. (2) 교집합을 좁혀 SQL NULL, 빈 집계, 타입/output shape처럼 안정된 의미부터 버전 있는 semantic case로 확장한다. (3) 별도 prototype adapter로 .aipoc/.aip 양쪽에서 읽기 전용 정규화 IR을 만들어 차등 비교한다. (4) 충분한 적합성·성능·호환성 증거 후에만 특정 공통 도메인의 공유 IR을 제안한다. 이번 작업은 단계 (1)이다.
