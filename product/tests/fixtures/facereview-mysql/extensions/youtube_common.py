# WBS 7.2.7: what the original's two YouTube jobs share (new-facereview@7948b7e7, common/scheduler/jobs/*): reading a duration, and
# saving a video the way each job's _save_video does: a Video row, then the two initial MongoDB documents of the video.
#
# Differences from the original (see extensions/README.md, Slice14-B):
#  - The row is made through the exposed Video create (access create.Video), so the create contract and the administrator policy
#    apply; the documents are upserts with $set and $setOnInsert only, which the server may send again after a lost answer.
#  - An exception inside the original's _save_video is logged and the next video goes on. Here only a video that already exists
#    (the unique youtube_url, WIRE.ALREADY_EXISTS) is skipped; any other failure ends the run, and a MongoDB that does not answer
#    asks the job to be tried again (the journal replays the row that was already created, so nothing is saved twice).
#  - The original wrote an empty title or channel name as an empty string; the Video fields are Text(1..255) and Text(1..100)?, so a
#    video without a title is skipped and an empty channel name becomes NULL.
import re
import time

EMOTIONS = ("neutral", "happy", "surprise", "sad", "angry")
_DURATION = re.compile(r"^P(?:(\d+)W)?(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+(?:\.\d+)?)S)?)?$")


def parse_duration(text):
    """ISO 8601 duration to whole seconds, 0 when it cannot be read: the original's int(isodate.parse_duration(x).total_seconds())
    with its except (TypeError, ValueError, OverflowError) -> 0. Years and calendar months have no fixed length and give 0."""
    match = _DURATION.match(text) if isinstance(text, str) else None
    if not match or text in ("P", "PT"):
        return 0
    weeks, days, hours, minutes, seconds = (float(part) if part else 0.0 for part in match.groups())
    return int(weeks * 604800 + days * 86400 + hours * 3600 + minutes * 60 + seconds)


def _now():
    return {"$date": {"$numberLong": str(int(time.time() * 1000))}}


def _error_code(error):
    return getattr(error, "code", None)


async def save_video(ctx, video, category):
    """Creates the Video row and the two initial documents. Returns True when the video is new, False when it already existed."""
    youtube_id = video["youtube_id"]
    title = video["title"][:255]
    if not title:
        return False
    try:
        video_id = await ctx.data.create("Video", {
            "youtube_url": youtube_id, "title": title, "channel_name": video["channel_name"][:100] or None,
            "category": category, "duration": video["duration"], "is_deleted": False,
        }, key=f"video:{youtube_id}")
    except Exception as error:
        if _error_code(error) == "WIRE.ALREADY_EXISTS":
            return False
        raise
    try:
        # VideoDistributionRepository.upsert(VideoDistribution(video_id=...)): zero counts, no dominant emotion yet.
        zeros = {label: 0.0 for label in EMOTIONS}
        await ctx.mongo.update_one("video_distribution", {"video_id": video_id}, {
            "$set": {"average_completion_rate": 0.0, "emotion_averages": zeros, "recommendation_scores": zeros,
                     "dominant_emotion": None, "updated_at": _now()},
            "$setOnInsert": {"video_id": video_id, "created_at": _now()},
        }, upsert=True, key=f"distribution:{youtube_id}")
        # VideoTimelineEmotionCountRepository.upsert(VideoTimelineEmotionCount(video_id=...)): no counts yet.
        await ctx.mongo.update_one("video_timeline_emotion_count", {"video_id": video_id}, {
            "$set": {"emotion_labels": list(EMOTIONS), "counts": {}},
            "$setOnInsert": {"video_id": video_id, "created_at": _now()},
        }, upsert=True, key=f"timeline:{youtube_id}")
    except Exception as error:
        if _error_code(error) == "WIRE.ADAPTER_UNAVAILABLE":
            raise ctx.retry_later("MongoDB does not answer", after_ms=1000)
        raise
    return True
