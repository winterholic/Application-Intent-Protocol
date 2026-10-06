# 확장 구현 예시(C B.7)와 시험용 변형(Python).
import asyncio, os, socket, sys

_last_ctx = None


async def stats(input, ctx):
    r = await ctx.data.aggregate("Apply", "approvedCount", {"clubId": input["clubId"]})
    return {"approvedApplicants": r["value"]}


async def statsOtherClub(input, ctx):
    r = await ctx.data.aggregate("Apply", "approvedCount", {"clubId": "11"})
    return {"approvedApplicants": r["value"]}


async def statsUndeclared(input, ctx):
    r = await ctx.data.aggregate("Recruitment", "bookmarkCount", {})
    return {"approvedApplicants": r["value"]}


async def statsImpersonate(input, ctx):
    r = await ctx.call("aggregate", "Apply.approvedCount", {"clubId": input["clubId"]}, actor=3)
    return {"approvedApplicants": r["value"]}


async def statsBadOutput(input, ctx):
    return {"approvedApplicants": "many"}


async def statsExtraOutput(input, ctx):
    return {"approvedApplicants": 1, "internalNote": "x"}


async def statsSlow(input, ctx):
    global _last_ctx
    _last_ctx = ctx
    await asyncio.sleep(3)
    return {"approvedApplicants": 0}


async def statsReuse(input, ctx):
    r = await _last_ctx.data.aggregate("Apply", "approvedCount", {"clubId": input["clubId"]})
    return {"approvedApplicants": r["value"]}


async def statsCrash(input, ctx):
    os._exit(1)


async def probeEnv(input, ctx):
    return {"approvedApplicants": len([k for k in os.environ if k.startswith("PG") or "DATABASE" in k])}


async def probeDb(input, ctx):
    try:
        socket.create_connection(("127.0.0.1", 5432), timeout=1).close()
        return {"approvedApplicants": 1}
    except OSError:
        return {"approvedApplicants": 0}


async def roleOut(input, ctx):
    return {"approvedApplicants": "ADMIN"}
