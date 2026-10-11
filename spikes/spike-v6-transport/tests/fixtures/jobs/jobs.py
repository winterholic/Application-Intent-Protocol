import asyncio, base64
def count(inp, ctx): return {"length": len(inp["text"])}
async def slow(inp, ctx):
    await asyncio.sleep(5)
    return {"length": 1}
async def report(inp, ctx):
    result = await ctx.jobs.run("length", inp, "measure")
    file = await ctx.files.put("reports", {"name":"report.txt", "mediaType":"text/plain", "base64":base64.b64encode(inp["text"].encode()).decode()}, "report-file")
    await ctx.events.publish("notifications", "report-ready", {"file":file["id"], "length":result["length"]})
    return {"file":file["id"],"length":result["length"]}
async def httpEcho(inp, ctx):
    response = await ctx.http.call("echo", {"text": inp["text"]}, "charge")
    return {"length": len(response["body"]["text"]), "status": response["status"]}
async def httpEchoReplay(inp, ctx):
    await ctx.http.call("echo", {"text": inp["text"]}, "charge")
    await ctx.jobs.run("length", inp, "measure")
    response = await ctx.http.call("echo", {"text": inp["text"]}, "charge")
    return {"length": len(response["body"]["text"]), "status": response["status"]}
async def httpHold(inp, ctx):
    response = await ctx.http.call("hold", {"text": inp["text"]}, "hold-once")
    return {"length": len(response["body"]["text"]), "status": response["status"]}
async def httpBroken(inp, ctx):
    await ctx.http.call("broken", {"text": inp["text"]}, "once")
    return {"ok": True}
async def httpBrokenCaught(inp, ctx):
    try:
        await ctx.http.call("broken", {"text": inp["text"]}, "once")
    except Exception:
        return {"ok": False}
    return {"ok": True}
async def budgetRoot(inp, ctx):
    return await ctx.jobs.run("budgetChild", {}, "budget-child")
async def budgetChild(inp, ctx):
    for index in range(64):
        await ctx.events.publish("notifications", f"budget-{index}", {"index": index})
    return {"length": 64}
async def knownFailChild(inp, ctx):
    return {"length": "wrong-type"}
async def catchKnownChildFailure(inp, ctx):
    recovered = 0
    for _ in range(2):
        try:
            await ctx.jobs.run("knownFailChild", {}, "known-failure")
        except Exception as error:
            if getattr(error, "code", None) != "STEP_FAILED":
                raise
            recovered += 1
    return {"length": recovered}

async def message(inp, ctx):
    await ctx.messages.publish("notifications", {"text": inp["text"]}, "dispatch")
    await ctx.messages.publish("notifications", {"text": inp["text"]}, "dispatch")
    return {"length": len(inp["text"])}
