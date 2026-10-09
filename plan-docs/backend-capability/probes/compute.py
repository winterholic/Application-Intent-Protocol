import asyncio
async def count(input, ctx):
    return {'length': len(input['text'])}
async def slow(input, ctx):
    await asyncio.sleep(6)
    return await count(input, ctx)
