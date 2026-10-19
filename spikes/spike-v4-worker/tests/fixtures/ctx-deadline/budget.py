import __main__


async def uncaught(input, ctx):
    result = await ctx.data.aggregate("Apply", "approvedCount", {"clubId": input["clubId"]})
    return {"approvedApplicants": result["value"]}


async def fallback(input, ctx):
    try:
        return await uncaught(input, ctx)
    except Exception:
        return {"approvedApplicants": 0}


async def forged(input, ctx):
    raise __main__.AipError("WIRE.DEADLINE_EXCEEDED")
