# WBS 7.2.3 Slice13-D: FaceReview recommendation APIs as AIP read extensions.
#   GET home/category      HomeService.get_videos_by_category_emotions   -> Video.homeCategory
#   GET home/personalized  HomeService.get_personalized_videos           -> Video.homePersonalized
#   GET watch/recommended  WatchService.get_recommended_videos           -> Video.watchRecommended
# Source of truth: new-facereview@7948b7e7 app/services/home_service.py, app/services/watch_service.py and
# common/utils/recommendation_alg.py. The arithmetic lives in recommendation_math.py and recommendation_pool_builder.py next to this
# file (pinned against the original by product/tests/slice13d_*_oracle_test.py); this module only gathers the inputs the original
# reads and shapes the answer. RDB reads go through ctx.data.read (row policy and principal enforced by the server); Mongo reads go
# through ctx.call("mongo", ...) with exact grants.
#
# Deliberate differences from the original (see extensions/README.md):
#  - No Redis pool cache. The original warms `facereview:reco:pool` from a Celery beat job and falls back to building the pool in the
#    request. A read extension cannot write a cache, so every request builds the pool from the RDB and Mongo (what the original does
#    on a cache miss). The cache is only a latency device: the pool is a pure function of the data.
#  - The pool is built from at most MAX_POOL_VIDEOS active videos and fails loudly beyond that instead of truncating.
#  - Randomness is the original's: the exploration sample of the personalized list, the fallback sample and the padding of thin
#    categories are drawn with the `random` module and are not reproducible.
#  - Where the original relies on an unspecified database order (all active videos, Mongo natural order) this module orders by
#    primary key / `_id`, which is what InnoDB and MongoDB return for those scans. The favourite genres come out of the unique index
#    (user_id, genre) in ENUM declaration order, which the module reproduces (see _favorite_genres).
#  - A failing call shows only WIRE.EXTENSION_ERROR; the original's 404 VIDEO_NOT_FOUND for an unknown or deleted video is that error.
import importlib.util
import json
import os

_here = os.path.dirname(os.path.abspath(__file__))


def _load(name):
    spec = importlib.util.spec_from_file_location(f"slice13d_{name}", os.path.join(_here, f"{name}.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_lists = _load("slice13b_lists")
_math = _load("recommendation_math")
_pool = _load("recommendation_pool_builder")

# GenreEnum order of common/enum/youtube_genre.py: it drives which categories are padded and in which order.
GENRES = ("drama", "eating", "travel", "cook", "show", "information", "game", "sports", "music", "animal", "beauty", "comedy",
          "horror", "exercise", "vlog", "etc")
MAX_POOL_VIDEOS = 5000
MAX_VIEW_LOGS = 5000
PERSONAL_LIMIT = 20
RECENT_SESSIONS = 20
BY_ID = [{"field": "id", "dir": "asc"}]


def _text(items):
    return json.dumps(items, ensure_ascii=False, separators=(",", ":"))


async def _active_videos(ctx):
    # Video.query.filter_by(is_deleted=0).all(): the row policy is exactly that predicate (and refuses an inactive caller).
    rows = await _lists._read_all(ctx, {
        "read": "Video", "select": ["id", "youtube_url", "title", "category", "view_count", "created_at", "likeCount"], "sort": BY_ID,
    }, cap=MAX_POOL_VIDEOS)
    videos = [{"video_id": row["id"], "youtube_url": row["youtube_url"], "title": row["title"], "category": row["category"],
               "view_count": row["view_count"], "created_at": row["created_at"], "is_deleted": 0} for row in rows]
    return videos, {row["id"]: row["likeCount"] for row in rows}


async def _distributions(ctx, video_ids):
    docs = {}
    for chunk in _lists._chunks(list(video_ids), _lists.MONGO_IN):
        res = await _lists._mongo(ctx, "video_distribution", command="find", filter={"video_id": {"$in": chunk}}, limit=_lists.MONGO_IN)
        docs.update({doc["video_id"]: doc for doc in res["docs"]})
    return docs


async def _ranked_pool(ctx):
    # HomeService._build_and_cache_ranked_pool without the cache: (slim pool, {category: slim entries}).
    videos, like_map = await _active_videos(ctx)
    docs = await _distributions(ctx, [video["video_id"] for video in videos])
    return _pool.build_ranked_pool(videos, docs, like_map, list(GENRES))


def _dto(entry, bookmarked):
    return {"video_id": entry["video_id"], "youtube_url": entry["youtube_url"], "title": entry["title"],
            "dominant_emotion": entry.get("dominant_emotion"), "dominant_emotion_per": entry.get("dominant_emotion_per", 0.0),
            "is_bookmarked": bookmarked}


async def homeCategory(input, ctx):
    _, by_category = await _ranked_pool(ctx)
    ids = [entry["video_id"] for entries in by_category.values() for entry in entries]
    marked = await _lists._bookmarked_ids(ctx, ids) if ids else set()
    categories = [{"category_name": name, "videos": [_dto(entry, entry["video_id"] in marked) for entry in entries]}
                  for name, entries in by_category.items() if entries]
    return {"categories": _text(categories)}


async def _favorite_genres(ctx):
    rows = await _lists._read_all(ctx, {"read": "UserFavoriteGenre", "select": ["id", "genre"], "sort": BY_ID})
    # `user.favorite_genres.all()` is an unordered query that MySQL serves from the unique index (user_id, genre), so the original's
    # "favourite rank" is the ENUM declaration order of the column (GENRES), not the order the member picked them.
    return sorted((row["genre"] for row in rows), key=GENRES.index)


async def _viewed_video_ids(ctx):
    rows = await _lists._read_all(ctx, {"read": "VideoViewLog", "select": ["id", "video"], "sort": BY_ID}, cap=MAX_VIEW_LOGS)
    return {row["video"] for row in rows}


async def _recent_watching(ctx, pool):
    # find_recent_summaries_by_user_id(user_id, limit=20): the host adds the caller's user_id to the filter.
    res = await _lists._mongo(ctx, "youtube_watching_data", command="find", filter={},
                              projection={"video_id": 1, "dominant_emotion": 1, "emotion_percentages": 1},
                              sort_by="created_at", desc=True, limit=RECENT_SESSIONS)
    category_of = {entry["video_id"]: entry["category"] for entry in pool}
    recent = []
    for doc in res["docs"]:
        dominant = doc.get("dominant_emotion")
        if dominant is None:
            continue
        recent.append({"category": category_of.get(doc.get("video_id")), "dominant_emotion": dominant,
                       "emotion_percentages": doc.get("emotion_percentages", {})})
    return recent


async def homePersonalized(input, ctx):
    favorites = await _favorite_genres(ctx)
    viewed = await _viewed_video_ids(ctx)
    pool, _ = await _ranked_pool(ctx)
    recent = await _recent_watching(ctx, pool)
    picked = _math.rank_personalized(pool=pool, recent_watching=recent, favorite_genres=favorites, viewed_ids=viewed,
                                     limit=PERSONAL_LIMIT, top_n=150, random_n=50)
    if not picked:
        unwatched = [entry for entry in pool if entry.get("video_id") not in viewed]
        by_genre = [entry for entry in unwatched if entry.get("category") in favorites]
        candidates = by_genre if by_genre else unwatched
        if not candidates:
            return {"videos": "[]"}
        picked = _math.random.sample(candidates, min(PERSONAL_LIMIT, len(candidates)))
    marked = await _lists._bookmarked_ids(ctx, [entry["video_id"] for entry in picked])
    return {"videos": _text([_dto(entry, entry["video_id"] in marked) for entry in picked])}


def _doc_order(doc):
    oid = doc.get("_id")
    return oid["$oid"] if isinstance(oid, dict) else str(oid)


async def watchRecommended(input, ctx):
    video_id, page, size = input["video_id"], input["page"], input["size"]
    current = await _lists._read_rows(ctx, {"read": "Video", "select": ["id", "category"],
                                            "filter": [{"field": "id", "op": "eq", "value": video_id}], "limit": 1})
    if not current:
        raise Exception("VIDEO_NOT_FOUND")
    res = await _lists._mongo(ctx, "video_distribution", command="find_one", filter={"video_id": video_id})
    distribution = res["doc"]
    # find_by_video_id -> VideoDistribution.from_dict: a missing dominant_emotion key becomes 'neutral', a stored null stays None.
    emotion = distribution.get("dominant_emotion", "neutral") if distribution else None

    # The filter grammar has no `!=`, so the current video is dropped after the read (the cap leaves room for it).
    base = {"read": "Video", "select": ["id", "youtube_url", "title"],
            "filter": [{"field": "category", "op": "eq", "value": current[0]["category"]}]}
    if emotion:
        same = await _lists._read_all(ctx, {**base, "sort": BY_ID}, cap=MAX_POOL_VIDEOS)
    else:
        # No representative emotion yet: most viewed first, newest among equals.
        same = await _lists._read_all(ctx, {**base, "select": ["id", "youtube_url", "title", "view_count", "created_at"],
                                            "sort": [{"field": "view_count", "dir": "desc"}, {"field": "created_at", "dir": "desc"}]},
                                      cap=MAX_POOL_VIDEOS)
    same = [row for row in same if row["id"] != video_id]
    if not same:
        return _lists._result([], 0, page, size, False)
    info = {row["id"]: row for row in same}

    scored = []
    if emotion:
        docs = []
        for chunk in _lists._chunks([row["id"] for row in same], _lists.MONGO_IN):
            found = await _lists._mongo(ctx, "video_distribution", command="find",
                                        filter={"video_id": {"$in": chunk}, "dominant_emotion": emotion}, limit=_lists.MONGO_IN)
            docs.extend(found["docs"])
        # The original iterates the cursor in natural (insertion) order before its stable sort by score.
        docs.sort(key=_doc_order)
        for doc in docs:
            if doc["video_id"] not in info:
                continue
            percent = doc.get("emotion_averages", {}).get(emotion, 0.0) * 100.0
            scored.append({"video_id": doc["video_id"], "youtube_url": info[doc["video_id"]]["youtube_url"],
                           "title": info[doc["video_id"]]["title"], "dominant_emotion": emotion,
                           "dominant_emotion_per": round(percent, 2), "score": doc.get("recommendation_scores", {}).get(emotion, 0.0)})
        scored.sort(key=lambda entry: entry["score"], reverse=True)
    else:
        docs = await _distributions(ctx, list(info))
        for row in same:
            doc = docs.get(row["id"])
            dominant = doc.get("dominant_emotion") if doc else None
            percent = 0.0
            if doc and dominant:
                percent = round(doc.get("emotion_averages", {}).get(dominant, 0.0) * 100.0, 2)
            scored.append({"video_id": row["id"], "youtube_url": row["youtube_url"], "title": row["title"],
                           "dominant_emotion": dominant, "dominant_emotion_per": percent})

    total = len(scored)
    start = (page - 1) * size
    shown = scored[start:start + size]
    marked = await _lists._bookmarked_ids(ctx, [entry["video_id"] for entry in shown]) if shown else set()
    return _lists._result([_dto(entry, entry["video_id"] in marked) for entry in shown], total, page, size, start + size < total)
