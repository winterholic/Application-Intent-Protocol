async def flaky(input, ctx):
    if ctx.job["attempt"] < 3:
        raise ctx.retry_later("upstream not ready")
    return {"attempt": ctx.job["attempt"], "id": ctx.job["id"]}


async def exhausted(input, ctx):
    raise ctx.retry_later("never ready")


async def boom(input, ctx):
    if ctx.job["attempt"] < 2:
        raise RuntimeError("transient bug")
    return {"attempt": ctx.job["attempt"]}


async def callout(input, ctx):
    first = await ctx.http.call("crm", {}, "first")
    second = "sent"
    try:
        await ctx.http.call("crm", {}, "second")
    except Exception as error:
        second = getattr(error, "code", "UNKNOWN")
    replayed = await ctx.http.call("crm", {}, "first")
    return {"first": first["status"], "second": second, "replayed": replayed["status"]}
