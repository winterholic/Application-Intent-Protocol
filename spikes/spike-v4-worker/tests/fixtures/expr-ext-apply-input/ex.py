# Extensions of tests/expr_ext_apply_input.rs: `ctx.data.apply` of a transition with an `input { ... }` block (P0-B).
async def takeStock(input, ctx):
    result = await ctx.data.apply("Product", "take", [input["id"]], input={"qty": input["qty"]})
    return {"ok": len(result["changed"]) == 1}


async def takeWithoutInput(input, ctx):
    await ctx.data.apply("Product", "take", [input["id"]])
    return {"ok": True}
