# WBS 7.2.5: FaceReview watch detail as an AIP read extension.
#   GET watch/video  WatchService.get_video_detail -> Video.watchDetail (+ the `Video.view` transition for `view_count += 1`)
# Source of truth: new-facereview@7948b7e7 app/services/watch_service.py. RDB reads go through ctx.data.read (row policy and principal
# enforced by the server). The all-viewer emotion timeline comes from the derived Mongo collection `video_emotion_timeline`, which the
# server computes (the arithmetic of `_get_compressed_timeline_data`); no session document or user id is readable through it.
#
# Differences from the original (see extensions/README.md):
#  - The view count is not incremented inside this read. The caller applies `Video.view` after the read, and this answer carries the
#    count from before the increment, as the original's does.
#  - A video with no usable timeline key gets the original's default timeline (one point at x=1, y=0.0 per emotion).
#  - An unknown or soft-deleted video is WIRE.EXTENSION_ERROR (original: 404 VIDEO_NOT_FOUND).
import importlib.util
import json
import os

_here = os.path.dirname(os.path.abspath(__file__))


def _load(name):
    spec = importlib.util.spec_from_file_location(f"slice13f_{name}", os.path.join(_here, f"{name}.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_lists = _load("slice13b_lists")

DEFAULT_POINT = [{"x": 1, "y": 0.0}]
EMOTIONS = ("happy", "neutral", "surprise", "sad", "angry")


def _text(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


async def watchDetail(input, ctx):
    video_id = input["video_id"]
    rows = await _lists._read_rows(ctx, {
        "read": "Video",
        "select": ["id", "youtube_url", "title", "channel_name", "category", "duration", "view_count", "likeCount", "commentCount"],
        "filter": [{"field": "id", "op": "eq", "value": video_id}], "limit": 1,
    })
    if not rows:
        raise Exception("VIDEO_NOT_FOUND")
    video = rows[0]
    likes = await _lists._read_rows(ctx, {"read": "VideoLike", "select": ["id"],
                                          "filter": [{"field": "video", "op": "eq", "value": video_id}], "limit": 1})
    bookmarked = await _lists._bookmarked_ids(ctx, [video_id])
    res = await _lists._mongo(ctx, "video_emotion_timeline", command="timeline", video_id=video_id, duration=video["duration"])
    timeline = res["timeline"] or {emotion: DEFAULT_POINT for emotion in EMOTIONS}
    return {
        "video_id": video["id"], "youtube_url": video["youtube_url"], "title": video["title"],
        "channel_name": video["channel_name"] or "", "category": video["category"], "duration": video["duration"],
        "view_count": video["view_count"], "like_count": video["likeCount"], "comment_count": video["commentCount"],
        "user_is_liked": bool(likes), "is_bookmarked": video_id in bookmarked,
        "timeline_data": _text({emotion: timeline[emotion] for emotion in EMOTIONS}),
    }
