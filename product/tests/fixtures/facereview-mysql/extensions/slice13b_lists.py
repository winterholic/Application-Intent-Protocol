# WBS 7.2.3 Slice13-B: FaceReview home list APIs (home/search, home/video/all, home/bookmark) as AIP read extensions.
# Source of truth: new-facereview@7948b7e7 app/services/home_service.py get_search_videos / get_all_videos / get_bookmark_videos
# and app/dto/home.py. RDB reads go through ctx.data.read (row policy and principal enforced by the server); Mongo reads go
# through ctx.call("mongo", ...) with the exact `video_distribution` grant.
#
# Server limits this module has to live with (they are enforced by the host, not here):
#   RDB read:  rows <= 100 per call, `in` <= 50 values, offset <= 100000, OR <= 10 conditions
#   Mongo:     limit <= 100, skip <= 100000, `$in` <= 100 values, filter/sort fields are allowlisted
# Where the original does something unbounded (emotion-filtered search, bookmark list), this module chunks within those limits up to
# MAX_CANDIDATES and then fails loudly. It never truncates silently.
import json

EMOTION_LABELS = ("neutral", "happy", "surprise", "sad", "angry")
RDB_ROWS = 100
RDB_IN = 50
MONGO_IN = 100
MONGO_SKIP_MAX = 100000
# Upper bound of video ids the extension will gather before Mongo paging (emotion-filtered search, bookmark list).
MAX_CANDIDATES = 5000
# The original sorts `[('created_at', -1), ('_id', -1)]`. The host's Mongo `sort` object loses key order (the JSON map is
# alphabetical, so `_id` would win), which would change page membership. The single-key `sort_by`/`desc` form keeps `created_at`
# first; documents with an identical created_at (millisecond) then come back in Mongo's natural order instead of `_id` desc. The
# chunked paths below (more than MONGO_IN ids) sort client-side by (created_at, _id) and are exact.
# The RDB IN lookup order of the original is unspecified; this port answers in Mongo page order.


def _chunks(items, size):
    for start in range(0, len(items), size):
        yield items[start:start + size]


async def _read_rows(ctx, query):
    return (await ctx.data.read(query))["rows"]


async def _read_all(ctx, query, cap=MAX_CANDIDATES):
    # Pages an RDB read with limit/offset. The sort the caller gives plus the server's id tie-breaker keep pages stable.
    rows = []
    offset = 0
    while True:
        page = await _read_rows(ctx, {**query, "limit": RDB_ROWS, "offset": offset})
        rows.extend(page)
        if len(rows) > cap:
            raise Exception(f"more than {cap} rows; unsupported by this extension")
        if len(page) < RDB_ROWS:
            return rows
        offset += RDB_ROWS


async def _visible_videos(ctx, video_ids):
    # Video rows the caller may read (row policy: not deleted, active caller or guest). The original queries without
    # is_deleted here and so can expose deleted videos; that is not reproduced.
    found = {}
    for chunk in _chunks(list(video_ids), RDB_IN):
        rows = await _read_rows(ctx, {
            "read": "Video", "select": ["id", "youtube_url", "title"],
            "filter": [{"field": "id", "op": "in", "value": chunk}], "limit": RDB_ROWS,
        })
        for row in rows:
            found[row["id"]] = row
    return found


async def _bookmarked_ids(ctx, video_ids):
    # The caller's own bookmarks among these videos (policy: user = actor). A guest has none, as in _get_bookmarked_ids.
    marked = set()
    for chunk in _chunks(list(video_ids), RDB_IN):
        rows = await _read_rows(ctx, {
            "read": "VideoBookmark", "select": ["id", "video"],
            "filter": [{"field": "video", "op": "in", "value": chunk}], "limit": RDB_ROWS,
        })
        marked.update(row["video"] for row in rows)
    return marked


async def _all_bookmark_video_ids(ctx):
    rows = await _read_all(ctx, {
        "read": "VideoBookmark", "select": ["id", "video"],
        "sort": [{"field": "created_at", "dir": "desc"}],
    })
    return [row["video"] for row in rows]


def _mongo(ctx, collection, **spec):
    return ctx.call("mongo", collection, spec)


async def _mongo_count(ctx, query):
    return (await _mongo(ctx, "video_distribution", command="count_documents", filter=query))["count"]


async def _mongo_page(ctx, query, page, size):
    skip = (page - 1) * size
    if skip > MONGO_SKIP_MAX:
        raise Exception(f"page {page} x size {size} exceeds the Mongo skip limit {MONGO_SKIP_MAX}")
    res = await _mongo(ctx, "video_distribution", command="find", filter=query, sort_by="created_at", desc=True, skip=skip, limit=size)
    return res["docs"]


def _sort_key(doc):
    # Extended-JSON shapes of created_at and _id as the host returns them; anything else is refused rather than mis-sorted.
    created = doc.get("created_at")
    if isinstance(created, dict) and "$date" in created:
        created = created["$date"]
        if isinstance(created, dict):
            created = created.get("$numberLong")
    oid = doc["_id"]["$oid"] if isinstance(doc.get("_id"), dict) else doc.get("_id")
    if created is None or oid is None:
        raise Exception("video_distribution document has no sortable created_at/_id")
    return (str(created) if not isinstance(created, (int, float)) else f"{int(created):020d}", str(oid))


async def _mongo_page_over_ids(ctx, video_ids, extra, page, size):
    # `{'video_id': {'$in': ids}, **extra}` with the original's count/sort/skip/limit, for any number of ids.
    # video_id is unique in the collection, so per-chunk counts add up and per-chunk results are disjoint.
    if len(video_ids) <= MONGO_IN:
        query = {"video_id": {"$in": video_ids}, **extra}
        return await _mongo_count(ctx, query), await _mongo_page(ctx, query, page, size)
    total = 0
    light = []
    for chunk in _chunks(video_ids, MONGO_IN):
        query = {"video_id": {"$in": chunk}, **extra}
        total += await _mongo_count(ctx, query)
        res = await _mongo(ctx, "video_distribution", command="find", filter=query, projection={"video_id": 1, "created_at": 1}, limit=MONGO_IN)
        light.extend(res["docs"])
    light.sort(key=_sort_key, reverse=True)
    skip = (page - 1) * size
    wanted = [doc["video_id"] for doc in light[skip:skip + size]]
    if not wanted:
        return total, []
    full = {}
    for chunk in _chunks(wanted, MONGO_IN):
        res = await _mongo(ctx, "video_distribution", command="find", filter={"video_id": {"$in": chunk}}, limit=MONGO_IN)
        full.update({doc["video_id"]: doc for doc in res["docs"]})
    return total, [full[video_id] for video_id in wanted if video_id in full]


def _percent(doc, dominant):
    averages = doc.get("emotion_averages", {})
    return round(averages.get(dominant, 0.0) * 100.0, 2)


def _dto(video, doc, bookmarked):
    # get_all_videos / get_bookmark_videos / emotion-filtered search: vd.get('dominant_emotion', 'neutral').
    dominant = doc.get("dominant_emotion", "neutral")
    return {
        "video_id": video["id"], "youtube_url": video["youtube_url"], "title": video["title"],
        "dominant_emotion": dominant, "dominant_emotion_per": _percent(doc, dominant), "is_bookmarked": bookmarked,
    }


def _result(videos, total, page, size, has_next):
    return {"videos": json.dumps(videos, ensure_ascii=False, separators=(",", ":")), "total": total, "page": page, "size": size,
            "has_next": has_next}


async def _from_docs(ctx, docs, total, page, size, bookmarked=None):
    # Mongo-first assembly shared by video/all, bookmark and emotion-filtered search.
    if not docs:
        return _result([], total, page, size, False)
    ids = [doc["video_id"] for doc in docs]
    videos = await _visible_videos(ctx, ids)
    marked = await _bookmarked_ids(ctx, ids) if bookmarked is None else bookmarked
    dtos = [_dto(videos[doc["video_id"]], doc, doc["video_id"] in marked) for doc in docs if doc["video_id"] in videos]
    return _result(dtos, total, page, size, (page * size) < total)


async def homeVideoAll(input, ctx):
    page, size, emotion = input["page"], input["size"], input["emotion"]
    query = {} if emotion == "all" else {"dominant_emotion": emotion}
    total = await _mongo_count(ctx, query)
    docs = await _mongo_page(ctx, query, page, size)
    return await _from_docs(ctx, docs, total, page, size)


async def homeBookmark(input, ctx):
    page, size, emotion = input["page"], input["size"], input["emotion"]
    ids = await _all_bookmark_video_ids(ctx)
    if not ids:
        return _result([], 0, page, size, False)
    extra = {} if emotion == "all" else {"dominant_emotion": emotion}
    total, docs = await _mongo_page_over_ids(ctx, ids, extra, page, size)
    # The list is the caller's own bookmark list: always bookmarked, no per-video lookup.
    return await _from_docs(ctx, docs, total, page, size, bookmarked=set(ids)) if docs else _result([], total, page, size, False)


def _keyword_filter(keyword_type, keyword):
    if keyword_type == "title":
        return [{"field": "title", "op": "like", "value": keyword}]
    if keyword_type == "channel_name":
        return [{"field": "channel_name", "op": "like", "value": keyword}]
    return [{"or": [{"field": "title", "op": "like", "value": keyword}, {"field": "channel_name", "op": "like", "value": keyword}]}]


def _parse_emotions(text):
    emotions = [part for part in text.split(",") if part]
    for emotion in emotions:
        if emotion not in ("happy", "sad", "neutral", "surprise", "angry"):
            raise Exception(f"unknown emotion `{emotion}`")
    return emotions


async def homeSearch(input, ctx):
    page, size, keyword = input["page"], input["size"], input["keyword"]
    filters = _keyword_filter(input["keyword_type"], keyword)
    emotions = _parse_emotions(input["emotions"])
    base = {"read": "Video", "select": ["id", "youtube_url", "title"], "filter": filters,
            "sort": [{"field": "created_at", "dir": "desc"}]}

    if not emotions:
        # total = query.count(): the extension cannot pass a caller filter to the totalCount aggregate, so count by paging ids.
        total_rows = await _read_all(ctx, {**base, "select": ["id"]})
        total = len(total_rows)
        videos = await _read_rows(ctx, {**base, "limit": size, "offset": (page - 1) * size})
        if not videos:
            return _result([], total, page, size, False)
        ids = [video["id"] for video in videos]
        docs = {}
        for chunk in _chunks(ids, MONGO_IN):
            res = await _mongo(ctx, "video_distribution", command="find", filter={"video_id": {"$in": chunk}}, limit=MONGO_IN)
            docs.update({doc["video_id"]: doc for doc in res["docs"]})
        marked = await _bookmarked_ids(ctx, ids)
        dtos = []
        for video in videos:
            doc = docs.get(video["id"])
            if not doc:
                # original quirk kept: a video without a distribution document is dropped while total still counts it
                continue
            # VideoDistribution.from_dict: a missing dominant_emotion key becomes 'neutral', a stored null stays None (not yet reliable)
            dominant = doc.get("dominant_emotion", "neutral")
            percent = round(doc.get("emotion_averages", {}).get(dominant, 0.0) * 100.0, 2) if dominant else 0.0
            dtos.append({"video_id": video["id"], "youtube_url": video["youtube_url"], "title": video["title"],
                         "dominant_emotion": dominant, "dominant_emotion_per": percent, "is_bookmarked": video["id"] in marked})
        return _result(dtos, total, page, size, (page * size) < total)

    # emotion filter: all matching video ids from the RDB, then Mongo does count/sort/pagination
    candidate_rows = await _read_all(ctx, {**base, "select": ["id"]})
    candidate_ids = [row["id"] for row in candidate_rows]
    if not candidate_ids:
        return _result([], 0, page, size, False)
    total, docs = await _mongo_page_over_ids(ctx, candidate_ids, {"dominant_emotion": {"$in": emotions}}, page, size)
    return await _from_docs(ctx, docs, total, page, size)

