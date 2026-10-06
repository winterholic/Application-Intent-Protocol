# A. 창시자 의도 반영표

> 기준: [통합 지침 원문](../sources/founder-integrated-directive-2026-10-03.md). [FOUNDER]는 제품 기준, 아래 구현 관찰은 2026-10-03 코드 읽기 결과다.
> 코드 실행·성능·공격 재현은 하지 않았다. 구현 경로 존재와 안전성 보장을 구분한다. 기존 테스트 수를 새 검증 결과로 인용하지 않는다.

## 1. 최초 7가지 원칙

| 원칙 | 현재 코드 근거 | 기존 설계 반영 | 판정과 수정 방향 |
|---|---|---|---|
| 1 백엔드 개별 개발 최소화 | [폼 계약](../../crates/aip-contract/src/forms.rs), [모집 정의](../../examples/ariari/app.aip)의 query 고정 select | DD-01·02의 capability 후보, 01-purpose | 부분 반영. 표준 폼은 반복을 흡수하지만 결과 모양은 서버 정의에 고정. [C](C-syntax-proposal.md) read/apply로 화면별 서버 선언 감소 검증 |
| 2 Application Intent Protocol·서버 최종 통제 | [HTTP](../../crates/aip-runtime/src/http.rs) 43·123행의 이름 경로, [엔진](../../crates/aip-runtime/src/engine.rs) 140·159행의 계획/입력 조회 | DD-01·03·08·14 | 정책 실행 경로는 있지만 조합 요청 의미가 없음. 선언 이름 호출만 허용하는 현 구조와 목적이 충돌. 기존 경로를 확장/업무 동작용으로 재사용할 수 있는지 비교 |
| 3 Rust와 JS/TS·Python | [서버 시작](../../crates/aip-runtime/src/lib.rs) 99행, [TS 생성](../../crates/aip-contract/src/tsgen.rs) 238·257행 | DD-04·06, T03 | Rust 서버·생성 TS는 존재. 현재 제품 crates에서 host 정의/공식 Node/Python worker 연결 경로 찾지 못함. 설치·진단·기동 연결 설계 필요 |
| 4 정형화·선택 최소화 | [AST](../../crates/aip-syntax/src/ast.rs), [IR](../../crates/aip-ir/src/lib.rs), [표준 폼](../../crates/aip-ir/src/forms.rs) | DD-04·08·13 | 부분 반영. 정형 모델은 있으나 작성 방식/선택 수의 실제 이점 미측정. 정확한 한 작성 흐름 후보를 비교하고 IR 자체를 목표로 두지 않음 |
| 5 선택적 구조화 설명 | AST 선언·IR Program/forms에 설명 metadata 필드 검색 결과 없음 | DD-05·T04는 자연어 의미를 창시자 질문으로 남김 | 현재 확인 범위에서 미반영. 의미는 이번 지침으로 확인됨. 선택적 소속·타입·공개 범위만 설계, 자연어 정책 판단 금지 |
| 6 SPR | [고정 실행 계획](../../crates/aip-plan/src/lib.rs), HTTP·tsgen 경로 | T05, DD-01·13 | 정의/실행 분리는 있으나 Presentation은 이름 호출 중심. 실제 요청 표현과 공개 계약 경계를 추가 검토. SPR로 crate 개수를 고정하지 않음 |
| 7 공식 host 확장 | [효과 dispatch](../../crates/aip-runtime/src/dispatch.rs) 223행의 mail/auth/export prefix | DD-01·06, T09 | 컴파일 허용 목록과 runtime provider를 구분해야 함. 제품 코드의 JS/TS·Python 공식 실행 경로 확인 못함. typed access·효과·자원·worker 수명 설계 |

metadata 조사 범위: aip-syntax lexer/parser/AST, aip-sema lowering, aip-ir lib/forms. host 실행 조사 범위: crates/tools 및 생성 TS, 별도 spikes. TS 초기 spike는 현재 Rust 제품의 공식 연동으로 세지 않는다.

## 2. 나머지 [FOUNDER] 항목 전체 대응

각 원문 구절을 아래 행으로 연결했다. 같은 원칙을 반복하는 §9 답은 기존 질문 번호에 맞춰 별도 행으로 남겼다.

| ID | 원문 항목 | 현재 코드·문서 반영 | 충돌/누락·이번 설계 처리 |
|---|---|---|---|
| FI-01 | §1.1 AI First·사람 사용성 | syntax 진단·표준 폼, [01-purpose](../01-purpose.md) | 정형성이 유지보수 감소를 입증하지는 않음. [D](D-development-experience.md)에서 AI 선택·수정 계층·도구 비용 함께 비교 |
| FI-02 | §1.2 이중 구현 최소화·서버 책임 | http/engine, DD-01·02·07 | 고정 query/TS 입력·결과가 화면 요구를 서버에 반복 기록. 열린 계약 안 변경과 신규 계약 변경을 분리 |
| FI-03 | §1.3 평가 질문 7개 | 기존 00-rules 관문·[02-principles](../02-principles.md) | 코드 양 외 권한·사람 검토·복잡성 이동까지 C/D/E 검증 조건에 반영 |
| FI-04 | §3.1 자체 포트 서버 | [CLI Run](../../crates/aip-cli/src/main.rs) 66행·[runtime run](../../crates/aip-runtime/src/lib.rs) | 서버 구조 자체는 목적과 일치. 독립 포트만으로 설치 완료/일반 기능 준비 경험을 충족하지 않음. 배포 바인딩·worker 기동 확인 필요 |
| FI-05 | §3.2 별도 공식 프론트 라이브러리 | tsgen, DD-13 | 이름 호출 생성기만 존재. read/apply 표현·타입 추론·실패/캐시 계약 추가 설계 |
| FI-06 | §3.3 필드·관계·filter·sort·쓰기·확장 표현 | query select/params, DD-01·02 | caller tree 없음. 표준 요청으로 server named query 고정 모양을 대체하는 후보 작성 |
| FI-07 | §4.1 풍부한 표준 기능 탐색 | IR forms·jobs·approval, examples | 모든 제어 흐름 승인으로 해석 금지. W0/W1/W2와 표준 카탈로그 후보를 실제 업무로 비교 |
| FI-08 | §4.2 엔진의 공통 구현·언어 경계 | Rust runtime + 하드코딩 dispatch | 전부 Rust 실행이라는 결론 없음. 조건·컬렉션은 bounded 후보, 특수 로직은 host 확장 |
| FI-09 | §4.3 확장 자연스러움·불필요 예외 승인 지양 | T09·DD-06 제안 | 새 확장마다 사람이 특별 승인해야 한다는 제안은 원문과 충돌. 계약/배포 정책과 구분 |
| FI-10 | §5.1 정형 문법·타입·명확한 소속 | lexer/parser/span·IR 구조 | 후보의 기호·scope·진단 위치·숨은 동작을 C EQ 기준에 포함 |
| FI-11 | §5.2 선언형 A 선호 | DD-04 A/E 후보 | A형 방향 확인. 기존 예시·파일 확장자를 확정 문법으로 승격하지 않음 |
| FI-12 | §5.4 자율 문법 검토 | 기존 reviews 기록 | 이번에는 Claude 토큰 소진으로 참여 불가. Codex 메인 설계+Luna high 독립 대조를 기록하며 Claude 합의로 표기하지 않음 |
| FI-13 | §6.1 설명 소속 | AST/IR 설명 필드 검색 결과 없음, DD-05 | docs는 후보 키. 안정 선언 식별자·source span으로 소속 보존 제안 |
| FI-14 | §6.2 설명 선택적 | DD-05 여러 효과 대안 | 설명 없음은 오류 아님. 제공된 값의 형식 오류만 별도 검증 가능 |
| FI-15 | §6.3 의미 검증 비필수·실행 분리 | DD-05 C 효과 후보 | 자연어→권한/코드 생성 후보는 기본 범위에서 제외. docs 변경이 실행 facts에 영향 없는지 비교 |
| FI-16 | §7.1 별도 선택적 개발 검사 | [CLI check/check-docs](../../crates/aip-cli/src/main.rs) 16·102·343행, [pipeline](../../crates/aip-sema/src/pipeline.rs) | 일부 CLI 존재. 반복 구현·복잡성·미사용 진단 전체는 미확인. 검증 계층 후보와 자동수정 범위 별도 설계 |
| FI-17 | §7.2 필수 안전 검사와 optional 검사 분리 | run의 compile_full·engine 입력 검증·선언 rate limit | 서버 시작/호출 검증 경로 존재. 일반 intent의 보편 비용·시간 상한은 확인 못함. optional lint 비실행과 안전 검사 비활성 연결 금지 |
| FI-18 | §8.1 자유로운 오픈소스·기여 | 현재 루트 목록에서 LICENSE/CONTRIBUTING 확인 못함 | 방향 확인, 저장소 공개·라이선스 선택은 이번 수행 아님. 수익 모델을 설계 목표에 끼워 넣지 않음 |
| FI-19 | §9 01·04 읽기 조합·공식 확장 | DD-01 F1/F4 | 목적 답변 반영. capability/집계·확장 경계 상세는 검증 |
| FI-20 | §9 05·06 단순 쓰기·풍부한 동작 | DD-02 | 표준 쓰기 확인. ‘모든 복잡 변경은 결과 이름만’으로 고정 금지. W1 검증 열어 둠 |
| FI-21 | §9 08·09·10 고유표현·선언형·양 생태계 | DD-04 | 독립 .aip/임베드 블록/호스트 선언 및 첫 출시 동등성 미정 유지 |
| FI-22 | §9 11·12·13 설명 의미·역할·실행 분리 | DD-05 | 의미 질문 답변됨. 구체 metadata key/schema 공개 범위만 기술 작업 |
| FI-23 | §9 16·17 자체 서버·확장 작성자 | DD-06 | 자체 서버+SDK 구조 답변됨. 자동 기동/임베딩/서드파티 생태계는 세부 검토 |
| FI-24 | §9 24 공식 프론트 호출 라이브러리 | DD-13 | 필수 제품 요소 확인. React/중립 코어 출시 순서는 미정 |

## 3. 기존 구현을 재사용할 조건

| 자산 | 제안 | 재사용 전 확인할 조건 |
|---|---|---|
| 서버 정책·테넌트·입력·불변식 처리 | 유지 후보 | 조합 요청의 모든 스캔/관계/집계/쓰기/확장에 동일 적용. 기존 이름 경로 테스트만으로 새 경로 보장 불가 |
| Rust 자체 서버·CLI | 유지 후보 | 일반 서버/컨테이너 바인딩·설정·worker startup·오류 전달. localhost bind를 원격 배포 가능 근거로 쓰지 않음 |
| forms·outbox·jobs·마이그레이션 | 유지 후보 | 실제 업무 의미·동시성·트랜잭션/비동기 효과·변경 호환성 검증 |
| Core IR·aip-plan·파서 | 비교 후보 | typed 요청/최소 계약 모델 대비 이점·변환 손실·진단 비용. 현재 구조가 있다고 전체 확정하지 않음 |
| 고정 SQL·tsgen | 부분 재사용 후보 | typed 호출/확장/저장된 조회 경로는 쓸 수 있음. caller 조합 API·public capability 타입 파생 별도 필요 |
| tests·골든·예제 | 자료/회귀 후보 | 원문과 충돌하는 골든을 새 설계의 정답으로 고정하지 않음. 기대값 재분류 후 실행 |

## 4. 검증 수준

이번 확인은 소스 경로와 선언 구조의 재검토다. engine이 입력을 검증한다는 관찰은 모든 권한·부작용·원자성 보장이 아니라 ‘그 코드 경로가 있다’는 뜻이다. 보편적인 비용 제한·host 확장·선택적 검사 전체의 구현 완료는 주장하지 않는다.

현재 서버는 컴파일된 intent SQL을 매개변수 바인딩으로 실행한다([exec.rs](../../crates/aip-runtime/src/exec.rs)). 런타임 전체에 SQL 생성이 전혀 없다는 설명은 부정확하다. counter/actor/outbound 경로의 동적 SQL과 호출자 query planner의 부재를 구분한다.

## 변경 이력

- 2026-10-03 최초 7원칙과 나머지 [FOUNDER] 구절을 현재 소스·DD·후속 검증으로 연결. 기존 코드 안전성 보장과 source inspection 구분.
