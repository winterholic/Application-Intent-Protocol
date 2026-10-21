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
export async function jobView(input, ctx) {
  const result = await ctx.data.apply("Video", "increment", [input.id], "increment");
  return { count: result.changed.length };
}

export async function jobRetry(input, ctx) {
  const result = await ctx.data.apply("Video", "increment", [input.id], "increment");
  if (ctx.job.attempt === 1) throw ctx.retryLater("retry after committed effect");
  return { count: result.changed.length };
}

export async function jobSlow() {
  await new Promise(resolve => setTimeout(resolve, 5000));
  return { count: 1 };
}

export async function jobNested(input, ctx) {
  return await ctx.jobs.run("incrementJob", input, "child");
}

// The body is the provider's raw text; the server verified its signature before this job existed.
export async function jobHook(input, ctx) {
  const result = await ctx.data.apply("Video", "increment", [JSON.parse(input.body).id], "hook");
  return { count: result.changed.length };
}

export async function jobTick(input) {
  return { count: input.note === "tick" ? 1 : 0 };
}

export async function jobViewed(input) {
  return { count: JSON.parse(input.event).topic === "video.viewed" ? 1 : 0 };
}

// The first attempt asks to be retried after the call completed; the second must not send it again.
export async function jobMail(input, ctx) {
  const reply = await ctx.http.call("mail", { video: input.id }, "send");
  if (ctx.job.attempt === 1) throw ctx.retryLater("retry after the committed call");
  return { count: reply.status === 200 ? 1 : 0 };
}

export async function jobEvent(input, ctx) {
  const published = await ctx.events.publish("audit", "video-" + input.id, { video: input.id });
  if (ctx.job.attempt === 1) throw ctx.retryLater("retry after the committed event");
  return { count: published.cursor === "1" ? 1 : 0 };
}

export async function jobMessage(input, ctx) {
  const sent = await ctx.messages.publish("orders", { video: input.id }, "video-" + input.id);
  if (ctx.job.attempt === 1) throw ctx.retryLater("retry after the committed message");
  return { count: sent.message.state === "READY" ? 1 : 0 };
}
