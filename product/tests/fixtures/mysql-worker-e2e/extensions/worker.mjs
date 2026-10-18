// Each export is one worker action. The server derives the actor and grants; the worker only asks.
export async function view(input, ctx) {
  const result = await ctx.data.apply("Video", "increment", [input.id]);
  return { count: result.changed.length + result.unchanged.length };
}

// First callback succeeds on the caller's own video, the second targets someone else's video and is refused.
export async function twice(input, ctx) {
  await ctx.data.apply("Video", "increment", [input.first]);
  await ctx.data.apply("Video", "increment", [input.second]);
  return { count: 2 };
}

// The callback commits nothing on its own; the output violates Int(0..10), so the whole transaction must roll back.
export async function badOutput(input, ctx) {
  await ctx.data.apply("Video", "increment", [input.id]);
  return { count: 11 };
}

// `reset` is exposed publicly but is not in this extension's access list.
export async function undeclared(input, ctx) {
  await ctx.data.apply("Video", "increment", [input.id]);
  await ctx.data.apply("Video", "reset", [input.id]);
  return { count: 1 };
}
