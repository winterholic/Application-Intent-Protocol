// 현재 우회: 줄을 읽어 재고를 확인하고, 계산한 새 재고를 patch+expect 전이로 쓰고, 합계를 계산해 주문을 확정한다.
// 서버 쪽 코드가 이 파일 하나뿐이다. Controller·DTO·Repository는 없다.
export async function place(input, ctx) {
  const lines = await ctx.data.read({ read: "OrderLine", select: ["qty", "unitPrice", "product"],
    filter: [{ field: "order", op: "eq", value: input.id }] });
  if (lines.rows.length === 0) throw ctx.reject("EMPTY_ORDER", "주문 줄이 없습니다");
  let total = 0;
  for (const line of lines.rows) {
    const productId = String(line.product.id ?? line.product);
    const page = await ctx.data.read({ read: "Product", select: ["id", "stock"],
      filter: [{ field: "id", op: "eq", value: productId }] });
    const stock = page.rows[0].stock;
    if (stock < line.qty) throw ctx.reject("INSUFFICIENT_STOCK", "재고가 부족합니다");
    await ctx.data.apply("Product", "setStock", [productId], undefined, { stock: stock - line.qty }, { stock });
    total += line.qty * line.unitPrice;
  }
  await ctx.data.apply("Order", "finish", [input.id], undefined, { total }, undefined);
  return { id: input.id, total };
}
