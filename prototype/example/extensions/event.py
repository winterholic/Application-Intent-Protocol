async def confirm(input, ctx):
    result = await ctx.data.apply('Event', 'mark', [input['id']])
    return {'id': (result['changed'] + result['unchanged'])[0], 'count': len(result['changed'])}
