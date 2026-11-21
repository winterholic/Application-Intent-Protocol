// Every line that ends in an ERR comment must be refused by the type checker, and no other line may be (tests/worker_ctx_types.rs).
// This file is never run.

/** @param {import("../../../workers/ctx.js").Ctx} ctx */
export async function wrong(ctx) {
  await ctx.data.apply("Todo", "complete", "10"); // ERR ids must be a list
  await ctx.data.apply("Todo", "complete", [{ id: 10 }]); // ERR an id is a string, number or bigint
  await ctx.data.apply("Todo", "complete", ["10"], 7); // ERR key is a string
  await ctx.data.create("Todo", "title"); // ERR values is an object
  await ctx.data.create("Todo"); // ERR values is required
  await ctx.data.create({ resource: "Todo", values: {} }); // ERR positional, not an object
  await ctx.data.read("Todo"); // ERR read takes the query object
  await ctx.data.read({ read: "Todo" }); // ERR select is required
  await ctx.data.read({ read: "Todo", select: ["id"], limt: 5 }); // ERR a misspelled key
  await ctx.data.read({ read: "Todo", select: ["id"], sort: [{ field: "id", dir: "up" }] }); // ERR dir is asc or desc
  await ctx.data.crete("Todo", {}); // ERR a misspelled method
  await ctx.dat.create("Todo", {}); // ERR a misspelled namespace
  throw ctx.reject(409, "x"); // ERR the business code is a string
}

/** @param {import("../../../workers/ctx.js").Ctx} ctx */
export async function typedResults(ctx) {
  const made = await ctx.data.create("Todo", {});
  /** @type {boolean} */
  const asBool = made; // ERR the id is a string or number
  /** @type {string | number} */
  const strict = made; // ERR create may resolve to undefined
  const done = await ctx.data.apply("Todo", "complete", ["10"]);
  const first = done.changed[0].toFixed(); // ERR an id may be a string
  const page = await ctx.data.read({ read: "Todo", select: ["id"] });
  page.rows.push({}); // ERR rows are read-only
  return [asBool, strict, first];
}

/** @type {import("../../../workers/ctx.js").ExtensionFunction<{ n: number }, { double: number }>} */
export const badOutput = (input) => ({ double: String(input.n) }); // ERR output is a number
