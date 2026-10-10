async def confirm(input, ctx):
    result = await ctx.data.apply('Event', 'mark', [input['id']])
    return {'id': (result['changed'] + result['unchanged'])[0], 'count': len(result['changed'])}

async def textLength(input, ctx):
    return {'length': len(input['text'])}
