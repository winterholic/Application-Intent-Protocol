# 11. 창시자 의도 정합성 감사 (2026-10-02)

> 상태: 감사 보고. 코드는 수정하지 않았다.
> 기준: 사용자가 2026-10-02에 다시 정리한 원칙표(아래 §0). 이전 기준 문서들(`docs/origin/design_draft.md`, `full_context_handoff.md`, `philosophy_handoff_2026-10-01.md`)은 모두 GPT와의 대화를 정리한 것이며, 그 과정에서 원래 의도가 일부 흐려졌다는 것이 사용자의 판단이다.
> 검증 못함: 사용자가 처음 제시한 "7가지"의 원문은 저장소에 없다. 이 감사는 §0 표를 원래 의도의 최선 근사로 사용한다.

## 0. 감사 기준 (사용자 정정본)

| 구분 | 절대 잃으면 안 되는 원칙 |
|---|---|
| 존재 목적 | 백엔드 개별 개발과 유지보수의 극단적 최소화 |
| 작동 모델 | 호출자가 원하는 데이터·동작을 표현 |
| 보안 | 서버가 최종 권한과 실행을 통제 |
| 프로토콜 | Application Intent Protocol |
| 아키텍처 | SPR |
| 구현 | Rust 핵심 엔진, JS/Python 생태계 |
| AI 철학 | 최대한 많은 반복 구현을 표준 문법·기능으로 흡수 |
| 표현 방식 | 자연어의 구조화·문법화, 단순 주석과 구별 |
| 확장 | 표준 범위 밖에서는 JS/Python을 자연스럽게 활용 |

핵심 문장: "서버는 안전한 실행 환경을 제공하고, 프론트엔드에서 필요한 데이터와 동작을 표현하면 백엔드에 매번 API를 추가하지 않고도 처리할 수 있어야 한다." 그리고 "요청의 자유와 실행 권한을 분리한다."

## A. 현재 AIP 아키텍처 (코드 기준)

```
.aip 텍스트 (독자 언어)
  → aip-syntax  파서 2.6천 줄
  → aip-sema    이름·타입 해석(F 규칙), to_core
  → aip-ir      Core Semantic IR + validate + analyze(S 규칙) + diff + 진단 레지스트리
  → aip-pg      PostgreSQL DDL + SQL을 컴파일 시점에 고정한 실행 계획
  → aip-runtime Rust 단일 바이너리: HTTP POST /aip/{intent}, WebSocket 구독, 트랜잭션, outbox, job, 타이머, 웹훅
  → aip-contract 공개 계약(/aip/describe), 생성 TS 클라이언트
```

- 실행할 수 있는 것: `.aip`에 **서버 쪽에서 미리 선언된** query/command와 L3 형식(approval, job, grant link, verification, webhook, rule, tenant, publishable, consent, impersonate, search, outbound webhooks, subscribe, migration). 네 예제(ariari, shop, saas, cms)가 실제 PostgreSQL에서 e2e로 돈다. `cargo test -q --workspace` → passed 182 failed 0 (2026-10-02 감사 시점).
- 호출자(클라이언트)가 할 수 있는 것: 선언된 intent 이름 + 입력 값을 보내는 것뿐이다. 조회 필드·조건·정렬은 서버 선언(`select`, `where`, `sort`)이 정한다. 생성 TS 클라이언트는 `aip.queries.X(input)` 형태의 타입 있는 호출 함수다.
- 개발자가 쓰는 백엔드 코드: Java/Spring 계층 코드는 없다. 대신 **기능마다 `.aip` 선언을 서버 쪽에 추가**한다. 아리아리 대조: 원본 Java 478파일 21,552줄(테스트 포함 여부 미구분, 확인 필요) ↔ `.aip` 1,075줄. 프런트 호출 코드는 생성(1,142줄 생성물).
- JS/Python: 저장소에 작성 코드 없음. TS는 생성된 클라이언트뿐이고 Python 관련 코드는 0.
- 확장: extension 효과는 런타임 `dispatch.rs`의 `match`에 하드코딩된 개발용 제공자다(`mail.*` → `_aip_mail` 테이블에 기록, `auth.*`/`export.*` → 로그만). Extension ABI 없음. `fn … wasm`은 파싱·IR까지만 가고 실행 경로 없음.
- CI 없음(`.github` 없음).

## B. 원칙별 정합성

| 원칙 | 현재 상태 | 근거 | 판정 |
|---|---|---|---|
| 존재 목적: 백엔드 개발·유지보수 최소화 | 계층 코드는 사라졌지만 새 화면·기능이 필요하면 서버 명세에 intent를 추가해야 한다. 유지보수 대상이 Java에서 `.aip`로 옮겨 갔다. 스키마 진화·계약 호환 검사·진단이 유지보수 비용을 낮춘다 | `examples/ariari/app.aip`, `aip-pg/src/evolve.rs`, `aip-contract/src/compat.rs` | **부분 일치**. 양은 크게 줄었으나(측정 M6 미완) "매번 API를 추가하지 않는다"는 아니다 |
| 작동 모델: 호출자가 원하는 데이터·동작을 표현 | 호출자는 서버가 이름 붙인 intent만 호출. 결정 D-P8 "클라이언트 임의 쿼리(GraphQL식 포함) 없음"이 이를 명시적으로 막는다 | `docs/DECISIONS.md` D-P8, `docs/design/00-strategy.md` §5, `aip-runtime/src/http.rs`, 생성 TS | **충돌 (가장 중대)** |
| 보안: 서버가 최종 권한·실행 통제 | 강하게 구현. allow/requires, 가시성 기반 NOT_FOUND, 필드 가시성, 테넌트 격리(트리거 고정), 대리 접속 매 호출 재확인, SSRF·DNS rebinding 차단, 멱등, 서명 토큰 | `aip-pg/src/plan.rs`, `ddl.rs`, `aip-runtime/src/engine.rs`, `outbound.rs`, e2e의 negative control | 일치 |
| 요청의 자유와 실행 권한의 분리 | 실행 권한은 서버에 있다. 그러나 요청의 자유가 거의 없다(이름+입력만). 분리가 아니라 자유 쪽을 없애는 방식으로 안전을 얻었다 | 위와 같음 | **불일치** |
| 프로토콜: Application Intent Protocol | HTTP/JSON + WebSocket, `/aip/describe` 기계 판독 계약, 프로토콜 버전 분리, 오류 레지스트리. Intent = 서버 사전 정의 capability 호출 | `aip-contract/src/lib.rs`, `spec/diagnostics.md` | 부분 일치. 형식은 갖췄으나 의미가 "이름 붙은 RPC"에 가깝다(`full_context_handoff.md` §0이 오해라고 경고한 "단순 RPC framework"와 겉모습이 비슷) |
| 아키텍처: SPR | Specification=`.aip`, Presentation=생성 TS 클라이언트, Runtime=Rust. 서로 내부 구현에 의존하지 않는다 | `aip-contract`가 IR만 의존 | 구조는 일치. Presentation이 "표현"하지 못하고 "호출"만 한다 |
| 구현: Rust 핵심 엔진 | Rust 단일 바이너리 런타임 | `crates/aip-runtime` | 일치(사용자 09-30 승인) |
| 구현: JS/Python 생태계 | 작성 수단이 독자 언어 `.aip`. JS/Python으로 정의하거나, JS/Python 앱에 임베드하는 경로 없음 | `crates/aip-syntax`, 저장소 내 JS/Python 작성 코드 0 | **충돌** |
| AI 철학: 반복 구현을 표준 문법·기능으로 흡수 | 매우 많은 반복 패턴을 형식으로 흡수(approval, job, tenant, versioned, search, 웹훅, outbox…). 한 의미 한 표현, 구조화 진단, `aip explain-code` | `docs/design/08-coverage-catalog.md`, `aip-ir/src/codes.rs` | 일치 |
| 표현 방식: 자연어의 구조화·문법화, 주석과 구별 | 해당 개념이 문법에 없다. `.aip`는 형식 언어이고 설명은 주석뿐이다 | `spec/grammar.md`에 doc/description 구문 없음 | **미반영** |
| 확장: 표준 밖은 JS/Python | 없음. `fn … wasm` 미실행, extension은 런타임 하드코딩 | `aip-runtime/src/dispatch.rs::effect`, `aip-ir/src/lib.rs` `FnBody::Wasm` | **미반영** |

## C. 중대한 설계 불일치 (중요도순)

1. **호출자가 원하는 데이터·동작을 표현할 수 없다.** 현재 모델에서 새 화면에 필요한 데이터가 생기면 서버 명세에 query를 하나 더 써야 한다. 이것은 AIP의 존재 이유("백엔드에 매번 API를 추가하지 않고도")와 정면으로 어긋난다. 원인: 기준 문서 자체가 이 방향을 굳혔다(`design_draft.md` §3 "CancelOrder는 서버에서 사전에 정의되고 검증된 capability", `full_context_handoff.md` §0 "GraphQL 비슷한 query language로 이해하면 잘못"). 내가 쓴 `00-strategy.md`와 D-P8이 이를 "하지 않을 것"으로 못 박았다.
2. **작성 수단이 새 프로그래밍 언어다.** 사용자의 의도는 JS/Python 생태계를 유지하면서 풍부한 정형 문법을 쓰는 것이었다. 09-30에 GPT 문서의 "독립 DSL"을 별도 언어·컴파일러로 해석해 구현했다. 파서·문법이 가장 큰 투자 중 하나다.
3. **표준 밖 확장 경로가 없다.** JS/Python으로 자연스럽게 빠져나갈 길이 없고, extension도 계약(ABI) 없이 런타임에 박혀 있다. 실제 서비스에서 표준이 못 덮는 1~2할을 처리할 수단이 비어 있다.
4. **자연어의 구조화 표현이 빠져 있다.** 원칙표에 있는 독립 항목인데 설계·구현 어디에도 없다.
5. **런타임 형태(Standalone)를 승인 없이 택했다.** `full_context_handoff.md` §16은 Standalone vs Embedded를 "아직 미결정"이라 했다. 지금 구현은 Standalone 단일 바이너리이고, JS/Python 임베드(바인딩)는 고려되지 않았다. 2·3번과 묶인 문제다.
6. **수단이 목적보다 앞서 갔다.** 10-01~02의 작업(Core IR, 진단 레지스트리, 스키마 진화, 암호화 등)은 품질은 높지만, 우선순위를 원칙표가 아니라 내 판단으로 정했다. 특히 IR은 "호출자의 Intent를 서버가 안전하게 실행하는 내부 도구"로 재평가돼야 한다.

## D. 잘 구현된 부분 (유지 권장)

- **서버 권한 강제 층 전체**: 정책, 가시성, 필드 가시성, 테넌트, 대리 접속, 멱등, 감사. 호출자에게 표현의 자유를 넓힐 때 오히려 이 층이 핵심 자산이 된다. 호출자가 조합한 요청도 같은 정책을 통과시키면 되기 때문이다.
- **Core IR과 그 위의 검증**(`aip-ir` validate/analyze, IR JSON만으로 같은 진단이 나온다는 테스트). 작성 언어와 의미를 분리해 두었기 때문에 C-2(JS/Python 작성)로 옮겨 갈 길이 열려 있다. 위치는 "내부 도구"로.
- **효과 등급과 outbox**(외부 효과를 트랜잭션 원자성으로 가장하지 않음), job·타이머·웹훅 내구성.
- **기계 판독 계약과 구조화 오류**(`/aip/describe`, 코드 레지스트리, `explain-code`), 생성 타입 클라이언트.
- **스키마 진화와 계약 호환 판정**: 유지보수 최소화 목표에 직접 기여.
- **검증 방식**: 골든 바이트 비교, 상시 negative control, 실제 PostgreSQL e2e.

## E. 승인 없이 결정된 것처럼 구현된 부분

| 항목 | 현재 구현 | 근거 문서상 상태 |
|---|---|---|
| 작성 언어 `.aip` | 독자 언어, 정본 | GPT 문서의 "독립 DSL" 해석. 사용자 정정과 충돌 |
| 클라이언트 임의 요청 금지 (D-P8) | 이름 붙은 intent만 | 사용자 정정과 충돌 |
| Standalone 런타임 | 단일 바이너리 | 원문 "미결정" |
| PostgreSQL 전용 (D-P6) | PG만, capability 검증 층 없음 | 원문은 DB별 capability 검증을 요구 |
| Core IR 스키마(aip-core/0.11~) | 구현·버전 관리 | 사용자 정정: IR은 재검토 대상 |
| SQL 컴파일 시점 고정 (D-P5) | 모든 SQL 사전 고정 | 호출자 조합 요청을 허용하면 재검토 필요 |
| 테넌트·publishable·consent·impersonate·구독·스키마 진화 문법과 의미 | 구현·테스트 | 모두 Proposed, 사용자 미확인 |
| 필드 암호화 | 10-02 진행 중 중단(한도), 테스트는 통과 | Proposed, 미확인 |

## F. 수정 제안 (기존 구현 최대 보존)

1. **"호출자 조합 요청" 층을 서버 권한 층 위에 추가한다.** 서버는 리소스(엔티티), 정책(allow, 가시성, 필드 가시성, 테넌트), 비용 상한(최대 깊이·행 수·허용 정렬/필터 필드)을 선언한다. 호출자는 그 범위 안에서 필요한 필드·관계·조건·정렬을 조합해 요청한다. 런타임은 요청을 IR로 받아 검증·계획·실행한다. 기존 정책 층과 IR 검증기를 그대로 재사용할 수 있다. 쓰기는 비즈니스 규칙이 담긴 command를 기본으로 두되, 단순 쓰기는 엔티티 정책(`expose` 계열)만으로 허용하는 범위를 정한다. 사전 고정 SQL(D-P5)은 "선언된 intent" 경로에 남기고, 조합 요청 경로에는 런타임 계획기(상한 포함)를 둔다.
2. **작성 언어를 JS/Python으로 옮긴다.** Core IR이 언어 독립이므로 TS/Python 정형 API(빌더 또는 데코레이터)가 같은 IR을 만들게 할 수 있다. `.aip`는 (a) 폐기, (b) 내부 정규 표기·디버그 출력, (c) 대안 프런트엔드 중 하나로 남길 수 있다.
3. **확장 경로**: JS/Python 함수를 효과 서명(입력·출력 타입, 효과 등급, 멱등성)과 함께 등록하는 계약을 만들고, 런타임이 서브프로세스나 임베드로 호출한다. 하드코딩된 extension도 같은 계약으로 옮긴다.
4. **자연어 구조화**: 의미를 확인한 뒤 설계(아래 G-4).
5. 이 결정들이 나기 전까지는 **새 L3 형식 추가를 멈추는 것**을 권한다. 지금 추가하는 문법은 C-2 결정에 따라 다시 써야 할 수 있다.

## G. 창시자에게 확인할 질문

1. **호출자가 무엇을 얼마나 표현할 수 있어야 하나?**
   - 현재: 서버가 선언한 intent 이름 + 입력만.
   - 선택지: (a) 읽기는 조합 가능(필드·관계·필터·정렬·페이지, 정책·상한 안에서), 쓰기는 선언된 command만 / (b) 읽기·단순 쓰기 모두 조합 가능, 복잡한 비즈니스 쓰기만 command / (c) 현재 유지.
   - 영향: (a)(b)는 런타임 계획기와 비용 상한이 필요하고, SQL 사전 고정(D-P5)은 선언 경로에만 남는다. 정책 층은 재사용된다.
   - 추천: (a)로 시작. 읽기 조합이 "매번 API 추가"의 대부분을 없애고, 쓰기는 비즈니스 규칙이 몰리는 곳이라 선언형 command가 안전하다.
2. **정의를 어디에 쓰나?**
   - 현재: 독자 언어 `.aip`.
   - 선택지: (a) TypeScript 정형 API(+Python) / (b) `.aip` 유지 + TS/Python 바인딩 / (c) 둘 다.
   - 영향: (a)는 파서 대신 TS/Python 프런트엔드를 만들고, Core IR·검증·런타임은 그대로 쓴다. `.aip` 예제 4개를 옮겨야 한다.
   - 추천: (a). 정정본의 "JS/Python 생태계 유지"와 가장 맞고, IR이 이미 언어 독립이다.
3. **런타임과 JS/Python 앱의 관계는?**
   - 선택지: (a) Standalone Rust 서버 + JS/Python은 정의·확장 코드만 제공 / (b) Rust 엔진을 Node/Python 패키지로 임베드(바인딩) / (c) 둘 다.
   - 영향: (b)는 확장 코드 호출이 자연스럽지만 배포 형태와 프로세스 경계가 바뀐다.
   - 추천: 질문 2의 답에 따라. 확장(JS/Python 함수 호출)이 자주 필요하면 (b) 쪽 가치가 크다.
4. **"자연어의 구조화·문법화, 단순 주석과 구별"이 구체적으로 무엇인가?**
   - 예: 각 intent·규칙에 붙는 구조화된 서술(목적, 전제, 결과)을 기계가 읽고 계약·문서·검증에 쓰는 것인지, 아니면 문법 자체를 자연어에 가깝게 만드는 것인지.
   - 이 답이 없으면 설계할 수 없다.
5. **DB 범위**: PostgreSQL 전용을 유지할지, DB별 capability 검증 층을 지금 설계에 넣을지.

## 부록: 14개 설계 의제 상태

| # | 의제 | 상태 | 근거 |
|---|---|---|---|
| 1 | 독립 DSL | 구현 진행 중, **철학과 충돌**(새 언어로 구현) | `crates/aip-syntax`, `spec/grammar.md` |
| 2 | Canonical Typed IR | 구현 진행 중(aip-core/0.11). 사용자 승인 없음, 재검토 대상 | `crates/aip-ir` |
| 3 | 사용자 정의 비즈니스 로직 | 구현 진행 중. 선언 문법(let, when, rule, require, 형식들)으로만, 범용 코드 없음 | `aip-ir/src/lib.rs` Stmt |
| 4 | JS/Python Escape Hatch | **미구현**. wasm fn 미실행 | `FnBody::Wasm` |
| 5 | 인증·인가 | 구현 완료·검증(HMAC 토큰, allow, 가시성, 테넌트, 대리). 실제 OIDC 제공자 없음 | `aip-runtime/src/auth.rs`, e2e |
| 6 | DB 추상화 | PG 전용(D-P6 Proposed). capability 검증 없음 | `aip-pg` |
| 7 | Query Planner | 구현 진행 중. 컴파일 시점 SQL 고정, 비용 기반 계획 없음 | `aip-pg/src/plan.rs` |
| 8 | CRUD 지원 범위 | 구현(`expose` 설탕). CRUD를 넘는 형식 다수 | `aip-sema/src/lower.rs` |
| 9 | Runtime 및 언어 연동 | Standalone만 구현, 연동 **미결정 상태에서 한쪽을 택함** | `aip-cli` `aip run` |
| 10 | 통신 프로토콜 | 구현 진행 중(HTTP/JSON, WebSocket, 계약, 버전). 호출자 표현 범위는 G-1에 달림 | `aip-runtime/src/http.rs`, `aip-contract` |
| 11 | Migration | 구현 완료·검증(IR 비교 스키마 진화, 데이터 migration) | `aip-pg/src/evolve.rs`, e2e |
| 12 | Extension ABI | **미구현**. 런타임 하드코딩 개발 제공자 | `aip-runtime/src/dispatch.rs` |
| 13 | Transaction·Side Effect | 구현 진행 중(트랜잭션, outbox, 효과 등급, 멱등). 보상(`on failure`) 미실행 | `aip-runtime` |
| 14 | Frontend Cache·동기화 | 일부(구독 WebSocket, 서버 캐시). 클라이언트 캐시·`@aip/react` 없음 | `aip-runtime` subscribe |
