# WBS 7.2.9: the administrator dashboard of FaceReview, the parts a plain aggregate cannot give.
#   AdminService._get_video_request_pipeline -> VideoRequest.pipeline      (pending count, average minutes of the last 30 days)
#   AdminService._get_content_health (Mongo)  -> Video.contentHealth       (distribution averages, dominant emotion counts)
# Source of truth: new-facereview@7948b7e7 app/services/admin_service.py. The counts per period, the weekly active members and the views per
# genre are the aggregates User.signupCount, User.weeklyActive and Video.categoryViews, called by the client (see extensions/README.md,
# Slice14-H).
#
# systemStatus is the same kind of administrator-only call (AdminService.get_system_status, see its comment below).
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


_REQUESTS_KEY = "facereview:metrics:requests:1h"
_ERRORS_KEY = "facereview:metrics:errors:1h"
_RESPONSE_TIMES_KEY = "facereview:metrics:response_times"


async def systemStatus(input, ctx):
    """AdminService.get_system_status. The host figures come from the server (`mongo.server_status`, Linux only, otherwise null); the request
    figures from the Redis keys the original middleware wrote (nothing in this port writes them: they are 0 until the deployment does)."""
    mysql = "ok"
    try:
        await ctx.data.aggregate("User", "totalCount", {})
    except Exception as error:
        if getattr(error, "code", None) == "WIRE.ACCESS_DENIED":
            raise
        mysql = "error"
    try:
        host = await ctx.call("mongo", "server_status", {"command": "host"})
    except Exception:
        host = {"cpu_usage": None, "memory_usage": None, "memory_total_mb": None, "disk_usage": None}
    total_requests, avg_response_time_ms, error_rate = 0, 0.0, 0.0
    try:
        count = await ctx.redis.get(_REQUESTS_KEY)
        total_requests = int(count) if count else 0
        error_count = await ctx.redis.get(_ERRORS_KEY)
        errors = int(error_count) if error_count else 0
        error_rate = round(errors / total_requests * 100, 2) if total_requests > 0 else 0.0
        times = await ctx.redis.lrange(_RESPONSE_TIMES_KEY, 0, -1)
        if times:
            avg_response_time_ms = round(sum(float(t) for t in times) / len(times), 2)
    except Exception:
        pass
    redis = "ok"
    try:
        await ctx.redis.exists(_REQUESTS_KEY)
    except Exception:
        redis = "error"
    mongodb = "ok"
    try:
        await ctx.call("mongo", "server_status", {"command": "ping"})
    except Exception:
        mongodb = "error"
    return {"body": _text({
        "server": {"cpu_usage": host["cpu_usage"], "memory_usage": host["memory_usage"], "memory_total_mb": host["memory_total_mb"],
                   "disk_usage": host["disk_usage"]},
        "api": {"total_requests_1h": total_requests, "avg_response_time_ms": avg_response_time_ms, "error_rate_1h": error_rate},
        "connections": {"mysql": mysql, "redis": redis, "mongodb": mongodb},
        "checked_at": datetime.now(timezone.utc).replace(tzinfo=None).isoformat()})}
