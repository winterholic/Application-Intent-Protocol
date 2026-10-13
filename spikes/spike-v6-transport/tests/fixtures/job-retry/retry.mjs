export async function flaky(input, ctx) {
  if (ctx.job.attempt < 3) throw ctx.retryLater("upstream not ready");
  return { attempt: ctx.job.attempt, id: ctx.job.id };
}
export async function exhausted(input, ctx) {
  throw ctx.retryLater("never ready");
}
export async function boom(input, ctx) {
  if (ctx.job.attempt < 2) throw new Error("transient bug");
  return { attempt: ctx.job.attempt };
}
export async function callout(input, ctx) {
  const first = await ctx.http.call("crm", {}, "first");
  let second = "sent";
  try {
    await ctx.http.call("crm", {}, "second");
  } catch (error) {
    second = error.code;
  }
  // The recorded 503 replays from the journal even while the circuit is open.
  const replayed = await ctx.http.call("crm", {}, "first");
  return { first: first.status, second, replayed: replayed.status };
}
export async function later(input, ctx) {
  if (ctx.job.attempt < 2) throw ctx.retryLater("rate limited", { afterMs: 3000 });
  return { attempt: ctx.job.attempt };
}
