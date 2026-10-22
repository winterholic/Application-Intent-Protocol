# WBS 7.2.8: FaceReview my page emotion screens as AIP read extensions.
#   GET mypage/videos/recent   MypageService.get_recent_videos  -> User.myRecentVideos
#   GET mypage/highlight       MypageService.get_highlight      -> User.myHighlight
#   GET mypage/emotion/calendar MypageService.get_emotion_calendar -> User.myEmotionCalendar
#   GET mypage/moments         MypageService.get_moments        -> User.myMoments
#   GET mypage/emotion-dna     MypageService.get_emotion_dna    -> User.myEmotionDna
#   GET mypage/emotion/summary MypageService.get_emotion_summary -> User.myEmotionSummary (a write extension, Slice14-F)
# Source of truth: new-facereview@7948b7e7 app/services/mypage_service.py. The sessions of the caller live in MongoDB with per-frame
# timelines far too large for an extension to read, so the server reduces them (derived collection `user_watch_stats`, decision
# D-P-USERSTATS) to the figures the screens need: the latest sessions, the 40 bucket sums of a timeline, the calendar rows, the peak
# frames of the latest 200 sessions and the totals of the DNA. The rules of the screens (rounding, grouping, the DNA types) stay here,
# copied from the original, and the videos come from ctx.data.read (row policy and caller enforced by the server).
#
# Differences from the original (see extensions/README.md, Slice14-E):
#  - Soft-deleted videos are invisible (the row policy); the original's moments and DNA also count them.
#  - The DNA is computed on every call. The original keeps it in user_emotion_dna for 7 days and recomputes after 5 new sessions.
#  - The DNA reads the genres of at most about 3000 distinct watched videos (one read per 50 ids, 64 calls per request); beyond that
#    the request fails instead of answering with a wrong genre count.
#  - A timeline key Python's int() accepts but Rust's parse does not (digit separators) makes the session count as unparsable.
import json
from datetime import datetime, timedelta

EMOTIONS = ["neutral", "happy", "surprise", "sad", "angry"]
RDB_IN = 50
MAX_DNA_VIDEOS = 3000

_DNA_TYPES = {
    'JOYFUL_EXPLORER': ('유쾌한 탐험가', '새로운 장르를 두려워하지 않고, 어디서든 웃음을 찾아내는 당신!'),
    'EMOTIONAL_DIVER': ('감성 다이버', '기쁨과 슬픔을 깊게 느끼며 콘텐츠에 완전히 몰입하는 당신!'),
    'THRILL_SEEKER': ('스릴 추구자', '예상치 못한 순간에 짜릿함을 느끼는 당신!'),
    'CALM_OBSERVER': ('차분한 관찰자', '감정에 흔들리지 않고 냉철하게 콘텐츠를 분석하는 당신!'),
    'MOOD_SURFER': ('무드 서퍼', '감정의 파도를 자유롭게 타며 시청하는 당신!'),
    'COMFORT_LOVER': ('안정 추구자', '익숙한 장르에서 편안함을 찾는 당신!'),
    'NIGHT_OWL': ('밤의 시청자', '고요한 밤에 콘텐츠를 즐기는 당신!'),
    'BINGE_MASTER': ('정주행 마스터', '한번 시작하면 끝까지 보는 완주의 달인!'),
}
_TRAIT_POOL = {
    'happy_score': '웃음 포인트', 'explorer_score': '장르 탐험가', 'immersion_score': '몰입형 시청자', 'thrill_score': '짜릿함 추구자',
    'calm_score': '차분한 관찰자', 'night_score': '야행성 시청자', 'depth_score': '감정 깊이',
}
_DNA_TRAIT_KEYS = {
    'JOYFUL_EXPLORER': ['happy_score', 'explorer_score', 'immersion_score'],
    'EMOTIONAL_DIVER': ['depth_score', 'immersion_score', 'happy_score'],
    'THRILL_SEEKER': ['thrill_score', 'immersion_score', 'explorer_score'],
    'CALM_OBSERVER': ['calm_score', 'immersion_score', 'explorer_score'],
    'MOOD_SURFER': ['depth_score', 'explorer_score', 'happy_score'],
    'COMFORT_LOVER': ['immersion_score', 'calm_score', 'happy_score'],
    'NIGHT_OWL': ['night_score', 'immersion_score', 'happy_score'],
    'BINGE_MASTER': ['immersion_score', 'happy_score', 'explorer_score'],
}


def _text(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def _stats(ctx, **spec):
    return ctx.call("mongo", "user_watch_stats", spec)


async def _videos(ctx, video_ids, select):
    """The visible videos among these ids, by id."""
    found = {}
    ids = list(video_ids)
    for start in range(0, len(ids), RDB_IN):
        page = await ctx.data.read({"read": "Video", "select": select, "limit": RDB_IN,
                                    "filter": [{"field": "id", "op": "in", "value": ids[start:start + RDB_IN]}]})
        for row in page["rows"]:
            found[row["id"]] = row
    return found


def _watch_url(youtube_id):
    return f"https://www.youtube.com/watch?v={youtube_id}"


def _timeline_dto(packed):
    """The original `_compress_timeline` result from the server's bucket sums and counts (`{}` when there is nothing to draw)."""
    if packed is None:
        return {}
    result = {emotion: [] for emotion in EMOTIONS}
    for x in range(len(packed["counts"])):
        count = packed["counts"][x]
        for i, emotion in enumerate(EMOTIONS):
            y = round(packed["sums"][x][i] / count, 1) if count > 0 else 0.0
            result[emotion].append({'x': x + 1, 'y': y})
    return result


async def myRecentVideos(input, ctx):
    emotion, page, size = input["emotion"], input["page"], input["size"]
    skip = (page - 1) * size
    spec = {"command": "sessions", "skip": skip, "limit": size + 1}
    if emotion != 'all':
        spec["emotion"] = emotion
    sessions = (await _stats(ctx, **spec))["sessions"]
    has_next = len(sessions) > size
    docs = sessions[:size]
    video_map = await _videos(ctx, {doc["video_id"] for doc in docs}, ["id", "youtube_url", "title", "duration"])
    shown = [doc for doc in docs if doc["video_id"] in video_map]
    timelines = []
    if shown:
        packed = (await _stats(ctx, command="compress", items=[
            {"session": doc["session"], "duration": video_map[doc["video_id"]]["duration"]} for doc in shown]))["timelines"]
        timelines = packed
    videos = []
    for doc, packed in zip(shown, timelines):
        video = video_map[doc["video_id"]]
        percentages = doc["emotion_percentages"] or {}
        dominant = doc["dominant_emotion"] or 'neutral'
        videos.append({
            'video_id': video["id"],
            'youtube_url': _watch_url(video["youtube_url"]),
            'title': video["title"],
            'dominant_emotion': dominant,
            'dominant_emotion_per': round(float(percentages.get(dominant, 0.0)) * 100, 2),
            'watched_at': doc["created_at"],
            'timeline_data': _timeline_dto(packed),
        })
    return {"body": _text({'videos': videos, 'total': skip + len(videos) + (1 if has_next else 0), 'page': page, 'size': size, 'has_next': has_next})}


async def myHighlight(input, ctx):
    docs = (await _stats(ctx, command="highlight"))["sessions"]
    if not docs:
        return {"body": _text({'emotion_videos': [], 'category_emotions': [], 'most_watched_category': 'none', 'most_felt_emotion': 'neutral'})}
    video_map = await _videos(ctx, {doc["video_id"] for doc in docs}, ["id", "youtube_url", "title", "category"])
    emotion_videos_map = {}
    category_emotions_map = {}
    category_counts = {}
    total_emotions = {'neutral': 0.0, 'happy': 0.0, 'surprise': 0.0, 'sad': 0.0, 'angry': 0.0}
    for doc in docs:
        video_id = doc["video_id"]
        ep = doc["emotion_percentages"] or {}
        for emotion, percentage in ep.items():
            if emotion not in emotion_videos_map or percentage > emotion_videos_map[emotion][1]:
                emotion_videos_map[emotion] = (video_id, percentage)
        for emotion in total_emotions:
            total_emotions[emotion] += float(ep.get(emotion, 0.0))
        video = video_map.get(video_id)
        if not video:
            continue
        category = video["category"]
        if category not in category_emotions_map:
            category_emotions_map[category] = {'neutral': 0.0, 'happy': 0.0, 'surprise': 0.0, 'sad': 0.0, 'angry': 0.0}
        for emotion in category_emotions_map[category]:
            category_emotions_map[category][emotion] += float(ep.get(emotion, 0.0))
        category_counts[category] = category_counts.get(category, 0) + 1
    emotion_videos = []
    for emotion in EMOTIONS:
        if emotion in emotion_videos_map:
            video_id, percentage = emotion_videos_map[emotion]
            video = video_map.get(video_id)
            if video:
                emotion_videos.append({'emotion': emotion, 'video_id': video["id"], 'youtube_url': _watch_url(video["youtube_url"]),
                                       'title': video["title"], 'emotion_percentage': percentage})
    category_emotions = []
    for category, emotions in category_emotions_map.items():
        total = sum(emotions.values())
        dominant_emotion = max(emotions, key=emotions.get)
        percentage = round((emotions[dominant_emotion] / total) * 100, 1) if total > 0 else 0.0
        category_emotions.append({'category': category, 'dominant_emotion': dominant_emotion, 'percentage': percentage})
    most_watched_category = max(category_counts, key=category_counts.get) if category_counts else 'none'
    most_felt_emotion = max(total_emotions, key=total_emotions.get) if any(total_emotions.values()) else 'neutral'
    return {"body": _text({'emotion_videos': emotion_videos, 'category_emotions': category_emotions,
                           'most_watched_category': most_watched_category, 'most_felt_emotion': most_felt_emotion})}


def _millis(moment):
    return int((moment - datetime(1970, 1, 1)).total_seconds() * 1000)


async def myEmotionCalendar(input, ctx):
    year = input["year"] if input.get("year") is not None else datetime.utcnow().year
    month = input.get("month")
    start_date = datetime(year, month or 1, 1)
    if month:
        end_date = datetime(year + 1, 1, 1) if month == 12 else datetime(year, month + 1, 1)
    else:
        end_date = datetime(year + 1, 1, 1)
    rows = (await _stats(ctx, command="calendar", start=_millis(start_date), end=_millis(end_date)))["rows"]
    daily_map = {}
    for row in rows:
        date_str = row["created_at"][:10]
        daily_map.setdefault(date_str, []).append({
            'dominant': row["dominant"], 'intensity': float(row["intensity"]), 'watch_secs': row["timeline_len"] // 2})
    data = []
    for date_str in sorted(daily_map.keys()):
        sessions = daily_map[date_str]
        emotion_counts = {}
        total_intensity = 0.0
        total_secs = 0
        for s in sessions:
            emotion_counts[s['dominant']] = emotion_counts.get(s['dominant'], 0) + 1
            total_intensity += s['intensity']
            total_secs += s['watch_secs']
        data.append({'date': date_str, 'dominant_emotion': max(emotion_counts, key=emotion_counts.get),
                     'intensity': round(min(total_intensity / len(sessions), 1.0), 3),
                     'watch_count': len(sessions), 'total_watch_time': total_secs})
    return {"body": _text({'year': year, 'month': month, 'data': data})}


async def myMoments(input, ctx):
    emotion, page, size = input["emotion"], input["page"], input["size"]
    sessions = (await _stats(ctx, command="moments", limit=200))["sessions"]
    # The original reads every video of these sessions, deleted ones included; the row policy hides the deleted ones here.
    video_map = await _videos(ctx, {doc["video_id"] for doc in sessions}, ["id", "youtube_url", "title"])
    all_moments = []
    for doc in sessions:
        video = video_map.get(doc["video_id"])
        if not video:
            continue
        for moment in doc["moments"]:
            all_moments.append({
                'video_id': video["id"], 'video_title': video["title"], 'youtube_url': _watch_url(video["youtube_url"]),
                'timestamp_seconds': round(moment["offset"] / 100.0, 1), 'emotion': moment["emotion"],
                'emotion_percentage': round(moment["score"], 2),
                'thumbnail_url': f"https://img.youtube.com/vi/{video['youtube_url']}/hqdefault.jpg", 'watched_at': doc["created_at"],
            })
    if emotion != 'all':
        all_moments = [m for m in all_moments if m['emotion'] == emotion]
    all_moments.sort(key=lambda m: m['emotion_percentage'], reverse=True)
    total = len(all_moments)
    start = (page - 1) * size
    return {"body": _text({'moments': all_moments[start:start + size], 'total': total, 'has_next': (start + size) < total})}


def _determine_dna_type(avg_completion, night_ratio, happy_pct, sad_pct, surprise_pct, neutral_pct, num_genres, total_sessions):
    if avg_completion >= 0.85:
        return 'BINGE_MASTER'
    if night_ratio >= 0.40:
        return 'NIGHT_OWL'
    if surprise_pct >= 0.30:
        return 'THRILL_SEEKER'
    if neutral_pct >= 0.40:
        return 'CALM_OBSERVER'
    if (happy_pct + sad_pct) >= 0.50 and avg_completion >= 0.70:
        return 'EMOTIONAL_DIVER'
    if happy_pct >= 0.35:
        return 'JOYFUL_EXPLORER'
    if num_genres <= 2 and total_sessions >= 3:
        return 'COMFORT_LOVER'
    if max(happy_pct, sad_pct, surprise_pct, neutral_pct) < 0.30:
        return 'MOOD_SURFER'
    return 'JOYFUL_EXPLORER'


def _compute_traits(dna_type, happy_pct, sad_pct, surprise_pct, neutral_pct, avg_completion, num_genres, night_ratio):
    scores = {
        'happy_score': int(happy_pct * 100),
        'explorer_score': int(min(num_genres / 16 * 100, 100)),
        'immersion_score': int(avg_completion * 100),
        'thrill_score': int(surprise_pct * 100),
        'calm_score': int(neutral_pct * 100),
        'night_score': int(night_ratio * 100),
        'depth_score': int((happy_pct + sad_pct) / 2 * 100),
    }
    keys = _DNA_TRAIT_KEYS.get(dna_type, ['happy_score', 'explorer_score', 'immersion_score'])
    return [{'trait': _TRAIT_POOL[k], 'score': scores[k]} for k in keys]


def _compute_fun_facts(happy_pct, avg_completion, total_sessions):
    AVG_HAPPY = 0.30
    AVG_COMPLETION = 0.60
    facts = []
    happy_diff = int(abs(happy_pct - AVG_HAPPY) / AVG_HAPPY * 100)
    if happy_pct >= AVG_HAPPY:
        facts.append(f"당신은 평균보다 {happy_diff}% 더 자주 웃어요")
    else:
        facts.append(f"당신은 평균보다 {happy_diff}% 덜 웃는 편이에요")
    completion_diff = int(abs(avg_completion - AVG_COMPLETION) / AVG_COMPLETION * 100)
    if avg_completion >= AVG_COMPLETION:
        facts.append(f"영상 완주율이 평균보다 {completion_diff}% 높아요")
    else:
        facts.append(f"총 {total_sessions}편의 영상을 시청했어요")
    return facts


async def myEmotionDna(input, ctx):
    stats = await _stats(ctx, command="dna")
    n = stats["count"]
    if n == 0:
        title, description = _DNA_TYPES['JOYFUL_EXPLORER']
        return {"body": _text({
            'dna_type': 'JOYFUL_EXPLORER', 'dna_title': title, 'dna_description': description,
            'traits': [{'trait': '웃음 포인트', 'score': 0}, {'trait': '장르 탐험가', 'score': 0}, {'trait': '몰입형 시청자', 'score': 0}],
            'emotion_radar': {'happy': 0, 'surprise': 0, 'neutral': 0, 'sad': 0, 'angry': 0},
            'fun_facts': ['아직 시청 데이터가 없어요. 영상을 더 시청해보세요!'],
            'generated_at': datetime.utcnow().isoformat(), 'based_on_videos': 0})}
    sums = dict(zip(EMOTIONS, stats["sums"]))
    happy_pct, sad_pct, surprise_pct = sums['happy'] / n, sums['sad'] / n, sums['surprise'] / n
    neutral_pct, angry_pct = sums['neutral'] / n, sums['angry'] / n
    avg_completion = stats["completion"] / n
    night_ratio = stats["night"] / n
    if len(stats["videos"]) > MAX_DNA_VIDEOS:
        raise Exception("TOO_MANY_VIDEOS: the genre count needs more videos than one request can read")
    videos = await _videos(ctx, stats["videos"], ["id", "category"])
    num_genres = len({video["category"] for video in videos.values()})
    dna_type = _determine_dna_type(avg_completion, night_ratio, happy_pct, sad_pct, surprise_pct, neutral_pct, num_genres, n)
    title, description = _DNA_TYPES[dna_type]
    generated_at = datetime.utcnow()
    return {"body": _text({
        'dna_type': dna_type, 'dna_title': title, 'dna_description': description,
        'traits': _compute_traits(dna_type, happy_pct, sad_pct, surprise_pct, neutral_pct, avg_completion, num_genres, night_ratio),
        'emotion_radar': {'happy': int(happy_pct * 100), 'surprise': int(surprise_pct * 100), 'neutral': int(neutral_pct * 100),
                          'sad': int(sad_pct * 100), 'angry': int(angry_pct * 100)},
        'fun_facts': _compute_fun_facts(happy_pct, avg_completion, n),
        'generated_at': generated_at.isoformat(), 'based_on_videos': n})}


_SUMMARY_COLUMNS = ["neutral_seconds", "happy_seconds", "surprise_seconds", "sad_seconds", "angry_seconds"]


def _summary_dto(seconds):
    """The original `_build_emotion_summary_from_docs` result for totals that are already seconds per emotion."""
    total = float(sum(seconds.values()))
    return {
        'emotion_percentages': {e: round((s / total) * 100, 1) if total > 0 else 0.0 for e, s in seconds.items()},
        'emotion_seconds': {e: int(s) for e, s in seconds.items()},
    }


def _utc_text(text):
    return text if text.endswith("Z") else text + "Z"


async def myEmotionSummary(input, ctx):
    page = await ctx.data.read({"read": "UserEmotionSummary", "limit": 1, "select": ["id", "last_finalized_at", "last_session_id", "lock_version"] + _SUMMARY_COLUMNS})
    row = page["rows"][0] if page["rows"] else None
    if row is not None and row["last_finalized_at"] is not None:
        after = {"at": row["last_finalized_at"], "session": row["last_session_id"] or ""}
    else:
        after = None
    stats = await _stats(ctx, command="emotion_seconds", **({"after": after} if after else {}))
    if row is None:
        # The first call creates the member's row with zero totals (the original's `UserEmotionSummary(user_id=...)`).
        try:
            await ctx.data.create("UserEmotionSummary", {"id": stats["user"]})
        except Exception as error:
            # Another call of the same member created the row first (the primary key refused this one): the server cancels this call's
            # writes and the caller sends the request again, like a lock_version conflict.
            if getattr(error, "code", None) == "WIRE.ALREADY_EXISTS":
                raise type(error)("WIRE.CONFLICT")
            raise
        row = {"id": stats["user"], "lock_version": 0, **{column: 0 for column in _SUMMARY_COLUMNS}}
    base = {emotion: int(row[column]) for emotion, column in zip(EMOTIONS, _SUMMARY_COLUMNS)}
    if after is not None and stats["checkpoint"] is None:
        return {"body": _text(_summary_dto(base))}
    merged = {emotion: base[emotion] + int(stats["seconds"][i]) for i, emotion in enumerate(EMOTIONS)}
    checkpoint = stats["checkpoint"]
    values = {column: merged[emotion] for emotion, column in zip(EMOTIONS, _SUMMARY_COLUMNS)}
    values["last_finalized_at"] = _utc_text(checkpoint["at"])
    values["last_session_id"] = checkpoint["session"]
    await ctx.data.apply("UserEmotionSummary", "applyDelta", [row["id"]], values=values, expect={"lock_version": row["lock_version"]})
    return {"body": _text(_summary_dto(merged))}
