# 세션 actor가 비공개 youtube_watching_data에서 어떤 문서를 보는지 확인하는 확장.
async def watched(input, ctx):
    docs = await ctx.mongo.find("youtube_watching_data", {})
    first = docs[0]["video_id"] if docs else -1
    return {"clubName": "watched", "distCount": len(docs), "cachedReco": str(first)}
