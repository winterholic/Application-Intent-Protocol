async def managed(inp, ctx):
    return await ctx.http.call("echo", {"text": inp["text"]}, "charge")
async def missing(inp, ctx):
    return await ctx.http.call("echo", {"text": inp["text"]})
