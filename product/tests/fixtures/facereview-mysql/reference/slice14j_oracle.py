"""WBS 7.2.9: the administrator's test data of the AIP server versus the ORIGINAL AdminService.generate_dummy_data.

  ORACLE  the pinned original's AdminService.generate_dummy_data(user_id, count) and its frame generator, seeded with Python's `random.seed`.
  HTTP    POST /jobs/enqueue of generateDummyData to `aip service serve` (workers.mongo with the write collection youtube_watching_data),
          as the administrator.

Two parts. (1) The frame generator, the session statistics and the distribution rules are called in one process with the same seed on both
sides and must give the same frames. (2) A run of the job on a database, then the same database again with the original seeded with the
job's run id (the job draws every random number from random.Random(run id) in the original's order): the chosen videos, the session
documents, the view log rows, the views and the recomputed distributions must be equal. Times that come from the clock are compared
with the gap between the two runs allowed.

Required environment: S14J_MYSQL_ADMIN_URL, S14J_MONGO_URL, S14J_AIP_BIN, S14J_ORIGINAL_ROOT, S14J_WORK_DIR (empty). Run with the Python
that has flask, flask_sqlalchemy, pymysql, pymongo, isodate, requests, psutil, cryptography and PyJWT, and not from a directory that holds a
numbers.py.
"""
import importlib.util
import json
import os
import random
import sys
import time
import urllib.parse
from datetime import datetime
from pathlib import Path

import pymongo
import pymysql

sys.path.insert(0, str(Path(__file__).resolve().parent))
import slice13d_oracle as base  # noqa: E402
import slice14b_oracle as jobs  # noqa: E402

ADMIN, MEMBER = "bob", "alice"
GENRES = ["drama", "eating", "travel", "cook", "show", "information", "game", "sports", "music", "animal", "beauty", "comedy", "horror",
          "exercise", "vlog", "etc"]


def connect(admin_url):
    parsed = urllib.parse.urlparse(admin_url)
    return pymysql.connect(host=parsed.hostname, port=parsed.port, user=urllib.parse.unquote(parsed.username),
                           password=urllib.parse.unquote(parsed.password or ""), autocommit=True, charset="utf8mb4", database=base.SOURCE_DB)


def load_port():
    spec = importlib.util.spec_from_file_location("slice14j_dummy", base.FIXTURE / "extensions" / "slice14j_dummy.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def seed_videos(connection):
    """36 videos of every genre, 20 to 90 seconds long, a deleted one and one without a length. (A video over two hours is left out: the port
    skips it before it draws its frames, so the two generators would part there. The live test covers it.)"""
    cursor = connection.cursor()
    now = datetime.utcnow()
    rows = [(f"cccccccc-0000-4000-8000-{n:012d}", f"dummyvid{n:04d}", GENRES[n % 16], 20 + (n * 7) % 71, 0, now)
            for n in range(36 + int(os.environ.get("S14J_EXTRA_VIDEOS", "0")))]
    rows += [("cccccccc-9999-4000-8000-000000000001", "dummydel0001", "vlog", 60, 1, now),          # deleted
             ("cccccccc-9999-4000-8000-000000000002", "dummyzero001", "game", 0, 0, now)]           # no length
    if os.environ.get("S14J_LONG"):
        rows.append(("cccccccc-9999-4000-8000-000000000003", "dummylong002", "cook", int(os.environ["S14J_LONG"]), 0, now))
    for video_id, url, genre, duration, deleted, at in rows:
        cursor.execute("INSERT INTO video (video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted, created_at, updated_at) "
                       "VALUES (%s,%s,%s,'c',%s,%s,%s,%s,%s,%s)", (video_id, url, f"dummy {url}", genre, duration, 10, deleted, at, at))


def main():
    admin_url, mongo_url = base.need("S14J_MYSQL_ADMIN_URL"), base.need("S14J_MONGO_URL")
    base.check_endpoint(admin_url, "S14J_MYSQL_ADMIN_URL")
    base.check_endpoint(mongo_url, "S14J_MONGO_URL")
    work = Path(base.need("S14J_WORK_DIR"))
    if any(work.iterdir()):
        sys.exit("S14J_WORK_DIR must be empty")
    base.seed_mysql(admin_url)
    base.seed_mongo(mongo_url)
    # The sessions of the shared seed have no video_view_log_id, which the unique index of the write grant refuses; the test needs none of them.
    pymongo.MongoClient(mongo_url)[base.MONGO_DB]["youtube_watching_data"].delete_many({})
    app, _, _, _ = base.load_original(base.need("S14J_ORIGINAL_ROOT"), admin_url, mongo_url)
    import app.services.admin_service as admin_module  # noqa: E402
    port = load_port()
    failures, count = [], 0

    def check(name, ok, detail=""):
        nonlocal count
        count += 1
        print(f"{'PASS' if ok else 'FAIL'} {name}")
        if not ok:
            print("  ", detail)
            failures.append(name)

    # --- (1) the generator, from one seed ---
    for seed, category, duration in ((1, "comedy", 40), (2, "horror", 90), (3, "unknown-genre", 25), (4, "drama", 1), (5, "etc", 120), (6, "sports", 61)):
        random.seed(seed)
        frames = admin_module._generate_session_frames(category, duration)
        stats = admin_module._compute_session_stats(frames)
        after_original = random.random()
        rng = random.Random(seed)
        mine = port.generate_session_frames(rng, category, duration)
        mine_stats = port.compute_session_stats(mine)
        check(f"frames and statistics of {category} {duration}s (seed {seed}) equal the original's, and the generators end alike",
              frames == mine and stats == mine_stats and after_original == rng.random(), f"{len(frames)} {len(mine)}")

    # --- (2) the job against the original on the same rows ---
    deployment = jobs.deploy(work, admin_url, mongo_url, "http://127.0.0.1:9/youtube/v3/videos", base.need("S14J_AIP_BIN"), enable_writes=True,
                             write_collections=["youtube_watching_data"])
    connection = connect(admin_url)
    seed_videos(connection)
    mongo = pymongo.MongoClient(mongo_url)[base.MONGO_DB]
    admin_id = base.SUBJECTS[ADMIN]
    cursor = connection.cursor()

    cursor.execute("SELECT video_view_log_id FROM video_view_log WHERE user_id = %s", (admin_id,))
    old_logs = {row[0] for row in cursor.fetchall()}

    def snapshot():
        cursor.execute("SELECT video_id, view_count FROM video ORDER BY video_id")
        views = dict(cursor.fetchall())
        distributions = {d["video_id"]: d for d in mongo["video_distribution"].find({}, {"_id": 0})}
        return views, distributions

    def reset(before):
        views, distributions = before
        mongo["youtube_watching_data"].delete_many({"user_id": admin_id})
        cursor.execute("SELECT video_view_log_id FROM video_view_log WHERE user_id = %s", (admin_id,))
        for (log_id,) in [row for row in cursor.fetchall() if row[0] not in old_logs]:
            cursor.execute("DELETE FROM video_view_log WHERE video_view_log_id = %s", (log_id,))
        for video_id, view_count in views.items():
            cursor.execute("UPDATE video SET view_count = %s WHERE video_id = %s", (view_count, video_id))
        mongo["video_distribution"].delete_many({})
        if distributions:
            mongo["video_distribution"].insert_many(list(distributions.values()))

    def outcome():
        sessions = {d["video_id"]: d for d in mongo["youtube_watching_data"].find({"user_id": admin_id}, {"_id": 0})}
        cursor.execute("SELECT video_id, user_id, created_at, updated_at, video_view_log_id FROM video_view_log WHERE user_id = %s", (admin_id,))
        logs = {row[0]: row for row in cursor.fetchall() if row[4] not in old_logs}
        views, distributions = snapshot()
        return sessions, logs, views, distributions

    try:
        before = snapshot()
        random_member = deployment.request("/jobs/enqueue", {"operation": "generateDummyData", "key": "dummy-member",
                                                             "input": {"user": base.SUBJECTS[MEMBER], "count": 10}, "delayMs": 0}, MEMBER)
        check("a member cannot start the job", not random_member.get("ok"), random_member)
        other = deployment.request("/jobs/enqueue", {"operation": "generateDummyData", "key": "dummy-other",
                                                     "input": {"user": base.SUBJECTS[MEMBER], "count": 10}, "delayMs": 0}, ADMIN)
        check("an administrator cannot start it for another member's id", not other.get("ok"), other)
        for bad in (9, 301):
            refused = deployment.request("/jobs/enqueue", {"operation": "generateDummyData", "key": f"dummy-count-{bad}",
                                                           "input": {"user": admin_id, "count": bad}, "delayMs": 0}, ADMIN)
            check(f"count {bad} is outside 10 to 300", not refused.get("ok"), refused)
        wanted = int(os.environ.get("S14J_WANTED", "40"))
        cursor.execute("SELECT COUNT(*) FROM video WHERE is_deleted = 0 AND duration > 0")
        pool = cursor.fetchone()[0]
        started = datetime.utcnow()
        accepted = deployment.request("/jobs/enqueue", {"operation": "generateDummyData", "key": "dummy-1", "input": {"user": admin_id, "count": wanted}, "delayMs": 0}, ADMIN)
        if not accepted.get("ok"):
            sys.exit(f"enqueue failed: {accepted}")
        run_id = accepted["run"]["id"]
        for _ in range(900):
            status = deployment.request("/jobs/status", {"id": run_id}, ADMIN)["run"]
            if status["state"] in ("SUCCEEDED", "FAILED", "CANCELLED"):
                break
            time.sleep(0.2)
        check("the job succeeds", status["state"] == "SUCCEEDED", status)
        result = deployment.request("/jobs/result", {"id": run_id}, ADMIN)["output"]
        port_ids = json.loads(result["video_ids"])
        port_state = outcome()
        reset(before)
        random.seed(run_id)
        with app.app_context():
            original = admin_module.AdminService.generate_dummy_data(admin_id, wanted)
        original_time = datetime.utcnow()
        original_state = outcome()
        gap = (original_time - started).total_seconds()

        check("created_count equals the original's", result["created_count"] == original["created_count"] == len(port_ids),
              f"{result['created_count']} {original['created_count']} {len(port_ids)}")
        check("the same videos in the same order (never a deleted one or one without a length)",
              port_ids == original["video_ids"], f"{port_ids[:3]} {original['video_ids'][:3]}")
        check(f"as many videos as the pool allows ({pool} visible with a length, {wanted} wanted)", len(port_ids) == min(wanted, pool), len(port_ids))
        p_sessions, p_logs, p_views, p_dists = port_state
        o_sessions, o_logs, o_views, o_dists = original_state
        check("a session document for each video", set(p_sessions) == set(o_sessions) == set(port_ids), f"{len(p_sessions)} {len(o_sessions)}")
        same_docs, drift = True, []
        for video_id in port_ids:
            a, b = p_sessions[video_id], o_sessions[video_id]
            keys_equal = {k: v for k, v in a.items() if k not in ("video_view_log_id", "created_at")} == {k: v for k, v in b.items() if k not in ("video_view_log_id", "created_at")}
            offset = (b["created_at"] - a["created_at"]).total_seconds()
            if not keys_equal or not (-1 <= offset <= gap + 2):
                same_docs = False
                drift.append((video_id, keys_equal, offset))
        check("the session documents equal the original's field by field (timelines, percentages, completion, empty seconds, no client info); "
              f"created_at differs by the gap of the runs only ({gap:.1f}s)", same_docs, drift[:3])
        check("one view log row for each video, of the administrator, at the session's time", set(p_logs) == set(o_logs) == set(port_ids) and
              all(row[1] == admin_id and row[2] == row[3] and abs((row[2] - datetime.utcfromtimestamp(0)).total_seconds() -
                  (p_sessions[row[0]]["created_at"] - datetime.utcfromtimestamp(0)).total_seconds()) <= 1 for row in p_logs.values()),
              f"{len(p_logs)} {len(o_logs)}")
        check("every chosen video has one more view, the others none", p_views == o_views,
              [(k, p_views[k], o_views[k]) for k in p_views if p_views[k] != o_views.get(k)][:3])

        def strip(distribution):
            return {k: v for k, v in distribution.items() if k not in ("created_at", "updated_at")}
        check("the distributions equal the original's recomputation", {k: strip(v) for k, v in p_dists.items()} == {k: strip(v) for k, v in o_dists.items()},
              [k for k in p_dists if strip(p_dists[k]) != strip(o_dists.get(k, {}))][:3])
        again = deployment.request("/jobs/enqueue", {"operation": "generateDummyData", "key": "dummy-1", "input": {"user": admin_id, "count": wanted}, "delayMs": 0}, ADMIN)
        check("the same key is the same run (nothing is generated twice)", again["run"]["id"] == run_id, again)
    finally:
        deployment.close()
        connection.close()
    pymongo.MongoClient(mongo_url).drop_database(base.MONGO_DB)
    if failures:
        sys.exit(f"FAILED {len(failures)} of {count}: " + ", ".join(failures))
    print(f"the test data matched the original in {count} cases")


if __name__ == "__main__":
    main()
