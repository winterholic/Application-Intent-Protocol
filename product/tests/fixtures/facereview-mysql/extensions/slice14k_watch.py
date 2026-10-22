# WBS 7.2.6 slice 2: first-frame session setup. The socket supplies `user` from the freshly verified principal;
# this function never reads a user id from the browser's watch_frame payload.
import hashlib
import json
import math


SESSION_PREFIX = "facereview:watch:active:"
DEFAULT_TTL_SECONDS = 10800
MAX_DURATION_SECONDS = 1_000_000


async def startWatching(input, ctx):
    view_log_id = input["id"]
    video_id = input["video"]
    user_id = input["user"]
    duration = input.get("duration")
    if not isinstance(view_log_id, str) or not view_log_id:
        raise Exception("invalid session id")
    if not isinstance(video_id, str) or not video_id:
        raise Exception("invalid video id")
    if not isinstance(user_id, str) or not user_id:
        raise Exception("authenticated user required")
    if duration is not None and (isinstance(duration, bool) or not isinstance(duration, int) or not 0 <= duration <= MAX_DURATION_SECONDS):
        raise Exception("invalid duration")

    query = {
        "read": "VideoViewLog",
        "select": ["id", "video", "user", "created_at"],
        "limit": 1,
        "filter": [{"field": "id", "op": "eq", "value": view_log_id}],
    }
    # The row policy shows only the caller's own rows, so another member's row looks absent and the create below is
    # refused by the primary key (WIRE.ALREADY_EXISTS) without revealing it.
    rows = (await ctx.data.read(query))["rows"]
    if not rows:
        # created_at/updated_at stay the database's own clock; this function never sends a time.
        await ctx.data.create("VideoViewLog", {"id": view_log_id, "video": video_id, "user": user_id})
        rows = (await ctx.data.read(query))["rows"]
    row = rows[0]
    if row["video"] != video_id or row["user"] != user_id:
        raise Exception("session does not match")
    created_at = row["created_at"]

    ttl = DEFAULT_TTL_SECONDS if duration in (None, 0) else math.ceil(duration * 1.5)
    if not 1 <= ttl <= 30 * 24 * 60 * 60:
        raise Exception("session ttl out of range")
    session_key = SESSION_PREFIX + hashlib.sha256(view_log_id.encode("utf-8")).hexdigest()
    session = json.dumps({
        "video_view_log_id": view_log_id,
        "user_id": user_id,
        "video_id": video_id,
        "duration": duration or 0,
        "created_at": created_at,
    }, separators=(",", ":"), ensure_ascii=False)
    # A write extension names no effect key (only durable jobs do); a failed call or an unknown outcome is settled by compensate.
    await ctx.redis.set_ex(session_key, session, ttl, compensate=True)
    return {"initialized": True}
