# AIP PoC

AIP(Application Intent Protocol) 설계 초안(`docs/design_draft.md`)을 검증하기 위한 PoC입니다.
DSL로 entity·query·command·policy·invariant를 선언하면 컴파일러가 정적 분석을 하고, 런타임이 PostgreSQL 위에서 HTTP/JSON으로 실행합니다.

- 언어: TypeScript (Node 26 type stripping으로 빌드 없이 실행)
- DB: PostgreSQL 전용 (로컬 17 기준 확인)
- 상태: iteration 1. 설계 검증용이며 production 사용 불가

## 빠른 실행

```bash
npm install
npm test                                        # 분석기 15 + Postgres 통합 7 + HTTP e2e 3
node src/cli.ts check examples/shop/app.aip     # 정적 분석
node src/cli.ts plan examples/shop/app.aip      # 쿼리 실행 계획
createdb aip_dev
node src/cli.ts migrate examples/shop/app.aip --reset
node src/cli.ts run examples/shop/app.aip --port 4000
curl localhost:4000/aip/describe
```

테스트는 `aip_test` DB를 매번 새로 만듭니다. 접속 정보는 `AIP_TEST_ADMIN_URL`(기본 `postgres://localhost/postgres`), 런타임은 `DATABASE_URL`(기본 `postgres://localhost/aip_dev`)을 씁니다.

## 구조

```
src/
  ir.ts            Typed IR (JSON 직렬화 가능, DSL과 분리)
  lexer.ts parser.ts   DSL → IR. 절 순서를 강제해 표현을 하나로 고정
  analyzer.ts      이름·타입·정책·race·idempotency 정적 분석
  schema.ts        entity → DDL. invariant는 CHECK, ref는 FK + index
  planner.ts       query → 관계 edge당 1개 SQL (batch), explain
  contract.ts      /aip/describe 계약 (입력·출력·정책·effect·에러 reason)
  clientgen.ts     계약 → TypeScript 클라이언트
  runtime/         command 실행, 입력 검증, 에러 매핑, HTTP 서버
examples/shop/     주문 도메인 예제와 생성된 클라이언트
```

## DSL 요약 (iteration 1)

```
actor User
enum OrderStatus { PENDING PAID CANCELLED }

entity Order {
  user: User                       // many-to-one, 컬럼 user_id + FK + index
  status: OrderStatus
  total: Int
  items: OrderItem[] via order     // one-to-many 역참조
  invariant total_non_negative: total >= 0
}

query OrderDetail(orderId: UUID) {
  from Order as o
  by orderId                       // 또는 where <expr>
  policy: owner(o.user) | role(ADMIN)
  select { id status items { quantity product { name } } }
}

command CancelOrder(orderId: UUID) {
  load order: Order by orderId lock
  policy: owner(order.user) | role(ADMIN)
  require order.status in [PENDING, PAID] else ORDER_NOT_CANCELLABLE
  transaction {
    each order.items as item { increment item.product.stock by item.quantity }
    set order.status = CANCELLED
  }
  emit OrderCancelled { orderId: order.id }
  returns order
}
```

command 절 순서는 `idempotent → load → policy → require → transaction → emit → returns`, query는 `from → by|where → sort → limit → policy → select`로 고정입니다.

## 런타임 의미론

- command 하나는 DB 트랜잭션 하나입니다. load(`lock`이면 `FOR UPDATE`), policy, require, 변경, outbox 기록, idempotency 기록이 한 트랜잭션에 들어갑니다.
- `increment`는 `col = col + n`으로 실행되어 lock 없이도 원자적입니다.
- `each`는 행마다 돌지 않고 문장당 UPDATE 1개로 컴파일됩니다. 참조를 거친 증감은 대상별로 SUM 후 적용합니다(같은 상품이 두 줄이어도 정확).
- query는 관계 edge마다 `= ANY($ids)` 배치 조회 1개입니다. 왕복 수는 select 모양으로 정해지고 행 수와 무관합니다. 여러 SQL은 `REPEATABLE READ READ ONLY` 스냅샷 하나에서 읽습니다.
- list query의 policy는 SQL WHERE로 내려갑니다. 볼 수 없는 행은 읽지도 않습니다.
- `emit`은 같은 트랜잭션에서 `_aip_outbox`에 기록됩니다. dispatcher는 아직 없습니다.
- `idempotent` command는 `Idempotency-Key`가 필수이고, 같은 키의 동시 요청은 한 번만 실행됩니다. 같은 키에 다른 입력이면 `AIP.IDEMPOTENCY.KEY_REUSED`입니다.
- invariant 위반은 DB CHECK에서 막히고 `AIP.INVARIANT.VIOLATED` + invariant 이름으로 돌아갑니다.

## 정적 분석 규칙

| 코드 | 내용 |
|---|---|
| E100 | 문법 오류, 절 순서 위반 |
| E101-E106 | 중복 이름, 모르는 타입·필드·이름, `id` 직접 선언, 잘못된 `via` |
| E110 | actor 미선언 상태에서 actor/인증 정책 사용 |
| E201-E216 | 타입 불일치, 모르는 enum 값, 대입 불가 필드, create 필수 필드 누락, `each` 제약, 이벤트 스키마 불일치 등 |
| E213 | 관계를 건너 읽기(`a.b.c`). 암묵적 로딩 금지 |
| E220 | 관계 필드를 sub-selection 없이 select |
| E301 | policy 없는 query/command |
| E303-E305 | 잘못된 owner/role 정책 |
| E401 | check-then-act race: require로 확인한 필드를 lock 없이 변경 |
| W302 | 비인증 호출 가능한 command |
| W403 | limit 없는 list query |
| W501 | 행을 만드는 command가 idempotent가 아님 |

## iteration 1에서 확인한 것

설계 초안의 가설 중 실제로 성립한 것과 부딪힌 것을 적습니다.

성립한 것
- 암묵적 로딩을 금지하고 query가 데이터 그래프 전체를 선언하게 하면 N+1은 "검출"이 아니라 구조적으로 불가능해집니다.
- check-then-act race(E401), idempotency 누락(W501), 정책 누락(E301)은 싸게 정적 검출됩니다. 일반 프레임워크에서는 코드 리뷰에 의존하는 부분입니다.
- 계약에 command별 writes·emits·에러 reason을 자동으로 넣을 수 있고, 생성된 클라이언트가 reason까지 타입으로 받습니다.

부딪힌 것 (다음 설계 결정 필요)
1. 주문 취소 하나에 이미 반복(`each`)이 필요했습니다. set 기반 UPDATE로만 허용해 범용 루프가 되는 건 막았지만, DSL 팽창 압력은 첫 도메인부터 나타납니다.
2. 관계를 거친 정책(`owner(part.box.owner)`)을 쓰려면 `load`를 더 써야 합니다. multi-tenant 서비스에서는 이게 기본이라 표현 비용이 큽니다.
3. lock과 check를 붙여두려면 command 전체가 트랜잭션이어야 해서, 단일 DB에서는 `transaction {}` 블록의 의미가 약합니다. 이 primitive의 진짜 쓰임은 외부 effect와의 경계일 가능성이 큽니다.
4. load가 policy보다 먼저라서 권한 없는 사용자도 NOT_FOUND와 FORBIDDEN 차이로 존재 여부를 알 수 있습니다. 숨길지 정책으로 정해야 합니다.
5. `each` 안의 다중 행 UPDATE는 동시 취소끼리 deadlock이 날 수 있습니다(추론, 재현 테스트 없음. 확인 필요). 지금은 `AIP.CONCURRENCY.CONFLICT`(retryable)로 돌려줍니다.

## 알려진 한계

- 인증은 dev 전용입니다. `x-aip-actor` 헤더의 사용자 id를 그대로 믿습니다.
- migration은 `--reset`(전체 drop 후 재생성)만 됩니다.
- 외부 API effect, retry/compensation, Redis, Kafka, outbox dispatcher, extension 모델은 아직 없습니다.
- 쿼리 필터는 root의 직접 필드만 쓸 수 있고, 집계·페이지네이션 커서는 없습니다.

## 다음 후보

1. 외부 effect primitive (`external`, retry/idempotency/compensation 선언과 E3021류 검사), outbox dispatcher
2. 상태 전이 선언 (`transitions`)으로 status 관련 require 반복 제거
3. 관계 경유 정책의 표현 방식 결정 (위 2번)
4. 설계 초안 22절의 실제 프로젝트 비즈니스 로직을 가져와 표현력 검증
5. LLM에게 요구사항만 주고 `.aip`를 생성시켜 편차와 compiler error 자가수정률 측정
