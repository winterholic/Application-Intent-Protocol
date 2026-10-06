# AIP 프로젝트 설계 철학 및 아키텍처 인계 프롬프트

> 출처: 사용자가 2026-10-01 GPT와의 대화에서 정리해 전달한 원문. 수정하지 않고 보존한다. 해석과 현재 구현 대조는 `docs/design/10-philosophy-alignment.md`, 결정 상태는 `docs/DECISIONS.md`.

## 0. 이 문서의 목적

이 문서는 AIP(Application Intent Protocol) 프로젝트의 핵심 철학과 아키텍처 방향을 전달하기 위한 설계 인계 문서다.

단순한 기능 요구사항이나 구현 지시서가 아니다.

**AIP가 무엇을 만들려는 프로젝트인지, 기존 웹 개발의 어떤 구조적 문제를 해결하려는지, 앞으로 모든 설계 결정에서 무엇을 우선해야 하는지를 지속적으로 인지시키는 것이 목적이다.**

이 문서를 읽었다고 해서 즉시 대규모 구현을 시작하지 마라.

먼저 현재 코드베이스와 기존 설계 문서를 검토하고, 이번에 전달하는 철학과 실제 구현이 일치하는지 분석하라.

기존 구현이 작동한다는 이유만으로 해당 구조를 정당화하지 마라.

반대로 새로운 철학과 다르다는 이유만으로 검토 없이 기존 코드를 폐기하지도 마라.

**프로젝트의 완성도와 장기적인 아키텍처 일관성을 단기적인 구현 속도보다 우선하라.**

---

# 1. AIP는 무엇인가?

AIP는 Application Intent Protocol의 약자다.

AIP는 다음 세 가지 성격을 동시에 가진다.

1. Backend Framework
2. Frontend Library
3. Communication Protocol

그러나 이 세 가지를 단순히 묶은 제품을 만들려는 것이 아니다.

AIP의 본질은 다음과 같다.

> 개발자가 애플리케이션의 의도와 행위를 명확한 규격으로 정의하면, 프레임워크가 해당 정의를 분석하고 검증하여 안전하게 실행하도록 만드는 새로운 애플리케이션 개발 모델.

기존 웹 개발에서는 하나의 비즈니스 기능을 구현하기 위해 여러 계층에서 비슷한 의미를 반복해서 표현한다.

예를 들어 주문 취소 기능 하나에도 다음 작업이 필요하다.

- 프론트엔드 API 호출 코드
- 요청 및 응답 타입 정의
- Controller
- DTO
- Validation
- Service
- Repository
- Authorization
- Transaction
- Error Handling
- API Documentation
- 관련 테스트

물론 각각의 계층에는 존재 이유가 있다.

AIP는 이러한 책임을 없애거나 무시하려는 것이 아니다.

**반복적인 구현 책임을 애플리케이션 개발자에게서 표준화된 프레임워크와 런타임으로 이동시키려는 것이다.**

AIP가 지향하는 개발 경험은 다음과 같다.

> Describe your application once.

개발자는 무엇을 수행할지 정의하고, AIP는 그것을 어떻게 검증하고 실행할지 책임진다.

단, 애플리케이션별 비즈니스 규칙의 정의와 변경 책임까지 자동으로 사라진다고 주장해서는 안 된다.

AIP는 백엔드 서버를 제거하려는 프로젝트가 아니다.

**애플리케이션마다 반복적으로 작성하고 유지보수하는 백엔드 구현 코드를 극단적으로 줄이려는 프로젝트다.**

---

# 2. 최상위 설계 철학

## 2.1 AI-Native by Design

AIP는 처음부터 AI 에이전트가 사용하는 환경을 중요한 설계 대상으로 삼는다.

기존 프로그래밍 언어와 프레임워크는 같은 기능을 구현하는 방법이 다양하다.

각 방식에는 역사적 배경과 장단점이 존재한다.

하지만 AI 에이전트에게 지나치게 많은 구현 선택지를 제공하면 다음 문제가 발생한다.

- 동일한 기능을 매번 다른 패턴으로 구현한다.
- 프레임워크의 권장 구조를 잘못 해석한다.
- 불필요한 추상화를 생성한다.
- 프로젝트 내부의 일관성이 무너진다.
- 수정할 때 기존 코드의 의도를 파악하기 어려워진다.
- 기능이 증가하면서 유지보수 비용이 커진다.

AIP는 이 문제를 줄이기 위해 의도적으로 표현의 자유도를 제한한다.

**AI가 더 많은 선택지를 갖도록 만드는 것이 아니라, 불필요한 선택 자체를 하지 않아도 되는 개발 환경을 지향한다.**

## 2.2 One Intent, One Canonical Expression

동일한 의미의 작업을 표현하는 여러 방법을 무분별하게 제공하지 않는다.

예를 들어 동일한 의미의 단일 엔티티 조회를 수행하기 위해 다음과 같은 별도 방식을 동시에 제공하는 것은 지양한다.

- findById
- getById
- findOne
- queryOne
- selectByPrimaryKey

가능한 한 하나의 정규화된 의미와 표현을 제공한다.

단, 비슷하게 보이는 작업이라도 실제 실행 의미, 일관성 보장, 동시성, 실패 처리 방식이 다르다면 서로 구분해야 한다.

**표현의 통일을 위해 의미의 정확성을 희생해서는 안 된다.**

또한 임의의 두 프로그램이 의미적으로 동일한지 일반적으로 판별할 수 있다고 가정해서는 안 된다.

Canonicalization은 AIP가 정의한 제한된 언어와 의미 체계 안에서 수행한다.

## 2.3 Constrain Expression, Preserve Capability

AIP의 중요한 설계 원칙이다.

> 표현의 자유도를 제한하되, 애플리케이션이 구현할 수 있는 기능의 범위는 최대한 넓힌다.

정형화된 문법을 제공한다는 이유로 현실적인 비즈니스 로직을 구현할 수 없게 만들어서는 안 된다.

다만 기능의 표현력을 높이기 위해 무분별하게 예외 문법이나 임의 실행 함수를 추가하는 것도 피해야 한다.

새로운 요구사항을 발견하면 먼저 기존 Primitive의 조합으로 표현할 수 있는지 검토한다.

불가능하다면 의미 체계의 확장이 필요한지 분석한다.

---

# 3. SPR Architecture

AIP가 지향하는 애플리케이션 아키텍처 모델의 잠정 명칭은 다음과 같다.

**SPR Architecture — Specification, Presentation, Runtime**

이 명칭은 현재 프로젝트 내부에서 사용하는 설계 명칭이다.

독창성이 검증된 새로운 학술적 패턴이라고 주장해서는 안 된다.

## 3.1 Specification

개발자가 애플리케이션의 의도와 행위를 정의하는 영역이다.

여기에는 다음과 같은 정보가 포함될 수 있다.

- Domain Model
- Entity
- Type
- Intent
- Input / Output
- Validation
- Authorization
- Business Constraints
- Effects
- Transaction Requirements
- Error Contract

Specification은 프론트엔드 코드도, 백엔드 구현 코드도 아니다.

**애플리케이션 자체의 의미를 정의하는 독립적인 계약이다.**

## 3.2 Presentation

Specification을 기반으로 사용자와 상호작용하는 영역이다.

대표적인 소비자는 다음과 같다.

- React
- Vue
- Next.js
- 모바일 애플리케이션
- CLI
- AI Agent

Presentation은 허용된 Intent를 발견하고 호출한다.

서버 내부의 데이터베이스 접근 방법이나 트랜잭션 구현을 알 필요가 없어야 한다.

또한 Presentation은 실행 권한의 최종 결정권자가 아니다.

## 3.3 Runtime

신뢰할 수 있는 서버 환경에서 Intent를 실행하는 영역이다.

Runtime은 다음 책임을 가진다.

- Authentication
- Authorization
- Input Validation
- Execution
- Data Access
- Transaction
- Concurrency Control
- External I/O
- Error Handling
- Observability

Runtime은 프론트엔드의 특정 UI 구현을 알 필요가 없어야 한다.

또한 클라이언트가 임의로 제출한 실행 규칙이나 권한 정책을 신뢰해서는 안 된다.

## 3.4 핵심 관계

Specification이 Presentation과 Runtime의 공통 의미 계약이다.

Presentation은 Specification으로부터 허용된 호출 인터페이스를 얻는다.

Runtime은 신뢰할 수 있는 Specification을 기반으로 요청을 검증하고 실행한다.

이것은 Presentation과 Runtime이 절대로 통신하지 않는다는 뜻이 아니다.

두 영역은 AIP Protocol을 통해 통신한다.

중요한 것은 **서로의 내부 구현에 의존하지 않고 공통 Specification의 계약을 따른다는 것**이다.

---

# 4. 공통 IR 아키텍처

다음 방향은 현재 확정된 설계 원칙이다.

**AIP는 언어 독립적인 공통 Semantic IR을 가진다.**

JavaScript/TypeScript와 Python 생태계를 주요 대상으로 고려한다.

하지만 각 언어별로 서로 다른 의미 체계를 만들지 않는다.

## 4.1 목표 구조

Application Definition
→ Language Frontend
→ Canonical Semantic IR
→ Validation / Static Analysis
→ Execution Planning
→ AIP Runtime

Presentation을 위한 타입과 클라이언트 인터페이스 역시 공통 의미 모델에서 파생한다.

## 4.2 작성 언어

초기 구현은 TypeScript DSL을 우선 검토한다.

Python은 중요한 확장 대상이다.

다만 Python을 초기부터 동시에 구현할 필요는 없다.

초기 TypeScript 구현을 진행하면서도 Python 지원이 불가능해지는 구조적 종속성은 피해야 한다.

JavaScript 단독 사용 시 TypeScript의 정적 타입 지원이 없다는 점도 고려해야 한다.

## 4.3 IR 단계

다음 개념을 구분한다.

### Source AST

원본 작성 언어의 구문 구조를 나타낸다.

### Semantic IR

애플리케이션의 의미를 언어 독립적으로 표현한다.

### Validated IR

타입 검사와 의미 검증을 통과한 표현이다.

### Execution Plan

실제 실행 환경에 맞게 구성된 계획이다.

이것들이 반드시 모두 별도 파일 형식이나 영구 저장 구조여야 한다는 뜻은 아니다.

논리적으로 구분해야 한다는 의미다.

## 4.4 주의사항

Semantic IR에 다음과 같은 특정 구현 기술의 개념을 직접 종속시키지 않는다.

- 특정 ORM
- 특정 웹 프레임워크
- 특정 데이터베이스 드라이버
- 특정 클라우드 서비스
- TypeScript 전용 실행 객체

또한 IR의 버전, 직렬화, 호환성, 결정적 생성, 검증 가능성을 고려해야 한다.

---

# 5. Semantic Primitive

이 영역은 아직 상세 설계가 확정되지 않았다.

임의로 Primitive 목록을 확정하지 마라.

현재 개발자의 의도는 다음과 같다.

> 애플리케이션의 다양한 비즈니스 행위를 정형화된 의미 단위로 표현하고, 각 작업이 다른 작업과 자연스럽고 엄밀하게 연결되도록 한다.

Primitive는 단순한 유틸리티 함수 모음이 아니다.

각 작업은 자신의 의미와 실행 조건을 명확히 표현할 수 있어야 한다.

검토해야 할 요소는 다음과 같다.

- 입력 타입
- 출력 타입
- 실행 전제조건
- 실행 후 보장
- 발생 가능한 Effect
- 실패 의미
- 작업 간 데이터 전달
- 트랜잭션과의 관계
- 동시성 제약
- 권한 요구사항

현재 검토 가능한 분류 후보는 다음과 같다.

1. Value / Computation
2. Control Flow
3. Effects
4. Constraints / Policies

이 분류는 확정된 표준이 아니다.

실제 백엔드 사례를 분석하면서 적절성을 검증해야 한다.

## 중요한 주의사항

Controller, Service, Repository와 같은 기존 계층을 이름만 바꿔 재현하지 마라.

AIP의 목표는 기존 계층을 DSL 형태로 다시 작성하게 만드는 것이 아니다.

가능한 한 적은 의미 단위로 폭넓은 애플리케이션 동작을 표현할 수 있어야 한다.

그러나 Primitive 수를 줄이기 위해 서로 다른 의미를 억지로 통합해서도 안 된다.

---

# 6. Type System

Type System의 상세 설계는 아직 미확정이다.

현재 유력한 방향은 다음과 같다.

**AIP가 독립적인 타입 의미 모델을 소유하고, 언어별 타입과 Runtime Validator를 해당 모델로부터 파생한다.**

초기 검토 대상은 다음과 같다.

- String
- Boolean
- Integer
- Float
- Decimal
- UUID
- DateTime
- Object
- Array
- Map
- Enum
- Union
- Optional
- Nullable
- Entity Reference
- Result / Error

특히 다음 문제를 신중하게 다뤄야 한다.

### Missing과 Null

JavaScript의 `undefined`, Python의 `None`, JSON의 `null`은 완전히 동일한 개념이 아니다.

AIP의 의미 모델에서는 값의 부재와 명시적인 null을 구분할 수 있어야 한다.

### Decimal과 Money

금액을 일반 부동소수점 숫자로 취급하면 정밀도 문제가 발생할 수 있다.

### 런타임 검증

TypeScript 타입 정보는 런타임에서 자동으로 보장되지 않는다.

따라서 정적 타입과 실제 요청 데이터의 검증을 분리해서 고려해야 한다.

### 언어 간 일관성

같은 Specification을 JavaScript와 Python에서 작성했을 때 동일한 의미로 해석되어야 한다.

단, 서로 다른 언어의 모든 표현을 무조건 허용할 필요는 없다.

AIP가 지원하는 명확한 표현 집합을 정의해야 한다.

---

# 7. Custom Extension과 Escape Hatch

이 부분은 AIP의 장기적인 완성도에 직접적인 영향을 준다.

**Custom Extension은 지원하되, 일반적인 개발의 주류가 되어서는 안 된다.**

AIP를 사용하는 개발자가 대부분의 기능을 직접 작성한 custom 코드로 구현해야 한다면, AIP는 기존 프레임워크와 크게 다르지 않은 결과물이 될 수 있다.

따라서 다음 원칙을 적용한다.

1. 기본 Primitive 조합으로 표현 가능한지 먼저 검토한다.
2. 표현할 수 없는 이유를 분석한다.
3. 반복되는 패턴은 표준 의미 모델로 흡수할 수 있는지 검토한다.
4. Custom Extension에는 가능한 한 명시적인 계약을 요구한다.
5. 내부를 분석할 수 없는 코드의 정적 보장 범위를 과장하지 않는다.

Pure Custom, Effect-Declared Custom, Unsafe Custom과 같은 단계적 확장 모델은 검토 후보로 남겨둔다.

아직 확정된 API는 아니다.

또한 Escape Hatch 사용 비율을 관찰하는 것은 유용할 수 있지만, 임의의 목표 비율을 성공 기준으로 확정하지 않는다.

**Custom을 적게 사용하는 것 자체보다 현실적인 기능을 표준 모델로 정확하게 표현할 수 있는지가 중요하다.**

---

# 8. Security와 Authority

AIP가 Presentation과 Runtime 사이에 공통 Specification을 둔다고 해서 클라이언트를 신뢰할 수 있는 것은 아니다.

다음 원칙은 반드시 유지한다.

- 서버가 최종 권한을 가진다.
- 클라이언트가 전달한 권한 규칙을 신뢰하지 않는다.
- 클라이언트가 전달한 임의 실행 계획을 신뢰하지 않는다.
- Secret은 클라이언트에 노출하지 않는다.
- 인증과 인가를 구분한다.
- 객체 단위 및 필드 단위 접근 통제를 고려한다.
- TOCTOU 및 동시성 문제를 고려한다.
- 중요한 불변조건은 서버 측에서 보장한다.

특히 클라이언트가 Specification의 일부를 조회할 수 있다는 사실과 서버가 사용하는 신뢰 가능한 Specification은 구분해야 한다.

---

# 9. Execution Semantics

AIP는 단순히 작업을 순서대로 실행하는 도구가 아니다.

각 작업의 의미와 실행 보장을 고려해야 한다.

주요 검토 대상은 다음과 같다.

- Transaction Boundary
- Isolation
- Concurrency
- Retry
- Idempotency
- Timeout
- Cancellation
- Event Delivery
- Compensation
- Partial Failure
- External Side Effects

예를 들어 외부 결제 API 호출은 일반적인 데이터베이스 트랜잭션으로 원자적 롤백을 보장할 수 없다.

따라서 모든 작업을 하나의 자동 트랜잭션으로 처리할 수 있다고 가정하지 마라.

Runtime의 자동화 수준은 명확하게 보장할 수 있는 범위 안에서 설계해야 한다.

---

# 10. AIP Protocol

AIP는 단순한 SDK 함수 호출 규격이 아니라 통신 프로토콜의 성격을 가진다.

따라서 다음 사항을 검토해야 한다.

- Intent Identification
- Request / Response Contract
- Error Contract
- Protocol Version
- Capability Discovery
- Schema Introspection
- Authentication Context
- Streaming
- Compatibility
- Client Generation

HTTP, WebSocket 등의 전송 수단을 어떻게 사용할지는 아직 확정하지 않는다.

프로토콜의 의미와 전송 기술을 가능한 한 분리해서 설계한다.

---

# 11. AI Agent 친화적인 개발 경험

AIP는 AI가 규격을 정확하게 이해하고 사용할 수 있어야 한다.

따라서 문서만 제공하는 것으로 충분하다고 가정하지 않는다.

다음과 같은 기능을 검토한다.

- 기계가 읽을 수 있는 Specification
- Capability Discovery
- Schema Introspection
- 구조화된 Compiler Diagnostics
- 안정적인 Error Code
- Canonical Example
- 자동 검증 도구
- 기계가 처리할 수 있는 수정 제안

예를 들어 AI가 비표준 표현을 작성했을 때 단순히 실패시키는 대신, 가능한 경우 어떤 규칙을 위반했고 어떤 표준 표현을 사용해야 하는지 명확하게 알려주는 것이다.

단, Compiler가 임의의 프로그램을 완벽하게 이해하거나 자동 수정할 수 있다고 가정하지 않는다.

**AI가 추측해야 하는 영역을 줄이고, 검증 가능한 계약을 늘리는 것이 목표다.**

---

# 12. 프로젝트 개발 원칙

이 프로젝트는 빠른 데모를 만들기 위한 프로젝트가 아니다.

장기적으로 높은 완성도와 일관성을 갖춘 범용 개발 기반을 만드는 것을 지향한다.

따라서 다음 원칙을 지켜라.

## 12.1 기존 코드를 먼저 이해하라

변경 전 현재 구조, 구현 상태, 테스트, 문서를 조사한다.

기존 설계와 새로운 철학이 충돌한다면 충돌 지점을 명확히 보고한다.

## 12.2 설계가 불명확하면 임의로 확정하지 마라

중요한 아키텍처 결정은 선택지와 트레이드오프를 제시한다.

단순 구현 세부사항까지 매번 사용자에게 묻지는 않되, 다음과 같은 결정은 임의로 확정하지 않는다.

- Semantic IR의 핵심 구조
- 타입 시스템의 의미
- Primitive의 의미
- 권한 모델
- 실행 보장
- 프로토콜 호환성
- 확장 메커니즘

## 12.3 구현 편의를 위해 철학을 훼손하지 마라

다음과 같은 방식은 특히 경계한다.

- 모든 동작을 임의 JavaScript 함수로 위임
- 기존 프레임워크를 얇게 감싼 뒤 AIP라고 명명
- 의미 모델 없이 API 생성 기능만 구현
- 정적 분석 없이 선언형 문법만 제공
- 클라이언트가 서버 실행 규칙을 결정
- 언어별로 다른 비즈니스 의미 구현
- Custom Extension으로 미지원 기능을 무조건 우회

## 12.4 검증 가능한 개발을 하라

각 구현 단위에 다음 항목을 포함한다.

- 정상 동작 테스트
- 잘못된 입력 테스트
- 타입 불일치 테스트
- 실패 및 복구 테스트
- 보안 경계 테스트
- 언어 독립성 검증
- 회귀 테스트

테스트가 통과했다고 해서 설계 철학까지 충족했다고 간주하지 않는다.

구현의 의미와 아키텍처 적합성을 별도로 검토한다.

## 12.5 불필요한 복잡성을 만들지 마라

완성도를 높인다는 이유로 과도한 추상화나 미래 기능을 모두 선구현하지 않는다.

대신 향후 확장이 가능하도록 명확한 계약과 경계를 마련한다.

---

# 13. 문서 관리 원칙

README는 외부 개발자가 AIP의 목적을 빠르게 이해하도록 작성한다.

세부 설계 철학과 아키텍처는 별도 Markdown 문서로 관리한다.

검토할 수 있는 문서 구성은 다음과 같다.

- PHILOSOPHY.md
- ARCHITECTURE.md
- SPECIFICATION.md
- IR.md
- TYPE_SYSTEM.md
- PRIMITIVES.md
- EFFECT_SYSTEM.md
- SECURITY.md
- RUNTIME.md
- PROTOCOL.md
- EXTENSIONS.md
- AI_NATIVE_DESIGN.md
- DECISIONS.md

이 목록은 권장 후보이며 반드시 모든 파일을 즉시 생성하라는 뜻은 아니다.

중복된 문서를 무분별하게 만들지 말고 기존 문서 구조와 통합하라.

특히 설계 결정 기록에는 다음 상태를 구분한다.

- Accepted: 사용자가 명시적으로 결정한 원칙
- Proposed: 검토 중인 설계안
- Open: 아직 해결되지 않은 문제
- Rejected: 검토 후 채택하지 않은 방식

확정되지 않은 제안을 확정된 사실처럼 기록하지 마라.

---

# 14. 현재 확정된 방향과 미확정 사항

## 확정된 방향

- AIP는 Backend Framework, Frontend Library, Communication Protocol의 성격을 가진다.
- 애플리케이션의 의도를 명확하게 선언하고 Runtime이 실행한다.
- 반복적인 백엔드 구현 및 유지보수 부담을 줄인다.
- SPR Architecture를 프로젝트의 아키텍처 모델로 검토·발전시킨다.
- 공통 Semantic IR을 중심으로 설계한다.
- JavaScript/TypeScript와 Python 생태계를 주요 대상으로 삼는다.
- AI가 해석하기 쉬운 정형화된 표현을 우선한다.
- 동일한 의미에 대한 불필요한 표현 다양성을 제한한다.
- Custom Extension은 가능해야 하지만 일반적인 구현 수단이 되어서는 안 된다.
- 철저한 검증과 장기적인 설계 일관성을 우선한다.

## 아직 확정되지 않은 사항

- TypeScript DSL의 구체적인 문법
- Python 작성 방식
- Canonical IR의 정확한 스키마
- Primitive의 종류와 최소 집합
- Type System의 세부 의미
- Effect System의 표현 방식
- Custom Extension의 단계 및 제약
- 실행 계획의 생성 시점
- 트랜잭션과 분산 작업의 보장 범위
- 프로토콜의 전송 방식
- 버전 관리 및 마이그레이션
- Capability Discovery의 구체적인 인터페이스
- 성능 최적화 전략

이 항목들은 설계 검토 대상으로 유지한다.

---

# 15. 이번 인계 이후 수행할 작업

즉시 대규모 코드를 생성하지 마라.

먼저 다음 순서로 진행하라.

1. 현재 AIP 코드베이스와 기존 문서를 조사한다.
2. 이번 문서에서 제시한 철학과 기존 설계의 일치 여부를 분석한다.
3. 기존 구현에서 아키텍처적으로 위험한 부분을 식별한다.
4. 확정된 설계 원칙과 미확정 사항을 구분한다.
5. 공통 Semantic IR 중심의 아키텍처 개선 방향을 제안한다.
6. 구현 우선순위와 검증 전략을 수립한다.
7. 중요한 미확정 결정은 사용자에게 검토를 요청한다.

최종 보고에는 반드시 다음 내용을 포함한다.

- 현재 구현 상태
- AIP 철학과의 일치 여부
- 발견한 구조적 문제
- 유지할 설계
- 수정이 필요한 설계
- 아직 결정하지 않아야 할 사항
- 다음 구현 단계
- 사용자 판단이 필요한 질문

---

# 16. 가장 중요한 마지막 지시

AIP를 단순한 CRUD 자동 생성기, 새로운 ORM, RPC 라이브러리, 백엔드 보일러플레이트 생성기로 축소해서 이해하지 마라.

AIP가 추구하는 것은 더 근본적인 변화다.

**애플리케이션의 의미를 하나의 표준화된 계약으로 정의하고, Presentation과 Runtime이 해당 계약을 기반으로 동작하도록 만드는 것.**

그리고 이 구조를 통해 기존 웹 개발에서 반복되는 구현 노동과 유지보수 부담을 크게 줄이는 것이다.

AIP는 사람이 사용하기 편리한 것뿐 아니라 AI가 일관되고 정확하게 이해할 수 있는 구조를 지향한다.

앞으로 설계하거나 구현하는 모든 기능에 대해 다음 질문을 반복하라.

1. 이 기능은 AIP의 핵심 철학에 부합하는가?
2. 같은 의미를 표현하는 불필요한 방법을 추가하고 있지 않은가?
3. AI가 명세만으로 올바른 사용법을 이해할 수 있는가?
4. 공통 Semantic IR의 의미를 훼손하지 않는가?
5. 특정 언어나 프레임워크에 불필요하게 종속되지 않는가?
6. 기존 백엔드 개발의 반복적인 구현 책임을 실제로 줄이는가?
7. 예외적인 Custom Code를 불필요하게 증가시키지 않는가?
8. 타입, 권한, 실행 효과와 실패 조건을 검증할 수 있는가?
9. 실제 서비스에서도 신뢰할 수 있는 실행 보장을 제공하는가?
10. 지금 선택한 구조가 미래의 확장과 유지보수에 어떤 영향을 주는가?

**빠르게 동작하는 결과물을 만드는 것보다, AIP가 지향하는 개발 패러다임을 정확하게 구현하는 것이 중요하다.**

이 철학을 지속적으로 유지하면서 신중하고 일관되게 개발하라.
