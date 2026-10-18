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
# alphabetical, so `_id` would win), which would change page membership. So the order is rebuilt client-side:
#   - id-list queries (emotion-filtered search, bookmarks): every matching document is fetched (video_id is unique, at most
#     MONGO_IN per chunk), sorted by (created_at, _id) here, then sliced. Exact for any number of ids.
#   - video/all: `sort_by created_at` pages through a window around the requested slice, groups of equal created_at inside the
#     window are re-sorted by `_id` desc. If a group of equal created_at reaches the window edge (more than TIE_MARGIN equal
#     timestamps next to the page) or the host answers inconsistently, the call fails loudly instead of guessing.
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


TIE_MARGIN = 100
LIGHT = {"video_id": 1, "created_at": 1}


async def _light_window(ctx, query, start, stop):
    # Documents at sorted positions [start, stop) as {_id, video_id, created_at}, fetched in host-sized pieces.
    docs = []
    position = start
    while position < stop:
        take = min(MONGO_IN, stop - position)
        res = await _mongo(ctx, "video_distribution", command="find", filter=query, projection=LIGHT, sort_by="created_at", desc=True,
                           skip=position, limit=take)
        docs.extend(res["docs"])
        if len(res["docs"]) < take:
            break
        position += take
    return docs


def _created(doc):
    return _sort_key(doc)[0]


async def _full_docs(ctx, video_ids):
    full = {}
    for chunk in _chunks(list(video_ids), MONGO_IN):
        res = await _mongo(ctx, "video_distribution", command="find", filter={"video_id": {"$in": chunk}}, limit=MONGO_IN)
        full.update({doc["video_id"]: doc for doc in res["docs"]})
    return full


async def _mongo_page(ctx, query, page, size):
    # `find(query).sort([('created_at', -1), ('_id', -1)]).skip((page - 1) * size).limit(size)` without an id list.
    skip = (page - 1) * size
    start = max(0, skip - TIE_MARGIN - 1)
    stop = skip + size + TIE_MARGIN + 1
    if stop > MONGO_SKIP_MAX + MONGO_IN:
        raise Exception(f"page {page} x size {size} exceeds the Mongo skip limit {MONGO_SKIP_MAX}")
    window = await _light_window(ctx, query, start, stop)
    ids = [doc["video_id"] for doc in window]
    if len(set(ids)) != len(ids):
        raise Exception("the host returned overlapping Mongo pages (unstable order among equal created_at)")
    stamps = [_created(doc) for doc in window]
    page_lo, page_hi = skip - start, skip - start + size
    if len(window) <= page_lo:
        return []  # the page starts beyond the last document
    if start > 0 and not any(stamps[i] != stamps[i + 1] for i in range(0, min(page_lo, len(stamps) - 1))):
        raise Exception(f"more than {TIE_MARGIN} documents share one created_at before the requested page")
    if len(window) == stop - start and not any(stamps[i] != stamps[i + 1] for i in range(max(page_hi - 1, 0), len(stamps) - 1)):
        raise Exception(f"more than {TIE_MARGIN} documents share one created_at after the requested page")
    window.sort(key=_sort_key, reverse=True)
    wanted = [doc["video_id"] for doc in window[page_lo:page_hi]]
    full = await _full_docs(ctx, wanted)
    return [full[video_id] for video_id in wanted if video_id in full]


async def _mongo_page_over_ids(ctx, video_ids, extra, page, size):
    # `{'video_id': {'$in': ids}, **extra}` with the original's count/sort/skip/limit, for any number of ids.
    # video_id is unique, so the per-chunk results are disjoint and `total` is their sum.
    light = []
    for chunk in _chunks(video_ids, MONGO_IN):
        query = {"video_id": {"$in": chunk}, **extra}
        res = await _mongo(ctx, "video_distribution", command="find", filter=query, projection=LIGHT, limit=MONGO_IN)
        if len(res["docs"]) > len(chunk):
            raise Exception("more Mongo documents than ids: video_id is not unique in video_distribution")
        light.extend(res["docs"])
    light.sort(key=_sort_key, reverse=True)
    skip = (page - 1) * size
    wanted = [doc["video_id"] for doc in light[skip:skip + size]]
    full = await _full_docs(ctx, wanted)
    return len(light), [full[video_id] for video_id in wanted if video_id in full]


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
