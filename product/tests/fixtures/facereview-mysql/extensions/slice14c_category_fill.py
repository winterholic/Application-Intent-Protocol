# WBS 7.2.7: the original's Celery beat task fill_youtube_category_videos (Tuesday and Friday 03:00 Asia/Seoul) as an AIP durable job.
# Source: new-facereview@7948b7e7 common/scheduler/jobs/youtube_category_fill_job.py (YoutubeCategoryFillJob).
#
# The plan is the original's: count the active videos of each genre, take the genres under TARGET_MIN_COUNT (or the five lowest when all
# reach it), search YouTube for each genre's queries (1 to 3 by the shortage), read the details of the ids found, and save the videos
# the service does not have yet under the searched genre. The search budget is the original's: MAX_SEARCH_CALLS searches and the quota
# it counts (a search costs 100, a details call 1; a 403 spends the rest of the run).
#
# Integrations (server configuration, see Slice14-C in extensions/README.md): youtube_search (GET, query_params part/q/type/regionCode/
# relevanceLanguage/maxResults/order) and youtube_videos (GET, query_params including id); the API key is the server's.
#
# Differences from the original: as in slice14b_trending.py a call that cannot be made asks for the run to be tried again (the journal
# replays every answer that did arrive, so the quota count and the plan come out the same), and a video is saved as described in
# youtube_common.py. The original counted videos with a GROUP BY; the Video read here pages the genres and counts them, one call per 100
# active videos.
import importlib.util
import os

_here = os.path.dirname(os.path.abspath(__file__))


def _load(name):
    spec = importlib.util.spec_from_file_location(f"slice14c_{name}", os.path.join(_here, f"{name}.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_common = _load("youtube_common")

CATEGORY_SEARCH_QUERIES = {
    "eating": ["먹방 한국", "맛집 리뷰", "mukbang asmr"],
    "drama": ["한국드라마 명장면", "kdrama 클립", "드라마 하이라이트"],
    "cook": ["요리 레시피 한국", "집밥 만들기", "간단 요리 초보"],
    "horror": ["공포 영상 한국", "귀신 영상", "호러 콘텐츠"],
    "exercise": ["홈트 운동 루틴", "다이어트 운동", "헬스 운동 초보"],
    "vlog": ["일상 브이로그 한국", "데일리 vlog", "주간 vlog 한국"],
    "beauty": ["메이크업 튜토리얼 한국", "스킨케어 루틴", "뷰티 리뷰 한국"],
    "comedy": ["웃긴 영상 한국", "개그 콘텐츠", "코미디 클립"],
    "animal": ["강아지 고양이 귀여운", "동물 영상 한국", "펫 vlog"],
    "information": ["지식 교육 영상 한국", "유익한 정보 한국", "교양 콘텐츠"],
    "sports": ["스포츠 하이라이트 한국", "축구 야구 농구 한국", "스포츠 영상"],
    "travel": ["국내여행 브이로그", "해외여행 영상 한국", "여행 vlog"],
    "show": ["예능 하이라이트 한국", "버라이어티 클립", "웃긴 예능"],
    "music": ["신곡 뮤직비디오 한국", "kpop mv", "한국 음악 최신"],
    "game": ["게임 플레이 한국", "게임 리뷰 한국", "인디게임 한국"],
    "etc": ["한국 유튜브 인기 영상", "화제 영상 한국"],
}
TARGET_MIN_COUNT = 20
MAX_SEARCH_CALLS = 12
SEARCH_MAX_RESULTS = 20
DETAILS_BATCH = 50


class Run:
    """The state the original keeps on the job object across one execute()."""

    def __init__(self, ctx):
        self.ctx = ctx
        self.quota_used = 0
        self.calls = 0

    def key(self, kind):
        self.calls += 1
        return f"{kind}:{self.calls}"

    async def _ask(self, integration, query, kind):
        try:
            return await self.ctx.http.call(integration, query, key=self.key(kind))
        except Exception as error:
            if getattr(error, "code", None) in ("WIRE.DELIVERY_FAILED", "WIRE.CIRCUIT_OPEN"):
                raise self.ctx.retry_later("YouTube did not answer", after_ms=5000)
            raise

    async def search_video_ids(self, query):
        answer = await self._ask("youtube_search", {
            "part": "id", "q": query, "type": "video", "regionCode": "KR", "relevanceLanguage": "ko", "maxResults": SEARCH_MAX_RESULTS,
            "order": "viewCount",
        }, "search")
        status = answer["status"]
        if status == 403:
            # Quota exceeded: the rest of this run is over (the loops stop on quota_used).
            self.quota_used = MAX_SEARCH_CALLS * 100 + 1
            return []
        if not 200 <= status < 300:
            return []
        self.quota_used += 100
        try:
            items = (answer["body"] or {}).get("items", [])
            return [item["id"]["videoId"] for item in items if item.get("id", {}).get("kind") == "youtube#video"]
        except Exception:
            return []

    async def fetch_video_details(self, video_ids):
        results = []
        for start in range(0, len(video_ids), DETAILS_BATCH):
            batch = video_ids[start:start + DETAILS_BATCH]
            answer = await self._ask("youtube_videos", {"part": "snippet,contentDetails,statistics", "id": ",".join(batch)}, "details")
            if not 200 <= answer["status"] < 300:
                continue
            self.quota_used += 1
            try:
                for item in (answer["body"] or {}).get("items", []):
                    snippet = item.get("snippet", {})
                    results.append({
                        "youtube_id": item["id"],
                        "title": snippet.get("title", ""),
                        "channel_name": snippet.get("channelTitle", ""),
                        "duration": _common.parse_duration(item.get("contentDetails", {}).get("duration", "PT0S")),
                    })
            except Exception:
                continue
        return results


async def check_category_counts(ctx):
    """Active videos per genre: the original's SELECT category, COUNT(*) ... WHERE is_deleted = 0 GROUP BY category."""
    counts = {}
    after = None
    while True:
        query = {"read": "Video", "select": ["id", "category"], "sort": [{"field": "id", "dir": "asc"}], "limit": 100}
        if after is not None:
            query["after"] = {"id": after}
        rows = (await ctx.data.read(query))["rows"]
        for row in rows:
            counts[row["category"]] = counts.get(row["category"], 0) + 1
        if len(rows) < 100:
            return counts
        after = rows[-1]["id"]


def priority_targets(counts):
    targets = [(genre, counts.get(genre, 0)) for genre in CATEGORY_SEARCH_QUERIES if counts.get(genre, 0) < TARGET_MIN_COUNT]
    if not targets:
        # Every genre has enough: the five lowest are still topped up to bring in new videos.
        targets = sorted([(genre, counts.get(genre, 0)) for genre in CATEGORY_SEARCH_QUERIES], key=lambda item: item[1])[:5]
    targets.sort(key=lambda item: item[1])
    return targets


async def fillCategoryVideos(input, ctx):
    run = Run(ctx)
    counts = await check_category_counts(ctx)
    targets = priority_targets(counts)
    total_saved = 0
    search_calls = 0
    for genre, current_count in targets:
        if search_calls >= MAX_SEARCH_CALLS:
            break
        if run.quota_used > MAX_SEARCH_CALLS * 100:
            break
        queries = CATEGORY_SEARCH_QUERIES.get(genre, [])
        shortage = TARGET_MIN_COUNT - current_count
        if shortage > 10:
            number = min(3, len(queries))
        elif shortage > 5:
            number = min(2, len(queries))
        else:
            number = 1
        for query in queries[:number]:
            if search_calls >= MAX_SEARCH_CALLS:
                break
            video_ids = await run.search_video_ids(query)
            search_calls += 1
            if not video_ids:
                continue
            details = await run.fetch_video_details(video_ids)
            if details:
                existing = await _common.existing_youtube_ids(ctx, [video["youtube_id"] for video in details])
                for video in details:
                    if video["youtube_id"] in existing:
                        continue
                    if await _common.save_video(ctx, video, genre):
                        total_saved += 1
    return {"saved": total_saved, "searches": search_calls, "quota": run.quota_used}
