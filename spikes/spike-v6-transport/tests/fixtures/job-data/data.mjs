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
export async function sneaky(input, ctx) {
  const result = await ctx.data.apply("Order", "pay", ["11"], "sneak");
  return { paid: result.changed.length };
}
export async function crashy(input, ctx) {
  const before = await ctx.data.read({ read: "Order", select: ["id", "paid"] });
  const seenPaid = before.rows.find((row) => row.id === 11).paid;
  const result = await ctx.data.apply("Order", "pay", [11], "crash-pay");
  // The first attempt dies after the committed transition; the retry must replay the journal.
  if (!seenPaid) process.exit(1);
  return { seenPaid, changed: result.changed.length };
}
export async function receive(input, ctx) {
  const event = JSON.parse(input.body);
  const result = await ctx.data.apply("Order", "pay", [event.order], `hook-${event.order}`);
  return { paid: result.changed.length };
}
export async function sweep(input, ctx) {
  const page = await ctx.data.read({ read: "Order", select: ["id", "paid"] });
  let paid = 0;
  for (const row of page.rows.filter((row) => !row.paid)) {
    paid += (await ctx.data.apply("Order", "pay", [row.id], `sweep-${row.id}`)).changed.length;
  }
  return { paid };
}
export async function fulfil(input, ctx) {
  const event = JSON.parse(input.event);
  const result = await ctx.data.apply("Order", "ship", [event.sourceId], `ship-${event.id}`);
  return { shipped: result.changed.length };
}
