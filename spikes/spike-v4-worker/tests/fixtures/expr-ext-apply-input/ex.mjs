// Extensions of tests/expr_ext_apply_input.rs: `ctx.data.apply` of a transition with an `input { ... }` block (P0-B).
/** @typedef {import("../../../workers/ctx.js").Ctx} Ctx */

/** @param {{ id: string, qty: number }} input @param {Ctx} ctx */
export async function takeStock(input, ctx) {
  const result = await ctx.data.apply("Product", "take", [input.id], undefined, undefined, undefined, { qty: input.qty });
  return { ok: result.changed.length === 1 };
}

// The amounts are declared, so leaving `input` out is refused by the server and the call does not commit.
/** @param {{ id: string, qty: number }} input @param {Ctx} ctx */
export async function takeWithoutInput(input, ctx) {
  await ctx.data.apply("Product", "take", [input.id]);
  return { ok: true };
}
