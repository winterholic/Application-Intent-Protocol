# 쓰기 확장 예시(Python): 관리자 위임.
import asyncio

_held = None


async def delegate(input, ctx):
    await ctx.data.apply("ClubMember", "makeAdmin", [input["target"]])
    await ctx.data.apply("ClubMember", "resignAdmin", [input["self"]])
    return {"done": True}


async def delegateThrow(input, ctx):
    await ctx.data.apply("ClubMember", "makeAdmin", [input["target"]])
    raise RuntimeError("중간 실패")


async def delegateSwallow(input, ctx):
    await ctx.data.apply("ClubMember", "makeAdmin", [input["target"]])
    try:
        await ctx.data.apply("ClubMember", "resignAdmin", [input["target"]])
    except Exception:  # noqa: BLE001 시험: 확장이 ctx 오류를 삼키는 경우
        pass
    return {"done": True}


async def delegateSlow(input, ctx):
    global _held
    _held = ctx
    await ctx.data.apply("ClubMember", "makeAdmin", [input["target"]])
    await asyncio.sleep(1.5)
    return {"done": True}


async def lateWrite(input, ctx):
    await _held.data.apply("ClubMember", "resignAdmin", [input["self"]])
    return {"done": True}


async def onlyPromote(input, ctx):
    await ctx.data.apply("ClubMember", "makeAdmin", [input["target"]])
    return {"done": True}


async def undeclared(input, ctx):
    await ctx.data.apply("Recruitment", "close", ["100"])
    return {"done": True}


async def delegateAfter500(input, ctx):
    await asyncio.sleep(0.5)
    return await delegate(input, ctx)


async def staleWriter(input, ctx):
    await asyncio.sleep(0.2)
    await ctx.data.apply("ClubMember", "resignAdmin", [input["self"]])
    return {"done": True}


async def delegateAfter350(input, ctx):
    await asyncio.sleep(0.35)
    return await delegate(input, ctx)


async def cancelPending(input, ctx):
    # ctx 작업을 기다리다 취소한다. 서버 쪽 쓰기는 취소되지 않는다(응답 대기만 취소).
    t = asyncio.create_task(ctx.data.apply("ClubMember", "makeAdmin", [input["target"]]))
    await asyncio.sleep(0.01)
    t.cancel()
    try:
        await t
    except asyncio.CancelledError:
        pass
    await ctx.data.apply("ClubMember", "resignAdmin", [input["self"]])
    return {"done": True}
