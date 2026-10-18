# Mirrors worker.mjs. The server derives the actor and grants; the worker only asks.
async def view(input, ctx):
    result = await ctx.data.apply("Video", "increment", [input["id"]])
    return {"count": len(result["changed"]) + len(result["unchanged"])}


async def twice(input, ctx):
    await ctx.data.apply("Video", "increment", [input["first"]])
    await ctx.data.apply("Video", "increment", [input["second"]])
    return {"count": 2}


async def badOutput(input, ctx):
    await ctx.data.apply("Video", "increment", [input["id"]])
    return {"count": 11}


async def undeclared(input, ctx):
    await ctx.data.apply("Video", "increment", [input["id"]])
    await ctx.data.apply("Video", "reset", [input["id"]])
    return {"count": 1}
