# WBS 7.2.7: the offline recommendation pool refresh of the original (Celery beat task rebuild_recommendation_pool, every 30 minutes) as
# an AIP durable job. Source: new-facereview@7948b7e7 common/tasks/scheduled_tasks.py and HomeService._build_and_cache_ranked_pool.
#
# The pool itself is the pure builder next to this file (recommendation_pool_builder, pinned against the original). This module only
# gathers the inputs the original reads (RDB videos with their like counts, MongoDB distributions) and writes what the original writes:
#   facereview:reco:pool          JSON list of the top 1000 slim entries           SET with EX 7200
#   facereview:reco:category      hash category -> JSON list of the top 20         DEL, HSET per category, EXPIRE 7200
#   facereview:reco:category_order  JSON list of the category names in hset order   (new: the host's hash keeps no field order)
#
# Differences from the original:
#  - Every write carries a stable effect key. The server journals it, so a retry of the same run neither repeats a finished write nor
#    loses an unfinished one. The three writes are not one transaction (the original used one Redis pipeline, which is not atomic across
#    a crash either); a reader that finds the pool but not the categories builds the categories itself.
#  - The original reads every active video. A job makes at most limits.job_calls ctx calls (64 unless the operator raises it); each
#    100 videos cost one RDB page and one MongoDB chunk, so MAX_VIDEOS videos need about MAX_VIDEOS / 50 calls. Beyond MAX_VIDEOS the
#    job fails loudly instead of ranking a truncated catalog.
#  - The category hash is replaced in one host command (replace_hash_ex), which is one Lua script on the Redis side (delete, hset,
#    expire), so a reader never sees the hash half built; the original deleted it first and filled it afterwards.
import importlib.util
import json
import os

_here = os.path.dirname(os.path.abspath(__file__))


def _load(name):
    spec = importlib.util.spec_from_file_location(f"slice14a_{name}", os.path.join(_here, f"{name}.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_lists = _load("slice13b_lists")
_pool = _load("recommendation_pool_builder")
_rec = _load("slice13d_recommend")

MAX_VIDEOS = 15000
CACHE_TTL = 7200


async def _videos(ctx):
    rows = await _lists._read_all(ctx, {
        "read": "Video", "select": ["id", "youtube_url", "title", "category", "view_count", "created_at", "likeCount"], "sort": _rec.BY_ID,
    }, cap=MAX_VIDEOS, cursor=True)
    videos = [{"video_id": row["id"], "youtube_url": row["youtube_url"], "title": row["title"], "category": row["category"],
               "view_count": row["view_count"], "created_at": row["created_at"], "is_deleted": 0} for row in rows]
    return videos, {row["id"]: row["likeCount"] for row in rows}


async def _write_cache(ctx, pool, categories):
    await ctx.redis.set_ex(_rec.POOL_KEY, json.dumps(pool), CACHE_TTL, effect_key="pool")
    if categories:
        await ctx.redis.replace_hash_ex(_rec.CATEGORY_KEY, categories, CACHE_TTL, effect_key="category")
        await ctx.redis.set_ex(_rec.CATEGORY_ORDER_KEY, json.dumps(list(categories)), CACHE_TTL, effect_key="order")
    else:
        await ctx.redis.delete(_rec.CATEGORY_KEY, effect_key="category")
        await ctx.redis.delete(_rec.CATEGORY_ORDER_KEY, effect_key="order")


async def rebuildRecommendationPool(input, ctx):
    videos, like_map = await _videos(ctx)
    docs = await _rec._distributions(ctx, [video["video_id"] for video in videos])
    pool, by_category = _pool.build_ranked_pool(videos, docs, like_map, list(_rec.GENRES))
    categories = {name: json.dumps(entries) for name, entries in by_category.items() if entries}
    try:
        await _write_cache(ctx, pool, categories)
    except Exception as error:
        # Redis does not answer: the run is tried again after a pause, and the journal keeps the writes that already happened.
        if getattr(error, "code", None) == "WIRE.ADAPTER_UNAVAILABLE":
            raise ctx.retry_later("the recommendation cache is unavailable", after_ms=1000)
        raise
    return {"pool": len(pool), "categories": len(categories)}
