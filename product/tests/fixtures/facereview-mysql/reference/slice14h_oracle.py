"""WBS 7.2.9: the administrator dashboard of the AIP server versus the ORIGINAL AdminService (get_signup_trend, get_business_stats).

  ORACLE  the pinned original's AdminService.get_signup_trend(period) for every period, and the parts of get_business_stats that read MySQL
          (the 7 day signup trend, weekly active users, the video request pipeline, the views per genre).
  HTTP    the admin aggregates User.signupCount, User.weeklyActive, VideoRequest.pendingCount and Video.categoryViews of
          `aip service serve`, composed by this script the way a client composes them (the original answers one JSON from one route;
          the contract here is the aggregates and the caller makes the buckets), and the admin read extensions of the dashboard.

Every case runs the original and the port on the same rows, which the script spreads over three years around today (UTC) with the
boundaries of every period: the first and last second of the day a period starts, deactivated members, view logs of guests and of
members at the 7 day edge, requests processed inside and outside the last 30 days, videos of every genre with deleted ones.

Required environment: S14H_MYSQL_ADMIN_URL, S14H_MONGO_URL, S14H_AIP_BIN, S14H_ORIGINAL_ROOT, S14H_WORK_DIR (empty). Optional:
S14H_WRITE_GOLDEN=1 writes slice14h-golden.json. Run with the Python that has flask, flask_sqlalchemy, pymysql, pymongo, isodate, requests,
psutil, cryptography and PyJWT, and not from a directory that holds a numbers.py.
"""
import json
import os
import sys
import urllib.parse
import uuid
from datetime import date, datetime, timedelta
from pathlib import Path

import pymongo
import pymysql

sys.path.insert(0, str(Path(__file__).resolve().parent))
import slice13d_oracle as base  # noqa: E402
import slice14b_oracle as jobs  # noqa: E402

GENRES = ["drama", "eating", "travel", "cook", "show", "information", "game", "sports", "music", "animal", "beauty", "comedy", "horror",
          "exercise", "vlog", "etc"]
PERIODS = {"7d": (7, "day"), "30d": (30, "day"), "3m": (90, "week"), "1y": (365, "month"), "3y": (1095, "month")}
ADMIN, MEMBER = "bob", "alice"


def connect(admin_url):
    parsed = urllib.parse.urlparse(admin_url)
    return pymysql.connect(host=parsed.hostname, port=parsed.port, user=urllib.parse.unquote(parsed.username),
                           password=urllib.parse.unquote(parsed.password or ""), autocommit=True, charset="utf8mb4", database=base.SOURCE_DB)


def iso(moment):
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


def seed(connection, today):
    """Rows around `today` (a UTC date). Returns nothing; both sides read the same tables."""
    cursor = connection.cursor()
    midnight = datetime(today.year, today.month, today.day)
    n = 0
    # Members: for every period edge the first and last second of the day, plus the days between, some of them deactivated.
    offsets = [0, 0, 1, 2, 5, 6, 6, 7, 8, 13, 28, 29, 29, 30, 31, 60, 88, 89, 89, 90, 91, 92, 100, 180, 200, 330, 364, 364, 365, 366, 367, 400, 700, 729,
               730, 1000, 1093, 1094, 1094, 1095, 1096, 1200, 1500]
    for i, back in enumerate(offsets):
        for second in (0, 86399):
            n += 1
            at = midnight - timedelta(days=back) + timedelta(seconds=second)
            if at > datetime.utcnow() - timedelta(seconds=30):
                continue
            cursor.execute("INSERT INTO user (user_id, email, password, name, role, profile_image_id, is_tutorial_done, is_verify_email_done, is_deleted, "
                           "created_at, updated_at) VALUES (%s,%s,'x',%s,'GENERAL',NULL,0,0,%s,%s,%s)",
                           (f"dddddddd-0000-4000-8000-{n:012d}", f"dash{n}@example.com", f"dash{n}", 1 if i % 5 == 0 else 0, at, at))
    # View logs: members at the 7 day edge (the original cuts at utcnow - 7 days), a member twice, guests (user NULL).
    now = datetime.utcnow()
    edge = now - timedelta(days=7)
    logs = [("dddddddd-0000-4000-8000-000000000001", now - timedelta(hours=1)), ("dddddddd-0000-4000-8000-000000000001", now - timedelta(days=2)),
            ("dddddddd-0000-4000-8000-000000000003", edge + timedelta(minutes=5)), ("dddddddd-0000-4000-8000-000000000005", edge - timedelta(minutes=5)),
            ("dddddddd-0000-4000-8000-000000000007", now - timedelta(days=30)), (base.SUBJECTS["alice"], now - timedelta(days=3)),
            (base.SUBJECTS["dave"], now - timedelta(days=1)), (None, now - timedelta(hours=2)), (None, now - timedelta(days=1))]
    for k, (user, at) in enumerate(logs):
        cursor.execute("INSERT INTO video_view_log (video_view_log_id, video_id, user_id, guest_token, created_at, updated_at) VALUES (%s,%s,%s,%s,%s,%s)",
                       (f"eeeeeeee-0000-4000-8000-{k + 100:012d}", "aaaaaaaa-0000-4000-8000-000000000001", user, None if user else f"guest-{k}", at, at))
    # Requests: PENDING, processed inside and outside the last 30 days, with gaps that are not whole minutes.
    cursor.execute("DELETE FROM video_request")
    requests = [("PENDING", now - timedelta(days=3), now - timedelta(days=3)), ("PENDING", now - timedelta(days=90), now - timedelta(days=90)),
                ("ACCEPTED", now - timedelta(days=5, minutes=7, seconds=45), now - timedelta(days=5)),
                ("REJECTED", now - timedelta(days=2, hours=1, seconds=59), now - timedelta(days=2)),
                ("ACCEPTED", now - timedelta(days=10, seconds=30), now - timedelta(days=10)),
                ("ACCEPTED", now - timedelta(days=29, hours=23), now - timedelta(days=29, hours=22)),
                ("REJECTED", now - timedelta(days=45, hours=3), now - timedelta(days=40)),
                ("ACCEPTED", now - timedelta(days=31, minutes=10), now - timedelta(days=30, hours=1))]
    # 230 requests processed inside the window (more than two pages of 100), gaps from 0 to 500 minutes and seconds that do not round.
    requests += [("ACCEPTED" if k % 3 else "REJECTED", now - timedelta(days=k % 28, hours=1, minutes=k * 2, seconds=k % 60), now - timedelta(days=k % 28, hours=1))
                 for k in range(230)]
    for k, (status, created, updated) in enumerate(requests):
        cursor.execute("INSERT INTO video_request (video_request_id, user_id, youtube_url, youtube_full_url, status, admin_comment, created_at, updated_at) "
                       "VALUES (%s,%s,%s,%s,%s,NULL,%s,%s)",
                       (f"ffffffff-0000-4000-8000-{k + 100:012d}", base.SUBJECTS["alice"], f"dashreq{k:04d}", f"https://www.youtube.com/watch?v=dashreq{k:04d}",
                        status, created, updated))
    # Videos: every genre but two, deleted ones, zero views, large sums.
    for k, genre in enumerate(GENRES[:14] + GENRES[:3]):
        for j in range(3):
            cursor.execute("INSERT INTO video (video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted, created_at, updated_at) "
                           "VALUES (%s,%s,%s,'c',%s,10,%s,%s,%s,%s)",
                           (f"bbbbbbbb-1111-4000-8000-{k * 10 + j:012d}", f"dashvid{k:03d}{j}", f"t{k}{j}", genre, [0, 5, 4000000000 + k][j] if k % 7 == 0 else (k + 1) * (j + 1) * 100,
                            1 if (k + j) % 6 == 5 else 0, now, now))


def seed_distributions(mongo_url):
    """Documents of video_distribution around the ones base.seed_mongo made: frames or none, missing fields, every dominant emotion."""
    collection = pymongo.MongoClient(mongo_url)[base.MONGO_DB]["video_distribution"]
    collection.delete_many({})
    emotions = ["neutral", "happy", "surprise", "sad", "angry"]
    documents = []
    for k in range(24):
        documents.append({"video_id": f"bbbbbbbb-1111-4000-8000-{k:012d}", "total_frames": [0, 120, 4, 9000][k % 4],
                          "average_completion_rate": [0.1234567, 0.5, 0.987654321, 1.0][k % 4] * (1 + k / 100),
                          "dominant_emotion": emotions[k % 5] if k % 7 else "",
                          "emotion_averages": {name: round(((k + j * 3) % 10) / 10.0 / 3, 6) for j, name in enumerate(emotions)}})
    documents.append({"video_id": "bbbbbbbb-1111-4000-8000-000000009999", "total_frames": 50, "dominant_emotion": "happy"})  # no averages at all
    documents.append({"video_id": "bbbbbbbb-1111-4000-8000-000000009998", "total_frames": 50, "average_completion_rate": "bad",
                      "emotion_averages": {"happy": None}, "dominant_emotion": "sad"})
    collection.insert_many([dict(document) for document in documents])
    return documents


def main():
    admin_url, mongo_url = base.need("S14H_MYSQL_ADMIN_URL"), base.need("S14H_MONGO_URL")
    base.check_endpoint(admin_url, "S14H_MYSQL_ADMIN_URL")
    base.check_endpoint(mongo_url, "S14H_MONGO_URL")
    work = Path(base.need("S14H_WORK_DIR"))
    if any(work.iterdir()):
        sys.exit("S14H_WORK_DIR must be empty")
    base.seed_mysql(admin_url)
    base.seed_mongo(mongo_url)
    app, _, _, _ = base.load_original(base.need("S14H_ORIGINAL_ROOT"), admin_url, mongo_url)
    from app.services.admin_service import AdminService  # noqa: E402
    deployment = jobs.deploy(work, admin_url, mongo_url, "http://127.0.0.1:9/youtube/v3/videos", base.need("S14H_AIP_BIN"), enable_writes=True)
    connection = connect(admin_url)
    seed(connection, datetime.utcnow().date())
    distributions = seed_distributions(mongo_url)
    failures, count, golden, health_golden = [], 0, [], None

    def aggregate(name, value, subject=ADMIN):
        answer = deployment.request("/read", {"query": {"aggregate": name, "input": value}}, subject)
        return answer["rows"][0] if answer.get("ok") else answer

    def signup_count(low, high):
        return aggregate("User.signupCount", {"from": iso(low), "to": iso(high)})

    def midnight(day):
        return datetime(day.year, day.month, day.day)

    def compose_trend(period):
        """The buckets of AdminService.get_signup_trend, each counted by one signupCount call."""
        days, granularity = PERIODS.get(period, (7, "day"))
        today = datetime.utcnow().date()
        start = today - timedelta(days=days - 1)
        tomorrow = midnight(today + timedelta(days=1))
        points = []
        if granularity == "day":
            for i in range(days):
                day = start + timedelta(days=i)
                points.append((str(day), signup_count(midnight(day), midnight(day + timedelta(days=1)))))
        elif granularity == "week":
            cursor = start - timedelta(days=start.weekday())
            while cursor <= today:
                low, high = max(cursor, start), min(cursor + timedelta(days=7), today + timedelta(days=1))
                points.append((str(cursor), signup_count(midnight(low), midnight(high))))
                cursor += timedelta(days=7)
        else:
            cursor = start.replace(day=1)
            while cursor <= today:
                following = (cursor.replace(day=28) + timedelta(days=4)).replace(day=1)
                low, high = max(cursor, start), min(following, today + timedelta(days=1))
                points.append((str(cursor), signup_count(midnight(low), midnight(high))))
                cursor = following
        assert tomorrow
        return {"points": [{"date": d, "count": c} for d, c in points], "granularity": granularity}

    def case(name, want, got):
        nonlocal count
        count += 1
        ok = want == got
        golden.append({"name": name, "expected": want})
        print(f"{'PASS' if ok else 'FAIL'} {name}")
        if not ok:
            print("   original:", json.dumps(want, default=str)[:900])
            print("   aip     :", json.dumps(got, default=str)[:900])
            failures.append(name)

    try:
        for period in ("7d", "30d", "3m", "1y", "3y", "bogus"):
            with app.app_context():
                want = json.loads(json.dumps(AdminService.get_signup_trend(period)))
            case(f"signup trend {period}", want, compose_trend(period))
        # get_business_stats: the parts that read MySQL.
        with app.app_context():
            stats = json.loads(json.dumps(AdminService.get_business_stats(), default=str))
        case("business stats: the 7 day signup trend", stats["signup_trend"], compose_trend("7d")["points"])
        case("business stats: weekly active users", stats["weekly_active_users"], aggregate("User.weeklyActive", {"since": iso(datetime.utcnow() - timedelta(days=7))}))
        case("business stats: pending requests", stats["video_request_pipeline"]["pending_count"], aggregate("VideoRequest.pendingCount", {}))
        original_categories = {row["category"]: row["view_count"] for row in stats["content_health"]["category_top5"]}
        ported = {}
        for genre in GENRES:
            visible = deployment.request("/read", {"query": {"aggregate": "Video.totalCount", "input": {}, "filter": [{"field": "category", "op": "eq", "value": genre}]}}, ADMIN)
            if visible["rows"][0]:
                views = aggregate("Video.categoryViews", {"category": genre})
                if isinstance(views, dict):
                    sys.exit(f"Video.categoryViews {genre}: {views}")
                ported[genre] = int(views or 0)
        case("business stats: views per genre (genres without a visible video are absent)", original_categories, ported)
        case("business stats: the order of the genres is by views", [row["category"] for row in stats["content_health"]["category_top5"]],
             [genre for genre, _ in sorted(ported.items(), key=lambda item: -item[1])])
        # The pipeline and the content health (extensions); the original answers the same DTOs.
        piped = deployment.post("VideoRequest.pipeline", {}, ADMIN)
        case("business stats: the request pipeline (pending count and average minutes of 238 processed requests)", stats["video_request_pipeline"],
             json.loads(piped["output"]["body"]) if piped.get("ok") else piped)
        health = deployment.post("Video.contentHealth", {}, ADMIN)
        want_health = dict(stats["content_health"])
        want_health.pop("category_top5")
        want_health["dominant_emotion_video_counts"] = sorted(want_health["dominant_emotion_video_counts"], key=lambda row: (-row["video_count"], row["emotion"]))
        health_golden = {"documents": distributions, "expected": want_health}
        case("business stats: the content health of the distributions", want_health, json.loads(health["output"]["body"]) if health.get("ok") else health)
        for extension in ("VideoRequest.pipeline", "Video.contentHealth"):
            for subject in (MEMBER, None):
                count += 1
                answer = deployment.post(extension, {}, subject)
                if answer.get("ok"):
                    failures.append(f"denied {extension} {subject}")
                    print(f"FAIL {extension} answered {subject}", answer)
                else:
                    print(f"PASS {extension} refused for {subject or 'a guest'}: {answer.get('code')}")
        # Who may ask: a member is refused every aggregate, a guest too.
        for name, value in (("User.signupCount", {"from": iso(datetime.utcnow() - timedelta(days=1)), "to": iso(datetime.utcnow())}),
                            ("User.weeklyActive", {"since": iso(datetime.utcnow())}), ("VideoRequest.pendingCount", {}), ("Video.categoryViews", {"category": "vlog"})):
            for subject in (MEMBER, None):
                count += 1
                answer = deployment.request("/read", {"query": {"aggregate": name, "input": value}}, subject)
                if answer.get("ok"):
                    failures.append(f"denied {name} {subject}")
                    print(f"FAIL {name} answered {subject}", answer)
                else:
                    print(f"PASS {name} refused for {subject or 'a guest'}: {answer.get('code')}")
    finally:
        deployment.close()
        connection.close()
    pymongo.MongoClient(mongo_url).drop_database(base.MONGO_DB)
    if failures:
        sys.exit(f"FAILED {len(failures)} of {count}: " + ", ".join(failures))
    if os.environ.get("S14H_WRITE_GOLDEN"):
        (base.FIXTURE / "slice14h-health.json").write_text(json.dumps(health_golden, ensure_ascii=False, indent=1) + "\n")
    print(f"the dashboard matched the original in {count} cases")


if __name__ == "__main__":
    main()
