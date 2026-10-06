# 10. 철학 정합성 검토 (2026-10-01)

> 2026-10-02: 이 검토의 기준(`philosophy_handoff_2026-10-01.md`)은 창시자가 원래 의도에서 흐려졌다고 판단한 문서다. 이후 기준은 `docs/PRINCIPLES.md`, 재검토 결과는 `11-founder-intent-audit.md`. 특히 §8 질문 1의 선택지 (a)(`.aip` 정본)는 기각됐다(D-R6).

> 상태: 검토 보고. 기준 문서는 `docs/origin/philosophy_handoff_2026-10-01.md`(이하 "철학 문서", §번호는 그 문서의 절). 결정 상태는 `docs/DECISIONS.md`가 정본이다.

## 1. 현재 구현 상태

| 영역 | 상태 | 근거 |
|---|---|---|
| 작성 언어 | 독자 텍스트 언어 `.aip` 하나. 문법 정본 `spec/grammar.md` | `crates/aip-syntax` |
| 의미 분석 | 이름 해석, 타입, 정적 규칙(E1xx~E5xx, W4xx~W5xx). AST 위에서 동작 | `crates/aip-sema` |
| Semantic IR | **없었다.** 설계(03)에는 Core IR이 있지만 구현은 AST에서 바로 PostgreSQL 실행 계획으로 갔다. 이번 회차에 `crates/aip-ir`(Core IR v0)를 새로 만들었다 | §3.1 |
| 실행 계획 | SQL을 컴파일 시점에 고정한 계획. 기존 이름 `aip-ir`을 실제 역할대로 `aip-plan`으로 바꿨다 | `crates/aip-plan`, `crates/aip-pg` |
| 런타임 | axum HTTP, 트랜잭션, 잠금 순서, outbox, 멱등, 타이머, job, webhook, rule | `crates/aip-runtime` |
| Presentation | `/aip/describe` 계약, TS 클라이언트 생성 | `aip-plan/src/contract.rs`, `tsgen.rs` |
| 실증 | 아리아리(70+ intent), Shop 두 도메인을 실제 PostgreSQL에서 e2e | `crates/aip-cli/tests` |

## 2. 철학과의 일치 여부

| 철학 문서 | 판정 | 설명 |
|---|---|---|
| §1 의도 선언, 런타임이 검증·실행 | 일치 | 권한, 상태 전이, 불변식, 멱등, 효과 등급을 선언하고 런타임이 강제한다 |
| §2.1 AI-native, §2.2 하나의 정규 표현 | 대체로 일치 | 문법이 표현을 하나로 묶고(`02-language.md`), 진단이 코드·도움말을 낸다. 단 "비표준 표현 → 표준 표현 제안"은 일부 진단에만 있다 |
| §2.3 표현 제한, 능력 보존 | 일치(검증 진행 중) | 커버리지 카탈로그 203건, 아리아리 전체 포팅 |
| §3 SPR | 부분 일치 | Specification(`.aip`)과 Runtime은 분리돼 있다. 그러나 Presentation 산출물(계약, TS 타입)이 **실행 계획에서** 파생된다(§3.2) |
| §4 공통 Semantic IR | **불일치 → 이번 회차에 착수** | §3.1 |
| §4.2 TypeScript DSL 우선 | **충돌** | 현재 작성 언어는 `.aip`. §5 질문 1 |
| §4.4 IR이 특정 DB에 종속되지 않음 | 불일치였음 | 옛 `aip-ir`은 SQL 문자열을 담은 실행 계획이었다. 이름을 `aip-plan`으로 바로잡고, 새 `aip-ir`은 SQL·HTTP·Rust 객체를 담지 않는다 |
| §5 Primitive 미확정 | 주의 | 문법에 L3 형식 다수(approval, job, grant link…)가 이미 있다. Primitive 집합을 "확정"한 것은 아니며 03 §10.1이 L3는 Core로 하강해야 한다고 정해 두었다. 구현은 아직 하강하지 않는다(§3.3) |
| §6 타입 시스템 | 부분 불일치 | Missing/Null 미구분, Decimal/Money가 wire에서 float(§3.4, §3.5) |
| §7 Extension·Escape hatch | 일치(미완) | 1st-party 외 extension은 W502. `fn … wasm`은 문법만 있고 실행하지 않는다. 단계(Pure/Effect-Declared/Unsafe)는 미정 |
| §8 서버 최종 권한 | 일치 | 클라이언트는 intent 이름과 입력만 보낸다. 실행 계획·권한 식은 서버의 컴파일 결과에서만 온다. 공개 계약에 정책 식은 싣지 않는다 |
| §9 실행 의미 | 일치 | 효과 등급 local/staged/reservable/deferred/observational, outbox at-least-once, 외부 효과를 트랜잭션 원자성으로 가장하지 않음(00 전략 "분산 부작용의 거짓말"). `on failure` 보상 실행은 미구현 |
| §10 프로토콜 | 부분 일치 | 계약에 `transport` 절이 HTTP 경로를 박아 둔다. 프로토콜 버전이 IR 버전 문자열을 그대로 쓴다(§3.6) |
| §11 AI 친화 | 부분 | `check --json` 구조화 진단, `/aip/describe`. `spec/diagnostics.md`는 2026-10-02 단일 레지스트리(`aip-ir/src/codes.rs`)에서 생성하고 `aip explain-code`로 조회한다 |
| §12.4 언어 독립성 검증 | 불가능했음 | 두 번째 작성 언어가 없고 IR도 없었다. Core IR이 생기면서 "같은 의미 ⇒ 같은 digest" 검사가 가능해진다 |

## 3. 구조적 문제

### 3.1 Semantic IR 부재 (가장 큼)
`aip-pg::compile`이 `aip_syntax::ast`와 `aip_sema::Analysis`를 직접 읽는다. 결과적으로
- 의미가 `.aip` 문법과 PostgreSQL 백엔드 사이에서만 존재하고, 다른 작성 언어(TS, Python)가 들어올 자리가 없다.
- 정적 분석(`check.rs`)이 AST에 묶여 있어, 다른 프런트엔드가 만든 프로그램은 검증할 수 없다.
- 03 설계 문서와 구현이 어긋나 있었는데 문서는 그 사실을 적지 않았다.

조치(이번 회차): `crates/aip-ir` Core IR v0 정의(위치 정보 없음, 이름 해석 완료, 설탕 제거, 결정적 직렬화, `digest`), `.aip` → Core IR 하강(`aip-sema/src/to_core.rs`), 프런트엔드 독립 구조 검증(`aip-ir/src/validate.rs`), `aip core` CLI.
(S3, 2026-10-02 완료) `aip-pg`가 AST 대신 Core IR만 소비한다. 실행 계획 골든 바이트 동일로 검증(09 Pass 6).
(S2, 2026-10-02 완료) 의미 규칙 45곳을 `aip-ir/src/analyze.rs`로 옮겼다. IR JSON만으로 같은 진단이 나옴을 conformance로 검증.
남은 단계: (S4) 두 번째 프런트엔드로 언어 독립성 실증.

### 3.2 Presentation이 실행 계획에서 파생된다
`contract.rs`, `tsgen.rs`가 `aip-plan::Program`(SQL을 가진 실행 계획)에서 계약과 TS 타입을 만든다. 철학 §4.1은 Presentation 타입을 공통 의미 모델에서 파생하라고 한다. 지금은 실행 계획의 `output`(JSON 값)과 `ParamSpec`이 사실상 공개 계약을 겸한다. S3 이후 계약 생성기를 Core IR 위로 옮긴다. 그 전까지는 Core IR의 `Shape`와 계약의 `output`이 같은지 테스트로 묶는 것이 다음 안전장치다.

### 3.3 L3 형식이 Core로 하강하지 않는다
03 §10.1은 L3 형식(approval, job, …)이 Core IR에 고유 노드를 갖지 않는다고 정했다. 구현은 `plan/approval.rs` 등에서 L3를 곧장 SQL과 내부 테이블(`_aip_approval`)로 바꾼다. Core IR v0는 이 형식들을 `forms`(과도기 표시)로 담는다. 철학 §5("기존 계층을 이름만 바꿔 재현하지 마라", "Primitive 수를 줄이려고 의미를 억지로 통합하지 마라") 사이에서 L3를 어디까지 Core로 내릴지는 Open이다(OI-IR2).

### 3.4 Missing과 Null을 구분하지 않는다
`validate.rs`가 `obj.get(name).unwrap_or(Null)`로 부재와 `null`을 같게 다룬다. 부분 수정 명령에서 "안 보냄 = 그대로 둠"과 "null = 지움"을 표현할 수 없다. 철학 §6이 명시적으로 요구하는 구분이다. 타입 의미 결정이라 Open(OI-T1)으로 두고, Core IR은 지금 `optional` 하나로 표현하며 그 의미를 주석에 적었다.

### 3.5 Decimal/Money가 wire에서 부동소수점이다
- 입력 검증이 문자열을 `f64`로 파싱해 받아들인다. `"NaN"`, `"inf"`, `"1e400"`이 통과하고 PostgreSQL `numeric`은 `NaN`과 `Infinity`를 받는다. 금액이 NaN이 될 수 있다는 뜻이다(결함).
- 생성 TS 클라이언트가 `decimal`, `money`를 `number`로 타입한다. 정밀도 손실을 클라이언트 쪽 타입이 허용한다.
- 응답도 `serde_json::Value`(f64)를 거친다(확인 필요: `arbitrary_precision` 미사용).
입력 쪽 NaN/Infinity/지수 표기 거부는 의미 결정이 아니라 결함 수정이라 바로 고친다. wire 표현을 문자열로 바꾸는 것은 프로토콜 변경이라 Proposed(D-P4)로 기록하고 함께 구현한다.

### 3.2a 조치(2026-10-02)
`crates/aip-contract`가 Core IR에서 계약·TS를 만든다. 골든 바이트 동일로 검증. §3.2는 해소.

### 3.6 프로토콜 의미와 전송이 섞여 있다
계약(`/aip/describe`)의 `transport` 절이 `POST /aip/{intent}` 같은 HTTP 세부를 의미 계약 안에 둔다. 프로토콜 버전도 따로 없이 IR 버전 문자열을 쓴다. 철학 §10에 따라 "의미 계약(protocol)"과 "바인딩(HTTP, WebSocket)"을 나누는 것이 맞다. 우선순위는 Core IR 뒤.

### 3.7 그 밖
- (해소 2026-10-02) 계약의 `webhooks[].signature.secret_env`가 비밀 이름을 공개 계약에 실었다. 이제 `aip contract --operator`로만 나온다. 프로토콜 버전도 `aip-protocol/0.1`로 분리했다. `transport` 절의 HTTP 세부는 아직 계약 안에 있다(OI-P1).
- `fn … wasm`(Pure Custom) 경로는 문법만 있고 실행되지 않는다. 확장 단계 모델이 정해지기 전까지 E602로 막아야 하는지 확인 필요.

## 4. 유지할 설계
- 컴파일 시점 SQL 고정과 `aip explain`(실행 계획이 읽을 수 있는 산출물).
- 효과 일관성 등급과 outbox. "증명할 수 없는 것은 계약에 적는다"(00 전략).
- 가시성 기반 NOT_FOUND, 잠금 전역 순서, 멱등 키, 버전 충돌 409.
- 문법이 표현을 하나로 강제하는 방식(`!=` 하나, 커서 파라미터 금지 등 SC-1~5).
- Rust 단일 바이너리 런타임(ADR-001). 철학 문서는 런타임 구현 언어를 정하지 않는다. TS/Python은 **작성 언어**(프런트엔드) 문제다.

## 5. 수정이 필요한 설계
1. Core IR을 파이프라인 중심에 둔다: `.aip`/TS/Python → Core IR → 검증 → 실행 계획(§3.1).
2. 계약·클라이언트 생성을 Core IR에서(§3.2).
3. Decimal/Money wire 표현(§3.5).
4. Missing/Null 의미(§3.4) — 결정 후.
5. 프로토콜 의미/전송 분리, 프로토콜 버전(§3.6).

## 6. 지금 결정하지 않을 것
철학 문서 §14의 미확정 목록을 그대로 유지한다. 특히 TS DSL의 구체 문법, Primitive 최소 집합, Effect System 표현, Custom Extension 단계, 분산 작업 보장 범위, 전송 방식. Core IR v0의 노드 목록은 **현재 `.aip`가 표현하는 것의 정규형**일 뿐 Primitive 집합의 확정이 아니다.

## 7. 다음 구현 단계(우선순위)
| 순서 | 단계 | 검증 |
|---|---|---|
| 1 | Core IR v0 + `.aip` 하강 + 구조 검증 + `aip core` — 완료 | 두 예제 진단 0, 왕복, 결정성, 공백·주석 무관 digest |
| 2 | Decimal/Money 결함 수정 — 완료 | NaN/Infinity/지수 거부 테스트, 정밀 금액 e2e |
| 3 | 계약·TS 생성기를 Core IR로 이전 — 완료(`aip-contract`) | 두 예제 전 intent 비교 |
| 4 | `aip-pg`가 Core IR 소비(S3) — 완료 | 실행 계획 골든 바이트 동일, e2e 통과 |
| 5 | 두 번째 프런트엔드(S4): 형태는 사용자 결정 후 | 같은 도메인을 두 언어로 써서 digest 동일 |
| 6 | 정적 규칙을 Core IR 위로(S2), `spec/diagnostics.md` — 완료 | conformance 24건이 IR 입력으로도 같은 코드 |

## 8. 사용자 판단이 필요한 질문
1. **작성 언어**: 철학 문서는 "초기 구현은 TypeScript DSL을 우선 검토"라고 하고, 지금까지 만든 것은 독자 언어 `.aip`다. 선택지:
   - (a) `.aip`를 정본 작성 언어로 유지, TS·Python은 Core IR을 만드는 내장 DSL(빌더)로 추가. 장점: 문법이 표현을 하나로 강제하는 힘이 가장 크다(AI-native §2.2). 단점: 새 언어 학습·에디터 지원 비용.
   - (b) TS 내장 DSL을 정본으로, `.aip`는 보조. 장점: 생태계·IDE·타입체커를 그대로 씀. 단점: TS의 표현 자유(임의 함수, 루프)를 막기 어렵고 "하나의 정규 표현"을 지키려면 별도 린트가 필요.
   - (c) 둘 다 1급, Core IR만 정본. 어느 쪽이든 Core IR이 먼저라 지금 작업은 선택과 무관하게 유효하다.
2. **Missing vs Null**(OI-T1): 부분 수정 명령에서 "안 보냄"과 `null`을 구분할지, 구분한다면 문법에 어떻게 드러낼지(예: `Patch<T>` 같은 별도 타입 vs 필드별 `clearable`).
3. **Decimal/Money wire 표현**(D-P4): 문자열(`"12000.50"`) 고정을 제안한다. 숫자도 함께 받을지.
4. **L3 형식의 Core 하강 범위**(OI-IR2): approval·job 같은 형식을 Core 엔티티+intent로 완전히 내릴지, Core IR에 1급 형식으로 남길지.
