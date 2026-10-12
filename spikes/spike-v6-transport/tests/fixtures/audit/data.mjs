export async function settle(input, ctx) {
  const page = await ctx.data.read({ read: "Order", select: ["id", "paid"] });
  let paid = 0;
  for (const row of page.rows) {
    if (row.paid) continue;
    const result = await ctx.data.apply("Order", "pay", [row.id], `pay-${row.id}`);
    paid += result.changed.length;
  }
  let denied = "none";
  try {
    await ctx.data.apply("Order", "pay", [input.other], "pay-other");
  } catch (error) {
    denied = error.code;
  }
  const first = page.rows[0].id;
  const again = await ctx.data.apply("Order", "pay", [first], `pay-${first}`);
  return { paid, visible: page.rows.length, denied, replayed: again.changed.length === 1 };
}
export async function payNow(input, ctx) {
  const result = await ctx.data.apply("Order", "pay", [input.id]);
  return { id: result.changed[0] };
}
