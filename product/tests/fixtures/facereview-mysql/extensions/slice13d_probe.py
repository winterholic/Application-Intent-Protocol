# Slice13-D probe: an extension that declares NO read grant calls ctx.data.read anyway.
async def undeclared_read(input, ctx):
    try:
        out = await ctx.data.read({'read': 'User', 'select': ['id'], 'limit': 1})
        return {'result': 'READ_OK rows=%d' % len(out.get('rows', []))}
    except Exception as e:
        return {'result': getattr(e, 'code', str(e))}
