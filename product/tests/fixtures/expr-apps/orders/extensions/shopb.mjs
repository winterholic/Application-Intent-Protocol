// 변형 B: 주문 생성, 줄 생성, 재고 차감, 합계 확정이 한 트랜잭션이다. 호출자는 단가나 합계를 보내지 않는다.
// 확장은 호출자 id를 알 수 없어 owner를 입력으로 받는다. 다른 사람의 id를 넣으면 create allow(owner = actor)가 거절한다.
export async function placeAtomic(input, ctx) {
  const page = await ctx.data.read({ read: "Product", select: ["id", "price", "stock"], filter: [{ field: "id", op: "eq", value: input.product }] });
  if (page.rows.length === 0) throw ctx.reject("NO_SUCH_PRODUCT", "상품이 없습니다");
  const { price, stock } = page.rows[0];
  if (stock < input.qty) throw ctx.reject("INSUFFICIENT_STOCK", "재고가 부족합니다");
  const total = input.qty * price;
  // 위 확인과 이 줄 사이에 다른 요청이 재고를 줄여도, take의 from(stock >= qty)이 행을 잠근 뒤 다시 판정한다.
  // take를 줄 생성보다 먼저 부른다: 줄의 FK가 상품 행에 공유 잠금을 먼저 잡으면 동시 호출끼리 서로 기다린다(A7a 참고).
  await ctx.data.apply("Product", "take", [input.product], undefined, undefined, undefined, { qty: input.qty });
  const order = await ctx.data.create("Order", { owner: input.owner, total: 0, placed: false });
  await ctx.data.create("OrderLine", { order, product: input.product, qty: input.qty, unitPrice: price });
  await ctx.data.apply("Order", "settle", [order], undefined, { total });
  return { id: order, total };
}
