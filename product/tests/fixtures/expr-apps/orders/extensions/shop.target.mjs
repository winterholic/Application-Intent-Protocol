// 목표(미실행): 변형 B. 주문 생성, 줄 생성, 재고 차감을 한 트랜잭션에서 한다. 서버가 단가를 읽으므로 호출자는 가격을 정하지 못한다.
// ctx.data.create는 P0-C(feat/expr-p0c-ext-create-20261121), take의 input은 P0-B 이후 가능하다.
// 확인 필요: ctx.data.apply가 input을 어떤 인자로 받는지는 P0-B 문서에 아직 없다. 아래는 values·expect 뒤 일곱 번째 인자로 가정한 것이다.
export async function placeAtomic(input, ctx) {
  const page = await ctx.data.read({ read: "Product", select: ["id", "price"], filter: [{ field: "id", op: "eq", value: input.product }] });
  if (page.rows.length === 0) throw ctx.reject("NO_SUCH_PRODUCT", "상품이 없습니다");
  const price = page.rows[0].price;
  const total = input.qty * price;
  const order = await ctx.data.create("Order", { owner: String(ctx.actor.id), total, placed: true });
  await ctx.data.create("OrderLine", { order, product: input.product, qty: input.qty, unitPrice: price });
  await ctx.data.apply("Product", "take", [input.product], undefined, undefined, undefined, { qty: input.qty });
  return { id: order, total };
}
