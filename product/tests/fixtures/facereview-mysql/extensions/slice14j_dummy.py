# WBS 7.2.9: the administrator's test data of FaceReview, AdminService.generate_dummy_data (new-facereview@7948b7e7
# app/services/admin_service.py), as a durable job.
#
# The original picks `count` of the visible videos at random and, for each one, makes a finished viewing session of the administrator:
# frames at two per second whose emotions follow the genre, the session document in MongoDB (youtube_watching_data), the view log row
# (a time up to 90 days back), one more view of the video and the video's distribution recomputed from every session of that video.
# A video that fails is skipped. The same steps run here with the platform's building blocks: the view log from the exposed create
# VideoViewLog (administrators only), the session through the Mongo grant of the job (the server puts the job owner's id in it), the view
# from the transition Video.view, the sums of the video's sessions from the derived collection video_session_stats and the distribution
# from an upsert.
#
# Every random choice comes from random.Random(run id) and consumes the generator in the original's order, so a retry of the run plans
# exactly the same videos and frames (the journal then replays the steps that were done) and, given the same generator, the plan is the
# original's (reference/slice14j_oracle.py runs both from one seed).
#
# Differences from the original (see extensions/README.md, Slice14-J): a job answers through its result, not the response of the call; it
# has no saga, so a failure inside one video can leave that video's earlier steps (the retry finishes them); a video longer than two hours
# is skipped (its session document would not fit the 1 MiB a call may carry); more than 5000 visible videos fail instead of being sampled
# from a part; a ctx call that the server refuses as BAD_VALUE (for instance one more view on a video that already has the largest count the
# field holds) ends the whole run, where the original skips that video; the service must set limits.job_calls to 2000 or more for 300 videos (five calls each).
import json
import random
import time
from datetime import datetime, timedelta, timezone

EMOTIONS = ["neutral", "happy", "surprise", "sad", "angry"]
MAX_SESSION_SECONDS = 7200
MAX_VIDEOS = 5000
PAGE = 100
MIN_RELIABLE_FRAMES = 30

CATEGORY_WEIGHTS = {
    'drama':       {'neutral': 0.5, 'happy': 2.5, 'surprise': 2.0, 'sad': 3.0, 'angry': 2.0},
    'eating':      {'neutral': 1.0, 'happy': 4.0, 'surprise': 2.5, 'sad': 0.2, 'angry': 0.5},
    'travel':      {'neutral': 0.8, 'happy': 3.5, 'surprise': 3.0, 'sad': 0.5, 'angry': 0.3},
    'cook':        {'neutral': 2.5, 'happy': 3.0, 'surprise': 1.5, 'sad': 0.3, 'angry': 0.5},
    'show':        {'neutral': 0.3, 'happy': 4.5, 'surprise': 2.5, 'sad': 1.0, 'angry': 0.8},
    'information': {'neutral': 3.0, 'happy': 1.5, 'surprise': 2.5, 'sad': 0.5, 'angry': 1.5},
    'horror':      {'neutral': 0.5, 'happy': 0.3, 'surprise': 5.0, 'sad': 1.5, 'angry': 2.0},
    'exercise':    {'neutral': 2.0, 'happy': 3.5, 'surprise': 1.5, 'sad': 0.5, 'angry': 1.5},
    'vlog':        {'neutral': 2.5, 'happy': 2.5, 'surprise': 1.5, 'sad': 1.5, 'angry': 1.0},
    'game':        {'neutral': 1.5, 'happy': 3.0, 'surprise': 2.5, 'sad': 1.5, 'angry': 2.5},
    'sports':      {'neutral': 1.0, 'happy': 3.5, 'surprise': 4.0, 'sad': 1.0, 'angry': 1.5},
    'music':       {'neutral': 1.5, 'happy': 4.0, 'surprise': 2.0, 'sad': 2.5, 'angry': 1.0},
    'animal':      {'neutral': 1.0, 'happy': 4.5, 'surprise': 3.0, 'sad': 0.8, 'angry': 0.2},
    'beauty':      {'neutral': 2.0, 'happy': 3.5, 'surprise': 2.5, 'sad': 0.5, 'angry': 0.5},
    'comedy':      {'neutral': 0.2, 'happy': 5.0, 'surprise': 2.0, 'sad': 0.3, 'angry': 0.5},
    'etc':         {'neutral': 2.5, 'happy': 2.0, 'surprise': 2.0, 'sad': 1.5, 'angry': 1.5},
}
DEFAULT_WEIGHTS = {'neutral': 2.0, 'happy': 2.0, 'surprise': 2.0, 'sad': 2.0, 'angry': 2.0}


def pick_dominant(rng, category, prev=None):
    # 78% the same emotion as the frame before (continuity), 12% an emotion of any genre (mood shift), else the genre's weights.
    if prev and rng.random() < 0.78:
        return prev
    if rng.random() < 0.12:
        return rng.choice(EMOTIONS)
    weights = CATEGORY_WEIGHTS.get(category, DEFAULT_WEIGHTS)
    return rng.choices(EMOTIONS, weights=[weights[e] for e in EMOTIONS], k=1)[0]


def make_frame_scores(rng, dominant):
    dominant_score = rng.uniform(72.0, 95.0)
    remaining = 100.0 - dominant_score
    others = [e for e in EMOTIONS if e != dominant]
    raw = [rng.random() ** 2 for _ in others]
    total = sum(raw) or 1.0
    scores = {dominant: dominant_score}
    for e, r in zip(others, raw):
        scores[e] = remaining * r / total
    s = sum(scores.values())
    return {e: round(scores[e] / s * 100, 2) for e in EMOTIONS}


def generate_session_frames(rng, category, duration):
    frames = []
    prev = None
    for i in range(duration * 2):  # two frames a second; the key is in centiseconds
        dominant = pick_dominant(rng, category, prev)
        scores = make_frame_scores(rng, dominant)
        frames.append({'time_key': str(i * 50), 'scores': scores, 'dominant': dominant})
        prev = dominant
    return frames


def compute_session_stats(frames):
    totals = {e: 0.0 for e in EMOTIONS}
    for f in frames:
        for e in EMOTIONS:
            totals[e] += f['scores'][e]
    n = len(frames)
    percentages = {e: round(totals[e] / n / 100.0, 3) for e in EMOTIONS}
    dominant = max(percentages, key=percentages.get)
    return {'percentages': percentages, 'dominant': dominant}


def _date(moment):
    """A UTC datetime as the extended JSON date the Mongo grant accepts (milliseconds, like the driver stores it)."""
    millis = int((moment - datetime(1970, 1, 1)).total_seconds() * 1000)
    return {"$date": {"$numberLong": str(millis)}}


def _time_text(moment):
    return moment.strftime("%Y-%m-%dT%H:%M:%S.%f") + "Z"


def _code(error):
    return getattr(error, "code", None)


def session_document(video_id, view_log_id, created_at, frames, stats):
    return {
        'video_id': video_id,
        'video_view_log_id': view_log_id,
        'created_at': _date(created_at),
        'completion_rate': 1.0,
        'dominant_emotion': stats['dominant'],
        'emotion_percentages': stats['percentages'],
        'most_emotion_timeline': {f['time_key']: f['dominant'] for f in frames},
        'emotion_score_timeline': {f['time_key']: [f['scores'][e] for e in EMOTIONS] for f in frames},
        'emotion_seconds': {e: 0 for e in EMOTIONS},
        'finalized_at': None,
        'client_info': {'ip_address': None, 'user_agent': None, 'device': {'os': None, 'browser': None, 'is_mobile': False}},
    }


def distribution_update(stats):
    """The fields WatchingDataService._update_video_distribution sets from the sums of the sessions (None when there is nothing to set)."""
    count = stats["count"]
    if not count:
        return None
    averages = {e: round(stats["sums"][e] / count, 3) for e in EMOTIONS}
    scores = {'neutral': round(averages['neutral'] * 2, 3), 'happy': round(averages['happy'] * 3, 3),
              'surprise': round(averages['surprise'] * 4, 3), 'sad': round(averages['sad'] * 3, 3), 'angry': round(averages['angry'] * 3, 3)}
    # The representative emotion is only trusted from MIN_RELIABLE_FRAMES observed frames.
    dominant = max(averages, key=averages.get) if stats["frames"] >= MIN_RELIABLE_FRAMES else None
    return {'average_completion_rate': round(stats["completion_sum"] / count, 3), 'emotion_averages': averages,
            'recommendation_scores': scores, 'dominant_emotion': dominant}


async def _visible_videos(ctx):
    """Video.query.filter(is_deleted == 0, duration > 0).all(), in id order (the primary key order a table scan returns). The pages continue
    after the last id: a plain offset costs more with every page and the read budget refuses it long before 5000 videos."""
    videos, after = [], None
    for _ in range(MAX_VIDEOS // PAGE):
        query = {"read": "Video", "select": ["id", "category", "duration"], "limit": PAGE, "sort": [{"field": "id", "dir": "asc"}]}
        if after is not None:
            query["after"] = {"id": after}
        page = await ctx.data.read(query)
        videos.extend(row for row in page["rows"] if (row["duration"] or 0) > 0)
        if len(page["rows"]) < PAGE:
            return videos
        after = page["rows"][-1]["id"]
    raise Exception(f"more than {MAX_VIDEOS} visible videos")


async def generateDummyData(input, ctx):
    run = str((ctx.job or {}).get("id", ""))
    rng = random.Random(run)
    all_videos = await _visible_videos(ctx)
    if not all_videos:
        raise Exception("VIDEO_NOT_FOUND: 더미 데이터를 생성할 영상이 없습니다.")
    videos = rng.sample(all_videos, min(input["count"], len(all_videos)))
    created_ids = []
    for index, video in enumerate(videos):
        if video["duration"] > MAX_SESSION_SECONDS:
            continue
        try:
            frames = generate_session_frames(rng, video["category"], video["duration"])
            stats = compute_session_stats(frames)
            created_at = datetime.now(timezone.utc).replace(tzinfo=None) - timedelta(
                days=rng.randint(0, 90), hours=rng.randint(0, 23), minutes=rng.randint(0, 59))
            tag = f"dummy-{run}-{index}"
            view_log = await ctx.data.create("VideoViewLog", {"video": video["id"], "user": input["user"], "created_at": _time_text(created_at),
                                                              "updated_at": _time_text(created_at)}, key=f"{tag}-log")
            await ctx.mongo.insert_one("youtube_watching_data", session_document(video["id"], view_log, created_at, frames, stats), key=f"{tag}-doc")
            await ctx.data.apply("Video", "view", [video["id"]], key=f"{tag}-view")
            try:
                sums = await ctx.call("mongo", "video_session_stats", {"command": "distribution_sums", "video_id": video["id"]})
                update = distribution_update(sums)
            except Exception as error:
                # A session of some viewer holds a value that is not a number: the original logs it and leaves the distribution as it is.
                if _code(error) != "WIRE.BAD_VALUE":
                    raise
                update = None
            if update is not None:
                now = _date(datetime.now(timezone.utc).replace(tzinfo=None))
                await ctx.mongo.update_one("video_distribution", {"video_id": video["id"]},
                                           {"$set": {**update, "updated_at": now}, "$setOnInsert": {"video_id": video["id"], "created_at": now}},
                                           upsert=True, key=f"{tag}-distribution")
            created_ids.append(video["id"])
        except Exception as error:
            if _code(error) in ("WIRE.ADAPTER_UNAVAILABLE", "WIRE.DB_UNAVAILABLE", "WIRE.CONFLICT", "WIRE.RETRY_LATER", "WIRE.DEADLINE_EXCEEDED"):
                raise ctx.retry_later("a store does not answer", after_ms=1000)
            if _code(error) in ("WIRE.COST_EXCEEDED", "WIRE.CALL_LIMIT"):
                raise
            # The original logs a failed video and goes on; so does the run (the video is not in the result).
            continue
    return {"created_count": len(created_ids), "video_ids": json.dumps(created_ids)}
