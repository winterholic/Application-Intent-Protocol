# AIP plan-docs

> 상태: 2026-10-03 통합 지침과 2026-10-09 백엔드 기능 포괄성 지침을 따른다. 창시자가 밝힌 목적·제품 구조와 에이전트의 미검증 상세 설계를 구분한다. 지침 보관이나 감사 착수가 설계안 전체 승인인 것은 아니다.

이 폴더는 AIP 설계와 실행 근거를 기록한다. 작은 실험의 실행 결과는 최종 설계 승인이 아니다. 2026-10-05 사용자가 운영 인증·일반 마이그레이션·제품 통합을 명시적으로 요청했으므로, 기존 `crates/` 보존 범위에서 새 `aip service` 조립층을 추가했다. 현재 경로와 한계는 [제품 서비스](../crates/aip-service/README.md), [진행 현황](STATUS.md)을 따른다.

## 기준 문서 (충돌하면 위가 이긴다)

1. [백엔드 기능 포괄성 감사 및 선언형 실행 모델 고도화](sources/founder-backend-capability-directive-2026-10-09.md): SQL 외 A~J 영역, Level 1~4, 여섯 실제 시나리오와 Claude Code·Codex 독립 대조를 요구한 최신 원문. 현재 단계는 분석·제안이며 승인 없는 대규모 구조 변경은 금지한다.
2. [창시자 의도 복구·설계 고도화 통합 지침](sources/founder-integrated-directive-2026-10-03.md): 기존 목적·제품 구조 기준. [FOUNDER]/[DIRECTION]/[OPEN] 원문 표식을 보존한다.
3. `sources/founder-intent-handoff-2026-10-03.md`: 이전 인계 원문. 토론 방식은 `sources/design-operating-rules-2026-10-03.md`(적용법은 `00-rules.md` §7). 통합 지침과 충돌하면 최신 지침을 따른다.
4. `../docs/PRINCIPLES.md`: 창시자 원칙과 설계 관문. 최신 원문을 따라야 한다.
5. `../docs/DECISIONS.md`: 이전 결정의 이력. 현재 질문 처리 상태는 [B](alignment/B-decision-reclassification.md).
6. `../docs/design/11-founder-intent-audit.md`, `12-design-reset.md`: 감사와 재개 계획. 당시 조건의 근거 자료다.
7. `../docs/origin/*`, `../docs/design/00~10`: 이전 기술 자료. 현 목적·원칙의 우선 근거가 아니다.

## 이번 결과물 A~E

| 결과물 | 문서 | 내용 |
|---|---|---|
| A | [창시자 의도 반영표](alignment/A-founder-intent-matrix.md) | 7가지 원칙·각 FOUNDER 항목의 코드/설계 대응과 누락 |
| B | [결정 상태 재분류](alignment/B-decision-reclassification.md) | 질문 25개·Q0~Q9·14개 의제, 확정 목적과 미정 상세 |
| C | [문법 고도화 제안](alignment/C-syntax-proposal.md) | A/E/H 서버 작성, 프론트 read/apply, 조합·공식 확장·metadata |
| D | [개발 경험 비교](alignment/D-development-experience.md) | 실제 아리아리 소스 근거, 10개 시나리오와 측정 방법 |
| E | [기술적 이견·검증](alignment/E-technical-risks.md) | 반례·실험 관문·미실행 범위와 Claude 미참여 한계 |

이 문서 세트는 **설계와 소스 대조 결과**다. 신규 문법/SDK/planner/worker가 실행된 결과는 아니다. [독립 대조 기록](reviews/INTEGRATED-codex-luna-r1.md)에 실제 발견과 반영을 남겼다.

문서 구조 검증의 실행 명령·출력·한계는 [V0 결과](alignment/V0-validation-results.md)에 있다.

후속 실행 상태는 [STATUS](STATUS.md)를 따른다. [실행 프로토타입](../prototype/README.md)은 check/gen/init/serve와 생성 SDK를 연결하고 소스와 로컬 설치형 SDK 각각에서 두 Id 후보의 실제 호출·정의 변경·재기동을 검증했다. 공식 READ 확장도 생성 타입과 HTTP·Node/Python worker를 연결해8개 SDK/wire/lang 흐름을 실행했다. 후속 HTTP 요청 수명·캐시 전 read decoder·관계 대상 집계를 보완했다. WRITE도 같은 생성 SDK·공개 전이·멱등 저장소/pending에 연결해 실제8개 조합을 실행했다. 후속 DB 소유권·SQL 기한·catalog 구조 비교·SDK pending 보존·커밋 이후 세션 만료 결과를 검증했으며 정의 입력·cache 반환·CLI 출력 경계까지 보완했고 이전 단계 Rust175개(V1 포함)·SDK47개는 [DB 결과](../prototype/README.md#정의-입력캐시-완료cli-출력-후속-결과)를 따른다. 최신 Chrome8개·재배치 bundle8개는 WRITE 손실 복구·재생·권한 거부를 포함하며 [후속 결과](../prototype/README.md#write-공식-확장의-실행-결과)를 따른다. 최신 [V15 결과](alignment/V15-filter-value-results.md)는 Url·Enum·Time read/where와 리터럴 접두어 실행 범위를 연결한다. [V14 결과](alignment/V14-typed-apply-results.md)는 읽기 없는 공개 전이까지 타입을 생성하고 응답 Id 검사·복구·결과 불변을 연결한다. [V13 결과](alignment/V13-id-boundary-results.md)는 큰 Id 왕복·쓰기 원자성·모드별 멱등 재생·계약 사전 거부를 비교한다. [V12 결과](alignment/V12-typed-transport-results.md)는 앱별 생성 타입과 HTTP·캐시·세션 경로를 연결하고 계약 불일치를 거부한다. [V11 결과](alignment/V11-optional-checks-results.md)는 선택적 개발 조언을 꺼도 필수 타입·권한·비용 검사가 유지되는지 검증한다. [V10](alignment/V10-session-recovery-results.md)은 같은 principal의 토큰 교체·미확정 쓰기 복구·멱등 scope 분리를 실제 HTTP/DB로 검증했다. [V9](alignment/V9-cache-lifetime-results.md)의 세션·시간·캐시 수명 검증을 이어간다.

## 읽는 순서

| 순서 | 문서 | 내용 |
|---|---|---|
| 1 | `00-rules.md` | 이 문서 세트를 쓰는 규칙, 문서 상태, 승인이 필요한 결정의 종류 |
| 2 | `01-purpose.md` | AIP가 왜 있는가. 어떤 구조적 문제를 풀려는가. 성공을 무엇으로 재는가 |
| 3 | `02-principles.md` | 원칙, 설계 판단 우선순위 P0~P3, 설계 관문 |
| 4 | `03-glossary.md` | 용어 정의 초안 |
| 5 | `04-current-state.md` | 기존 구현이 실제로 무엇인지, 원칙과 어디서 어긋나는지, 무엇을 재사용할 수 있는지 |
| 6 | `05-agenda-status.md` | 기존 14개 의제의 재분류와 topics 문서 대응 |
| 7 | `topics/T01~T13` | 의제별 설계 문서 (의제 전체 조망) |
| 7-1 | `decisions/` | 주요 의사결정 하나에 문서 하나. A~G 형식. 목록은 `decisions/README.md` |
| 8 | `90-open-questions.md` | 창시자가 결정할 질문만 모은 목록 |
| 9 | `91-roadmap.md` | 설계 의존 관계와 진행 순서 |

## topics 목록

| 문서 | 주제 | 원문 의제 | 창시자 결정 필요 |
|---|---|---|---|
| `topics/T01-caller-model.md` | 호출자 표현 모델 (읽기·쓰기 조합 범위) | 7, 8, PART VII §13 | 예 |
| `topics/T02-authoring-model.md` | 문법과 생태계 (`.aip`, TS/Python, 의미 모델) | 1, PART VII §14 | 예 |
| `topics/T03-engine-and-runtime.md` | Rust 엔진 역할, 실행 형태 | 9, PART VII §15 | 예 |
| `topics/T04-natural-language.md` | 선택적 구조화 설명 metadata | 통합 지침 §2·6·7 | 목적 답변됨. 키·소속·공개 상세는 기술 검증 |
| `topics/T05-spr.md` | SPR 경계와 책임 | PART I §7 | 예 |
| `topics/T06-protocol.md` | Application Intent Protocol | 10 | 일부 |
| `topics/T07-security.md` | 인증·인가, 위협 모델 | 5, PART I §2 | 일부 |
| `topics/T08-ir.md` | 공통 의미 모델(IR) | 2 | 검토 |
| `topics/T09-logic-and-extension.md` | 비즈니스 로직, Escape Hatch, Extension ABI | 3, 4, 12 | 일부 |
| `topics/T10-data-layer.md` | DB 추상화, Query Planner, CRUD 범위 | 6, 7, 8 | 예 (DB 범위) |
| `topics/T11-transaction-effects.md` | 트랜잭션과 부작용 | 13 | 검토 |
| `topics/T12-migration.md` | 마이그레이션 | 11 | 검토 |
| `topics/T13-frontend-sync.md` | 프론트엔드 캐시와 동기화 | 14 | 검토 |

## 모든 topics 문서의 공통 틀

1. 상태와 기준 원문 위치
2. 설계 관문 8문항에 대한 답 (아직 답할 수 없으면 "답할 수 없음: 이유")
3. 이 의제가 존재 목적(P0)에 어떻게 연결되는가
4. 현재 구현 사실 (파일 근거)
5. 선택지 비교 (최소 2개)
6. 지켜야 할 불변식
7. 추천 (있으면). 추천은 결정이 아니다
8. 창시자 확인 사항, 다른 topics와의 의존 관계
