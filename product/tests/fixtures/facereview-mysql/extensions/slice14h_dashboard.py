# WBS 7.2.9: the administrator dashboard of FaceReview, the parts a plain aggregate cannot give.
#   AdminService._get_video_request_pipeline -> VideoRequest.pipeline      (pending count, average minutes of the last 30 days)
#   AdminService._get_content_health (Mongo)  -> Video.contentHealth       (distribution averages, dominant emotion counts)
# Source of truth: new-facereview@7948b7e7 app/services/admin_service.py. The counts per period, the weekly active members and the views per
# genre are the aggregates User.signupCount, User.weeklyActive and Video.categoryViews, called by the client (see extensions/README.md,
# Slice14-H).
#
# Both extensions are administrator only: the aggregate they call first (VideoRequest.pendingCount, User.totalCount) is refused by the
# server for anyone else, and that refusal ends the call.
#
# Differences from the original: no Redis cache of the weekly active members exists in this port (every call reads the database); the
# average is read from the rows (at most MAX_PAGES x 100 processed requests; more fails instead of answering with a partial average);
# dominant emotions with the same count are ordered by name.
import json
from datetime import datetime, timedelta, timezone
from decimal import ROUND_HALF_UP, Decimal

PAGE = 100
MAX_PAGES = 50
WINDOW = timedelta(days=30)


def _text(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def _naive_utc(text):
    return datetime.fromisoformat(text.replace("Z", "+00:00")).astimezone(timezone.utc).replace(tzinfo=None)


def _minutes(created, updated):
    """MySQL TIMESTAMPDIFF(MINUTE, created, updated): whole minutes, truncated toward zero."""
    return int((updated - created).total_seconds() / 60)


async def pipeline(input, ctx):
    pending = (await ctx.data.aggregate("VideoRequest", "pendingCount", {}))["value"]
    cutoff = datetime.now(timezone.utc).replace(tzinfo=None) - WINDOW
    total, count, offset = 0, 0, 0
    for _ in range(MAX_PAGES):
        page = await ctx.data.read({
            "read": "VideoRequest", "select": ["id", "created_at", "updated_at"], "limit": PAGE, "offset": offset,
            "filter": [{"field": "status", "op": "in", "value": ["ACCEPTED", "REJECTED"]},
                       {"field": "updated_at", "op": "gte", "value": cutoff.strftime("%Y-%m-%dT%H:%M:%SZ")}],
            "sort": [{"field": "created_at", "dir": "asc"}, {"field": "id", "dir": "asc"}]})
        for row in page["rows"]:
            total += _minutes(_naive_utc(row["created_at"]), _naive_utc(row["updated_at"]))
            count += 1
        if len(page["rows"]) < PAGE:
            break
        offset += PAGE
    else:
        raise Exception(f"more than {MAX_PAGES * PAGE} requests were processed in the last 30 days")
    average = None
    if count:
        # MySQL AVG of integers is a DECIMAL with four fraction digits (rounded half up); the original rounds that to one place.
        average = round(float((Decimal(total) / Decimal(count)).quantize(Decimal("0.0001"), rounding=ROUND_HALF_UP)), 1)
    return {"body": _text({"pending_count": pending, "avg_processing_minutes": average})}


async def contentHealth(input, ctx):
    await ctx.data.aggregate("User", "totalCount", {})
    health = await ctx.call("mongo", "video_distribution_stats", {"command": "health"})
    return {"body": _text({
        "avg_completion_rate": health["avg_completion_rate"],
        "emotion_distribution": health["emotion_distribution"],
        "dominant_emotion_video_counts": [{"emotion": row["emotion"], "video_count": row["video_count"]} for row in health["dominant"]]})}
