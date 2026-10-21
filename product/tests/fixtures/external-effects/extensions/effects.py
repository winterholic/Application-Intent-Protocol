# WBS 7.2.5 test extension: one Redis dedupe key, one Mongo upsert, one counter and the relational `view`, in the order the original's
# watch handler does them. `scenario` picks how the call ends; every external write that can be undone asks for compensate=True.
import json

DATE = {"$date": "2026-10-22T00:00:00Z"}


async def track(input, ctx):
    log_id, video_id, scenario = input["log_id"], input["video_id"], input["scenario"]
    fresh = await ctx.redis.set_nx_ex(f"effects:dedupe:{log_id}", "1", 60, compensate=True)
    if not fresh:
        return {"counted": False, "frames": 0}
    doc = await ctx.mongo.find_one_and_update(
        "youtube_watching_data", {"video_view_log_id": log_id},
        {"$set": {"updated_at": DATE}, "$setOnInsert": {"video_id": video_id, "created_at": DATE}, "$inc": {"frame_count": 1}},
        upsert=True, compensate=True)
    await ctx.redis.incr_expire("effects:counter:requests", 3600)
    await ctx.data.apply("Video", "view", [video_id])
    if scenario == "fail_after":
        raise Exception("boom after the effects")
    return {"counted": True, "frames": doc["frame_count"]}
