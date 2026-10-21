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


async def jobHook(input, ctx):
    import json
    result = await ctx.data.apply("Video", "increment", [json.loads(input["body"])["id"]], "hook")
    return {"count": len(result["changed"])}


async def jobTick(input, ctx):
    return {"count": 1 if input["note"] == "tick" else 0}


async def jobViewed(input, ctx):
    import json
    return {"count": 1 if json.loads(input["event"])["topic"] == "video.viewed" else 0}


async def jobMail(input, ctx):
    reply = await ctx.http.call("mail", {"video": input["id"]}, "send")
    if ctx.job["attempt"] == 1:
        raise ctx.retry_later("retry after the committed call")
    return {"count": 1 if reply["status"] == 200 else 0}


async def jobEvent(input, ctx):
    published = await ctx.events.publish("audit", "video-" + input["id"], {"video": input["id"]})
    if ctx.job["attempt"] == 1:
        raise ctx.retry_later("retry after the committed event")
    return {"count": 1 if published["cursor"] == "1" else 0}


async def jobMessage(input, ctx):
    sent = await ctx.messages.publish("orders", {"video": input["id"]}, "video-" + input["id"])
    if ctx.job["attempt"] == 1:
        raise ctx.retry_later("retry after the committed message")
    return {"count": 1 if sent["message"]["state"] == "READY" else 0}


async def jobRelay(input, ctx):
    sent = await ctx.messages.publish("orders", {"body": input["body"]}, "relay-" + __import__("json").loads(input["body"])["language"])
    return {"count": 1 if sent["message"]["state"] == "READY" else 0}


async def jobHandle(input, ctx):
    import json
    return {"count": 1 if json.loads(input["message"])["payload"]["body"] else 0}
