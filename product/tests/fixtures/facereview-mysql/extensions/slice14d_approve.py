# WBS 7.2.9: the original's AdminService.approve_video_request (new-facereview@7948b7e7 app/services/admin_service.py) as a durable job.
#
# The original, in one union transaction (SQL plus the saga over MongoDB): the request must exist and be PENDING, no Video may have the
# request's youtube_url, then the Video is saved with the title, channel, length and genre the administrator chose, the two initial
# MongoDB documents are upserted, and the request becomes ACCEPTED. The same steps run here with the platform's building blocks: the
# Video comes from the exposed create, the documents from upserts that can be sent again, and the request from the transition accept,
# which only works while a Video with the request's youtube_url exists.
#
# Differences from the original (see extensions/README.md, Slice14-D): the administrator reads the new video id from the job result
# instead of the response of the call; a job has no saga, so a failure after the Video was made is repaired by the retry, which replays
# the journaled steps and finishes the rest; and the failures carry a name in the job's error text.
import importlib.util
import os

_here = os.path.dirname(os.path.abspath(__file__))


def _load(name):
    spec = importlib.util.spec_from_file_location(f"slice14d_{name}", os.path.join(_here, f"{name}.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_common = _load("youtube_common")


async def approveVideoRequest(input, ctx):
    page = await ctx.data.read({"read": "VideoRequest", "select": ["id", "status", "youtube_url"], "limit": 1,
                                "filter": [{"field": "id", "op": "eq", "value": input["request"]}]})
    if not page["rows"]:
        raise Exception("VIDEO_NOT_FOUND: 영상 요청을 찾을 수 없습니다.")
    request = page["rows"][0]
    if request["status"] != "PENDING":
        raise Exception("INVALID_INPUT_VALUE: 이미 처리된 요청입니다.")
    video_id = await _common.save_video(ctx, {
        "youtube_id": request["youtube_url"], "title": input["youtube_title"],
        "channel_name": input["channel_name"], "duration": input["duration"],
    }, input["category"])
    if video_id is None:
        raise Exception("VIDEO_DUPLICATE_URL")
    await ctx.data.apply("VideoRequest", "accept", [request["id"]], key=f"accept:{request['id']}")
    return {"video_id": video_id}
