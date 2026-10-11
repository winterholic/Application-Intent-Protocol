export async function managed(input, ctx) {
  return await ctx.http.call("echo", {text: input.text}, "charge");
}
export async function missing(input, ctx) {
  return await ctx.http.call("echo", {text: input.text});
}
