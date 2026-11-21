// An extension written against workers/ctx.d.ts: it type-checks (checkJs) and it runs in the real worker.mjs, so the declared
// argument order and result shapes are held to what the worker sends and returns (tests/worker_ctx_types.rs).

/**
 * @param {{ owner: string }} input
 * @param {import("../../../workers/ctx.js").Ctx} ctx
 */
export async function data(input, ctx) {
  const sum = await ctx.data.aggregate("Order", "revenue", { month: "2026-11" });
  const page = await ctx.data.read({
    read: "Todo",
    select: ["id", "title", { owner: { select: ["id"] } }],
    filter: [{ field: "id", op: "eq", value: "10" }],
    sort: [{ field: "id", dir: "asc" }],
    limit: 5,
  });
  const done = await ctx.data.apply("Todo", "complete", [10, "11", 12n], "complete-1", { note: "x" }, { note: null });
  const bare = await ctx.data.apply("Todo", "complete", ["10"]);
  await ctx.data.apply("Todo", "take", ["10"], undefined, undefined, undefined, { qty: 3 });
  const made = await ctx.data.create("Todo", { owner: input.owner, title: "next", done: false }, "todo-1");
  const noKey = await ctx.data.create("Todo", { owner: input.owner, title: "again", done: false });
  // The declared result shapes, held in place by assignments the checker would refuse if a declaration changed.
  /** @type {string | number | undefined} */
  const madeId = made;
  /** @type {readonly (string | number)[]} */
  const changedIds = done.changed;
  /** @type {readonly Record<string, unknown>[]} */
  const rows = page.rows;
  void [madeId, changedIds, rows];
  return { sum, rows: page.rows.length, changed: done.changed, unchanged: bare.unchanged, made, noKey };
}

/**
 * @param {{ owner: string }} input
 * @param {import("../../../workers/ctx.js").Ctx} ctx
 */
export async function external(input, ctx) {
  const found = await ctx.mongo.find("events", { a: 1 }, { _id: 0 }, 3);
  const inserted = await ctx.mongo.insertOne("events", { a: 1 }, true);
  const kept = await ctx.mongo.deleteOne("events", { a: 1 }, { compensate: true, key: "d-1" });
  const flag = await ctx.redis.setNxEx("lock:1", "v", 30, true);
  const items = await ctx.redis.lrange("list:1");
  return { found, inserted, kept, flag, items };
}

/**
 * @param {{ owner: string }} input
 * @param {import("../../../workers/ctx.js").Ctx} ctx
 */
export async function refuses(input, ctx) {
  throw ctx.reject("INSUFFICIENT_STOCK", "재고가 부족합니다");
}

/** @type {import("../../../workers/ctx.js").ExtensionFunction<{ n: number }, { double: number }>} */
export const double = (input) => ({ double: input.n * 2 });
