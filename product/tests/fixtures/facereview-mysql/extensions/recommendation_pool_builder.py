# Slice13-D: pure Tier1 ranked-pool builder for FaceReview GET /home (WBS 7.2.3).
#
# Provenance: app/services/home_service.py of the original new-facereview server
#   repository, pinned revision 7948b7e721837fdb73c427cb54e12cad4ec2d5af —
#   HomeService._load_scored_videos, HomeService._slim_pool_entry and
#   HomeService._build_and_cache_ranked_pool, with all I/O removed.
#
# The base score itself is NOT reimplemented here: it is delegated to
# recommendation_math.compute_base_score next to this file (which the
# slice13d_home_oracle_test.py suite pins against the original
# common/utils/recommendation_alg.py).
#
# Deliberate differences from the original, mirroring the read-extension policy:
#  - No DB/Redis query, no network, no file access, no global or cache state.
#    Every input the original reads inside those methods is an explicit argument:
#    videos (Video rows), dist_docs (VideoDistribution raw docs by video_id),
#    like_map (VideoLike GROUP BY counts by video_id), all_genres (GenreEnum values
#    in enum order) and raw_by_cat (per-category Video rows for random padding).
#  - No Redis SET/HSET/EXPIRE/DELETE and no _MEM_CACHE write. The caller owns the
#    cache; this function only returns what the original would have stored:
#    (pool, by_category).
#  - Randomness comes from the injected rng (default: the `random` module, the same
#    convention as recommendation_math.rank_personalized). The original shuffles the
#    padding source with the process-global random; pass random.Random(seed) to
#    reproduce a seeded original draw exactly.
#  - A video counts as active when its is_deleted flag is falsy (0/False/None/absent
#    all mean active). SQL NULL never matches `is_deleted = 0`, so a NULL flag is a
#    documented edge where this pure form keeps the row; fixtures use 0/1.
import importlib.util
import os
import random

_here = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location(
    'pool_builder_recommendation_math', os.path.join(_here, 'recommendation_math.py'))
_math = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_math)

RECO_POOL_SIZE = 1000
RECO_CATEGORY_SIZE = 20


def _slim(entry):
    return {
        'video_id': entry['video_id'],
        'youtube_url': entry['youtube_url'],
        'title': entry['title'],
        'category': entry['category'],
        'dominant_emotion': entry['dominant_emotion'],
        'dominant_emotion_per': entry['dominant_emotion_per'],
        'emotion_distribution': entry['emotion_distribution'],
        'base_score': entry['base_score'],
    }


def _category_of(row):
    cat = row.get('category') if isinstance(row, dict) else getattr(row, 'category', None)
    return cat.value if hasattr(cat, 'value') else cat


def _row_field(row, name):
    return row.get(name) if isinstance(row, dict) else getattr(row, name, None)


def _created_at_of(row):
    created = _row_field(row, 'created_at')
    if created is not None and not isinstance(created, str) and hasattr(created, 'isoformat'):
        return created.isoformat()
    return created


def build_ranked_pool(videos, dist_docs, like_map, all_genres, rng=None, raw_by_cat=None,
                      pool_size=RECO_POOL_SIZE, category_size=RECO_CATEGORY_SIZE):
    """Build (pool, by_category) exactly as the original Tier1 cache content.

    videos: iterable of mappings with video_id, youtube_url, title, category
        (plain value or Enum with .value), view_count, created_at (ISO string,
        datetime or None) and is_deleted (falsy means active).
    dist_docs: mapping video_id -> raw distribution doc with dominant_emotion,
        emotion_averages, total_frames, emotion_counts and average_completion_rate.
    like_map: mapping video_id -> like count; absent means 0.
    all_genres: genre values in GenreEnum order; drives the deficient-category scan.
    rng: shuffles the random-padding source; defaults to the `random` module.
    raw_by_cat: mapping category -> rows (video_id/youtube_url/title) used to pad
        deficient categories; when None it is derived from `videos` in input order.
    Returns (pool, by_category): pool is the slim top-pool_size list by base_score
    descending (stable for ties); by_category maps each seen category to its slim
    top-category_size list, padded with unscored rows exactly like the original.
    """
    rng = rng if rng is not None else random
    dist_docs = dist_docs or {}
    like_map = like_map or {}

    scored = []
    for v in videos or []:
        if _row_field(v, 'is_deleted'):
            continue
        doc = dist_docs.get(_row_field(v, 'video_id'))
        if not doc or not doc.get('dominant_emotion'):
            continue
        emotion_dist = doc.get('emotion_averages') or {}
        if not emotion_dist or sum(emotion_dist.values()) == 0:
            continue
        cat = _category_of(v)
        dominant = doc.get('dominant_emotion')
        entry = {
            'video_id': _row_field(v, 'video_id'),
            'youtube_url': _row_field(v, 'youtube_url'),
            'title': _row_field(v, 'title'),
            'category': cat,
            'dominant_emotion': dominant,
            'dominant_emotion_per': round(emotion_dist.get(dominant, 0.0) * 100.0, 2),
            'emotion_distribution': emotion_dist,
            'view_count': _row_field(v, 'view_count'),
            'like_count': like_map.get(_row_field(v, 'video_id'), 0),
            'created_at': _created_at_of(v),
            'average_completion_rate': doc.get('average_completion_rate', 0.0),
            'sample_frames': doc.get('total_frames') or sum((doc.get('emotion_counts') or {}).values()),
        }
        entry['base_score'] = _math.compute_base_score(entry)
        scored.append(entry)

    scored.sort(key=lambda x: x['base_score'], reverse=True)
    pool = [_slim(v) for v in scored[:pool_size]]

    by_category = {}
    for v in scored:
        bucket = by_category.setdefault(v['category'], [])
        if len(bucket) < category_size:
            bucket.append(_slim(v))

    deficient = [g for g in (all_genres or []) if len(by_category.get(g, [])) < category_size]
    if deficient:
        if raw_by_cat is None:
            raw_by_cat = {}
            for v in videos or []:
                if _row_field(v, 'is_deleted'):
                    continue
                cat = _category_of(v)
                if cat in deficient:
                    raw_by_cat.setdefault(cat, []).append(v)
        for cat in deficient:
            bucket = by_category.setdefault(cat, [])
            existing = {e['video_id'] for e in bucket}
            pad_source = [w for w in (raw_by_cat.get(cat) or []) if _row_field(w, 'video_id') not in existing]
            rng.shuffle(pad_source)
            for w in pad_source:
                if len(bucket) >= category_size:
                    break
                bucket.append({
                    'video_id': _row_field(w, 'video_id'),
                    'youtube_url': _row_field(w, 'youtube_url'),
                    'title': _row_field(w, 'title'),
                    'category': cat,
                    'dominant_emotion': None,
                    'dominant_emotion_per': 0.0,
                    'emotion_distribution': {},
                    'base_score': 0.0,
                })

    return pool, by_category
