# 표현력 한계와 확장 우회

AIP 1.0의 선언(`.aip`)은 읽기, 필터·정렬·페이지, 관계, 행·필드 권한, 상태 전이, 부분 수정, 원자 묶음을 서버 코드 없이 표현한다. 아래 E1~E9는 실제 앱에서 확인한 표현력 제약과 현재 경로다. E1·E2·E7은 PostgreSQL 제품 경로에서 개선됐다. 남은 제약에는 공식 서버 확장(`extension write`)이나 명시적인 호출 순서를 사용한다. 어떤 기능이 1.0 지원인지는 [지원 범위](SUPPORT.md)를 본다.

이 한계는 두 앱을 새로 설계해 끝까지 구현하면서 찾았다(다중 사용자 할일 공유, 테넌트별 재고·주문). 앱마다 확장 코드는 22~24줄이었고 서버 Controller·DTO·Service·Repository 코드는 없었다. 아래 예시 중 "실행"으로 표시한 것은 이 문서를 쓰며 실제 PostgreSQL 17과 `aip service serve`로 돌린 것이고, "check"는 `aip service check`만 통과시킨 것이다.

| # | 한계 | 우회 | 확인 |
| --- | --- | --- | --- |
| E1 | 부모·자식 원자 생성 | 한 `bundle`에서 앞 create의 ID를 `refs`로 전달 | HTTP·PostgreSQL |
| E2 | 가변량 증감 | 전이 `input`의 양수 항을 잠근 행 값에 더하거나 뺀다 | HTTP·PostgreSQL |
| E3 | 계산 집계(`Σ 수량×단가`) 불가 | 확장이 계산해 비정규화 필드에 저장 | 실행 |
| E4 | 참조 생성은 대상 행을 읽을 수 있어야 한다 | 디렉터리는 공개하고 민감 필드만 필드 정책으로 가린다 | check |
| E5 | cursor 페이지는 nullable 정렬과 함께 못 쓴다 | offset 페이지 | 실행(오류), check(우회) |
| E6 | 전이의 `from`은 필수다 | `from true` | check |
| E7 | PostgreSQL 쓰기 확장의 원자 생성 | `access create.<Resource>`와 `ctx.data.create` | HTTP·PostgreSQL |
| E8 | 확장 입력에 배열 타입이 없다 | `Json`으로 받고 확장이 모양을 검증한다 | 실행(오류), 소비자 앱 변형 B |
| E9 | 묶음의 연산 순서가 잠금 순서다 | 같은 행을 갱신하는 연산을 묶음 앞에 둔다 | PostgreSQL 동시 시험 |

## 우회에 쓰는 확장의 모양

확장은 정의에 계약을 선언하고, 서버가 격리된 worker에서 실행한다. 확장이 쓸 수 있는 읽기와 전이는 `access`에 적은 것뿐이다. 호출자가 직접 부를 수 없는 전이는 `expose apply … internal`로 표시하면 확장만 부를 수 있다. 서버는 확장 안의 읽기와 쓰기에도 호출자의 행·필드 정책과 계약을 그대로 적용하고, 호출 전체를 한 트랜잭션과 바깥 멱등 키 아래에서 커밋한다.

```aip
extension write place {
  input { id: Order.Id }
  output { id: Order.Id; total: Int }
  access read.OrderLine, read.Product, Product.setStock, Order.finish   // 읽을 resource와 부를 전이
  effect db
  deadline 2s
  implementation "shop.place"
}
```

설정은 `"workers": {"directory": "./extensions", "language": "node", "enable_writes": true}`다. 확장 자체의 계약, 격리, `ctx` 호출은 [관리형 실행](MANAGED-EXECUTION.md)을 따르고, 확장 ABI는 1.0에서 확정 전이다([지원 범위](SUPPORT.md#지원-범위-표) 16번). 아래 초기 주문 예제는 `ctx.data.read`와 `ctx.data.apply`만 사용했다. `ctx.data.apply(resource, transition, ids, key, values, expect)`의 `key`는 `undefined`로 둔다(바깥 요청의 멱등 키가 이미 확장 호출 전체를 덮는다).

아래 E1~E3는 하나의 앱(재고와 주문)으로 실행했다.

```aip
actor Member
resource Member { fields { id: Id } rows read when id = actor }
resource Product {
  fields { id: Id; sku: Text(1..40); stock: Int(0..1000000) }
  rows read when actor != null
  transition setStock { allow actor != null; from true; patch stock; expect stock }
  expose read { select id, sku, stock; filter id.eq; sort id; budget { rows 50; depth 1; deadline 1s; cost 100 } }
  expose apply setStock { target id; bulk maxRows 1; internal }
}
resource Order {
  fields { id: Id; owner: Member; total: Int(0..1000000000); placed: Bool }
  rows read when owner = actor
  transition finish { allow owner = actor; from placed = false; patch total; to placed = true }
  expose read { select id, total, placed; filter id.eq; sort id; budget { rows 50; depth 1; deadline 1s; cost 100 } }
  expose create { allow owner = actor; fields owner, total, placed }
  expose apply finish { target id; bulk maxRows 1; internal }
  extension write place {
    input { id: Order.Id }
    output { id: Order.Id; total: Int }
    access read.OrderLine, read.Product, Product.setStock, Order.finish
    effect db
    deadline 2s
    implementation "shop.place"
  }
}
resource OrderLine {
  fields { id: Id; order: Order; product: Product; qty: Int(1..1000); unitPrice: Int(0..1000000) }
  rows read when order.owner = actor
  expose read { select id, qty, unitPrice, product; filter order.eq; sort id; budget { rows 50; depth 1; deadline 1s; cost 100 } }
  expose create { allow order.owner = actor; fields order, product, qty, unitPrice }
}
```

`Product.setStock`과 `Order.finish`는 `internal`이라 호출자가 직접 부르면 `WIRE.NOT_EXPOSED`(`Product.setStock는 공개된 쓰기 동작이 아님`)이다. 확장만 호출할 수 있어서, 호출자가 재고에 임의 값을 쓰는 통로가 되지 않는다.

## E1. 부모·자식 원자 생성

`bundle`의 앞 create가 돌려준 ID를 뒤 create의 참조 필드에 넣을 수 있다. TypeScript의 `b.create()`가 반환하는 핸들은 생성한 리소스 타입을 보존한다. 서버는 연산 순서, 참조 타입, 호출자의 행 가시성, 각 create의 허용 규칙을 검사한다. 주문 줄 생성이나 재고 차감이 실패하면 같은 묶음의 주문도 롤백된다.

```ts
await client.bundle((b) => {
  const order = b.create("Order", { owner: memberId, total: 700, placed: true });
  b.create("OrderLine", { product: productId, qty: 3, unitPrice: 200 }, { order });
  b.apply({ apply: "Product.take", target: { ids: [productId] }, input: { qty: 3 } });
}, { key: "order-1" });
```

이 예제의 리소스와 타입은 [주문 fixture](tests/fixtures/expr-apps/orders/orders.target.aip)를 기준으로 한다. 주문 합계를 서버에서 계산하려면 E3의 확장 경로를 사용한다. 참조 가능한 범위는 [SDK 묶음과 조합](SDK.md#묶음bundle과-조합compose)을 본다.

## E2. 입력 수량을 이용한 원자 증감

전이에서 `input { qty: Int(1..1000) }`, `from stock >= input.qty`, `to stock = stock - input.qty`로 재고를 차감한다. PostgreSQL은 대상을 잠근 뒤 현재 재고를 검사하고, 범위를 벗어나거나 계산 대상 또는 항이 NULL이면 해당 쓰기를 거절한다. 제한 산술의 허용 타입, 항, 조건 범위는 [정의 문법](../spec/definition-syntax.md)을 따른다.

예전의 읽기·계산·`patch expect` 경로는 서버 코드에서 합계를 계산하는 예제로도 사용할 수 있다. `expect stock`은 읽은 뒤 재고가 바뀌었으면 `WIRE.CONFLICT`로 거절한다. 다음 코드는 초기 주문 fixture의 흐름이다.

```js
// extensions/shop.mjs
export async function place(input, ctx) {
  const lines = await ctx.data.read({ read: "OrderLine", select: ["qty", "unitPrice", "product"],
    filter: [{ field: "order", op: "eq", value: input.id }] });
  if (lines.rows.length === 0) throw new Error("empty order");
  let total = 0;
  for (const line of lines.rows) {
    const productId = String(line.product.id ?? line.product);
    const page = await ctx.data.read({ read: "Product", select: ["id", "stock"],
      filter: [{ field: "id", op: "eq", value: productId }] });
    const stock = page.rows[0].stock;
    if (stock < line.qty) throw new Error("out of stock");
    await ctx.data.apply("Product", "setStock", [productId], undefined, { stock: stock - line.qty }, { stock });
    total += line.qty * line.unitPrice;
  }
  await ctx.data.apply("Order", "finish", [input.id], undefined, { total }, undefined);
  return { id: input.id, total };
}
```

실행 결과: 재고 10과 5에서 주문(3개, 2개) 뒤 7과 3이 됐다. 둘째 줄의 재고가 모자라는 주문(줄 1은 2개, 줄 2는 100개)은 호출자에게 `WIRE.EXTENSION_ERROR`(`공식 WRITE 확장 호출 거부`)를 돌려주고, 첫 줄의 차감도 되돌아가 재고는 7과 3 그대로, 주문은 `placed: false`로 남았다. `ctx.data.read`의 필터는 호출 대상 resource의 `expose read … filter`에 있는 항목만 쓸 수 있다(위 정의의 `order.eq`, `id.eq`). 목록에 없으면 호출이 거부되고 그 호출 전체가 실패 처리된다. 확장이 예외를 잡아도 되돌릴 수 없다. 이 초기 예제에서 재고 부족을 일반 예외로 던졌을 때 호출자는 `WIRE.EXTENSION_ERROR`를 받았다. 업무 거절 코드를 반환하는 예제는 [확장 예제](tests/fixtures/expr-apps/orders/extensions/shopb.mjs)를 본다.

## E3. 계산 집계 불가

`release`는 `count`와 단일 필드의 `sum`, `min`, `max`뿐이다. `Σ 수량×단가` 같은 식 집계는 없다.

우회: 확장이 계산해서 값을 비정규화 필드에 저장한다. 위 `place`가 `total`을 합산해 `Order.finish`의 `patch total`로 쓴다. 실행 결과는 줄 `3×200 + 2×50`에서 `total: 700`이다. 저장된 합계는 줄을 바꾸는 다른 경로가 생기면 어긋날 수 있으므로, 줄을 바꾸는 경로를 이 확장 하나로 제한한다.

## E4. 참조 생성은 대상 행을 읽을 수 있어야 한다

`create`의 참조 값은 호출자가 **읽을 수 있는 행**이어야 한다. 안 보이는 행은 없는 행과 같은 `WIRE.MISSING_TARGET`이다. 목록 공유처럼 다른 사용자를 지정하려면 사용자 디렉터리를 읽을 수 있어야 한다. `Member`를 본인만 읽게 만들면 공유 생성이 항상 `MISSING_TARGET`으로 실패한다.

우회: 디렉터리 행은 읽게 하고, 민감한 필드만 필드 정책으로 가린다. 다른 사람의 `email`은 null로 보인다.

```aip
actor Member
resource Member {
  fields { id: Id; email: Email }
  rows read when actor != null
  field email read when id = actor.id
  expose read { select id, email; sort id; budget { rows 20; depth 1; deadline 1s; cost 100 } }
}
```

check로 통과를 확인했다. 필드 정책이 걸린 필드(`email`)는 filter, sort, 집계에 쓸 수 없다.

## E5. cursor 페이지는 nullable 정렬과 함께 못 쓴다

마감일처럼 nullable 필드로 정렬하는 목록에 cursor를 켜면 거부된다.

```
DEF.CURSOR_NULLABLE_SORT  cursor를 켜면 nullable 필드 `dueAt`는 sort에 쓸 수 없음
```

우회: `cursor` 대신 `offset N`을 쓴다(`budget { …; offset 200 }`). offset 페이지는 상한이 있어 뒤쪽 페이지를 깊게 넘길 수 없다. 거부 오류는 실행했고 offset 정의는 check로 통과를 확인했다. 정렬 필드를 non-null로 두는 방법(예: 마감일 없음을 먼 미래 값으로 저장)도 있으나 의미를 앱이 정해야 한다.

## E6. 전이의 `from`은 필수다

`patch`만 있는 전이도 `from`과 `allow`가 필수다. 빠지면 `DEF.MISSING_ITEM`이다. "조건 없는 수정"은 `from true`로 쓴다(`from version >= 0` 같은 항진식을 만들 필요가 없다).

```aip
transition setStock { allow actor != null; from true; patch stock; expect stock }
```

위 E1~E3 정의가 이 형태이고 check와 실행을 통과했다.

## E7. PostgreSQL 쓰기 확장에서 원자 생성

`access create.<Resource>`와 해당 리소스의 `expose create`를 선언하면 `ctx.data.create(resource, values)`를 부를 수 있다. 서버는 호출자의 정책으로 공개 create 경로를 실행하며, 생성 결과 ID를 확장 안의 다음 읽기·전이·생성에서 사용할 수 있다. 암호화 필드가 있는 리소스는 `WIRE.UNSUPPORTED`다. [관리형 실행](MANAGED-EXECUTION.md)에서 트랜잭션, 거절, 멱등 키 규칙을 확인할 수 있다.

주문·재고의 [확장 예제](tests/fixtures/expr-apps/orders/extensions/shopb.mjs)는 서버에서 합계를 계산한 뒤 주문과 줄 생성, 재고 차감, 합계 확정을 한 호출에서 처리한다. [실행 기록](../plan-docs/expressiveness-1.0/consumer-apps.md).

## E8. 확장 입력에 배열 타입이 없다

확장의 `input`에는 `List<Int>`나 `Int[]` 같은 배열 타입을 쓸 수 없다(`output`은 확인하지 않았다). 정의 검사가 파싱 오류로 거부한다. 한 호출에서 주문 줄 여러 개를 받으려면 `Json` 필드로 받고 확장이 모양과 값을 직접 검증해야 한다(`Json`은 서버가 내용을 검사하지 않는다). 주문·재고 [확장 예제](tests/fixtures/expr-apps/orders/extensions/shopb.mjs)(변형 B)는 이 때문에 한 호출에 상품 하나만 받는다. 여러 줄이 필요하면 E1의 `bundle`(`refs`)로 줄을 연산마다 만드는 쪽이 선언만으로 된다. 근거는 [소비자 앱 실행 기록 F14](../plan-docs/expressiveness-1.0/consumer-apps.md)다.

## E9. 묶음의 연산 순서가 잠금 순서다

`bundle`은 `atomic` 배열 순서대로 연산을 실행하고 각 연산의 대상 행을 그 시점에 잠근다. 따라서 배열 순서가 곧 잠금 순서이고, 같은 행을 건드리는 묶음이 동시에 오면 순서에 따라 서로 기다릴 수 있다. 주문 줄 생성(FK가 상품 행을 참조)을 재고 차감(`take`)보다 먼저 두는 묶음 6건을 동시에 보낸 실행에서, 잠금 모드 조정 전에는 PostgreSQL `deadlock detected`가 나고 6건이 모두 `WIRE.DEADLINE_EXCEEDED`였다. 조정 뒤(`824cc0e1`, `dc68e4e6`)에는 줄 먼저와 `take` 먼저 두 순서 모두 2건 성공, 4건 `WIRE.INVALID_STATE`, 최종 재고 0으로 끝났다.

지침: **같은 행을 갱신하는 연산(재고 차감처럼 경쟁이 몰리는 전이)을 묶음 앞에 두고, 그 행을 참조하는 create를 뒤에 둔다.** 모든 요청이 같은 순서로 잠그면 교착 자체가 생기지 않는다. 시험한 것은 위 주문·재고 시나리오 하나이므로 다른 모양의 묶음에서도 수렴한다는 보장은 아니다. `compose` 연산은 대상 수만큼 행을 잠그므로 대상이 많은 묶음일수록 교착 가능성이 커진다. 서버는 교착(`40P01`)을 서버가 소유하는 선언형 트랜잭션에서 최대 3회 다시 실행한다([지원 범위](SUPPORT.md) 11번). 근거는 [소비자 앱 실행 기록 8.2절과 11절](../plan-docs/expressiveness-1.0/consumer-apps.md)이다.

## 구분자 규칙(자주 틀리는 점)

한 정의 안에서도 블록마다 구분자가 다르다.

- 전이의 `to` 절의 대입은 **쉼표**다: `to done = true, views = views + 1`. 세미콜론은 `from`, `allow`, `to`, `patch` 같은 서로 다른 항목 사이에 쓴다.
- 효과 블록(`create`, `update`의 중괄호) 안은 **세미콜론**이다: `update Member where id = author { score = 5; level = 2 }`.
- Decimal 리터럴은 따옴표 문자열이다(`"0.00"`). `0`, `0.00`은 `DEF.BAD_VALUE`로 거부되고 올바른 형식을 안내한다.

그 밖의 알려진 제한(`avg` 없음, 관련 필드의 정렬·필터 없음, 다중 `groupKey` 없음)은 [지원 범위](SUPPORT.md) 4번과 [정의 문법](../spec/definition-syntax.md)을 본다.
