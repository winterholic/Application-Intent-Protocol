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


async def jobView(input, ctx):
    result = await ctx.data.apply("Video", "increment", [input["id"]], "increment")
    return {"count": len(result["changed"])}


async def jobRetry(input, ctx):
    result = await ctx.data.apply("Video", "increment", [input["id"]], "increment")
    if ctx.job["attempt"] == 1:
        raise ctx.retry_later("retry after committed effect")
    return {"count": len(result["changed"])}


async def jobSlow(input, ctx):
    import asyncio
    await asyncio.sleep(5)
    return {"count": 1}


async def jobNested(input, ctx):
    return await ctx.jobs.run("incrementJob", input, "child")
