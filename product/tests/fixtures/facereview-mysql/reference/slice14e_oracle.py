"""WBS 7.2.8: the my page emotion screens of the AIP server versus the ORIGINAL MypageService.

  ORACLE  the pinned original's MypageService (app/services/mypage_service.py) imported from the original checkout and called with the
          caller's user id: get_recent_videos, get_highlight, get_emotion_calendar, get_moments and get_emotion_dna, on the same MySQL
          and MongoDB the AIP side reads.
  HTTP    POST /extension of User.myRecentVideos, myHighlight, myEmotionCalendar, myMoments and myEmotionDna to `aip service serve`
          (derived Mongo collection `user_watch_stats`, decision D-P-USERSTATS).

The Mongo sessions are generated here with real datetimes: dictionary and array timelines, 130 frame sessions that cross the 60 frame
window, an unparsable key, an empty timeline, sessions of a deleted video and of a video that does not exist. Every answer must be equal
(the DNA is compared without its generated_at). Differences that are intended are listed in extensions/README.md, Slice14-E; the data
here avoids them (no peak in a session of a deleted video).

Required environment: S14E_MYSQL_ADMIN_URL, S14E_MONGO_URL, S14E_AIP_BIN, S14E_ORIGINAL_ROOT, S14E_WORK_DIR (empty). Run with the Python
that has flask, flask_sqlalchemy, pymysql, pymongo, isodate, requests, psutil, cryptography and PyJWT, and not from a directory that
holds a numbers.py.
"""
import json
import os
import sys
from datetime import datetime, timedelta
from pathlib import Path

import pymongo

sys.path.insert(0, str(Path(__file__).resolve().parent))
import slice13d_oracle as base  # noqa: E402
import slice14b_oracle as jobs  # noqa: E402

ALICE, BOB, DAVE = (base.SUBJECTS[name] for name in ("alice", "bob", "dave"))
VIDEO = "bbbbbbbb-0000-4000-8000-{:012d}"
EMOTIONS = ["neutral", "happy", "surprise", "sad", "angry"]


def shares(*values):
    return dict(zip(EMOTIONS, values))


def frames(count, step, peak_every, peak_emotion, *, as_list=False, base_score=20.0):
    """A timeline of `count` frames `step` centiseconds apart; every `peak_every`th frame peaks (>= 80) in `peak_emotion`."""
    timeline = {}
    for i in range(count):
        scores = {emotion: round(base_score + (i * 7 + n * 3) % 15, 2) for n, emotion in enumerate(EMOTIONS)}
        if peak_every and i % peak_every == peak_every - 1:
            scores[peak_emotion] = round(80.0 + (i % 13) * 1.25, 2)
        timeline[str(i * step)] = [scores[e] for e in EMOTIONS] if as_list else scores
    return timeline


def weak_peak():
    timeline = frames(20, 100, 0, "happy")
    timeline["500"] = {"neutral": 10.0, "happy": 82.5, "surprise": 5.0, "sad": 1.0, "angry": 1.0}
    return timeline


def session(user, video, created, dominant, percentages, completion, timeline, most=None):
    return {"user_id": user, "video_id": video, "video_view_log_id": f"log-{user[:4]}-{video[-4:]}-{created:%d%H%M}",
            "created_at": created, "dominant_emotion": dominant, "emotion_percentages": percentages, "completion_rate": completion,
            "emotion_score_timeline": timeline, "most_emotion_timeline": most or {}, "finalized_at": created + timedelta(minutes=5)}


def sessions():
    day = datetime
    docs = [
        session(ALICE, VIDEO.format(1), day(2026, 2, 3, 9, 5, 7, 123000), "happy", shares(.1, .6, .1, .1, .1), .9, frames(130, 50, 9, "happy")),
        session(ALICE, VIDEO.format(2), day(2026, 2, 3, 22, 40, 0, 0), "sad", shares(.2, .1, .1, .5, .1), .5, frames(40, 100, 7, "sad", as_list=True)),
        session(ALICE, VIDEO.format(3), day(2026, 2, 17, 23, 59, 59, 999000), "surprise", shares(.1, .1, .6, .1, .1), .7, frames(20, 200, 4, "surprise")),
        session(ALICE, VIDEO.format(8), day(2026, 3, 1, 0, 0, 0, 0), "neutral", shares(.7, .1, .05, .1, .05), 1.0, frames(95, 50, 0, "neutral")),
        session(ALICE, VIDEO.format(9), day(2026, 3, 1, 3, 30, 0, 500000), "angry", shares(.1, .1, .1, .1, .6), .35, frames(65, 60, 6, "angry")),
        session(ALICE, VIDEO.format(10), day(2026, 3, 9, 12, 0, 0, 1000), "happy", shares(.1, .5, .2, .1, .1), .6, frames(10, 100, 3, "happy", as_list=True)),
        session(ALICE, VIDEO.format(14), day(2026, 3, 9, 12, 30, 0, 2000), "happy", shares(.05, .8, .05, .05, .05), .95, frames(200, 25, 11, "happy")),
        session(ALICE, VIDEO.format(15), day(2026, 3, 31, 23, 59, 59, 999000), "sad", shares(.2, .1, .1, .55, .05), .4, frames(30, 100, 0, "sad")),
        # A deleted video, a video that does not exist, an empty timeline and an unparsable key (no peaks in the first two: see above).
        session(ALICE, VIDEO.format(6), day(2026, 3, 10, 8, 0, 0, 0), "neutral", shares(.5, .2, .1, .1, .1), .8, frames(30, 100, 0, "neutral")),
        session(ALICE, "ffffffff-ffff-4fff-8fff-ffffffffffff", day(2026, 3, 11, 8, 0, 0, 0), "happy", shares(.1, .7, .1, .05, .05), .8, frames(30, 100, 0, "happy")),
        session(ALICE, VIDEO.format(16), day(2026, 3, 12, 8, 0, 0, 0), "neutral", shares(.4, .3, .1, .1, .1), .2, {}),
        session(ALICE, VIDEO.format(17), day(2026, 3, 13, 8, 0, 0, 0), "happy", shares(.2, .5, .1, .1, .1), .6, {"abc": [10, 20, 30, 40, 50], "100": [90, 1, 2, 3, 4]}),
        session(ALICE, VIDEO.format(18), day(2026, 4, 2, 15, 15, 15, 150000), "surprise", shares(.1, .1, .5, .2, .1), .65, frames(45, 80, 5, "surprise")),
        session(ALICE, VIDEO.format(1), day(2026, 4, 20, 1, 1, 1, 1000), "neutral", shares(.45, .25, .1, .1, .1), .75, frames(33, 77, 8, "neutral")),
        # The only peak of the session is 82.5: it counts at the threshold 80 and would not at a stricter one.
        session(ALICE, VIDEO.format(4), day(2026, 4, 25, 10, 0, 0, 0), "happy", shares(.2, .5, .1, .1, .1), .55, weak_peak()),
        session(BOB, VIDEO.format(2), day(2026, 3, 5, 10, 0, 0, 0), "happy", shares(.2, .6, .1, .05, .05), .9, frames(25, 100, 5, "happy")),
    ]
    return docs


def seed_mongo(mongo_url):
    client = pymongo.MongoClient(mongo_url)
    client.drop_database(base.MONGO_DB)
    client[base.MONGO_DB]["youtube_watching_data"].insert_many(sessions())
    client.close()


def main():
    admin_url, mongo_url = base.need("S14E_MYSQL_ADMIN_URL"), base.need("S14E_MONGO_URL")
    base.check_endpoint(admin_url, "S14E_MYSQL_ADMIN_URL")
    base.check_endpoint(mongo_url, "S14E_MONGO_URL")
    work = Path(base.need("S14E_WORK_DIR"))
    if any(work.iterdir()):
        sys.exit("S14E_WORK_DIR must be empty")
    base.seed_mysql(admin_url)
    seed_mongo(mongo_url)
    # The fixture keeps a fresh cached DNA row for alice; the original would answer from it. The port recomputes on every call
    # (documented difference), so the cache is emptied to compare the computation itself.
    import urllib.parse  # noqa: E402

    import pymysql  # noqa: E402

    parsed = urllib.parse.urlparse(admin_url)
    connection = pymysql.connect(host=parsed.hostname, port=parsed.port, user=urllib.parse.unquote(parsed.username),
                                 password=urllib.parse.unquote(parsed.password or ""), autocommit=True, charset="utf8mb4")
    connection.cursor().execute(f"DELETE FROM `{base.SOURCE_DB}`.user_emotion_dna")
    connection.close()
    app, _, _, _ = base.load_original(base.need("S14E_ORIGINAL_ROOT"), admin_url, mongo_url)
    from app.services.mypage_service import MypageService  # noqa: E402

    deployment = jobs.deploy(work, admin_url, mongo_url, "http://127.0.0.1:9/youtube/v3/videos", base.need("S14E_AIP_BIN"))
    failures, count, golden = [], 0, []

    def case(name, extension, subject, ext_input, original_call, strip=()):
        nonlocal count
        count += 1
        with app.app_context():
            want = json.loads(json.dumps(original_call(), default=str))
        got = deployment.post(extension, ext_input, subject)
        port = json.loads(got["output"]["body"]) if got.get("ok") else None
        for key in strip:
            want.pop(key, None)
            if port is not None:
                port.pop(key, None)
        ok = port == want
        golden.append({"name": name, "extension": extension, "subject": subject, "input": ext_input, "expected": want})
        print(f"{'PASS' if ok else 'FAIL'} {name}")
        if not ok:
            print("   original:", json.dumps(want, ensure_ascii=False)[:1800])
            print("   aip     :", json.dumps(port if port is not None else got, ensure_ascii=False)[:1800])
            failures.append(name)

    try:
        for subject, actor in (("alice", ALICE), ("bob", BOB), ("dave", DAVE)):
            for emotion, page, size in (("all", 1, 5), ("all", 2, 5), ("all", 3, 5), ("happy", 1, 10), ("neutral", 1, 50), ("angry", 1, 3)):
                case(f"mypage/videos/recent {subject} {emotion} p{page} s{size}", "User.myRecentVideos", subject,
                     {"emotion": emotion, "page": page, "size": size}, lambda a=actor, e=emotion, p=page, s=size: MypageService.get_recent_videos(a, e, p, s))
            case(f"mypage/highlight {subject}", "User.myHighlight", subject, {}, lambda a=actor: MypageService.get_highlight(a))
            for year, month in ((2026, None), (2026, 2), (2026, 3), (2026, 4), (2026, 12), (2025, 12), (2026, 1)):
                case(f"mypage/emotion/calendar {subject} {year}-{month}", "User.myEmotionCalendar", subject,
                     {"year": year, "month": month}, lambda a=actor, y=year, m=month: MypageService.get_emotion_calendar(a, y, m))
            for emotion, page, size in (("all", 1, 10), ("all", 2, 10), ("all", 1, 50), ("happy", 1, 10), ("sad", 1, 10), ("angry", 1, 10), ("surprise", 2, 3)):
                case(f"mypage/moments {subject} {emotion} p{page} s{size}", "User.myMoments", subject,
                     {"emotion": emotion, "page": page, "size": size}, lambda a=actor, e=emotion, p=page, s=size: MypageService.get_moments(a, e, p, s))
            case(f"mypage/emotion-dna {subject}", "User.myEmotionDna", subject, {}, lambda a=actor: MypageService.get_emotion_dna(a),
                 strip=("generated_at",))
        # Another member's data is out of reach: bob's screens show only bob's sessions (checked above), and a guest has no my page.
        guest = deployment.post("User.myHighlight", {}, None)
        if guest.get("ok"):
            failures.append("guest")
            print("FAIL a guest got a my page answer")
        else:
            print("PASS a guest is refused")
    finally:
        deployment.close()
    pymongo.MongoClient(mongo_url).drop_database(base.MONGO_DB)
    if failures:
        sys.exit(f"FAILED {len(failures)} of {count}: " + ", ".join(failures))
    if os.environ.get("S14E_WRITE_GOLDEN"):
        def extended(value):
            if isinstance(value, datetime):
                delta = value - datetime(1970, 1, 1)
                return {"$date": {"$numberLong": str(delta.days * 86_400_000 + delta.seconds * 1000 + delta.microseconds // 1000)}}
            raise TypeError(type(value))
        (base.FIXTURE / "slice14e-golden.json").write_text(json.dumps(golden, ensure_ascii=False, indent=1) + "\n")
        (base.FIXTURE / "slice14e-sessions.json").write_text(json.dumps(sessions(), default=extended, ensure_ascii=False, indent=1) + "\n")
    print(f"the my page screens matched the original in {count} cases")


if __name__ == "__main__":
    main()
