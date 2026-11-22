// 여러 상품 주문(줄 최대 3개): 합계를 서버가 상품 단가에서 계산하고, 재고 차감·주문·줄 생성·합계 확정이 한 트랜잭션이다.
// 확장 입력에는 배열이 없고 Json은 어떤 값도 받지 못해(track-c.md 결함 C-D2) 줄은 product1..3·qty1..3 고정 슬롯으로 받는다.
// 상품 행은 id 순서로 잠근다: 같은 상품을 반대 순서로 담은 두 주문이 서로를 기다리지 않게 한다.
export async function checkout(input, ctx) {
  const items = [];
  for (const slot of [1, 2, 3]) {
    const product = input[`product${slot}`];
    const qty = input[`qty${slot}`];
    if ((product === null || product === undefined) !== (qty === null || qty === undefined)) throw ctx.reject("BAD_ITEMS", `줄 ${slot}은 product와 qty를 함께 줘야 한다`);
    if (product !== null && product !== undefined) items.push({ product, qty });
  }
  if (slotGap(input)) throw ctx.reject("BAD_ITEMS", "줄은 앞 슬롯부터 채운다");
  const seen = new Set();
  for (const item of items) {
    if (seen.has(item.product)) throw ctx.reject("DUPLICATE_PRODUCT", "같은 상품은 한 줄로");
    seen.add(item.product);
  }
  const sorted = [...items].sort((a, b) => Number(a.product) - Number(b.product));
  const page = await ctx.data.read({ read: "Product", select: ["id", "price", "stock"], filter: [{ field: "id", op: "in", value: sorted.map((i) => i.product) }] });
  const byId = new Map(page.rows.map((row) => [row.id, row]));
  for (const item of sorted) {
    const row = byId.get(item.product);
    if (!row) throw ctx.reject("NO_SUCH_PRODUCT", `상품 ${item.product} 없음`);
    if (row.stock < item.qty) throw ctx.reject("INSUFFICIENT_STOCK", `상품 ${item.product} 재고 부족`);
  }
  let total = 0;
  for (const item of sorted) {
    // 확인과 차감 사이에 다른 요청이 재고를 줄여도 take의 from(stock >= input.qty)이 잠근 행에서 다시 판정한다.
    await ctx.data.apply("Product", "take", [item.product], undefined, undefined, undefined, { qty: item.qty });
    total += item.qty * byId.get(item.product).price;
  }
  const order = await ctx.data.create("Order", { owner: input.owner, total: 0, paid: 0, state: "PLACED" });
  for (const item of sorted) {
    await ctx.data.create("OrderLine", { order, product: item.product, qty: item.qty, unitPrice: byId.get(item.product).price });
  }
  await ctx.data.apply("Order", "setTotal", [order], undefined, { total });
  return { id: order, total, lines: sorted.length };
}

// 취소: 주문을 CANCELLED로 바꾸고, 그 주문 줄 수량만큼 재고를 돌려준다. 전부 한 트랜잭션이라 취소 전이가 거절되면(이미 취소·발송됨, 동시 취소의 패자)
// 먼저 돌려준 재고도 함께 롤백된다. 취소를 앞에 두는 것은 거절을 빨리 알기 위해서다.
export async function cancelAndRestock(input, ctx) {
  await ctx.data.apply("Order", "cancel", [input.order]);
  const lines = await ctx.data.read({ read: "OrderLine", select: ["id", "product", "qty"], filter: [{ field: "order", op: "eq", value: input.order }] });
  const perProduct = new Map();
  for (const row of lines.rows) perProduct.set(row.product, (perProduct.get(row.product) ?? 0) + row.qty);
  let restored = 0;
  for (const [product, qty] of [...perProduct].sort((a, b) => Number(a[0]) - Number(b[0]))) {
    await ctx.data.apply("Product", "giveBack", [product], undefined, undefined, undefined, { qty });
    restored += qty;
  }
  return { id: input.order, restored };
}

function slotGap(input) {
  const present = [1, 2, 3].map((slot) => input[`product${slot}`] !== null && input[`product${slot}`] !== undefined);
  return present[1] === false && present[2] === true;
}

export async function probeJson(input, ctx) {
  return { ok: input.payload !== undefined };
}
