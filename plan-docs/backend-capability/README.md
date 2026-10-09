# AIP 백엔드 기능 포괄성 감사

> 2026-10-09 분석·제안 단계. [창시자 원문](../sources/founder-backend-capability-directive-2026-10-09.md)을 보관하고 제품/PoC 코드·공식 자료·실행 반례로 대조했다. 새 실행 모델의 대규모 구조 변경은 수행하지 않았다.

AIP의 목표는 SQL/CRUD 간소화에 머물지 않고 백엔드 반복 로직 전반을 선언·공통 Runtime으로 흡수하는 것이다. 현재 제품은 데이터·정책·전이·제한된 확장에 강점이 있지만 durable Job, 외부 효과, 관리형 파일, realtime, 운영·기능 catalog에는 제품 계약의 빈틈이 있다. 짧은 순수 계산·일부 사용량 집계는 이미 가능하므로 “SQL 밖 기능은 전부 불가”라고 단정하지 않는다.

## 요청 결과물

| 결과물 | 파일 | 범위 |
|---|---|---|
| A Backend Capability Matrix | [전체 CSV](capability-matrix.csv), [읽기용 표](capability-matrix.md), [추가 수명 계약](additional-capabilities.md) | 원문 A~J 105개 최소 항목과 별도 본인 확인·동의/개인 데이터 수명. 전통 구현/현재 제품/PoC/현재·목표 Level/프론트·서버·확장/제약/OSS/우선순위/중복 제거 가설 |
| B SQL 편향 감사 | [sql-bias-audit](sql-bias-audit.md) | resource/PG/효과/수명/transport 결합 원인, SQL-free 의미 구분 |
| C 실행 모델 고도화 | [execution-model-proposal](execution-model-proposal.md) | 설계 관문 8개, L1–4와 효과·수명·의존 축, 공통 계약/전문 executor 경계 |
| D 실제 시나리오 | [scenarios](scenarios.md), [validation](validation.md) | commerce/approval/SaaS/integration/realtime/SQL 없는 작업. 시나리오별 8항목과 실제/PoC/가상 문법 구분 |
| E 구현 로드맵 | [roadmap-and-risks](roadmap-and-risks.md) | 반복 glue·AI 선택·재사용·위험·비용의 정성 비교와 수직 검증 관문 |
| F 위험·미해결 | [roadmap-and-risks](roadmap-and-risks.md), [cross-review](cross-review.md) | 독립 검토 정정/이견, 미검증 provider·OS·durability, 후속 작은 제품 허점 |
| 조사 근거 | [sources](sources.md) | 제품/PoC 경로, 6 framework+관련 실행 프로젝트 공식 문서, 실제 앱 pinned source/test, 16 root license 파일 |

## 읽는 기준

- 제품 지원은 `aip service`에서 도달하는 경로로 판단한다. root `aip-runtime` PoC 기능은 별도 표시한다.
- Level1–4는 표준화 위치이며 구현 현황 점수가 아니다. 목표 Level을 현재 지원으로 합산하지 않는다.
- 새 JSON/YAML 실행 계약은 가상 제안이며 최종 DSL/SDK/쓰기 모델 결정이 아니다. 기존 유효 문법은 원본 fixture/probe로 연결한다.
- 외부 소스 관찰, AIP 적용 추론, 실제 AIP 실행 결과, 미검증 가설을 분리한다. 외부 프로젝트 전체를 실행하거나 신규 의존성을 채택하지 않았다.
- 이번 검증은 포괄성 감사와 일부 반례의 증거다. 여섯 전체 업무 흐름을 제품에서 완성했다는 결과가 아니다. 생산성·성능 개선 수치는 측정하지 않았다.

## 후속 작업에서 유지할 기준

새 기능을 SQL/CRUD 테스트만으로 닫지 않는다. 해당 기능이 Job·event·external/file·realtime 경로에서도 권한·tenant·멱등·실행 비용·실패·취소 계약을 유지하는지 확인한다. 반복되는 L4 glue부터 L1–3 후보를 찾고, 특수 알고리즘을 무리하게 DSL화하지 않는다. 실행 근거 없는 지원 주장이나 정정 전 독립 초안을 정본으로 사용하지 않는다.
