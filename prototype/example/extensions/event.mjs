export async function confirm(input, ctx) {
  const result = await ctx.data.apply('Event', 'mark', [input.id]);
  return { id: result.changed[0] ?? result.unchanged[0], count: result.changed.length };
}

export function textLength(input) {
  return {length: [...input.text].length};
}
