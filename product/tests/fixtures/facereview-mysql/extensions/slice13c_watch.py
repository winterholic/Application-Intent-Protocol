# Slice13-C: FaceReview watch/video and watch/recommended on the AIP worker.
# Source of truth: new-facereview@7948b7e721837fdb73c427cb54e12cad4ec2d5af app/services/watch_service.py
# (get_video_detail / _get_compressed_timeline_data / get_recommended_videos). The pure functions below keep the original
# arithmetic and ordering on purpose; slice13c_oracle.py runs the real original functions against the same data.
#
# What this module does NOT do (see the slice report, nothing here substitutes a sample or an empty success):
#   * timeline: youtube_watching_data is a private collection. The server forces user_id = actor, so the worker can only see
#     the caller's own sessions. timeline_scope says so; it is "caller" until a server-side aggregate contract exists.
#   * Mongo/Redis grants cannot yet be declared in a definition (extension access accepts Resource.aggregate/transition only).

import json

NOT_FOUND = "VIDEO_NOT_FOUND"
EMOTIONS = ["neutral", "happy", "surprise", "sad", "angry"]
N_BUCKETS = 100
PAGE = 100  # Video read budget rows and Mongo limit/$in are both capped at 100 per call
READ_OFFSET_CAP = 100000  # Video read budget offset
BOOKMARK_OR_CHUNK = 10  # planner MAX_OR_CONDITIONS; measured: a 25-wide OR is refused with WIRE.OR_LIST_EXCEEDED
DIST_COLLECTION = "video_distribution"
WATCH_COLLECTION = "youtube_watching_data"
VIDEO_FIELDS = ["id", "youtube_url", "title", "channel_name", "category", "duration", "view_count", "created_at"]


class VideoNotFound(Exception):
    """Original raises BusinessError(APIError.VIDEO_NOT_FOUND); the worker wire only carries it as EXTENSION_ERROR text."""

    def __init__(self):
        super().__init__(NOT_FOUND)


class BudgetExceeded(Exception):
    """A bounded read would silently drop rows; fail instead of returning a partial list as if it were complete."""


# ---------------------------------------------------------------------------------------------------------------------
# pure ports (no ctx)
# ---------------------------------------------------------------------------------------------------------------------
def default_timeline():
    point = [{"x": 1, "y": 0.0}]
    return {"happy": list(point), "neutral": list(point), "surprise": list(point), "sad": list(point), "angry": list(point)}


def compress_timeline(docs, duration):
    """_get_compressed_timeline_data after the Mongo read: docs carry emotion_score_timeline only."""
    if not docs:
        return default_timeline()

    emotion_sums = {}
    counts_per_key = {}
    for doc in docs:
        timeline = doc.get("emotion_score_timeline", {})
        for time_key, scores in timeline.items():
            if not isinstance(scores, (list, tuple)) or len(scores) < 5:
                continue
            if time_key not in emotion_sums:
                emotion_sums[time_key] = [0.0, 0.0, 0.0, 0.0, 0.0]
                counts_per_key[time_key] = 0
            for i in range(5):
                emotion_sums[time_key][i] += float(scores[i])
            counts_per_key[time_key] += 1

    if not counts_per_key:
        return default_timeline()

    total_cs = (duration or 1) * 100
    bucket_size = total_cs / N_BUCKETS
    bucket_sums = [[0.0] * 5 for _ in range(N_BUCKETS)]
    bucket_counts = [0] * N_BUCKETS
    for time_key, session_count in counts_per_key.items():
        try:
            cs = float(time_key)
        except (ValueError, TypeError):
            continue
        bucket_idx = min(int(cs / bucket_size), N_BUCKETS - 1)
        sums = emotion_sums[time_key]
        for i in range(5):
            bucket_sums[bucket_idx][i] += sums[i] / session_count
        bucket_counts[bucket_idx] += 1

    lists = {emotion: [] for emotion in EMOTIONS}
    for x in range(N_BUCKETS):
        count = bucket_counts[x]
        for i, emotion in enumerate(EMOTIONS):
            y = round(bucket_sums[x][i] / count, 1) if count > 0 else 0.0
            lists[emotion].append({"x": x + 1, "y": y})
    return {name: lists[name] for name in ("happy", "neutral", "surprise", "sad", "angry")}


def current_dominant_emotion(distribution_doc):
    """VideoDistribution.from_dict: a stored document without the key reads as 'neutral', an explicit null stays None."""
    if distribution_doc is None:
        return None
    return distribution_doc.get("dominant_emotion", "neutral")


def rank_recommendations(same_category, dist_docs, current_emotion):
    """The scored list before pagination.

    same_category: rows (id, youtube_url, title) already excluding the current video, in fallback order when
    current_emotion is falsy. dist_docs: video_distribution documents (any order).
    """
    info = {row["id"]: row for row in same_category}
    if current_emotion:
        scored = []
        for doc in dist_docs:
            vid = doc["video_id"]
            if vid not in info or doc.get("dominant_emotion") != current_emotion:
                continue
            rec_scores = doc.get("recommendation_scores", {})
            emotion_averages = doc.get("emotion_averages", {})
            scored.append({
                "video_id": vid,
                "youtube_url": info[vid]["youtube_url"],
                "title": info[vid]["title"],
                "dominant_emotion": current_emotion,
                "dominant_emotion_per": round(emotion_averages.get(current_emotion, 0.0) * 100.0, 2),
                "score": rec_scores.get(current_emotion, 0.0),
            })
        # Original sorts only by score (stable over Mongo's unspecified order). Ties fall back to video_id so pages are stable.
        scored.sort(key=lambda v: v["video_id"])
        scored.sort(key=lambda v: v["score"], reverse=True)
        return scored

    dist_map = {doc["video_id"]: doc for doc in dist_docs}
    scored = []
    for row in same_category:
        doc = dist_map.get(row["id"])
        dominant = doc.get("dominant_emotion") if doc else None
        per = 0.0
        if doc and dominant:
            per = round(doc.get("emotion_averages", {}).get(dominant, 0.0) * 100.0, 2)
        scored.append({
            "video_id": row["id"],
            "youtube_url": row["youtube_url"],
            "title": row["title"],
            "dominant_emotion": dominant,
            "dominant_emotion_per": per,
        })
    return scored


def page_slice(scored, page, size):
    start = (page - 1) * size
    return scored[start:start + size]


def page_recommendations(scored, page, size, bookmarked):
    end = (page - 1) * size + size
    videos = [
        {
            "video_id": v["video_id"],
            "youtube_url": v["youtube_url"],
            "title": v["title"],
            "dominant_emotion": v["dominant_emotion"],
            "dominant_emotion_per": v["dominant_emotion_per"],
            "is_bookmarked": v["video_id"] in bookmarked,
        }
        for v in page_slice(scored, page, size)
    ]
    return {"videos": videos, "total": len(scored), "page": page, "size": size, "has_next": end < len(scored)}


def build_recommendations(same_category, dist_docs, current_emotion, page, size, bookmark_lookup):
    """get_recommended_videos after the SQL/Mongo reads. bookmark_lookup(ids) -> set of bookmarked video ids."""
    scored = rank_recommendations(same_category, dist_docs, current_emotion)
    ids = [v["video_id"] for v in page_slice(scored, page, size)]
    return page_recommendations(scored, page, size, bookmark_lookup(ids) if ids else set())


# ---------------------------------------------------------------------------------------------------------------------
# worker entries
# ---------------------------------------------------------------------------------------------------------------------
def _dump(value):
    # Extension output is scalar-only on the wire; the original JSON shape is carried as text, unchanged.
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def _rows(response):
    return response.get("rows", []) if isinstance(response, dict) else response


def _count(response):
    if isinstance(response, dict):
        rows = response.get("rows")
        return rows[0] if rows else response.get("value", 0)
    return response


async def _video_by_id(ctx, video_id):
    rows = _rows(await ctx.data.read({
        "read": "Video",
        "select": VIDEO_FIELDS,
        "filter": [{"field": "id", "op": "eq", "value": video_id}],
        "limit": 1,
    }))
    return rows[0] if rows else None


async def _owned(ctx, resource, video_id):
    rows = _rows(await ctx.data.read({
        "read": resource,
        "select": ["id"],
        "filter": [{"field": "video", "op": "eq", "value": video_id}],
        "limit": 1,
    }))
    return bool(rows)


async def _watching_docs(ctx, video_id):
    docs = []
    skip = 0
    while True:
        res = await ctx.call("mongo", WATCH_COLLECTION, {
            "command": "find",
            "filter": {"video_id": video_id},
            "projection": {"emotion_score_timeline": 1},
            "sort_by": "_id",
            "desc": False,
            "limit": PAGE,
            "skip": skip,
        })
        page = res.get("docs", []) if isinstance(res, dict) else res
        docs.extend(page)
        if len(page) < PAGE:
            return docs
        skip += PAGE


async def watchDetail(input, ctx):  # noqa: N802 — worker entry names are the definition's implementation names
    video_id = input["video"]
    video = await _video_by_id(ctx, video_id)
    if video is None:
        raise VideoNotFound()

    liked = await _owned(ctx, "VideoLike", video_id)
    bookmarked = await _owned(ctx, "VideoBookmark", video_id)
    like_count = _count(await ctx.data.aggregate("VideoLike", "countForVideo", {"video": video_id}))
    comment_count = _count(await ctx.data.aggregate("Comment", "countForVideo", {"video": video_id}))
    docs = await _watching_docs(ctx, video_id)
    timeline = compress_timeline(docs, video.get("duration"))

    # view_count is the value before this visit, as in the original; recordView is the explicit +1 write.
    return {
        "video_id": video["id"],
        "youtube_url": video["youtube_url"],
        "title": video["title"],
        "channel_name": video.get("channel_name") or "",
        "category": video["category"],
        "duration": video.get("duration"),
        "view_count": video.get("view_count"),
        "like_count": like_count,
        "comment_count": comment_count,
        "user_is_liked": liked,
        "is_bookmarked": bookmarked,
        "timeline_data_json": _dump(timeline),
        "timeline_scope": "caller",
        # lets a client tell "no sessions visible" from a real all-zero timeline, which the default point alone cannot
        "timeline_sessions": len(docs),
    }


async def _same_category(ctx, video, ordered):
    sort = (
        [{"field": "view_count", "dir": "desc"}, {"field": "created_at", "dir": "desc"}, {"field": "id", "dir": "asc"}]
        if ordered
        else [{"field": "created_at", "dir": "asc"}, {"field": "id", "dir": "asc"}]
    )
    out = []
    offset = 0
    while True:
        if offset > READ_OFFSET_CAP:
            raise BudgetExceeded("same-category videos exceed the Video read offset budget")
        rows = _rows(await ctx.data.read({
            "read": "Video",
            "select": ["id", "youtube_url", "title", "view_count", "created_at"],
            "filter": [{"field": "category", "op": "eq", "value": video["category"]}],
            "sort": sort,
            "limit": PAGE,
            "offset": offset,
        }))
        out.extend(r for r in rows if r["id"] != video["id"])
        if len(rows) < PAGE:
            return out
        offset += PAGE


async def _distributions(ctx, ids, emotion):
    docs = []
    for i in range(0, len(ids), PAGE):
        chunk = ids[i:i + PAGE]
        flt = {"video_id": {"$in": chunk}}
        if emotion:
            flt["dominant_emotion"] = emotion
        res = await ctx.call("mongo", DIST_COLLECTION, {"command": "find", "filter": flt, "limit": PAGE})
        got = res.get("docs", []) if isinstance(res, dict) else res
        docs.extend(got)
    return docs


async def _bookmarked(ctx, ids):
    found = set()
    for i in range(0, len(ids), BOOKMARK_OR_CHUNK):
        chunk = ids[i:i + BOOKMARK_OR_CHUNK]
        rows = _rows(await ctx.data.read({
            "read": "VideoBookmark",
            "select": ["video"],
            "filter": [{"or": [{"field": "video", "op": "eq", "value": v} for v in chunk]}],
            "limit": PAGE,
        }))
        found.update(r["video"] for r in rows)
    return found


async def watchRecommended(input, ctx):  # noqa: N802
    video_id, page, size = input["video"], input["page"], input["size"]
    video = await _video_by_id(ctx, video_id)
    if video is None:
        raise VideoNotFound()

    current = await ctx.call("mongo", DIST_COLLECTION, {"command": "find_one", "filter": {"video_id": video_id}})
    current_doc = current.get("doc") if isinstance(current, dict) else current
    emotion = current_dominant_emotion(current_doc)

    same_category = await _same_category(ctx, video, ordered=not emotion)
    dist_docs = await _distributions(ctx, [r["id"] for r in same_category], emotion) if same_category else []

    scored = rank_recommendations(same_category, dist_docs, emotion)
    ids = [v["video_id"] for v in page_slice(scored, page, size)]
    bookmarked = await _bookmarked(ctx, ids)
    page_result = page_recommendations(scored, page, size, bookmarked)
    videos = page_result.pop("videos")
    return {**page_result, "videos_json": _dump(videos)}


async def recordView(input, ctx):  # noqa: N802
    result = await ctx.data.apply("Video", "incrementViewCount", [input["id"]])
    return {"changed": len(result["changed"])}
