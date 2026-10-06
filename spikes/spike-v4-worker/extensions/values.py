async def echo(input, ctx):
    return {"value": input["value"]}


async def nul(input, ctx):
    return {"value": "left\x00right"}
