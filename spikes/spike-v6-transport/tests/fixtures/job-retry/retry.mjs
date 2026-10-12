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
