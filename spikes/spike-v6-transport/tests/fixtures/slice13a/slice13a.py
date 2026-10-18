# Slice13-A MySQL relational worker extension tests

async def feed(input, ctx):
    # 1. MySQL read callback through ctx.data.read
    rec = await ctx.data.read({
        "read": "Recruitment",
        "select": ["id", "title"],
        "limit": 1,
    })
    rec_rows = rec.get("rows", [])
    title = rec_rows[0]["title"] if rec_rows else "Unknown"

    # 2. Managed Mongo capability through ctx.mongo.find
    dist = await ctx.mongo.find("video_distribution", {"clubId": str(input["clubId"])})

    # 3. Managed Redis capability through ctx.redis.get
    cached = await ctx.redis.get(f"facereview:reco:{input['clubId']}")

    return {
        "clubName": title,
        "distCount": len(dist),
        "cachedReco": cached or "none",
    }


async def readRedisOnly(input, ctx):
    cached = await ctx.redis.get(f"facereview:reco:{input['clubId']}")
    return {
        "clubName": "cached",
        "distCount": 0,
        "cachedReco": cached or "none",
    }


async def unauthMongoCollection(input, ctx):
    # Try accessing unauthorized collection 'admin_secrets'
    try:
        await ctx.mongo.find("admin_secrets", {})
        err_code = "UNEXPECTED_OK"
    except Exception as e:
        err_code = getattr(e, "code", str(e))
    return {"clubName": err_code, "distCount": 0, "cachedReco": "caught"}


async def unauthMongoWrite(input, ctx):
    # Try calling write operation 'insert' on Mongo
    try:
        await ctx.call("mongo", "video_distribution", {"command": "insert", "doc": {"id": 1}})
        err_code = "UNEXPECTED_OK"
    except Exception as e:
        err_code = getattr(e, "code", str(e))
    return {"clubName": err_code, "distCount": 0, "cachedReco": "caught"}


async def unauthRedisKey(input, ctx):
    # Try accessing unauthorized redis key 'other_app:secret'
    try:
        await ctx.redis.get("other_app:secret")
        err_code = "UNEXPECTED_OK"
    except Exception as e:
        err_code = getattr(e, "code", str(e))
    return {"clubName": err_code, "distCount": 0, "cachedReco": "caught"}


async def unauthApply(input, ctx):
    # Try calling ctx.data.apply in read extension
    try:
        await ctx.data.apply("Recruitment", "close", [input["clubId"]])
        err_code = "UNEXPECTED_OK"
    except Exception as e:
        err_code = getattr(e, "code", str(e))
    return {"clubName": err_code, "distCount": 0, "cachedReco": "caught"}


async def policyOwnerCheck(input, ctx):
    rec = await ctx.data.read({
        "read": "Recruitment",
        "select": ["id", "title"],
        "limit": 1,
    })
    rows = rec.get("rows", [])
    return {
        "clubName": rows[0]["title"] if rows else "empty",
        "distCount": len(rows),
        "cachedReco": "ok",
    }
