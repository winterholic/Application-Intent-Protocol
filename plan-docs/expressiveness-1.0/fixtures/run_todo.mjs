export async function completeAndRepeat(input, ctx) {
  const page = await ctx.data.read({ read: "Todo", select: ["id", "title"], filter: [{ field: "id", op: "eq", value: input.id }] });
  await ctx.data.apply("Todo", "complete", [input.id], undefined, undefined, undefined);
  const made = await ctx.data.create("Todo", { owner: "1", title: page.rows[0].title, done: false });
  return { next: String(made) };
}
