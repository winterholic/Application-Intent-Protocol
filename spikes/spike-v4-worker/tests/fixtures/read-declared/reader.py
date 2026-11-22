# Reads the resource it is told to through ctx.data.read and reports what the server answered.
async def run(input, ctx):
    try:
        result = await ctx.data.read({"read": input["name"], "select": ["id", "title"]})
        return {"status": "ok", "rows": len(result["rows"])}
    except Exception as error:
        return {"status": str(getattr(error, "code", error)), "rows": 0}
