# AIP 전체 대화 맥락 인계 문서

> 2026-10-02: 원문 보존. "GraphQL 비슷한 query language로 이해하면 잘못"이라는 문장은 호출자 표현을 배제하는 뜻으로 읽으면 창시자 원칙(`docs/PRINCIPLES.md`)과 충돌한다. AIP는 GraphQL 복제가 아니지만 호출자가 데이터·동작을 표현하는 것은 핵심이다.

## Application Intent Protocol --- Context Handoff for Codex / Claude / Coding Agents

> **이 문서는 구현 명세서가 아니다.**
>
> 목적은 AIP를 처음 접하는 개발 에이전트가 아이디어를
> `간단한 DSL + CRUD API generator`, `GraphQL wrapper`, `ORM wrapper`,
> `toy backend`로 축소하는 것을 막고, 아이디어가 만들어진
> 배경·문제의식·설계 철학·중요한 판단·미결정 사항까지 전달하는 것이다.
>
> **이 문서를 읽고 곧바로 장난감 구현부터 하지 말 것.** 먼저 문제의식과
> 목표 수준을 정확히 이해한다.

------------------------------------------------------------------------

# 0. 가장 먼저 이해해야 할 것

AIP를 다음 중 하나로 이해하면 잘못 이해한 것이다.

-   CRUD API 자동 생성기
-   DB schema를 읽고 REST endpoint를 만드는 도구
-   GraphQL 비슷한 query language
-   Spring Controller boilerplate generator
-   ORM wrapper
-   BaaS clone
-   low-code backend
-   단순 RPC framework
-   단순 DSL interpreter
-   AI용 syntax sugar

AIP가 탐구하는 질문은 이것이다.

> **사람이 백엔드를 직접 구현하면서 확보하던 신뢰성, 성능 통제, 권한,
> 비즈니스 규칙, transaction, 외부 dependency integration 등의 의미를
> 잃지 않으면서 반복적인 백엔드 구현 공수 자체를 거의 제거할 수 있는
> 새로운 application programming model을 만들 수 있는가?**

AI 시대라는 조건도 적극 이용한다.

> **AI에게 좋은 코드를 생성하라고 프롬프트로 부탁하는 대신, AI가 잘못된
> 백엔드를 만들 수 있는 표현 공간 자체를 언어와 runtime 차원에서 제한할
> 수 있는가?**

이상적인 최종 UX:

``` text
AIP dependency/runtime 준비
+
환경 변수 설정
+
작은 application definition
+
필요한 extension 설치
        ↓
aip run
        ↓
production-grade backend
```

------------------------------------------------------------------------

# 1. 아이디어의 출발점

회사 FDE 리더와의 대화에서 두 가지 관심사가 생겼다.

## 1.1 Design System 고도화

사용자는 `winterholic-design` repository처럼 디자인 시스템을 꼼꼼히
관리해 왔다. FDE 리더는 여기에 더해 다양한 layout, 화면 구성, component
규칙, theme, 상황별 조합을 미리 정의하고 AI가 이를 prompt로 조합하도록
관리하고 있었다.

여기서 생긴 문제의식:

> "왜 프롬프트로 AI에게 규칙을 지키라고 해야 하지?"
>
> "코드와 규칙으로 강제할 수 있는 영역 아닌가?"
>
> "좋은 디자인 조합만 가능한 자동 디자인 조립 시스템을 만들 수 있지
> 않을까?"

즉:

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
Constrained UI Composition System
      ↓
Valid UI
```

이 철학은 AIP와 연결된다.

## 1.2 "API가 꼭 필요할까?"

FDE 리더의 "꼭 API라는 게 필요할까?"라는 문제 제기에서 새로운 backend
framework 발상이 시작됐다.

초기 직관:

> 서버에 dependency와 runtime을 준비하고, 개발자가 Controller / API /
> Service / Repository를 일일이 구현하지 않아도 frontend가 필요한
> business operation을 호출할 수 있으면 어떨까?

곧바로 다음 우려가 생겼다.

``` text
그럼 frontend에서 query를 직접 때리는 것과 뭐가 다른가?
DB 직접 접근처럼 위험해지는 것 아닌가?
복잡한 business logic은 어떻게 하는가?
```

따라서 아이디어는 단순 "frontend가 DB query를 자유롭게 한다"는 방향과
다르다.

------------------------------------------------------------------------

# 2. 왜 자동 API 생성 자체를 불신했는가

사용자는 "공수가 거의 없는 자동 API"에 처음부터 불신이 있었다. 실제
backend 개발 경험 때문이다.

Java/Spring은 불편하고 boilerplate가 많지만 잘 설계하면 다음을 얻는다.

-   강한 타입 검사
-   실행 전 오류 검출
-   명확한 business logic
-   명시적 authorization
-   명시적 transaction
-   query/repository 통제
-   performance 추적
-   복잡한 예외 케이스 표현

실제 프로젝트, 특히 아리아리 같은 서비스에는 AI가 단순 구현하면 실수하기
쉬운 로직이 있었다.

``` text
이 사용자가 이 resource를 볼 수 있는가?
본인 resource라도 특정 상태에서는 수정 가능한가?
관리자는 어디까지 허용되는가?
연관 객체 조회에서 N+1은 없는가?
여러 변경은 하나의 transaction인가?
동시 요청에서 race condition은 없는가?
외부 호출 성공 후 DB commit 실패는 어떻게 하는가?
```

따라서:

``` text
DB Schema
   ↓
Auto Generate
   ↓
CRUD API
```

는 현실 backend의 어려운 부분을 해결하지 못한다.

------------------------------------------------------------------------

# 3. 핵심 발상

> **사람이 직접 구현해야 했던 복잡한 backend semantics를 문법 자체가
> 표현하고 framework가 그 의미를 이해하게 한다.**

초기 primitive 후보:

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

개념 예:

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

Framework는 다음 의미를 이해해야 한다.

-   누가 실행 가능한가
-   어떤 상태에서 가능한가
-   무엇을 읽고 변경하는가
-   transaction boundary
-   external side effect
-   emitted event
-   result type

------------------------------------------------------------------------

# 4. Java 비유

AIP의 중요한 철학이다.

Java는 불편하지만 잘못된 타입을 실행 전에 차단한다. AIP는 이 철학을
backend semantics 전체로 확대한다.

예:

``` text
ERROR

Potential N+1 query detected:
User.posts.comments

Declare loading strategy:
batch | join | explicit
```

또는:

``` text
Unsafe external side effect detected.

PaymentGateway.charge() is non-idempotent
but the containing command may be retried.

Declare idempotency strategy or disable retries.
```

핵심:

> 자동화했기 때문에 신뢰성을 포기하는 것이 아니라, framework가 전체
> 의미를 알기 때문에 imperative code보다 더 많은 검증을 가능하게 한다.

------------------------------------------------------------------------

# 5. API를 없애는 것이 본질은 아니다

기존:

``` text
Client
  ↓
API Endpoint
  ↓
Controller
  ↓
Service
  ↓
Repository
  ↓
DB
```

AIP:

``` text
Client Intent
      ↓
AIP Runtime
      ↓
Verified Execution Plan
      ↓
Domain / Data / Effects
```

REST syntax를 다른 syntax로 바꾸는 것이 아니다. 사람이 반복적으로
작성하던 구현 계층의 상당 부분을 application semantics +
compiler/runtime으로 흡수하는 것이 핵심이다.

------------------------------------------------------------------------

# 6. AIP --- Application Intent Protocol

현재 가장 선호하는 이름:

**AIP --- Application Intent Protocol**

API가 "어떤 interface를 통해 application을 호출하는가"라면 AIP는
"client가 application에 어떤 intent를 전달하는가"에 초점을 둔다.

``` text
REST:
POST /orders/123/cancel

AIP:
CancelOrder(orderId = 123)
```

`CancelOrder`는 임의 DB operation이 아니라 서버가 사전에 정의하고 검증한
business intent/capability다.

------------------------------------------------------------------------

# 7. Query / Command

CQRS의 semantic distinction은 참고할 가치가 있다.

``` text
Operation
├── Query   // 상태 읽기
└── Command // 상태 변경
```

AIP는 CQRS 구현체가 아니다. 읽기와 쓰기의 의미 구분을 활용하는 것이다.

------------------------------------------------------------------------

# 8. 보안 모델

Frontend/browser/client는 신뢰하지 않는다.

-   frontend bundle은 노출된다.
-   request는 위조 가능하다.
-   custom protocol도 분석될 수 있다.

따라서 목표는 "위조 불가능한 protocol"이 아니다.

> **요청을 위조하더라도 서버가 사전에 허용된 intent 이외의 작업을 수행할
> 수 없게 한다.**

Client에게 다음과 같은 arbitrary DB 표현력을 주지 않는다.

``` ts
backend.query({
    table: "users",
    where: {...}
})
```

대신:

``` ts
aip.query("UserProfile", { userId })
```

`UserProfile`은 server definition에 존재하는 검증된 intent다.

개념 pipeline:

``` text
Request
   ↓
Known Intent?
   ↓
Input Schema Validation
   ↓
Authentication
   ↓
Authorization
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

# 9. Query Planning / N+1

자동 backend가 N+1을 만드는 것은 허용하기 어렵다. 반대로 AIP가 전체
query graph를 안다면 더 많은 분석이 가능하다.

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

Compiler는:

``` text
Order
 └─ Items
      └─ Product
```

를 알고 있으므로 JOIN / batch / subquery / DB-specific strategy 등을
계획하거나 위험을 경고할 수 있다.

"N+1을 무조건 자동 해결한다"는 주장이 아니다. 전체 intent를 알고
있으므로 정적 분석·planner·optimizer hint가 가능하다는 가설이다.

------------------------------------------------------------------------

# 10. 이상적인 사용자 경험

Spring식:

``` text
build.gradle
application.yml
SecurityConfig
Entity
DTO
Controller
Service
Repository
Validation
ExceptionHandler
...
```

AIP 목표:

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

실행:

``` bash
aip run
```

AIP가 가능한 한 책임질 영역:

``` text
Server Bootstrap
DB Connection / Pool
Schema / Migration
Query Planning
CRUD mechanics
Serialization
Validation
Authentication integration
Authorization
Transaction
Error handling
Logging / Observability
Contract generation
Client SDK/type generation
```

핵심 문장:

> **백엔드 코드를 작성해서 서버를 만드는 것이 아니라, 백엔드의 규칙을
> 선언하면 서버가 존재한다.**

사용자가 표현한 이상적인 느낌은 "AIP만 당겨오고 환경변수를 잘 설정하면
default로 서버가 뜨고, 작은 정의 파일에서 규칙/금지/예외만 지정하는
것"에 가깝다.

단 보안상 위험한 외부 노출은 명시하는 방향이 좋다.

> **Safe things may be implicit; dangerous things must be explicit.**

------------------------------------------------------------------------

# 11. Dependency / Extension

Backend는 DB CRUD만 하지 않는다.

-   Redis
-   Kafka
-   S3
-   external HTTP
-   payment
-   mail
-   search
-   queue
-   기타 dependency

이 모든 것을 Core에 내장하지 않는다.

사용자가 선호한 발상:

> **각 library/dependency가 자신의 AIP 문법과 semantics를 함께
> 제공한다.**

``` text
AIP Core
   │
   ├─ PostgreSQL Extension
   │    ├─ DSL grammar
   │    ├─ types
   │    ├─ validation
   │    ├─ planner integration
   │    └─ runtime adapter
   │
   ├─ Redis Extension
   │    ├─ DSL grammar
   │    ├─ types
   │    ├─ validation
   │    └─ runtime adapter
   │
   └─ Kafka Extension
        ├─ DSL grammar
        ├─ event semantics
        ├─ validation
        └─ runtime adapter
```

장기 ecosystem 예:

``` text
aip-postgres
aip-mysql
aip-redis
aip-kafka
aip-s3
aip-stripe
```

Extension은 단순 function library가 아니라 compiler가 이해할 수 있는
syntax/types/capabilities/effects/validation/planner hooks/runtime
behavior를 제공해야 한다.

서드파티 extension이 Core safety model을 우회하면 안 된다.

------------------------------------------------------------------------

# 12. DB Capability Model

DB는 SQL syntax만 다른 것이 아니다.

-   transaction semantics
-   isolation
-   locking
-   JSON
-   indexing
-   RETURNING
-   UPSERT
-   full-text search
-   concurrency

따라서 lowest-common-denominator abstraction을 강요하지 않는다.

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

지원되지 않는 semantics는 조용히 degrade하지 않고 가능한 한
compile/startup 단계에서 실패한다.

``` text
AIP-C214

ReserveStock requires capability `row-lock`.

Current adapter:
    SQLite

PostgreSQL ✓
MySQL      ✓
SQLite     ✗
```

원칙:

> 서로 다른 시스템을 동일하다고 거짓말하는 abstraction을 만들지 않는다.

------------------------------------------------------------------------

# 13. Frontend Integration / AIP Client Library

Frontend가 raw AIP DSL이나 wire protocol을 배울 필요는 없다.

``` text
AIP Application Definition
        ↓
Compiler
        ↓
Contract / Schema
   ↙       ↓       ↘
TypeScript Kotlin  Swift
   SDK      SDK      SDK
```

React:

``` ts
import { aip } from "./generated/aip";

await aip.orders.cancel({ orderId });
```

Library ecosystem:

``` text
@aip/core
@aip/react
@aip/vue
@aip/svelte

aip-kotlin
aip-swift
aip-dart
```

React-specific UX:

``` tsx
const { data } = useQuery("GetMyProfile");
const cancelOrder = useCommand("CancelOrder");
await cancelOrder({ orderId });
```

사용자가 특히 마음에 들어 한 방향이다.

React가 DOM을 대체하지 않고 그 위에 새로운 programming model을 얹은
것처럼 AIP도 HTTP/DB/Redis/Kafka를 없애는 것이 아니라 그 위에 backend
application programming model을 제공한다.

------------------------------------------------------------------------

# 14. Runtime 구현 언어

사용자는 Python/Java 경험이 중심이며 내부 Runtime을 C/C++/Rust 중
무엇으로 구현해야 하는지 고민했다.

장기 Core 후보로 Rust가 매력적이라는 가설이 있다.

-   performance
-   memory safety
-   strong type system
-   Result / Option
-   concurrency safety
-   compiler/runtime 적합성

그러나:

> **PoC부터 Rust로 만들 필요는 없다.**

초기 가장 큰 위험은 CPU 성능이 아니라 잘못된 semantics/abstraction이다.

``` text
Python / Java / TypeScript PoC
        ↓
Semantics 검증
        ↓
IR 안정화
        ↓
Architecture 안정화
        ↓
필요하면 Rust Core
```

AIP Runtime의 역할은 모든 것을 직접 계산하는 것이 아니라 validate / plan
/ coordinate / enforce / call adapters 하는 것이다. 실제 query는 DB가,
cache는 Redis가, messaging은 Kafka가 처리한다.

------------------------------------------------------------------------

# 15. Typed IR

DSL을 Runtime과 직접 결합하지 않는 방향이 중요하다.

``` text
AIP DSL
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
Runtime
```

IR 목적:

-   syntax와 semantics 분리
-   analyzer 구현
-   tooling / IDE
-   deterministic compilation
-   여러 runtime target
-   LLM structured generation 가능성

장기 가능성:

``` text
          AIP Definition
                 ↓
              AIP IR
        ↙         ↓        ↘
Native Runtime  WASM   Serverless
```

DSL의 예쁜 syntax보다 IR과 semantic model이 먼저다.

------------------------------------------------------------------------

# 16. Standalone vs Embedded Runtime

아직 미결정.

Embedded:

``` text
Python ─┐
Node ───┼─→ Binding → AIP Core
Java ───┘
```

Standalone:

``` text
Application Definition
        ↓
     AIP Runtime
      ├─ DB
      ├─ Redis
      ├─ Kafka
      └─ External Systems
```

장기적으로 Standalone Runtime이 철학적으로 깔끔할 가능성이 있지만 custom
logic, extension ABI, process boundary 때문에 검증이 필요하다.

------------------------------------------------------------------------

# 17. Side Effect / Distributed Systems

AIP가 장난감이 되지 않으려면 반드시 다뤄야 한다.

``` text
transaction {
    DB.save(order)
    Redis.delete(cache)
    Kafka.publish(event)
}
```

이것을 keyword 하나로 모두 원자적이라고 가장하면 안 된다.

문제:

-   Kafka 성공 / DB rollback
-   Payment 성공 / server crash
-   cache invalidation 실패
-   duplicate event
-   retry
-   at-least-once
-   partial failure

필요한 semantic 후보:

-   DB transaction
-   outbox
-   saga / compensation
-   retry
-   idempotency
-   delivery guarantee
-   external side effect
-   crash recovery

예:

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

Compiler가 위험한 조합을 검출해야 한다.

------------------------------------------------------------------------

# 18. DSL Explosion 위험

현실 요구를 모두 DSL에 추가하다 보면:

``` text
condition
loop
function
async
exception
module
variable
```

가 생겨 결국 또 하나의 범용 언어가 될 수 있다.

핵심 연구 질문:

> **작은 primitive 집합으로 실제 backend business logic의 대부분을
> 표현할 수 있는가?**

초기 가설:

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

로 80\~90%를 표현하고 특수한 부분만 controlled escape hatch로 처리할 수
있는지 검증한다.

Escape hatch의 언어, isolation, effect declaration, transaction
integration, policy 우회 방지 등은 미결정이다.

------------------------------------------------------------------------

# 19. AI / LLM First-Class Consumer

AIP가 성공한 미래에는 client/server 코드를 LLM이 작성할 가능성이 높다.

따라서:

-   human-readable
-   machine-generatable
-   statically verifiable
-   canonical
-   deterministic
-   introspectable

해야 한다.

특히:

> **One intent, one canonical expression.**

같은 의미를 여러 syntax로 표현하게 하지 않는다.

나쁜 예:

``` text
allow: owner
policy: owner
authorize if currentUser == resource.owner
```

가능하면 하나의 canonical form만 둔다.

AI에게 자유를 많이 주는 것이 목표가 아니다. AI가 실수할 선택지를 줄이는
것이 목표다.

------------------------------------------------------------------------

# 20. Introspection / Contract

AIP server는 자신을 machine-readable하게 설명해야 한다.

``` ts
const schema = await aip.describe();
```

개념 결과:

``` text
Order
 ├─ query get
 ├─ query list
 └─ command cancel
      ├─ input: CancelOrderInput
      ├─ output: Order
      └─ errors:
           ORDER_NOT_FOUND
           NOT_OWNER
           ALREADY_SHIPPED
```

동일 contract를 다음이 사용한다.

-   Human
-   Generated SDK
-   IDE
-   Test Tool
-   LLM / Agent

LLM이 처음 보는 AIP server에서도 describe → intent 이해 → type-safe
client code generation이 가능해야 한다.

------------------------------------------------------------------------

# 21. Structured Error

문자열 오류 대신 표준화된 구조를 지향한다.

``` json
{
  "code": "AIP.AUTH.FORBIDDEN",
  "operation": "Order.Cancel",
  "reason": "OWNER_REQUIRED",
  "path": "order.owner",
  "retryable": false
}
```

Human, SDK, app code, logger, LLM이 같은 error contract를 이해할 수
있어야 한다.

------------------------------------------------------------------------

# 22. GraphQL / BaaS와의 차이

GraphQL은 주로 data shape query를 표현하지만 Resolver/Service/Business
Logic은 여전히 구현한다.

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

AIP는 이 아래의 반복 구현 계층과 business semantics 자체까지 다룬다.

기존 BaaS/auto backend가 흔히:

``` text
DB Schema
   ↓
CRUD API
```

라면 AIP는:

``` text
Domain
+
Business Intent
+
Policy
+
Invariant
+
Transaction
+
Effect
+
Consistency
      ↓
Verified Application Model
      ↓
Runtime
```

을 지향한다.

------------------------------------------------------------------------

# 23. Design System 아이디어와 공통 철학

UI:

``` text
AI
 ↓
Constrained Design Model
 ↓
Validator / Composer
 ↓
Consistent UI
```

Backend:

``` text
AI
 ↓
Constrained Application Model
 ↓
Compiler / Analyzer
 ↓
Verified Backend
```

공통 철학:

> **AI에게 규칙을 잘 지키라고 부탁하지 않는다. 좋은 결과만 만들기 쉬운
> 구조를 시스템이 제공한다.**

두 프로젝트를 지금 합칠 필요는 없다.

------------------------------------------------------------------------

# 24. 절대로 장난감으로 축소하지 말 것

다음 결과를 만들고 AIP PoC라고 부르지 않는다.

``` text
app.aip
↓
Parser
↓
FastAPI endpoint 자동 생성
↓
SQLite CRUD
```

또는:

``` text
entity User
entity Post
↓
CRUD endpoint generation
```

Todo App도 핵심 검증이 아니다.

**PoC 범위를 줄이는 것과 AIP가 풀려는 문제 자체를 CRUD 생성 문제로
바꾸는 것은 완전히 다르다.**

좋은 vertical slice:

``` text
PostgreSQL only
+
Order domain only
+
HTTP/JSON only
```

이건 괜찮다. 범위를 줄였지만 다음 핵심은 유지한다.

``` text
Business Intent
Policy
Invariant
Transaction
Query Planning
Side Effect
Static Analysis
Generated Contract
Client Integration
```

------------------------------------------------------------------------

# 25. 실제 PoC는 일부러 어려워야 한다

최소 검증 요소:

``` text
Entity Relation
Complex Authorization
State-dependent Authorization
Invariant
Transaction
N+1 Candidate
Cache
External API
Event / Queue
Concurrency
Retry
Idempotency
Partial Failure
Structured Error
```

Order domain 예:

``` text
CreateOrder
CancelOrder
PayOrder
ReserveStock
ReleaseStock
ApplyCoupon
RestoreCoupon
PublishOrderCreated
InvalidateOrderCache
```

Feature 수가 중요한 게 아니다. 현실 backend의 어려운 부분을 AIP
semantics가 실제로 표현하고 검증할 수 있는지가 중요하다.

------------------------------------------------------------------------

# 26. 구현 전에 할 일

먼저 현실 backend use case 10\~20개를 수집한다. 가능하면 사용자가 직접
과거 구현한 복잡한 logic을 활용한다.

각 case마다:

``` text
AIP primitive만으로 표현 가능한가?
새 primitive가 필요한가?
새 primitive는 일반화 가능한가?
DSL이 범용 언어로 팽창하지 않는가?
Static Analyzer가 무엇을 보장하는가?
Runtime에서만 검증 가능한 것은 무엇인가?
```

를 분석한다.

------------------------------------------------------------------------

# 27. Static Analyzer

Runtime만큼 중요할 수 있다.

검토 후보:

``` text
Type mismatch
Unknown field
Unknown capability
Authorization missing
Impossible policy
Potential N+1
Unsupported DB capability
Unsafe retry
Non-idempotent effect
Invalid transaction boundary
Unreachable state transition
Missing compensation
Schema mismatch
Client/server contract mismatch
```

과장하지 말고 다음으로 분류한다.

``` text
Guaranteed statically
Warned statically
Verified at startup
Verified at runtime
Impossible to guarantee
```

------------------------------------------------------------------------

# 28. Explainable Execution Plan

자동화가 opaque magic이 되면 안 된다.

예:

``` text
CancelOrder Execution Plan

1. Authenticate user
2. Load Order
3. Check owner/admin policy
4. Verify status
5. Begin DB transaction
6. Restore inventory
7. Restore coupon
8. Update order status
9. Write outbox event
10. Commit
11. Publish event
12. Return Order
```

장래 UX 예:

``` bash
aip explain CancelOrder
```

Trust, performance debugging, production debugging에 중요하다.

------------------------------------------------------------------------

# 29. Observability / Migration / Versioning

AIP가 전체 semantics를 안다는 장점을 활용할 수 있는 후보 영역이다.

Observability 후보:

-   operation latency
-   DB query count
-   generated SQL
-   cache hit/miss
-   external effect latency
-   retry
-   transaction duration
-   policy rejection
-   structured errors

Migration 질문:

-   Entity 변경 → migration 생성?
-   destructive migration → explicit approval?
-   migration explain?
-   production auto-apply?
-   DB-specific capability?

Versioning 질문:

-   Command input 변경
-   Query output 삭제
-   type 변경
-   policy 강화
-   event schema 변경
-   extension version 변경

Compiler가 breaking change를 탐지할 가능성을 검토한다.

------------------------------------------------------------------------

# 30. 가장 큰 장점 후보

-   프로젝트별 architecture 편차 감소
-   AI code generation reliability 증가
-   전체 application model 기반 static safety
-   machine-readable introspection
-   generated client로 contract drift 감소
-   execution plan explainability
-   dependency extension ecosystem
-   반복 backend boilerplate 제거

------------------------------------------------------------------------

# 31. 가장 큰 위험 후보

-   DSL Explosion
-   Leaky Abstraction
-   Runtime Magic
-   Performance Surprise
-   Extension Unsafety
-   Distributed Systems Oversimplification
-   Escape Hatch Dominance
-   Learning Cost

이 위험들을 숨기지 말고 PoC에서 공격적으로 검증한다.

------------------------------------------------------------------------

# 32. 현재 강하게 선호되는 방향 / 미결정 사항

## 강하게 선호

-   AIP = Application Intent Protocol
-   단순 Auto API가 아님
-   declarative application semantics
-   Query / Command distinction
-   Policy / Invariant / Transaction / Event / Effect
-   Static Analyzer
-   Typed IR
-   hostile client assumption
-   no arbitrary client DB query
-   generated client / contract
-   React 등 client library
-   extension architecture
-   DB capability model
-   LLM first-class consumer
-   canonical syntax
-   structured error
-   explainable execution
-   dangerous behavior explicit
-   현실적인 복잡한 PoC

## 미결정

-   최종 DSL syntax
-   최종 IR schema
-   Runtime 구현 언어
-   Rust 확정 여부
-   wire transport
-   custom binary protocol
-   standalone vs embedded
-   authentication 구현
-   migration semantics
-   exact query planner
-   extension ABI
-   escape hatch
-   distributed transaction model
-   client cache semantics

미결정 사항을 임의로 "확정된 요구사항"처럼 취급하지 않는다.

------------------------------------------------------------------------

# 33. Agent에게 요구하는 태도

아이디어를 무조건 긍정하지 않는다.

반대로 기존 기술과 비슷한 부분이 있다는 이유만으로:

``` text
Hasura가 이미 함
GraphQL이 이미 함
Supabase가 이미 함
```

으로 끝내지도 않는다.

비교 시 정확한 layer를 분석한다.

``` text
data access만 자동화하는가?
business semantics를 표현하는가?
authorization semantics를 compiler가 이해하는가?
side effect를 모델링하는가?
transaction/retry/idempotency를 분석하는가?
generated client contract가 있는가?
LLM canonical generation을 목표로 하는가?
```

------------------------------------------------------------------------

# 34. Agent에게 첫 번째로 시킬 작업

**코드부터 작성하지 않는다.**

먼저 다음 산출물을 만든다.

## A. Reality Test

현실 backend use case 15개. 단순 CRUD 금지.

각 case:

``` text
Requirements
Existing Spring-style implementation complexity
AIP representation hypothesis
Required primitive
Static guarantees
Runtime guarantees
Failure cases
Potential abstraction leak
```

## B. Semantic Core

최소 primitive 집합을 제안한다.

각 primitive마다:

``` text
왜 필요한가?
기존 primitive 조합으로 불가능한가?
일반화 가능한가?
LLM이 안정적으로 사용할 수 있는가?
```

## C. Typed IR Draft

Syntax보다 semantics 중심.

## D. Safety Matrix

``` text
Issue
Static Error
Static Warning
Startup Validation
Runtime Validation
Cannot Guarantee
```

## E. Execution Model

Query / Command / Effect의 실제 실행 단계를 설계한다.

그 이후에 최소 Runtime PoC로 간다.

------------------------------------------------------------------------

# 35. 사용자가 원하는 최종 그림

기존:

``` text
Requirement
   ↓
Human / AI
   ↓
Thousands of lines of framework code
   ↓
Framework conventions
   ↓
Backend
```

AIP:

``` text
Requirement
   ↓
Human / AI
   ↓
Canonical Application Intent Definition
   ↓
Compiler
   ↓
Verified Typed IR
   ↓
Static Analyzer
   ↓
Execution Planner
   ↓
AIP Runtime
   ↓
Complete Backend
```

Client:

``` text
React / Vue / Mobile / Agent
             ↓
      AIP Client Library
             ↓
       Generated Contract
             ↓
          AIP Runtime
```

Infrastructure:

``` text
                  AIP Runtime
                      │
        ┌─────────────┼─────────────┐
        ↓             ↓             ↓
   PostgreSQL       Redis         Kafka
    Extension      Extension     Extension
        │             │             │
     DB Engine      Redis       Kafka Broker
```

------------------------------------------------------------------------

# 36. 가장 중요한 문장

> **AIP는 API generator가 아니다.**

> **AIP는 client에게 DB를 자유롭게 query하게 하는 시스템이 아니다.**

> **AIP는 REST syntax를 다른 syntax로 바꾸는 프로젝트가 아니다.**

> **AIP의 핵심은 business semantics를 framework가 이해하는 것이다.**

> **AIP는 backend boilerplate를 제거하되 correctness-critical
> semantics는 숨기지 않는다.**

> **자동화했기 때문에 신뢰성을 포기하는 것이 아니라, 전체 application
> model을 알기 때문에 더 많은 검증을 가능하게 한다.**

> **Client는 항상 hostile할 수 있다고 가정한다.**

> **Safe things may be implicit; dangerous things must be explicit.**

> **One intent, one canonical expression.**

> **Unsupported semantics should fail early, not silently degrade.**

> **AI에게 잘하라고 부탁하지 않는다. AI가 잘못할 수 있는 공간을
> 제한한다.**

> **예쁜 DSL보다 올바른 semantic model과 Typed IR이 먼저다.**

> **Todo CRUD를 성공시킨 것은 AIP를 검증한 것이 아니다.**

> **현실의 더러운 backend case를 버텨야 AIP다.**

------------------------------------------------------------------------

# 37. One-line Definition

> **AIP (Application Intent Protocol) is an AI-native declarative
> backend programming model in which clients express pre-defined
> application intent, while a compiler-verified runtime owns repetitive
> backend implementation, safety enforcement, execution planning,
> dependency integration, and machine-readable contracts.**

------------------------------------------------------------------------

# 38. 마지막 인계 지시

범위를 줄여 PoC를 만드는 것은 가능하다. 하지만 반드시 **전체 비전을
검증하기 위해 의도적으로 잘라낸 vertical slice**여야 한다.

``` text
PostgreSQL only
Order domain only
HTTP/JSON only
```

처럼 infrastructure 범위를 줄이는 것은 괜찮다.

반대로:

``` text
Entity DSL
+
CRUD Generator
```

는 범위를 줄인 것이 아니라 **AIP가 풀려는 문제 자체를 다른 문제로 바꾼
것**이다.

## Agent Checklist Before Writing Code

-   [ ] AIP와 CRUD generator의 차이를 설명할 수 있는가?
-   [ ] AIP와 GraphQL의 차이를 설명할 수 있는가?
-   [ ] 왜 client에게 arbitrary query capability를 주지 않는지
    이해했는가?
-   [ ] 왜 Static Analyzer가 Runtime만큼 중요한지 이해했는가?
-   [ ] 왜 Typed IR을 DSL보다 먼저 고민하는지 이해했는가?
-   [ ] 왜 Extension이 단순 library wrapper가 아닌지 이해했는가?
-   [ ] 왜 DB capability model이 필요한지 이해했는가?
-   [ ] 왜 distributed side effect를 단순 transaction으로 취급하면 안
    되는지 이해했는가?
-   [ ] 왜 canonical syntax가 LLM 시대에 중요한지 이해했는가?
-   [ ] 왜 Todo CRUD PoC가 충분하지 않은지 이해했는가?
-   [ ] PoC가 전체 비전의 어떤 위험 가설을 검증하는지 설명할 수 있는가?

모두 답할 수 있을 때 설계를 시작한다.
