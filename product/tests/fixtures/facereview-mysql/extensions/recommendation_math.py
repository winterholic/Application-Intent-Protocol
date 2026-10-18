# Port of FaceReview's emotion-based recommendation math (WBS 7.2.3 Slice13-D).
#
# Provenance: common/utils/recommendation_alg.py of the original FaceReview server
#   repository  new-facereview (author winterholic), pinned revision 7948b7e721837fdb73c427cb54e12cad4ec2d5af
# The constants and the arithmetic below follow that file line for line. The only
# deliberate differences are (1) `now` and `rng` are injectable so a test can freeze
# time and the random draw, and (2) the original's inline diversity filter is split
# out as `diversify`. Equivalence is established by tests/slice13d_oracle_test.py,
# which executes the original file itself, not a copy of it.
import math
import random
from datetime import datetime, timezone

EMOTIONS = ['neutral', 'happy', 'surprise', 'sad', 'angry']
NON_NEUTRAL = ['happy', 'surprise', 'sad', 'angry']

BASE_WATCHER_WEIGHTS = {'engagement': 0.30, 'peak': 0.22, 'completion': 0.23}
BASE_EXTERNAL_WEIGHTS = {'popularity': 0.15, 'freshness': 0.10}

CONFIDENCE_K = 200

PERSONAL_AFFINITY_WEIGHT = 25.0
PERSONAL_DOMINANT_WEIGHT = 15.0
FAVORITE_GENRE_BONUS = [12.0, 8.0, 5.0]
RECENT_CATEGORY_BONUS_MAX = 8.0

RECENCY_DECAY = 0.88


def _utcnow_naive():
    return datetime.now(timezone.utc).replace(tzinfo=None)


def emotion_cosine(vec_a, vec_b):
    a = [vec_a.get(e, 0.0) for e in EMOTIONS]
    b = [vec_b.get(e, 0.0) for e in EMOTIONS]
    dot = sum(x * y for x, y in zip(a, b))
    mag_a = math.sqrt(sum(x * x for x in a))
    mag_b = math.sqrt(sum(y * y for y in b))
    return dot / (mag_a * mag_b) if (mag_a and mag_b) else 0.0


def _popularity_score(view_count, like_count):
    views = max(int(view_count or 0), 0)
    likes = max(int(like_count or 0), 0)
    view_part = min(math.log10(views + 1) / 6.0, 1.0)
    like_part = min(math.log10(likes + 1) / 4.0, 1.0)
    return view_part * 0.6 + like_part * 0.4


def _freshness_score(created_at, now=None):
    if not created_at:
        return 0.3
    if isinstance(created_at, str):
        try:
            created_at = datetime.fromisoformat(created_at.replace('Z', '+00:00'))
        except ValueError:
            return 0.3
    if created_at.tzinfo is not None:
        created_at = created_at.replace(tzinfo=None)
    days = ((now if now is not None else _utcnow_naive()) - created_at).days
    if days <= 2:
        return 1.0
    if days <= 7:
        return 0.8
    if days <= 14:
        return 0.6
    if days <= 30:
        return 0.4
    return max(0.3 * math.exp(-0.02 * days), 0.1)


def compute_base_score(stats, now=None):
    emotion_dist = stats.get('emotion_distribution') or {}
    neutral = emotion_dist.get('neutral', 0.0)
    engagement = max(0.0, min(1.0 - neutral, 1.0))
    peak = max((emotion_dist.get(e, 0.0) for e in NON_NEUTRAL), default=0.0)
    completion = max(0.0, min(float(stats.get('average_completion_rate') or 0.0), 1.0))

    frames = int(stats.get('sample_frames') or 0)
    confidence = frames / (frames + CONFIDENCE_K) if frames > 0 else 0.0

    watcher = (
        BASE_WATCHER_WEIGHTS['engagement'] * engagement +
        BASE_WATCHER_WEIGHTS['peak'] * peak +
        BASE_WATCHER_WEIGHTS['completion'] * completion
    )
    external = (
        BASE_EXTERNAL_WEIGHTS['popularity'] * _popularity_score(stats.get('view_count'), stats.get('like_count')) +
        BASE_EXTERNAL_WEIGHTS['freshness'] * _freshness_score(stats.get('created_at'), now)
    )
    return round(100.0 * (confidence * watcher + external), 4)


def build_user_emotion_profile(recent_watching):
    if not recent_watching:
        return {e: (0.0 if e == 'neutral' else 0.25) for e in EMOTIONS}
    acc = {e: 0.0 for e in EMOTIONS}
    for idx, d in enumerate(recent_watching):
        weight = RECENCY_DECAY ** idx
        for e, pct in (d.get('emotion_percentages') or {}).items():
            if e in acc:
                acc[e] += pct * weight
    total = sum(acc.values())
    if total <= 0:
        return {e: (0.0 if e == 'neutral' else 0.25) for e in EMOTIONS}
    return {e: v / total for e, v in acc.items()}


def _personal_bonus(video, user_profile, favorite_genres, recent_category_weight):
    bonus = 0.0

    video_emo = video.get('emotion_distribution') or {}
    if video_emo:
        bonus += emotion_cosine(user_profile, video_emo) * PERSONAL_AFFINITY_WEIGHT
        dominant = video.get('dominant_emotion')
        if dominant:
            bonus += user_profile.get(dominant, 0.0) * PERSONAL_DOMINANT_WEIGHT

    cat = video.get('category')
    if cat in favorite_genres:
        rank = favorite_genres.index(cat)
        bonus += FAVORITE_GENRE_BONUS[min(rank, len(FAVORITE_GENRE_BONUS) - 1)]

    if recent_category_weight:
        bonus += min(recent_category_weight.get(cat, 0.0), 1.0) * RECENT_CATEGORY_BONUS_MAX

    return bonus


def diversify(scored, limit):
    # scored: [(final_score, video)] already sorted by score descending.
    result, cats, emos, selected = [], [], [], set()
    for _, v in scored:
        cat = v.get('category')
        emo = v.get('dominant_emotion', 'neutral')
        if (len(cats) >= 2 and cats[-2:].count(cat) >= 2) or (len(emos) >= 3 and emos[-3:].count(emo) >= 3):
            continue
        result.append(v)
        selected.add(v.get('video_id'))
        cats.append(cat)
        emos.append(emo)
        if len(result) >= limit:
            return result

    for _, v in scored:
        if v.get('video_id') in selected:
            continue
        result.append(v)
        selected.add(v.get('video_id'))
        if len(result) >= limit:
            break
    return result


def rank_personalized(pool, recent_watching, favorite_genres, viewed_ids, limit=20,
                      top_n=150, random_n=50, rng=None):
    rng = rng if rng is not None else random
    viewed_ids = viewed_ids or set()
    fresh_pool = [v for v in pool if v.get('video_id') not in viewed_ids]
    if not fresh_pool:
        return []

    head = fresh_pool[:top_n]
    tail = fresh_pool[top_n:]
    explore = rng.sample(tail, min(random_n, len(tail))) if tail else []
    candidates = head + explore

    user_profile = build_user_emotion_profile(recent_watching)

    recent_category_weight = {}
    for idx, d in enumerate(recent_watching[:10]):
        cat = d.get('category')
        if cat:
            recent_category_weight[cat] = recent_category_weight.get(cat, 0.0) + RECENCY_DECAY ** idx
    if recent_category_weight:
        top_w = max(recent_category_weight.values())
        recent_category_weight = {k: v / top_w for k, v in recent_category_weight.items()}

    scored = []
    for v in candidates:
        final = v.get('base_score', 0.0) + _personal_bonus(v, user_profile, favorite_genres, recent_category_weight)
        scored.append((final, v))
    scored.sort(key=lambda x: x[0], reverse=True)

    return diversify(scored, limit)
