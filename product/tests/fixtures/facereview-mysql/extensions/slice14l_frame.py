# WBS 7.2.6 slice 3: the frame's emotion scores into the session document's timeline (decision D-P-WATCH-FRAMES).
#   original: video_watching_socket._update_realtime_statistics -> YoutubeWatchingDataRepository.upsert_frame
#
# The scores are NOT an input. A member who can call this extension must not be able to write scores into their own session (the
# all-viewer timeline is summed over every session), so the server stores them under a Redis key only it writes and this extension takes
# them once with getdel. No server code writes that key yet because no model runtime is connected: without it the extension does nothing.
#
# Differences from upsert_frame (deliberate):
#  - Only the two timeline maps are written. The original also $inc'd frame_count / emotion_sum and recomputed emotion_percentages
#    and dominant_emotion on every frame; a counter is not idempotent (a repeated frame was counted twice) and the write
#    capability's find_one_and_update returns the whole document (1 MiB limit). Those values belong to the finalization/aggregation
#    that reads the timeline.
#  - video, duration and user come from the caller's own VideoViewLog row and its video, never from the browser.
import hashlib
import json
import math
from datetime import datetime, timezone

STAGE_PREFIX = "facereview:watch:frame:"
MAX_CENTI = 100_000_000
# Order of the stored score array (the original's `emotion_scores`).
SCORE_ORDER = ("neutral", "happy", "surprise", "sad", "angry")
# Order the original's analyzer lists the emotions in; `max` keeps the first of equal scores, so it decides ties.
TIE_ORDER = ("happy", "surprise", "angry", "sad", "neutral")


def _bad(message):
    raise Exception(message)


def _staged_key(view_log_id, centi):
    return STAGE_PREFIX + hashlib.sha256(view_log_id.encode("utf-8")).hexdigest() + ":" + str(centi)


def _scores(text):
    try:
        value = json.loads(text)
    except Exception:
        _bad("invalid staged scores")
    if not isinstance(value, dict) or set(value) != set(SCORE_ORDER):
        _bad("invalid staged scores")
    for score in value.values():
        if isinstance(score, bool) or not isinstance(score, (int, float)) or not math.isfinite(score) or not 0 <= score <= 100:
            _bad("invalid staged scores")
    return value


def _date(moment):
    millis = int((moment - datetime(1970, 1, 1)).total_seconds() * 1000)
    return {"$date": {"$numberLong": str(millis)}}


async def recordFrame(input, ctx):
    view_log_id = input["id"]
    centi = input["centi"]
    if not isinstance(view_log_id, str) or not view_log_id:
        _bad("invalid session id")
    if isinstance(centi, bool) or not isinstance(centi, int) or not 0 <= centi <= MAX_CENTI:
        _bad("invalid frame time")

    rows = (await ctx.data.read({
        "read": "VideoViewLog", "select": ["id", "video", "user", "created_at"], "limit": 1,
        "filter": [{"field": "id", "op": "eq", "value": view_log_id}],
    }))["rows"]
    if not rows:
        _bad("session not found")
    video_id = rows[0]["video"]
    videos = (await ctx.data.read({
        "read": "Video", "select": ["id", "duration"], "limit": 1, "filter": [{"field": "id", "op": "eq", "value": video_id}],
    }))["rows"]
    duration = videos[0]["duration"] if videos else 0

    staged = await ctx.redis.getdel(_staged_key(view_log_id, centi), compensate=True)
    if staged is None:
        return {"recorded": False}
    scores = _scores(staged)
    most_emotion = max(TIE_ORDER, key=lambda name: scores[name])
    time_key = str(centi)
    now = _date(datetime.now(timezone.utc).replace(tzinfo=None))
    await ctx.mongo.update_one(
        "youtube_watching_data",
        {"video_view_log_id": view_log_id},
        {
            "$set": {
                f"most_emotion_timeline.{time_key}": most_emotion,
                f"emotion_score_timeline.{time_key}": [scores[name] for name in SCORE_ORDER],
                "updated_at": now,
            },
            "$setOnInsert": {
                "video_id": video_id,
                "video_view_log_id": view_log_id,
                "duration": duration,
                "created_at": now,
                "completion_rate": 0.0,
                "dominant_emotion": "neutral",
                "emotion_percentages": {name: 0.0 for name in SCORE_ORDER},
                "client_info": {"ip_address": None, "user_agent": None, "device": {"os": None, "browser": None, "is_mobile": False}},
            },
        },
        upsert=True,
    )
    return {"recorded": True}
