"""Port of the FaceReview emotion summary fold and emotion DNA (WBS 7.2.8 increment).

Source of truth: new-facereview@7948b7e721837fdb73c427cb54e12cad4ec2d5af
(`common/utils/emotion_summary.py`, `app/services/mypage_service.py`,
`app/models/user_emotion_summary.py`, `app/models/user_emotion_dna.py`).
Behaviour is copied on purpose, including the quirks `slice728_summary_oracle_test.py`
pins against the original functions. Nothing here corrects the original watermark rules.

The module is I/O free. Mongo reads, the MySQL cache row and the compare-and-swap are
injected, so the same logic can sit behind an AIP extension once the platform can carry
the Mongo read and the cache write in one actor-bound action (see the report: today a
`write ... effect db` extension has no `ctx.data.read` and `ctx.data.apply` has no values).
"""
from datetime import datetime, timedelta
from typing import Callable, Dict, List, Mapping, Optional, Tuple

EMOTIONS = ("neutral", "happy", "surprise", "sad", "angry")
SUMMARY_CAS_ATTEMPTS = 3
DNA_CACHE_TTL = timedelta(days=7)
DNA_CACHE_MAX_NEW_SESSIONS = 5  # a cache stays valid while fewer than 5 sessions were added

# Mongo projections the original sends for the first fold and for the incremental read.
FIRST_FOLD_PROJECTION = {
    "most_emotion_timeline": 1,
    "emotion_percentages": 1,
    "duration": 1,
    "completion_rate": 1,
    "frame_count": 1,
    "_id": 0,
}
SINCE_PROJECTION = {"emotion_score_timeline": 0, "most_emotion_timeline": 0, "client_info": 0}
SINCE_SORT = (("finalized_at", 1), ("video_view_log_id", 1))


def build_finalized_session_query(user_id, checkpoint_at=None, checkpoint_session_id=None) -> Dict:
    query = {"user_id": user_id}
    if checkpoint_at is None:
        return query
    query["$or"] = [
        {"finalized_at": {"$gt": checkpoint_at}},
        {"finalized_at": checkpoint_at, "video_view_log_id": {"$gt": checkpoint_session_id or ""}},
    ]
    return query


def _estimate_watch_seconds(doc: Dict) -> float:
    duration = doc.get("duration")
    completion_rate = doc.get("completion_rate")
    try:
        duration = float(duration)
        completion_rate = float(completion_rate)
    except (TypeError, ValueError):
        duration = 0.0
        completion_rate = 0.0
    if duration > 0 and completion_rate > 0:
        return max(0.0, duration * min(completion_rate, 1.0))

    timeline_len = doc.get("timeline_len")
    try:
        timeline_len = int(timeline_len)
    except (TypeError, ValueError):
        timeline_len = 0
    if timeline_len > 0:
        return timeline_len / 2.0

    frame_count = doc.get("frame_count")
    try:
        frame_count = int(frame_count)
    except (TypeError, ValueError):
        frame_count = 0
    if frame_count > 0:
        return frame_count / 2.0

    timeline = doc.get("emotion_score_timeline") or {}
    if timeline:
        parsed_keys = []
        for key in timeline.keys():
            try:
                parsed_keys.append(float(key))
            except (TypeError, ValueError):
                continue
        if parsed_keys:
            return max(parsed_keys) / 100.0
    return 0.0


def fold_summary(docs: List[Dict]) -> Dict:
    """Sum per-session emotion seconds as floats, then truncate once to int seconds."""
    emotion_seconds = {emotion: 0.0 for emotion in EMOTIONS}

    for doc in docs:
        precomputed = doc.get("emotion_seconds")
        if isinstance(precomputed, dict):
            for emotion in emotion_seconds:
                try:
                    emotion_seconds[emotion] += float(precomputed.get(emotion, 0))
                except (TypeError, ValueError):
                    continue
            continue

        timeline = doc.get("most_emotion_timeline") or {}
        per_second_emotion = {}
        for time_key, emotion in timeline.items():
            if emotion not in emotion_seconds:
                continue
            try:
                second = int(float(time_key) // 100)
            except (TypeError, ValueError):
                continue
            per_second_emotion[second] = emotion  # the last key of the same second wins

        if per_second_emotion:
            for emotion in per_second_emotion.values():
                emotion_seconds[emotion] += 1.0
            continue

        watch_secs = _estimate_watch_seconds(doc)
        percentages = doc.get("emotion_percentages") or {}
        for emotion in emotion_seconds:
            emotion_seconds[emotion] += float(percentages.get(emotion, 0.0)) * watch_secs

    total_time = sum(emotion_seconds.values())
    return {
        "emotion_percentages": {
            emotion: round((seconds / total_time) * 100, 1) if total_time > 0 else 0.0
            for emotion, seconds in emotion_seconds.items()
        },
        "emotion_seconds": {emotion: int(seconds) for emotion, seconds in emotion_seconds.items()},
    }


def summary_response(emotion_seconds: Mapping[str, int]) -> Dict:
    return fold_summary([{"emotion_seconds": dict(emotion_seconds)}])


class SummaryStore:
    """What the summary refresh needs from storage. All reads are for one actor."""

    def ensure_aggregate(self) -> Dict:
        raise NotImplementedError

    def reload_aggregate(self) -> Dict:
        raise NotImplementedError

    def first_fold_docs(self) -> List[Dict]:
        raise NotImplementedError

    def docs_since(self, checkpoint_at: datetime, checkpoint_session_id: Optional[str]) -> List[Dict]:
        raise NotImplementedError

    def apply_delta(self, expected_version: int, delta: Mapping[str, int],
                    checkpoint_at: datetime, checkpoint_session_id: str) -> bool:
        raise NotImplementedError

    def rollback(self) -> None:
        raise NotImplementedError


def _seconds_of(aggregate: Mapping) -> Dict[str, int]:
    return {emotion: int(aggregate[f"{emotion}_seconds"]) for emotion in EMOTIONS}


def refresh_summary(store: SummaryStore, now: Callable[[], datetime]) -> Dict:
    """Original `get_emotion_summary` after the user check: fold new sessions, bump with CAS."""
    aggregate = store.ensure_aggregate()
    for _ in range(SUMMARY_CAS_ATTEMPTS):
        base_seconds = _seconds_of(aggregate)

        if aggregate["last_finalized_at"] is None:
            # No watermark yet: fold every session once and stamp "now" with an empty session id.
            checkpoint_at = now()
            checkpoint_session_id = ""
            docs = store.first_fold_docs()
        else:
            docs = store.docs_since(aggregate["last_finalized_at"], aggregate["last_session_id"])
            if not docs:
                return summary_response(base_seconds)
            last_doc = docs[-1]
            checkpoint_at = last_doc["finalized_at"]
            checkpoint_session_id = last_doc["video_view_log_id"]

        delta = fold_summary(docs)["emotion_seconds"]
        if store.apply_delta(aggregate["lock_version"], delta, checkpoint_at, checkpoint_session_id):
            merged = {emotion: base_seconds[emotion] + delta[emotion] for emotion in base_seconds}
            return summary_response(merged)

        store.rollback()
        aggregate = store.reload_aggregate()

    # Repeated conflicts: report the stored totals; the next read retries the new sessions.
    return summary_response(_seconds_of(aggregate))


# ---------------------------------------------------------------- emotion DNA

DNA_TYPES = {
    "JOYFUL_EXPLORER": ("유쾌한 탐험가", "새로운 장르를 두려워하지 않고, 어디서든 웃음을 찾아내는 당신!"),
    "EMOTIONAL_DIVER": ("감성 다이버", "기쁨과 슬픔을 깊게 느끼며 콘텐츠에 완전히 몰입하는 당신!"),
    "THRILL_SEEKER": ("스릴 추구자", "예상치 못한 순간에 짜릿함을 느끼는 당신!"),
    "CALM_OBSERVER": ("차분한 관찰자", "감정에 흔들리지 않고 냉철하게 콘텐츠를 분석하는 당신!"),
    "MOOD_SURFER": ("무드 서퍼", "감정의 파도를 자유롭게 타며 시청하는 당신!"),
    "COMFORT_LOVER": ("안정 추구자", "익숙한 장르에서 편안함을 찾는 당신!"),
    "NIGHT_OWL": ("밤의 시청자", "고요한 밤에 콘텐츠를 즐기는 당신!"),
    "BINGE_MASTER": ("정주행 마스터", "한번 시작하면 끝까지 보는 완주의 달인!"),
}

TRAIT_POOL = {
    "happy_score": "웃음 포인트",
    "explorer_score": "장르 탐험가",
    "immersion_score": "몰입형 시청자",
    "thrill_score": "짜릿함 추구자",
    "calm_score": "차분한 관찰자",
    "night_score": "야행성 시청자",
    "depth_score": "감정 깊이",
}

DNA_TRAIT_KEYS = {
    "JOYFUL_EXPLORER": ["happy_score", "explorer_score", "immersion_score"],
    "EMOTIONAL_DIVER": ["depth_score", "immersion_score", "happy_score"],
    "THRILL_SEEKER": ["thrill_score", "immersion_score", "explorer_score"],
    "CALM_OBSERVER": ["calm_score", "immersion_score", "explorer_score"],
    "MOOD_SURFER": ["depth_score", "explorer_score", "happy_score"],
    "COMFORT_LOVER": ["immersion_score", "calm_score", "happy_score"],
    "NIGHT_OWL": ["night_score", "immersion_score", "happy_score"],
    "BINGE_MASTER": ["immersion_score", "happy_score", "explorer_score"],
}


def determine_dna_type(avg_completion, night_ratio, happy_pct, sad_pct,
                       surprise_pct, neutral_pct, num_genres, total_sessions) -> str:
    # Order matters: the first matching rule wins.
    if avg_completion >= 0.85:
        return "BINGE_MASTER"
    if night_ratio >= 0.40:
        return "NIGHT_OWL"
    if surprise_pct >= 0.30:
        return "THRILL_SEEKER"
    if neutral_pct >= 0.40:
        return "CALM_OBSERVER"
    if (happy_pct + sad_pct) >= 0.50 and avg_completion >= 0.70:
        return "EMOTIONAL_DIVER"
    if happy_pct >= 0.35:
        return "JOYFUL_EXPLORER"
    if num_genres <= 2 and total_sessions >= 3:
        return "COMFORT_LOVER"
    if max(happy_pct, sad_pct, surprise_pct, neutral_pct) < 0.30:
        return "MOOD_SURFER"
    return "JOYFUL_EXPLORER"


def compute_traits(dna_type, happy_pct, sad_pct, surprise_pct, neutral_pct,
                   avg_completion, num_genres, night_ratio) -> list:
    scores = {
        "happy_score": int(happy_pct * 100),
        "explorer_score": int(min(num_genres / 16 * 100, 100)),
        "immersion_score": int(avg_completion * 100),
        "thrill_score": int(surprise_pct * 100),
        "calm_score": int(neutral_pct * 100),
        "night_score": int(night_ratio * 100),
        "depth_score": int((happy_pct + sad_pct) / 2 * 100),
    }
    keys = DNA_TRAIT_KEYS.get(dna_type, ["happy_score", "explorer_score", "immersion_score"])
    return [{"trait": TRAIT_POOL[k], "score": scores[k]} for k in keys]


def compute_fun_facts(happy_pct, avg_completion, total_sessions) -> list:
    avg_happy = 0.30
    avg_completion_ref = 0.60
    facts = []

    happy_diff = int(abs(happy_pct - avg_happy) / avg_happy * 100)
    if happy_pct >= avg_happy:
        facts.append(f"당신은 평균보다 {happy_diff}% 더 자주 웃어요")
    else:
        facts.append(f"당신은 평균보다 {happy_diff}% 덜 웃는 편이에요")

    completion_diff = int(abs(avg_completion - avg_completion_ref) / avg_completion_ref * 100)
    if avg_completion >= avg_completion_ref:
        facts.append(f"영상 완주율이 평균보다 {completion_diff}% 높아요")
    else:
        facts.append(f"총 {total_sessions}편의 영상을 시청했어요")
    return facts


def default_dna(now: datetime) -> Dict:
    title, description = DNA_TYPES["JOYFUL_EXPLORER"]
    return {
        "dna_type": "JOYFUL_EXPLORER",
        "dna_title": title,
        "dna_description": description,
        "traits": [
            {"trait": "웃음 포인트", "score": 0},
            {"trait": "장르 탐험가", "score": 0},
            {"trait": "몰입형 시청자", "score": 0},
        ],
        "emotion_radar": {"happy": 0, "surprise": 0, "neutral": 0, "sad": 0, "angry": 0},
        "fun_facts": ["아직 시청 데이터가 없어요. 영상을 더 시청해보세요!"],
        "generated_at": now.isoformat(),
        "based_on_videos": 0,
    }


def dna_cache_is_fresh(cached: Optional[Mapping], total_sessions: int, now: datetime) -> bool:
    """Original rule: not expired yet and fewer than 5 sessions added since the cache."""
    return bool(
        cached
        and cached["expires_at"] > now
        and total_sessions - cached["based_on_videos"] < DNA_CACHE_MAX_NEW_SESSIONS
    )


def compute_dna(docs: List[Dict], video_categories: Mapping[str, object], now: datetime) -> Dict:
    """Original DNA aggregation over every session of one actor.

    `video_categories` holds only videos that still exist, like the original `Video` lookup.
    """
    ep_sums = {emotion: 0.0 for emotion in EMOTIONS}
    total_completion = 0.0
    night_count = 0
    video_ids = set()

    for doc in docs:
        ep = doc.get("emotion_percentages", {})
        for emotion in ep_sums:
            ep_sums[emotion] += float(ep.get(emotion, 0.0))
        total_completion += float(doc.get("completion_rate", 0.0))
        hour = doc["created_at"].hour
        if hour >= 22 or hour <= 3:
            night_count += 1
        video_ids.add(doc["video_id"])

    n = len(docs)
    happy_pct = ep_sums["happy"] / n
    sad_pct = ep_sums["sad"] / n
    surprise_pct = ep_sums["surprise"] / n
    neutral_pct = ep_sums["neutral"] / n
    angry_pct = ep_sums["angry"] / n
    avg_completion = total_completion / n
    night_ratio = night_count / n

    num_genres = len({
        category.value if hasattr(category, "value") else category
        for video_id, category in video_categories.items()
        if video_id in video_ids
    })

    dna_type = determine_dna_type(avg_completion, night_ratio, happy_pct, sad_pct,
                                  surprise_pct, neutral_pct, num_genres, n)
    title, description = DNA_TYPES[dna_type]
    return {
        "dna_type": dna_type,
        "dna_title": title,
        "dna_description": description,
        "traits": compute_traits(dna_type, happy_pct, sad_pct, surprise_pct, neutral_pct,
                                 avg_completion, num_genres, night_ratio),
        "emotion_radar": {
            "happy": int(happy_pct * 100),
            "surprise": int(surprise_pct * 100),
            "neutral": int(neutral_pct * 100),
            "sad": int(sad_pct * 100),
            "angry": int(angry_pct * 100),
        },
        "fun_facts": compute_fun_facts(happy_pct, avg_completion, n),
        "generated_at": now.isoformat(),
        "based_on_videos": n,
    }


def plan_dna(cached: Optional[Mapping], total_sessions: int, docs_loader: Callable[[], List[Dict]],
             categories_loader: Callable[[set], Mapping[str, object]],
             now: datetime) -> Tuple[Dict, Optional[Dict]]:
    """Return (response, cache_write). cache_write is None unless the cache must change.

    The first element of cache_write is the cache row to store ({"op": "insert"|"update", ...}).
    Reads happen in the original order: cache check first, then the sessions, then the videos.
    """
    if dna_cache_is_fresh(cached, total_sessions, now):
        return cached["dna_data"], None

    docs = docs_loader()
    if not docs:
        return default_dna(now), None  # not cached: nothing to base the cache on

    video_ids = {doc["video_id"] for doc in docs}
    result = compute_dna(docs, categories_loader(video_ids), now)
    row = {
        "op": "update" if cached else "insert",
        "dna_type": result["dna_type"],
        "dna_data": result,
        "based_on_videos": result["based_on_videos"],
        "generated_at": now,
        "expires_at": now + DNA_CACHE_TTL,
    }
    return result, row
