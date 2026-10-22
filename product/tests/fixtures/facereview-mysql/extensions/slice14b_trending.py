# WBS 7.2.7: the original's Celery beat task fetch_youtube_trending_videos (daily 06:00 Asia/Seoul) as an AIP durable job.
# Source: new-facereview@7948b7e7 common/scheduler/jobs/youtube_trending_job.py (YoutubeTrendingJob).
#
# The category mapping tables and both classifiers are the original's, copied and pinned by reference/slice14b_oracle.py against the
# original class. The YouTube Data API call is the integration youtube_videos: GET, query parameters part/chart/regionCode/maxResults/
# pageToken, and the API key added by the server (secret_query_env), so the key never reaches this worker.
#
# Differences from the original:
#  - The original swallowed a failed page (any requests error) and went on with the pages it had. Here an answer that is not a 2xx
#    (quota, a provider error) ends the paging the same way and the run goes on with what it has, while a call that could not be
#    made at all (the provider did not answer) asks for the run to be tried again: every page that did arrive is journaled and is
#    replayed, so a retry never asks YouTube twice for a page.
#  - A missing API key is not a log line at run time: the integration cannot be configured without it, and the service refuses to
#    start (INTEGRATION_CONFIG).
#  - The existing videos are looked up with Video.youtube_url.in, which sees only videos the Video row policy shows; a video an
#    administrator soft-deleted is still refused by the unique youtube_url when it is created again (WIRE.ALREADY_EXISTS), so it is
#    skipped as in the original.
#  - See youtube_common.py for how a video is saved.
import importlib.util
import os

_here = os.path.dirname(os.path.abspath(__file__))


def _load(name):
    spec = importlib.util.spec_from_file_location(f"slice14b_{name}", os.path.join(_here, f"{name}.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_common = _load("youtube_common")

# GenreEnum values of common/enum/youtube_genre.py used below.
YOUTUBE_CATEGORY_MAPPING = {
    "1": "etc", "2": "etc", "10": "music", "15": "animal", "17": "sports", "19": "travel", "20": "game", "22": "etc", "23": "comedy",
    "24": "show", "25": "information", "26": "beauty", "27": "information", "28": "information",
}

KEYWORD_CATEGORY_MAPPING = {
    "drama": ["드라마", "연속극", "drama", "series", "시리즈"],
    "eating": ["먹방", "음식", "맛집", "mukbang", "eating", "food", "asmr"],
    "travel": ["여행", "관광", "travel", "tour", "trip", "세계여행", "vlog"],
    "cook": ["요리", "레시피", "쿠킹", "cooking", "recipe", "만들기", "cook"],
    "show": ["예능", "버라이어티", "variety", "show", "토크쇼", "예능"],
    "information": ["정보", "교육", "강의", "education", "tutorial", "배우기", "learn", "뉴스", "news"],
    "horror": ["공포", "호러", "horror", "scary", "무서운", "귀신", "스릴러", "thriller"],
    "exercise": ["운동", "헬스", "피트니스", "exercise", "fitness", "workout", "gym", "요가", "yoga", "다이어트", "diet"],
    "vlog": ["일상", "브이로그", "vlog", "daily", "데일리", "daily life", "하루", "루틴", "routine"],
    "game": ["게임", "game", "gaming", "게이밍", "롤", "lol", "배그", "pubg"],
    "sports": ["스포츠", "운동", "sports", "축구", "야구", "soccer", "baseball", "농구"],
    "music": ["음악", "노래", "music", "song", "mv", "뮤직비디오", "kpop"],
    "animal": ["동물", "강아지", "고양이", "animal", "pet", "dog", "cat", "펫"],
    "beauty": ["뷰티", "화장", "메이크업", "beauty", "makeup", "패션", "fashion"],
    "comedy": ["코미디", "웃긴", "개그", "comedy", "funny", "humor", "유머"],
}

COOK_HINTS = {"요리", "레시피", "쿠킹", "cooking", "recipe", "집밥", "음식만들기", "cook"}
MAX_PAGES = 4
PAGE_SIZE = 50


def classify_category_by_keywords(title, tags, description):
    text = (title + " " + " ".join(tags) + " " + description).lower()
    scores = {}
    for category, keywords in KEYWORD_CATEGORY_MAPPING.items():
        scores[category] = sum(text.count(keyword.lower()) for keyword in keywords)
    best = max(scores, key=scores.get)
    return best if scores[best] > 0 else "etc"


def map_youtube_category(youtube_category_id, title, tags, description):
    mapped = YOUTUBE_CATEGORY_MAPPING.get(youtube_category_id, "etc")
    if mapped == "etc":
        mapped = classify_category_by_keywords(title, tags, description)
    elif mapped == "beauty":
        # Category 26 (Howto & Style) also holds cooking: a hint in the title, tags or the first 300 characters of the description.
        text = (title + " " + " ".join(tags) + " " + description[:300]).lower()
        if any(hint in text for hint in COOK_HINTS):
            mapped = "cook"
    return mapped


def _item_to_video(item):
    snippet = item.get("snippet", {})
    content_details = item.get("contentDetails", {})
    statistics = item.get("statistics", {})
    return {
        "youtube_id": item.get("id"),
        "title": snippet.get("title", ""),
        "channel_name": snippet.get("channelTitle", ""),
        "category_id": snippet.get("categoryId", ""),
        "tags": snippet.get("tags", []),
        "description": snippet.get("description", ""),
        "duration": _common.parse_duration(content_details.get("duration", "PT0S")),
        "view_count": int(statistics.get("viewCount", 0)),
    }


async def _fetch(ctx, region_code="KR", max_results=PAGE_SIZE, max_pages=MAX_PAGES):
    videos = []
    page_token = None
    for page in range(max_pages):
        query = {"part": "snippet,contentDetails,statistics", "chart": "mostPopular", "regionCode": region_code, "maxResults": max_results}
        if page_token:
            query["pageToken"] = page_token
        try:
            answer = await ctx.http.call("youtube_videos", query, key=f"trending:page:{page}")
        except Exception as error:
            if getattr(error, "code", None) in ("WIRE.DELIVERY_FAILED", "WIRE.CIRCUIT_OPEN"):
                raise ctx.retry_later("YouTube did not answer", after_ms=5000)
            raise
        if not 200 <= answer["status"] < 300:
            break
        try:
            body = answer["body"] or {}
            for item in body.get("items", []):
                videos.append(_item_to_video(item))
            page_token = body.get("nextPageToken")
        except Exception:
            break
        if not page_token:
            break
    return videos


async def fetchTrendingVideos(input, ctx):
    videos = await _fetch(ctx)
    if not videos:
        return {"fetched": 0, "fresh": 0, "saved": 0}
    # A video listed twice is saved once (the original tried twice and logged the second failure of the unique youtube_url).
    videos = list({video["youtube_id"]: video for video in reversed(videos)}.values())[::-1]
    youtube_ids = [video["youtube_id"] for video in videos]
    existing = await _common.existing_youtube_ids(ctx, youtube_ids)
    new_videos = [video for video in videos if video["youtube_id"] not in existing]
    saved = 0
    for video in new_videos:
        category = map_youtube_category(video["category_id"], video["title"], video["tags"], video["description"])
        if await _common.save_video(ctx, video, category):
            saved += 1
    return {"fetched": len(videos), "fresh": len(new_videos), "saved": saved}
