export async function view(input, ctx) {
  const result = await ctx.data.apply("Video", "increment", [input.id]);
  return { count: result.changed.length + result.unchanged.length };
}
