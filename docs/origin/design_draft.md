# AIP (Application Intent Protocol)

> 2026-10-02: 원문 보존. 이 문서의 "클라이언트는 서버에 사전 정의된 Intent만 요청" 해석과 독립 언어 전제는 창시자 원칙(`docs/PRINCIPLES.md`)과 다르다.

## AI-Native Declarative Backend Framework --- 설계 문서 초안

> Status: Draft / Concept & Architecture Exploration\
> 목적: Codex / Claude 등 개발 에이전트와 함께 AIP의 타당성을 검증하고
> PoC 설계를 진행하기 위한 기준 문서\
> 주의: 이 문서는 확정 스펙이 아니다. 특히 DSL 문법, 전송 프로토콜,
> Runtime 구현 언어, DB 추상화 수준은 PoC 검증을 통해 변경될 수 있다.

------------------------------------------------------------------------

## 1. 문제 정의

현대의 백엔드 개발은 프레임워크가 상당한 편의를 제공함에도 여전히
반복적인 구현 공수가 크다.

예를 들어 Spring 기반 애플리케이션에서는 일반적으로 다음 계층과 설정을
직접 구성한다.

-   Framework / application configuration
-   Dependency configuration
-   Entity / Domain Model
-   DTO
-   Controller / Endpoint
-   Service / Business Logic
-   Repository / ORM
-   Validation
-   Authentication / Authorization
-   Transaction
-   Exception handling
-   Serialization
-   Logging / Observability
-   API documentation
-   Client-side contract/type synchronization

단순 CRUD는 이미 자동화하기 쉽다. 그러나 실제 서비스는 CRUD만으로
구성되지 않는다.

현실의 백엔드에는 다음 문제가 존재한다.

-   복잡한 권한 및 인가 정책
-   상태에 따라 달라지는 비즈니스 규칙
-   여러 데이터 변경의 원자성
-   N+1 및 비효율적 데이터 접근
-   동시성 및 race condition
-   캐시 일관성
-   외부 API 호출
-   이벤트 발행
-   재시도와 idempotency
-   부분 실패와 compensation
-   DB별 기능 차이
-   메시지 브로커 / Redis / Storage 등 다양한 외부 의존성

따라서 단순한 "DB Schema → CRUD API 자동 생성"은 AIP가 해결하려는 문제가
아니다.

AIP가 해결하려는 핵심 문제는 다음과 같다.

> 사람이 직접 API와 반복적인 백엔드 코드를 구현하지 않아도, 사람이 직접
> 설계했을 때 기대하는 타입 안정성, 권한 통제, 성능, 트랜잭션 안정성 및
> 비즈니스 규칙을 유지하거나 더 강하게 검증할 수 있는 백엔드 프로그래밍
> 모델을 만들 수 있는가?

------------------------------------------------------------------------

## 2. 핵심 아이디어

AIP는 "API 자동 생성기"가 아니다.

AIP의 기본 아이디어는 다음과 같다.

> 애플리케이션의 데이터 구조와 허용되는 비즈니스 행위를 제한된 선언형
> 언어로 정의하고, Compiler / Static Analyzer / Planner / Runtime이 이를
> 검증하고 실행한다.

기존 구조:

``` text
Requirement
    ↓
Developer / AI
    ↓
Controller
    ↓
Service
    ↓
Repository
    ↓
Database / External Systems
```

AIP 구조:

``` text
Requirement
    ↓
Developer / AI
    ↓
Application Definition
    ↓
AIP Compiler
    ↓
Typed IR
    ↓
Static Analysis
    ↓
Execution Plan
    ↓
AIP Runtime
    ↓
DB / Cache / Queue / External Systems
```

AIP에서 개발자는 "어떻게 endpoint를 구현할 것인가"보다 다음을 선언한다.

-   무엇이 존재하는가
-   무엇을 읽을 수 있는가
-   무엇을 변경할 수 있는가
-   누가 그것을 할 수 있는가
-   어떤 조건이 항상 지켜져야 하는가
-   어떤 side effect가 발생하는가
-   어떤 transaction / consistency 특성이 필요한가

------------------------------------------------------------------------

## 3. AIP라는 이름의 의미

AIP는 현재 가칭으로 다음 의미를 사용한다.

**Application Intent Protocol**

기존 API(Application Programming Interface)가 애플리케이션에 접근하기
위한 "인터페이스"를 중심으로 사고한다면 AIP는 클라이언트가
애플리케이션에 전달하는 **Intent**를 중심으로 사고한다.

예:

``` text
REST:
POST /orders/{id}/cancel

AIP:
CancelOrder(orderId)
```

중요한 것은 endpoint 이름의 차이가 아니다.

`CancelOrder`는 서버에서 사전에 정의되고 검증된 비즈니스 capability이며,
클라이언트는 해당 intent를 요청할 뿐 실행 방법을 지정하지 않는다.

------------------------------------------------------------------------

## 4. 설계 철학

### 4.1 올바르지 않은 백엔드를 표현하기 어렵게 만든다

AIP의 목표는 단순히 코드를 적게 작성하는 것이 아니다.

> 편리해서 안전한 시스템이 아니라, 위험한 상태를 표현하기 어렵기 때문에
> 안전한 시스템을 지향한다.

Java와 같은 강한 타입 시스템이 일부 오류를 실행 전에 차단하는 것처럼
AIP도 가능한 많은 오류를 서버 실행 전에 발견해야 한다.

예:

``` text
Potential N+1 query detected:
Order.items.product

Declare or allow an appropriate loading strategy.
```

또는:

``` text
Unsafe external side effect detected.

PaymentGateway.charge() is non-idempotent,
but this command may be retried.

Declare an idempotency strategy or disable retries.
```

### 4.2 Safe things may be implicit; dangerous things must be explicit

AIP는 Convention over Configuration을 강하게 활용한다.

그러나 보안, 데이터 노출, destructive operation처럼 위험한 동작은
암묵적으로 허용하지 않는다.

예를 들어 Entity를 정의했다고 해서 자동으로 인터넷에 CRUD 전체가
공개되어서는 안 된다.

``` text
entity Post {
    id: UUID
    author: User
    title: String
}
```

외부 노출은 별도의 명시적인 정책으로 정의한다.

``` text
expose Post {
    read: public
    create: authenticated
    update: owner
    delete: owner
}
```

### 4.3 One intent, one canonical expression

동일한 의미를 표현하는 문법을 여러 개 제공하지 않는다.

LLM과 인간 모두에게 예측 가능한 문법을 제공하기 위해 가능한 한 하나의
canonical expression을 유지한다.

나쁜 예:

``` text
allow: owner
policy: owner
authorize if user == resource.owner
```

좋은 방향:

``` text
access {
    update: owner
}
```

### 4.4 Runtime magic보다 검증 가능한 semantics

AIP는 내부에서 많은 것을 자동화하더라도 동작 의미가 불분명한 "magic"을
최소화한다.

자동화된 동작은 다음 중 하나 이상을 만족해야 한다.

-   schema로 확인 가능
-   compiler가 검증 가능
-   introspection 가능
-   execution plan으로 설명 가능
-   structured error로 실패 원인을 표현 가능

------------------------------------------------------------------------

## 5. 목표 개발 경험

최종적으로 AIP 애플리케이션의 초기 경험은 다음과 같은 수준을 목표로
한다.

``` text
my-backend/
├── aip.toml
└── app.aip
```

환경변수:

``` text
DATABASE_URL=...
REDIS_URL=...
AIP_SECRET=...
```

설정:

``` text
runtime {
    database: postgres
    cache: redis
}

security {
    default: authenticated
}
```

애플리케이션 정의:

``` text
entity Post {
    id: UUID
    author: User
    title: String
    content: String
}

expose Post {
    read: public
    create: authenticated
    update: owner
    delete: owner
}
```

실행:

``` bash
aip run
```

AIP가 책임지는 영역의 목표:

-   서버 bootstrap
-   DB connection / pool
-   schema / migration
-   query execution
-   query planning
-   validation
-   serialization
-   authentication integration
-   authorization
-   transaction
-   structured error handling
-   logging / observability
-   contract/schema generation
-   client type generation
-   client SDK integration
-   가능한 범위의 성능 및 안전성 정적 분석

핵심 UX는 다음 문장으로 요약할 수 있다.

> 백엔드 코드를 작성해서 서버를 만드는 것이 아니라, 백엔드의 규칙을
> 선언하면 서버가 존재한다.

------------------------------------------------------------------------

## 6. 핵심 언어 Primitive 후보

초기 가설은 다음 primitive만으로 상당수의 백엔드 비즈니스 로직을 표현할
수 있는지 검증하는 것이다.

``` text
Entity
Query
Command
Policy
Invariant
Transaction
Event
```

### Entity

도메인의 데이터 구조 및 관계를 정의한다.

``` text
entity Order {
    id: UUID
    user: User
    items: OrderItem[]
    status: OrderStatus
}
```

### Query

상태를 변경하지 않는 읽기 intent.

``` text
query OrderHistory {
    ...
}
```

### Command

상태를 변경하는 비즈니스 intent.

``` text
command CancelOrder {
    ...
}
```

### Policy

누가 무엇을 수행할 수 있는지 정의한다.

``` text
policy:
    owner | ADMIN
```

### Invariant

항상 만족해야 하는 도메인 조건을 정의한다.

예:

``` text
require:
    order.status before SHIPPING
```

### Transaction

여러 변경 작업의 원자성 / 일관성 요구를 표현한다.

### Event

Command 이후 발생하는 domain event 및 전달 특성을 표현한다.

------------------------------------------------------------------------

## 7. 예시: 주문 취소

개념 검증용 문법 예시이며 확정 문법이 아니다.

``` text
command CancelOrder(orderId: OrderId) {

    policy:
        owner(orderId) | ADMIN

    require:
        order.status in [PAID, PREPARING]

    transaction {
        refund order.payment
        restore order.inventory
        restore order.coupon
        set order.status = CANCELLED
    }

    emit:
        OrderCancelled(order.id)

    returns:
        Order
}
```

이 정의에서 Compiler / Runtime은 최소한 다음을 이해할 수 있어야 한다.

-   input type
-   authorization
-   precondition
-   affected resources
-   transaction boundary
-   external side effects
-   emitted event
-   output type

------------------------------------------------------------------------

## 8. Compiler / IR / Runtime Architecture

AIP DSL을 Runtime이 직접 해석하는 구조에 강하게 결합하지 않는다.

중간 표현인 Typed IR을 둔다.

``` text
AIP Definition
      ↓
Parser
      ↓
Typed AIP IR
      ↓
Static Analyzer
      ↓
Execution Planner
      ↓
Execution Plan
      ↓
AIP Runtime
```

### Typed IR의 목적

-   DSL syntax와 Runtime 분리
-   static analysis 용이성
-   여러 DSL/front-end 가능성
-   LLM이 구조화된 IR을 생성하는 방식의 가능성
-   여러 Runtime target 지원 가능성
-   tooling / IDE integration
-   deterministic compilation

장기적으로 다음과 같은 확장 가능성을 열어둔다.

``` text
             AIP Definition
                    ↓
                 AIP IR
           ↙         ↓         ↘
     Native Runtime  WASM   Serverless Runtime
```

------------------------------------------------------------------------

## 9. Runtime 구현 언어

현 단계에서 확정하지 않는다.

다만 장기적인 Core Runtime / Compiler 구현 언어 후보로 Rust가 유력하다.

이유:

-   높은 실행 성능
-   memory safety
-   강한 type system
-   Result / Option 기반 오류 모델
-   concurrency safety
-   native binary 배포
-   Compiler / Runtime / Infrastructure 계층과의 적합성

C는 성능상 가능하지만 AIP가 직접 메모리 안전성 문제까지 떠안을 이유가
적다.

C++ 역시 가능하지만 신규 기반 시스템에서 선택해야 할 특별한 이유가
필요하다.

중요:

> 초기 PoC를 반드시 Rust로 만들 필요는 없다.

초기 단계의 가장 큰 비용은 CPU 성능이 아니라 잘못된 semantics와
abstraction을 다시 설계하는 비용이다.

따라서 Python / Java / TypeScript 등 빠르게 검증 가능한 환경에서
semantics와 IR을 먼저 검증하고, 구조가 안정된 뒤 Core를 Rust로 옮기는
전략을 허용한다.

------------------------------------------------------------------------

## 10. Database 추상화

AIP는 ORM이 겪어온 문제를 반복하지 않아야 한다.

PostgreSQL, MySQL, SQLite 등의 DB는 단순히 SQL syntax만 다른 것이
아니다.

차이의 예:

-   transaction semantics
-   isolation
-   locking
-   RETURNING
-   UPSERT
-   JSON
-   indexing
-   full-text search
-   concurrency behavior

따라서 모든 DB를 lowest common denominator로 강제로 동일하게 추상화하지
않는다.

### Capability-based Adapter

각 DB adapter는 자신이 제공하는 capability를 선언한다.

``` text
database postgres {
    capabilities {
        transaction
        returning
        json
        fullTextSearch
        rowLock
    }
}
```

애플리케이션이 지원되지 않는 기능을 사용하면 실행 시점이 아니라 가능한
한 compile 단계에서 실패한다.

``` text
AIP-C214

`ReserveStock` requires capability `row-lock`.

Current adapter:
    SQLite

Supported:
    PostgreSQL ✓
    MySQL      ✓
    SQLite     ✗
```

------------------------------------------------------------------------

## 11. Query Planning과 N+1

AIP의 중요한 가능성 중 하나는 데이터 접근을 framework가 전체적으로
이해할 수 있다는 점이다.

일반 ORM에서는 개발자가 객체 관계를 순차적으로 접근하면서 예상치 못한
N+1이 발생할 수 있다.

AIP에서는 Query가 전체 데이터 그래프를 선언한다.

``` text
query OrderHistory {
    select:
        Order[] {
            id
            items {
                product {
                    name
                }
            }
        }
}
```

Compiler / Planner는 다음 그래프를 사전에 파악할 수 있다.

``` text
Order
 └─ Items
      └─ Product
```

이를 기반으로 adapter capability와 cost model을 고려해 다음 전략 중
적절한 execution plan을 선택할 수 있다.

-   JOIN
-   batch query
-   subquery
-   기타 DB-specific optimization

목표는 "ORM의 모든 문제를 자동 해결한다"가 아니다.

목표는:

> 프레임워크가 전체 데이터 접근 의도를 알고 있으므로 사람이 직접 작성한
> imperative ORM 코드보다 더 많은 정적 분석 및 최적화 가능성을 확보한다.

필요한 경우 개발자가 strategy hint를 명시할 수 있어야 한다.

------------------------------------------------------------------------

## 12. Extension Architecture

AIP Core에 PostgreSQL, Redis, Kafka, S3, Payment Provider 등의 모든
기능을 내장하지 않는다.

각 dependency / integration이 자신의 문법과 semantics를 확장할 수 있는
구조를 지향한다.

``` text
AIP Core
   │
   ├─ Postgres Extension
   │    ├─ DSL extension
   │    ├─ Types
   │    ├─ Static validation
   │    ├─ Planner integration
   │    └─ Runtime adapter
   │
   ├─ Redis Extension
   │    ├─ DSL extension
   │    ├─ Types
   │    ├─ Validation
   │    └─ Runtime adapter
   │
   └─ Kafka Extension
        ├─ DSL extension
        ├─ Event semantics
        ├─ Validation
        └─ Runtime adapter
```

예상 ecosystem:

``` text
aip-postgres
aip-mysql
aip-redis
aip-kafka
aip-s3
aip-stripe
...
```

예시:

``` text
command CreateOrder {
    transaction {
        save Order
        cache.invalidate "orders:${user.id}"
        kafka.publish OrderCreated
    }
}
```

여기서 `cache` 및 `kafka` semantics는 해당 extension에서 제공한다.

### Extension이 제공해야 할 최소 계약 후보

-   namespace / syntax
-   type definitions
-   capability definitions
-   validation rules
-   effect metadata
-   planner hooks
-   runtime implementation
-   error definitions
-   introspection metadata

서드파티 extension도 Core의 안전성 모델을 우회하지 못하도록 설계해야
한다.

------------------------------------------------------------------------

## 13. Side Effect / Transaction / Distributed Systems

AIP의 실사용 가능성을 결정하는 가장 어려운 영역 중 하나다.

예:

``` text
transaction {
    DB.save(order)
    Redis.delete(cache)
    Kafka.publish(event)
}
```

단순히 하나의 `transaction` keyword로 이를 모두 원자적으로 처리할 수
있다고 가정하면 안 된다.

고려해야 할 문제:

-   DB commit 실패
-   Kafka publish 성공 후 DB rollback
-   cache invalidation 실패
-   process crash
-   at-least-once delivery
-   duplicated event
-   retry
-   external API partial success
-   distributed transaction
-   outbox pattern
-   compensation
-   idempotency

따라서 side effect는 first-class semantic으로 모델링할 필요가 있다.

개념 예:

``` text
command Payment {

    idempotent by order.id

    external payment {
        retry: safe
        compensation: RefundPayment
    }

    transaction {
        ...
    }

    event OrderPaid {
        delivery: at-least-once
    }
}
```

Compiler는 위험한 조합을 검출해야 한다.

``` text
AIP-E3021

Unsafe external side effect detected.

`PaymentGateway.charge()` is non-idempotent
but the containing command can be retried.

Declare an idempotency strategy or disable retries.
```

------------------------------------------------------------------------

## 14. 보안 모델

### 14.1 새로운 wire protocol 자체는 보안 해결책이 아니다

REST, GraphQL, WebSocket, binary protocol 또는 자체 transport를
사용하더라도 브라우저 / 클라이언트는 신뢰할 수 없다.

프론트 번들은 분석 가능하며 요청은 위조될 수 있다고 가정한다.

따라서 AIP의 보안 목표는:

> 위조할 수 없는 요청을 만드는 것이 아니라, 요청을 위조하더라도 서버가
> 사전에 허용된 capability 이외의 작업을 수행할 수 없도록 만드는 것이다.

### 14.2 클라이언트에 DB 표현력을 노출하지 않는다

피해야 할 모델:

``` text
backend.query({
    table: "users",
    where: {...}
})
```

선호 모델:

``` text
aip.query("UserProfile", {
    userId
})
```

`UserProfile`은 서버에서 사전에 정의되고 검증된 operation이다.

공격자가 임의의 query string 또는 다른 userId를 전송해도 Runtime의
policy와 operation registry를 우회할 수 없어야 한다.

### 14.3 요청 실행 Pipeline

개념적인 실행 순서:

``` text
Request
   ↓
Known Intent / Operation?
   ↓
Input Schema Validation
   ↓
Authentication
   ↓
Authorization Policy
   ↓
Invariant / Preconditions
   ↓
Compiled Execution Plan
   ↓
Transaction / Effects
   ↓
Output Validation
   ↓
Response
```

------------------------------------------------------------------------

## 15. Client Integration

프론트엔드가 AIP DSL이나 wire protocol을 직접 이해하도록 강제하지
않는다.

AIP 서버 정의로부터 contract 및 client SDK/type을 생성한다.

``` text
AIP Application Definition
        ↓
      Compiler
        ↓
   Contract / Schema
    ↙      ↓       ↘
TypeScript Kotlin  Swift
   SDK      SDK     SDK
```

### TypeScript / React 예

``` ts
import { aip } from "./generated/aip";

await aip.orders.cancel({
    orderId
});
```

React-specific package가 존재한다면:

``` text
@aip/core
@aip/react
@aip/vue
@aip/svelte
```

다른 ecosystem:

``` text
aip-kotlin
aip-swift
aip-dart
```

React adapter는 AIP Core client 위에 hooks / cache integration 등의
framework-specific UX를 제공할 수 있다.

예:

``` tsx
const { data } = useQuery("GetMyProfile");
const cancelOrder = useCommand("CancelOrder");

await cancelOrder({ orderId });
```

프론트 개발자는 transport, endpoint routing, serialization 내부 구현을
몰라도 된다.

------------------------------------------------------------------------

## 16. Contract와 Introspection

AIP 서버는 자신이 제공하는 기능을 machine-readable하게 설명할 수 있어야
한다.

예상 contract:

``` text
AIP Schema
├─ version
├─ capabilities
├─ queries
├─ commands
├─ inputs
├─ outputs
├─ policies / relevant constraints
└─ errors
```

개념 API:

``` ts
const schema = await aip.describe();
```

예:

``` text
Order
 ├─ query get
 ├─ query list
 └─ command cancel
      ├─ input: CancelOrderInput
      ├─ output: Order
      └─ errors
           ├─ ORDER_NOT_FOUND
           ├─ NOT_OWNER
           └─ ALREADY_SHIPPED
```

이를 통해 다음 consumer가 동일한 contract를 사용할 수 있다.

-   Human developer
-   Generated SDK
-   IDE
-   Static tooling
-   Test tooling
-   LLM
-   Autonomous coding agent

------------------------------------------------------------------------

## 17. LLM / Agent를 First-Class Consumer로 취급

AIP는 AI 시대에 만들어지는 프레임워크라는 점을 명시적으로 고려한다.

목표:

``` text
AIP Server
     ↓
Machine-readable Contract
     ↓
Human / SDK / IDE / LLM / Agent
```

LLM이 처음 보는 AIP 서버에서도 다음 흐름이 가능해야 한다.

``` text
DESCRIBE SERVER
      ↓
Contract 획득
      ↓
Available Intent 이해
      ↓
Input / Output / Error 이해
      ↓
Type-safe Client Code 생성
```

이를 위해 문법 및 contract는 다음 특성을 가져야 한다.

-   canonical
-   deterministic
-   structured
-   strongly typed
-   introspectable
-   machine-readable
-   stable naming
-   explicit versioning

LLM에게 자유도를 많이 주는 것이 목표가 아니다.

오히려:

> AI가 코드를 잘 짜도록 프롬프트로 설득하는 것이 아니라, AI가 잘못 짤 수
> 있는 범위를 언어와 시스템 차원에서 제한한다.

------------------------------------------------------------------------

## 18. Structured Error Model

문자열 기반 오류를 최소화한다.

피해야 할 예:

``` json
{
  "message": "Something went wrong"
}
```

AIP 표준 오류 예:

``` json
{
  "code": "AIP.AUTH.FORBIDDEN",
  "operation": "Order.Cancel",
  "reason": "OWNER_REQUIRED",
  "path": "order.owner",
  "retryable": false
}
```

오류는 최소한 다음 consumer가 안정적으로 처리할 수 있어야 한다.

-   application code
-   generated client
-   human developer
-   logging system
-   LLM / agent

Error taxonomy는 AIP specification의 중요한 일부로 취급한다.

------------------------------------------------------------------------

## 19. AIP와 GraphQL의 차이

GraphQL은 주로 클라이언트가 필요한 데이터 형태를 선언하는 query
language이다.

그러나 실제 비즈니스 로직, authorization, transaction, resolver 등의
구현은 여전히 서버 코드에 존재한다.

AIP는 그보다 아래 계층의 문제를 다룬다.

``` text
GraphQL
   ↓
Resolver
   ↓
Service
   ↓
Business Logic
   ↓
DB
```

AIP가 제거 또는 추상화하려는 대상은 단순 endpoint가 아니라 위의 반복적인
구현 계층 자체이다.

AIP는 데이터 query뿐 아니라:

-   business command
-   policy
-   invariant
-   transaction
-   event
-   side effect
-   consistency semantics

를 application definition의 일부로 취급한다.

------------------------------------------------------------------------

## 20. AIP와 기존 자동 Backend / BaaS의 차이

기존 자동 backend 계열은 흔히 다음 방향에 강하다.

``` text
DB Schema
   ↓
Generated CRUD / Data API
```

AIP의 목표:

``` text
Domain
+
Business Intent
+
Policy
+
Invariant
+
Effects
+
Consistency
      ↓
Compiler / Planner / Runtime
      ↓
Complete Backend Behavior
```

즉 핵심 차이는 **data access automation이 아니라 business logic의
declarative representation과 검증**이다.

------------------------------------------------------------------------

## 21. Escape Hatch 문제

범용성을 높이기 위해 모든 로직을 DSL에 추가하다 보면 다음 문제가 발생할
수 있다.

``` text
DSL
 ↓
condition
 ↓
loop
 ↓
function
 ↓
async
 ↓
exception
 ↓
module
 ↓
general-purpose programming language
```

결과적으로 Spring보다 배우기 어려운 자체 언어가 될 위험이 있다.

따라서 중요한 연구 질문:

> 실제 서비스의 80\~90% 로직을 작은 primitive 집합으로 표현하고, 나머지
> 특수한 로직만 안전한 escape hatch로 처리할 수 있는가?

Escape hatch가 존재한다면 다음을 검토해야 한다.

-   어떤 언어로 작성하는가
-   Runtime isolation은 어떻게 하는가
-   effect를 어떻게 선언하는가
-   static analyzer가 해당 코드를 어디까지 신뢰하는가
-   transaction / retry semantics와 어떻게 결합하는가
-   extension과 custom code의 경계는 무엇인가

------------------------------------------------------------------------

## 22. PoC 전략

Todo CRUD를 PoC 대상으로 사용하지 않는다.

CRUD는 기존 기술로도 이미 매우 쉽게 해결되므로 AIP의 핵심 가설을
검증하지 못한다.

실제 복잡한 서비스에서 나타나는 케이스를 의도적으로 포함한 작은 도메인을
사용한다.

필수 검증 요소:

-   Entity relation
-   복잡한 authorization
-   state-dependent permission
-   validation / invariant
-   transaction
-   N+1 가능성이 있는 조회
-   cache
-   external API
-   event / message broker
-   concurrency
-   retry
-   idempotency
-   partial failure
-   compensation
-   structured errors

기존에 구현 경험이 있는 실제 프로젝트의 복잡한 비즈니스 로직을 가져와
AIP로 다시 표현하는 방식이 유효하다.

------------------------------------------------------------------------

## 23. PoC 평가 질문

각 use case마다 다음 질문을 검증한다.

### 표현력

-   작은 primitive 집합으로 현실적인 비즈니스 로직을 표현할 수 있는가?
-   DSL이 general-purpose language로 팽창하지 않는가?

### 안정성

-   기존 프레임워크보다 더 많은 오류를 실행 전에 발견할 수 있는가?
-   authorization 누락을 검출할 수 있는가?
-   unsafe side effect를 검출할 수 있는가?

### 성능

-   N+1 등의 문제를 사전에 발견하거나 제거할 수 있는가?
-   generated execution plan이 수동 구현 대비 지나치게 비효율적이지
    않은가?
-   사용자가 필요한 경우 optimizer hint를 줄 수 있는가?

### 개발 생산성

-   Controller / Service / Repository / DTO 등의 반복 코드가 실질적으로
    사라지는가?
-   설정 파일과 application definition만으로 서버를 기동할 수 있는가?
-   코드량 감소가 abstraction complexity 증가보다 큰가?

### 확장성

-   Redis / Kafka / Storage / Payment 등을 extension으로 자연스럽게
    추가할 수 있는가?
-   extension이 Core safety model을 깨뜨리지 않는가?

### AI 적합성

-   LLM이 canonical AIP definition을 안정적으로 생성하는가?
-   같은 요구사항에서 결과 편차가 일반 framework code generation보다
    감소하는가?
-   Compiler error를 LLM이 읽고 스스로 수정하기 쉬운가?

------------------------------------------------------------------------

## 24. 초기 Architecture 후보

``` text
                         ┌──────────────────┐
                         │ AIP Definition   │
                         └────────┬─────────┘
                                  ↓
                         ┌──────────────────┐
                         │ Parser / Compiler│
                         └────────┬─────────┘
                                  ↓
                           Typed AIP IR
                                  ↓
                         ┌──────────────────┐
                         │ Static Analyzer  │
                         ├──────────────────┤
                         │ Types            │
                         │ Policies         │
                         │ Invariants       │
                         │ Effects          │
                         │ N+1 / Queries    │
                         │ Transactions     │
                         │ Capabilities     │
                         └────────┬─────────┘
                                  ↓
                           Execution Plan
                                  ↓
                         ┌──────────────────┐
                         │   AIP Runtime    │
                         └────────┬─────────┘
                                  │
          ┌───────────────────────┼───────────────────────┐
          ↓                       ↓                       ↓
     DB Adapters             Extensions             External I/O
  PostgreSQL/MySQL        Redis/Kafka/etc.          HTTP/Storage/etc.
```

Client side:

``` text
React / Vue / Native / Agent
             ↓
      Generated Client
             ↓
          AIP Core
             ↓
       AIP Transport
             ↓
        AIP Runtime
```

------------------------------------------------------------------------

## 25. Runtime Deployment Model 후보

아직 결정하지 않는다.

### Embedded Model

각 언어의 애플리케이션에 AIP Runtime을 binding으로 포함한다.

``` text
Python ─┐
Node ───┼─→ Native Binding → AIP Core
Java ───┘
```

장점:

-   기존 ecosystem과 자연스럽게 결합
-   host application과 integration 용이

단점:

-   언어별 binding 관리
-   lifecycle / ABI 복잡성
-   환경별 차이

### Standalone Runtime

AIP Runtime 자체가 독립 서버로 동작한다.

``` text
Application Definition
        ↓
AIP Runtime
   ├─ DB
   ├─ Redis
   ├─ Kafka
   └─ External Services
```

장점:

-   host language 독립
-   동일한 실행 semantics
-   배포 단순화 가능
-   Core를 하나만 유지

단점:

-   custom code / extension 모델 설계 난이도
-   IPC / process boundary 고려 필요

현재 철학에는 Standalone Runtime이 장기적으로 더 자연스러울 가능성이
있으나 PoC에서 검증한다.

------------------------------------------------------------------------

## 26. 전송 Protocol은 후순위 문제

AIP의 핵심은 REST를 새로운 wire protocol로 바꾸는 것이 아니다.

초기 PoC에서는 HTTP/JSON 등 기존 transport를 사용해도 된다.

먼저 검증해야 할 것:

1.  Application semantics
2.  DSL / IR
3.  Static validation
4.  Execution model
5.  Security model
6.  Extension model
7.  Client contract

그 이후 필요하다면 다음을 검토한다.

-   HTTP/JSON
-   HTTP binary encoding
-   WebSocket
-   streaming transport
-   custom protocol

Transport는 AIP의 semantics와 분리되어야 한다.

------------------------------------------------------------------------

## 27. 설계 원칙 요약

AIP의 초기 "헌법" 후보:

1.  **One intent, one canonical expression.**
2.  **Safe things may be implicit; dangerous things must be explicit.**
3.  **Every behavior should be statically verifiable where reasonably
    possible.**
4.  **Every behavior must be introspectable.**
5.  **Every contract must be machine-readable.**
6.  **Every failure must be structured.**
7.  **Clients are never trusted.**
8.  **Clients express intent, not arbitrary data access.**
9.  **Extensions must declare capabilities and effects.**
10. **Unsupported semantics should fail early rather than degrade
    silently.**
11. **The same contract should serve humans, SDKs, IDEs, and AI
    agents.**
12. **AIP should remove boilerplate without hiding correctness-critical
    semantics.**
13. **Do not solve abstraction leaks by silently pretending different
    systems are identical.**
14. **Do not grow the DSL into a general-purpose language unless
    evidence proves it unavoidable.**

------------------------------------------------------------------------

## 28. 첫 번째 연구 과제

구현보다 먼저 다음을 수행한다.

### Phase 1 --- Domain Case Collection

실제 백엔드에서 어려웠던 business logic 10\~20개를 수집한다.

예:

-   리소스 소유자 + 관리자 권한
-   상태별 수정 권한
-   nested relation 조회
-   N+1 발생 가능 조회
-   재고 감소
-   주문 취소
-   결제
-   쿠폰 복구
-   캐시 무효화
-   event publish
-   외부 API 실패
-   retry
-   concurrent update
-   idempotent request

### Phase 2 --- Minimal Semantics

위 케이스를 표현하기 위해 필요한 최소 primitive를 찾는다.

초기 후보:

``` text
Entity
Query
Command
Policy
Invariant
Transaction
Event
Effect
```

새 primitive는 실제 use case로 필요성이 증명될 때만 추가한다.

### Phase 3 --- IR First

DSL syntax를 완성하기 전에 IR을 정의한다.

질문:

-   어떤 정보가 Compiler에게 반드시 필요한가?
-   어떤 정보가 Static Analyzer에게 필요한가?
-   어떤 정보가 Planner에게 필요한가?
-   어떤 정보가 Runtime에게 필요한가?
-   어떤 정보가 Client Contract에 공개되어야 하는가?

### Phase 4 --- Minimal Runtime

하나의 DB부터 지원한다.

추천 초기 범위:

``` text
PostgreSQL only
HTTP/JSON transport
Single process
Basic auth abstraction
No distributed transaction guarantee
```

여기서 핵심 semantics를 검증한다.

### Phase 5 --- Stress Cases

다음 순서로 복잡도를 추가한다.

``` text
PostgreSQL
   ↓
Authorization
   ↓
Query Planning
   ↓
Transaction
   ↓
Redis
   ↓
Event / Kafka
   ↓
External HTTP
   ↓
Retry / Idempotency
   ↓
Failure Recovery
```

------------------------------------------------------------------------

## 29. 성공 조건

AIP PoC가 성공했다고 판단하기 위한 최소 조건 후보:

-   현실적인 복잡도를 가진 backend use case를 표현할 수 있다.
-   기존 framework 대비 반복 코드가 크게 감소한다.
-   중요한 권한/타입/side-effect 오류를 실행 전에 검출한다.
-   데이터 접근이 자동화되어도 execution plan을 설명할 수 있다.
-   성능 문제를 숨기지 않는다.
-   extension을 추가해도 Core semantics가 유지된다.
-   프론트가 AIP 내부 DSL을 알지 않고 generated client만으로 사용할 수
    있다.
-   LLM이 contract를 읽고 안정적으로 client 및 server definition을
    생성할 수 있다.
-   escape hatch 없이 상당수 일반 로직을 표현할 수 있다.
-   escape hatch가 필요한 경우에도 safety boundary를 설명할 수 있다.

------------------------------------------------------------------------

## 30. 실패 신호

다음 현상이 나타나면 설계를 재검토한다.

-   DSL이 빠르게 범용 프로그래밍 언어처럼 변한다.
-   Runtime magic 때문에 실제 SQL / effect를 예측할 수 없다.
-   DB abstraction을 위해 중요한 DB 기능을 포기한다.
-   extension 하나 추가할 때마다 Core를 수정해야 한다.
-   authorization이 optional convention으로 밀려난다.
-   distributed side effect를 단순 transaction처럼 가장한다.
-   client에게 임의 DB query 능력을 제공한다.
-   동일한 intent를 표현하는 방식이 계속 늘어난다.
-   LLM이 같은 요구사항을 계속 다른 구조로 표현한다.
-   AIP가 줄인 코드보다 AIP 자체를 이해하기 위한 복잡도가 더 커진다.

------------------------------------------------------------------------

## 31. 장기 비전

AIP가 성공적으로 발전한다면 목표는 단순한 "Spring보다 짧은 백엔드
프레임워크"가 아니다.

다음과 같은 프로그래밍 모델을 지향한다.

``` text
Application Intent
       +
Domain Definition
       +
Policies
       +
Invariants
       +
Effects
       ↓
Verified Application Model
       ↓
Compiler / Planner
       ↓
Deterministic Runtime
       ↓
Complete Backend
```

그리고 AI 개발 환경에서는:

``` text
Human Requirement
       ↓
LLM / Coding Agent
       ↓
Canonical AIP Definition
       ↓
Compiler Verification
       ↓
Execution Plan
       ↓
Runtime
```

핵심은 AI가 무제한의 일반 코드를 자유롭게 생성하도록 하는 것이 아니다.

> **AI는 의도를 구조화하고, 시스템은 아키텍처와 안전성을 강제한다.**

------------------------------------------------------------------------

## 32. 별도 연관 아이디어: Declarative UI / Design System

AIP와 별도로 논의된 UI 아이디어도 동일한 철학을 공유한다.

현재 많은 AI UI 생성은 prompt에 디자인 규칙을 설명하고 결과의 일관성을
기대한다.

대안은:

``` text
Design Rules
+
Components
+
Layout Rules
+
Theme
+
Context
      ↓
UI Composition System
      ↓
Application UI
```

즉 AI에게 디자인 시스템 준수를 "요청"하는 것이 아니라 코드/규칙 차원에서
가능한 조합을 제한하고 검증한다.

장기적으로 두 프로젝트는 다음 공통 철학을 공유할 수 있다.

``` text
AI
 ↓
Constrained Declarative Model
 ↓
Compiler / Validator
 ↓
Deterministic Output
```

AIP와 UI 시스템을 처음부터 하나의 프로젝트로 결합할 필요는 없다. 다만
"AI가 잘하기를 기대하는 대신 좋은 결과의 공간을 시스템으로 제한한다"는
공통 설계 철학은 유지할 가치가 있다.

------------------------------------------------------------------------

# Agent에게 요청할 다음 작업

이 문서를 받은 개발 에이전트는 바로 구현에 들어가기 전에 다음 순서로
작업한다.

1.  이 설계의 기술적 모순, 이미 알려진 실패 패턴, abstraction leak
    가능성을 비판적으로 검토한다.
2.  기존 REST / GraphQL / RPC / BaaS / declarative backend / ORM /
    workflow engine / policy engine과 비교해 AIP가 실제로 새롭게
    해결하는 영역을 분리한다.
3.  실제 복잡한 backend use case 10개를 정의한다.
4.  해당 use case를 표현할 최소 semantic model을 설계한다.
5.  DSL syntax보다 Typed IR schema를 먼저 제안한다.
6.  Static Analyzer가 보장할 수 있는 것과 보장할 수 없는 것을 명확히
    분리한다.
7.  PostgreSQL 단일 adapter를 기준으로 최소 Execution Planner를
    설계한다.
8.  이후에만 DSL syntax와 Runtime 구현 기술을 결정한다.

**중요:** 아이디어를 긍정하는 것이 목적이 아니다. 실제 production
backend를 대체할 수 있는지 공격적으로 검증하고, 불가능하거나 위험한
부분은 구체적인 counterexample과 함께 지적한다.

------------------------------------------------------------------------

## One-line Definition

> **AIP is an AI-native declarative backend model where clients express
> application intent and a compiler-verified runtime owns the repetitive
> implementation, safety checks, execution planning, and integration
> mechanics of the backend.**
