# WBS 7.2.9: POST home/video/recommend of FaceReview as an AIP write extension (VideoRequest.recommend).
# Source of truth: new-facereview@7948b7e7 app/services/home_service.py create_user_video_recommend. The original rejects the whole list
# when any URL was requested before (by anyone, in any status) and otherwise inserts one PENDING row per URL with the full YouTube URL
# it builds itself. Here the database builds that URL (default of youtube_full_url) and the unique youtube_url makes the duplicate check
# part of the insert, so the rule also holds for two requests at the same moment.
#
# Differences from the original (see extensions/README.md, Slice14-G):
#  - The caller sends its own id (`user`), as for every exposed create; the server refuses an id that is not the verified caller.
#  - At most 20 URLs (the ctx write limit of one call); the original has no limit. An id longer than 50 characters is refused by the
#    create's own field rule (the original fails on the column length).
#  - The same id twice in one list: the original inserts both rows (its check only looks at the table); here the second one is refused as a
#    duplicate and nothing is created.
#  - A duplicate answers WIRE.ALREADY_EXISTS (HTTP 200 with ok false), not the original's 409 V004.
import json

MAX_URLS = 20


async def recommend(input, ctx):
    try:
        urls = json.loads(input["youtube_url_list"])
    except ValueError:
        raise Exception("youtube_url_list must be a JSON array of text")
    if not isinstance(urls, list) or not 1 <= len(urls) <= MAX_URLS or not all(isinstance(url, str) and url for url in urls):
        raise Exception(f"youtube_url_list must hold 1 to {MAX_URLS} non-empty texts")
    for url in urls:
        # A refused create (duplicate, wrong owner, too long) ends the call with the same code and the server cancels every earlier row.
        await ctx.data.create("VideoRequest", {"user": input["user"], "youtube_url": url})
    return {"requested": len(urls)}
