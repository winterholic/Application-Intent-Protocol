import asyncio

async def length(input, ctx):
    return {"length": len(input["text"])}

async def invalid(input, ctx):
    return {"length": "invalid"}

async def ctxProbe(input, ctx):
    try:
        await ctx.data.aggregate("Hidden", "total", {})
    except Exception:
        pass
    return {"length": 1}

async def slow(input, ctx):
    await asyncio.sleep(0.5)
    return {"length": 1}

async def ranged(input, ctx):
    return {"length": input["count"] + 1}

async def unsafeInteger(input, ctx):
    return {"length": 9007199254740992}
