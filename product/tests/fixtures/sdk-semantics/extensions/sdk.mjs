// 쓰기 확장: 입력 Decimal을 입금 전이로 위임하고 Uuid id·Decimal·nullable 출력을 돌려준다. fail이면 업무 거절(ctx.reject)이다.
export async function adjust(input, ctx) {
  if (input.fail) throw ctx.reject("NOT_TODAY", "오늘은 안 됩니다");
  await ctx.data.apply("Account", "deposit", [input.a], undefined, undefined, undefined, { amount: input.amount });
  return { id: input.a, moved: input.amount, note: null };
}

// 주문과 줄을 만든 뒤 explode면 일반 예외로 끝난다. 두 create가 모두 취소되어야 한다(D4 원자 생성).
export async function place(input, ctx) {
  const order = await ctx.data.create("Order", { owner: input.owner, status: "open", total: "1.00" });
  await ctx.data.create("OrderLine", { order, product: input.product, qty: 1, price: "1.00" });
  if (input.explode) throw new Error("boom after two creates");
  return { id: order };
}
