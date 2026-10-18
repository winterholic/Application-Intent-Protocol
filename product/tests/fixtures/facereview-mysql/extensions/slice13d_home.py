# Slice13-D: AIP read extensions for FaceReview GET /home/category and GET /home/personalized (WBS 7.2.3).
#
# Source of the behaviour: app/services/home_service.py lines 251-439 of the original new-facereview revision
# 7948b7e721837fdb73c427cb54e12cad4ec2d5af (get_videos_by_category_emotions, get_personalized_videos and the
# helpers they call). The recommendation arithmetic is in recommendation_math.py next to this file.
#
# Deliberate differences from the original, none of which may be "fixed" by faking a result:
#  - No in-process _MEM_CACHE fallback. An absent Redis/Mongo adapter surfaces as WIRE.ADAPTER_UNAVAILABLE
#    from the server; it is never replaced by sample data or an empty list.
#  - Output is one JSON Text value (see _envelope) because extension outputs cannot be lists or records yet.
#  - The server only grants read capabilities. A cache miss would need the original's synchronous
#    _build_and_cache_ranked_pool (Redis SET/HSET/EXPIRE plus a per-video like count). That is reported as
#    SLICE13D.POOL_CACHE_MISS / SLICE13D.CATEGORY_CACHE_MISS instead of building a different answer.
#  - youtube_watching_data is private: the extension passes the caller's own user id and the server must
#    re-check it against the verified principal. A forged or foreign id is the server's to reject.
import importlib.util
import json
import os
import random
import sys

_here = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location('slice13d_recommendation_math', os.path.join(_here, 'recommendation_math.py'))
_math = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_math)

RECO_POOL_CACHE_KEY = 'facereview:reco:pool'
RECO_CATEGORY_CACHE_KEY = 'facereview:reco:category'
RECO_PERSONAL_TOP_N = 150
RECO_PERSONAL_RANDOM_N = 50
PERSONALIZED_LIMIT = 20          # the original route fixes limit=20
RECENT_SUMMARY_LIMIT = 20
PAGE_ROWS = 100                  # budget rows of the exposed reads
MAX_PAGES = 1000                 # offset budget 100000 / PAGE_ROWS

# Tests replace this with a seeded random.Random; production uses the module-level generator like the original.
_rng = random


class Slice13dError(RuntimeError):
    def __init__(self, code):
        super().__init__(code)
        self.code = code


def _fail(code):
    # The server forwards only AipError codes ("확장 실패: <code>"); any other exception collapses to WIRE.EXTENSION_ERROR.
    # The worker runs as __main__, so its AipError is reachable there; outside a worker (unit tests) the local class is used.
    aip_error = getattr(sys.modules.get('__main__'), 'AipError', None)
    return aip_error(code) if aip_error else Slice13dError(code)


def _envelope(value):
    # STOPGAP: a read extension output can only be scalar (Json/list are rejected with WIRE.OUTPUT_INVALID), so the
    # response travels as one JSON Text value. A caller must json.loads it; this is not the original response shape.
    return json.dumps(value, ensure_ascii=False, separators=(',', ':'))


async def _read_all(ctx, resource, select, sort=None):
    # Pages through a policy-filtered read. Stopping at MAX_PAGES silently would drop viewed videos or bookmarks,
    # so reaching the cap while pages are still full is an explicit failure.
    rows = []
    for page in range(MAX_PAGES):
        query = {'read': resource, 'select': select, 'limit': PAGE_ROWS}
        if page:
            query['offset'] = page * PAGE_ROWS
        if sort:
            query['sort'] = sort
        chunk = (await ctx.data.read(query)).get('rows', [])
        rows.extend(chunk)
        if len(chunk) < PAGE_ROWS:
            return rows
    raise _fail('SLICE13D.READ_TRUNCATED')


async def _bookmarked_ids(ctx, video_ids):
    # Original: one IN query restricted to the ids in the response. Here the caller's bookmark rows are read under the
    # row policy (user = actor) and intersected locally; a guest gets no rows.
    wanted = set(video_ids)
    if not wanted:
        return set()
    rows = await _read_all(ctx, 'VideoBookmark', ['video'], [{'field': 'created_at', 'dir': 'asc'}])
    return {r['video'] for r in rows} & wanted


def _video_dto(entry, bookmarked):
    return {
        'video_id': entry['video_id'],
        'youtube_url': entry['youtube_url'],
        'title': entry['title'],
        'dominant_emotion': entry.get('dominant_emotion'),
        'dominant_emotion_per': entry.get('dominant_emotion_per', 0.0),
        'is_bookmarked': entry['video_id'] in bookmarked,
    }


def _parse_json(raw, code):
    try:
        return json.loads(raw)
    except (TypeError, ValueError):
        raise _fail(code)


async def category(input, ctx):
    raw = await ctx.redis.hgetall(RECO_CATEGORY_CACHE_KEY)
    if not raw:
        raise _fail('SLICE13D.CATEGORY_CACHE_MISS')
    groups = []
    for cat_name, videos_json in raw.items():
        entries = _parse_json(videos_json, 'SLICE13D.CATEGORY_CACHE_CORRUPT')
        groups.append((cat_name, entries))
    bookmarked = await _bookmarked_ids(ctx, [e['video_id'] for _, entries in groups for e in entries])
    return {'video_data': _envelope([
        {'category_name': cat_name, 'videos': [_video_dto(e, bookmarked) for e in entries]}
        for cat_name, entries in groups
    ])}


def _recent_watching(summaries, pool_category_map):
    recent = []
    for wd in summaries:
        dominant_emotion = wd.get('dominant_emotion')
        if dominant_emotion is None:
            continue
        recent.append({
            'category': pool_category_map.get(wd.get('video_id')),
            'dominant_emotion': dominant_emotion,
            'emotion_percentages': wd.get('emotion_percentages', {}),
        })
    return recent


def rank_with_fallback(pool, recent_watching, favorite_genres, viewed_ids, limit, rng=None):
    rng = rng if rng is not None else _rng
    ranked = _math.rank_personalized(
        pool=pool, recent_watching=recent_watching, favorite_genres=favorite_genres, viewed_ids=viewed_ids,
        limit=limit, top_n=RECO_PERSONAL_TOP_N, random_n=RECO_PERSONAL_RANDOM_N, rng=rng,
    )
    if ranked:
        return ranked
    unwatched = [v for v in pool if v.get('video_id') not in viewed_ids]
    genre_videos = [v for v in unwatched if v.get('category') in favorite_genres]
    candidates = genre_videos if genre_videos else unwatched
    if not candidates:
        return []
    return rng.sample(candidates, min(limit, len(candidates)))


async def personalized(input, ctx):
    me = await ctx.data.read({'read': 'User', 'select': ['id'], 'limit': 1})
    if not me.get('rows'):
        raise _fail('SLICE13D.USER_NOT_FOUND')
    user_id = me['rows'][0]['id']

    # Original order is the unordered dynamic relationship, i.e. storage order; created_at, id is the closest stable choice.
    genre_rows = await _read_all(ctx, 'UserFavoriteGenre', ['genre'], [{'field': 'created_at', 'dir': 'asc'}])
    favorite_genres = [r['genre'] for r in genre_rows]
    viewed_ids = {r['video'] for r in await _read_all(ctx, 'VideoViewLog', ['video'], [{'field': 'created_at', 'dir': 'asc'}])}

    # Only the exclusion projection the server allows today; see the report for the field-allowlist gap.
    found = await ctx.call('mongo', 'youtube_watching_data', {
        'command': 'find', 'filter': {'user_id': user_id}, 'projection': {'emotion_score_timeline': 0},
        'sort': {'created_at': -1}, 'limit': RECENT_SUMMARY_LIMIT,
    })
    summaries = found.get('docs', []) if isinstance(found, dict) else found

    raw_pool = await ctx.redis.get(RECO_POOL_CACHE_KEY)
    if not raw_pool:
        raise _fail('SLICE13D.POOL_CACHE_MISS')
    pool = _parse_json(raw_pool, 'SLICE13D.POOL_CACHE_CORRUPT')
    pool_category_map = {v['video_id']: v['category'] for v in pool}

    recommended = rank_with_fallback(pool, _recent_watching(summaries, pool_category_map), favorite_genres, viewed_ids, PERSONALIZED_LIMIT)
    bookmarked = await _bookmarked_ids(ctx, [v['video_id'] for v in recommended])
    return {'videos': _envelope([_video_dto(v, bookmarked) for v in recommended])}
