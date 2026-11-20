# 표현력 한계와 확장 우회

AIP 1.0의 선언(`.aip`)은 읽기, 필터·정렬·페이지, 관계, 행·필드 권한, 상태 전이, 부분 수정, 원자 묶음을 서버 코드 없이 표현한다. 아래 일곱 가지는 **선언만으로는 안 되는 요구**다. 1.0은 이 한계를 설계 변경으로 풀지 않았고 한계로 문서화한다. 대부분은 서버 확장(`extension write`)이나 호출 순서로 풀 수 있다. 어떤 기능이 1.0 지원인지는 [지원 범위](SUPPORT.md)를 본다.

이 한계는 두 앱을 새로 설계해 끝까지 구현하면서 찾았다(다중 사용자 할일 공유, 테넌트별 재고·주문). 앱마다 확장 코드는 22~24줄이었고 서버 Controller·DTO·Service·Repository 코드는 없었다. 아래 예시 중 "실행"으로 표시한 것은 이 문서를 쓰며 실제 PostgreSQL 17과 `aip service serve`로 돌린 것이고, "check"는 `aip service check`만 통과시킨 것이다.

| # | 한계 | 우회 | 확인 |
| --- | --- | --- | --- |
| E1 | 부모와 자식을 한 번에 원자적으로 만들 수 없다 | 부모 먼저 생성, 그 id로 자식 묶음, 확장이 확정 | 실행 |
| E2 | 가변량 증감 불가(고정 정수만) | 확장이 읽고 계산해 `patch`+`expect`로 쓴다 | 실행 |
| E3 | 계산 집계(`Σ 수량×단가`) 불가 | 확장이 계산해 비정규화 필드에 저장 | 실행 |
| E4 | 참조 생성은 대상 행을 읽을 수 있어야 한다 | 디렉터리는 공개하고 민감 필드만 필드 정책으로 가린다 | check |
| E5 | cursor 페이지는 nullable 정렬과 함께 못 쓴다 | offset 페이지 | 실행(오류), check(우회) |
| E6 | 전이의 `from`은 필수다 | `from true` | check |
| E7 | PostgreSQL 쓰기 확장은 행을 생성하지 못한다 | 선언 효과 `create` 또는 클라이언트의 create 호출 | check |

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

설정은 `"workers": {"directory": "./extensions", "language": "node", "enable_writes": true}`다. 확장 자체의 계약, 격리, `ctx` 호출은 [관리형 실행](MANAGED-EXECUTION.md)을 따르고, 확장 ABI는 1.0에서 확정 전이다([지원 범위](SUPPORT.md#지원-범위-표) 16번). **PostgreSQL 쓰기 확장은 `ctx.data.read`와 `ctx.data.apply`만 지원한다.** 아래 코드는 이 둘만 쓴다. `ctx.data.apply(resource, transition, ids, key, values, expect)`의 `key`는 `undefined`로 둔다(바깥 요청의 멱등 키가 이미 확장 호출 전체를 덮는다).

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

## E1. 부모와 자식을 한 번에 원자적으로 만들 수 없다

`bundle`은 앞 연산이 만든 id를 뒤 연산에 넘기지 못한다(`compose`의 `item` 출처만 예외, [SDK](SDK.md#묶음bundle과-조합compose)). 그래서 주문과 주문 줄을 한 `bundle`로 만들 수 없고, 두 호출 사이에 줄 없는 주문이 남을 수 있다. "주문은 줄이 1개 이상"이라는 불변식은 선언할 수 없다.

우회: 부모를 먼저 만들고 응답의 `created[0].id`로 자식을 한 묶음에 만든다. 불변식은 확정 단계의 확장이 지킨다(줄이 없으면 거절).

```ts
const parent = await client.bundle({ atomic: [
  { create: "Order", values: { owner: "1", total: 0, placed: false } },
] }, { key: "order-1" });
const orderId = parent.created[0].id;
await client.bundle({ atomic: [
  { create: "OrderLine", values: { order: orderId, product: "1", qty: 3, unitPrice: 200 } },
  { create: "OrderLine", values: { order: orderId, product: "2", qty: 2, unitPrice: 50 } },
] }, { key: "order-1-lines" });
await client.writeExtension("Order.place", { id: orderId }, { key: "order-1-place" });
```

실행 결과: 주문 `placed: true`, 줄 두 개가 만들어졌다. 확장은 줄이 없으면 `throw new Error("empty order")`를 하도록 썼지만 이 분기는 실행하지 않았다. 호출 사이에 실패하면 `placed: false`인 주문이 남으므로, 미확정 주문을 치우는 규칙(예: 오래된 `placed = false` 정리)은 앱이 정한다.

## E2. 가변량 증감 불가

선언 전이의 증감은 고정 정수만 된다. `to stock = stock - 1`은 되지만 입력값이나 다른 필드로 줄이는 `to stock = stock - reorderLevel`은 거부된다.

```
DEF.ARITH_NOT_ALLOWED  증감 대입은 `stock = stock + 정수` 또는 `stock = stock - 정수`(0 제외)만
```

우회: 확장이 현재 값을 읽고 새 값을 계산한 뒤, `patch`와 `expect`가 있는 `internal` 전이로 쓴다. `expect stock`은 읽은 뒤에 다른 요청이 재고를 바꿨으면 [일반·부분 수정](GENERAL-UPDATE.md)의 규칙대로 `WIRE.CONFLICT`로 거부해 낙관적 잠금을 만든다(이 충돌은 실행하지 않았다). 재고가 모자라면 확장이 던지고 호출 전체가 롤백된다.

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

실행 결과: 재고 10과 5에서 주문(3개, 2개) 뒤 7과 3이 됐다. 둘째 줄의 재고가 모자라는 주문(줄 1은 2개, 줄 2는 100개)은 호출자에게 `WIRE.EXTENSION_ERROR`(`공식 WRITE 확장 호출 거부`)를 돌려주고, 첫 줄의 차감도 되돌아가 재고는 7과 3 그대로, 주문은 `placed: false`로 남았다. `ctx.data.read`의 필터는 호출 대상 resource의 `expose read … filter`에 있는 항목만 쓸 수 있다(위 정의의 `order.eq`, `id.eq`). 목록에 없으면 호출이 거부되고 그 호출 전체가 실패 처리된다. 확장이 예외를 잡아도 되돌릴 수 없다. 확장의 어떤 실패도 호출자에게는 같은 `WIRE.EXTENSION_ERROR`로 보이므로(재고 부족과 코드 오류의 구분 포함), 업무 거절 사유를 호출자에게 전하려면 별도 응답 필드나 호출 순서를 앱이 설계해야 한다.

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

## E7. PostgreSQL 쓰기 확장은 행을 생성하지 못한다

PostgreSQL의 쓰기 확장에서 `ctx.data.create`는 지원하지 않고 `WIRE.UNSUPPORTED`다([관리형 실행](MANAGED-EXECUTION.md)에 기재). 반복 할일에서 "다음 회차 항목 만들기"를 확장 안에서 할 수 없다. 이 항목은 문서 근거이고 실행하지 않았다.

우회는 둘이다.

1. **선언 효과 `create`**. 전이가 같은 트랜잭션에서 행을 만든다. 값은 상수이거나 대상 행(`this`)의 필드에서 온다.

   ```aip
   transition complete {
     allow owner = actor; from done = false; to done = true
     create Todo { owner = this.owner; title = this.title; done = false }   // 효과 블록 안의 구분자는 세미콜론
   }
   ```

   check로 통과를 확인했고 실행하지는 않았다. 새 행의 값을 날짜 규칙처럼 계산해야 하면 선언으로 만들 수 없다.
2. **클라이언트가 확장 호출 뒤 create를 부른다.** 확장이 계산한 값을 응답으로 돌려주고, 호출자가 그 값으로 `create`(또는 E1의 `bundle`)를 호출한다. 두 호출은 원자적이지 않다. 두 번째 호출이 실패하면 첫 번째의 효과는 남으므로, 첫 번째를 멱등으로 설계하고 같은 키로 다시 시도한다.

## 구분자 규칙(자주 틀리는 점)

한 정의 안에서도 블록마다 구분자가 다르다.

- 전이의 `to` 절의 대입은 **쉼표**다: `to done = true, views = views + 1`. 세미콜론은 `from`, `allow`, `to`, `patch` 같은 서로 다른 항목 사이에 쓴다.
- 효과 블록(`create`, `update`의 중괄호) 안은 **세미콜론**이다: `update Member where id = author { score = 5; level = 2 }`.
- Decimal 리터럴은 따옴표 문자열이다(`"0.00"`). `0`, `0.00`은 `DEF.BAD_VALUE`로 거부되고 올바른 형식을 안내한다.

그 밖의 알려진 제한(Decimal `sum` 불가, `avg` 없음, 관련 필드의 정렬·필터 없음, 다중 `groupKey` 없음)은 [지원 범위](SUPPORT.md) 4번과 [정의 문법](../spec/definition-syntax.md)을 본다.
