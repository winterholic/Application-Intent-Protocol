"""WBS 7.2.8: GET mypage/emotion/summary of the AIP server versus the ORIGINAL MypageService.get_emotion_summary.

  ORACLE  the pinned original's MypageService.get_emotion_summary(user_id), which creates the member's user_emotion_summary row, sums the
          sessions since the checkpoint and moves the row with an optimistic lock, on the same MySQL and MongoDB the AIP side uses.
  HTTP    POST /apply of the write extension User.myEmotionSummary to `aip service serve` (derived Mongo collection `user_watch_stats`
          command `emotion_seconds`, the transition UserEmotionSummary.applyDelta with `expect lock_version`).

Every case sets the member's summary row to a given state, runs the original, restores the state, runs the port and compares the answer
and the row afterwards (the totals, the checkpoint, the session id and lock_version; timestamps of "now" only for being recent). The
sessions cover the rules of both modes: the first call sums timelines (last frame of a second wins), the later calls sum the stored
emotion_seconds of the sessions finalized after the checkpoint (ties on finalized_at ordered by video_view_log_id) and fall back to
emotion_percentages times the estimated watch seconds (duration x completion, timeline_len, frame_count).

Required environment: S14F_MYSQL_ADMIN_URL, S14F_MONGO_URL, S14F_AIP_BIN, S14F_ORIGINAL_ROOT, S14F_WORK_DIR (empty). Run with the Python
that has flask, flask_sqlalchemy, pymysql, pymongo, isodate, requests, psutil, cryptography and PyJWT, and not from a directory that
holds a numbers.py.
"""
import json
import os
import sys
import threading
import time
import urllib.parse
import uuid
from datetime import datetime, timedelta
from pathlib import Path

import pymongo
import pymysql

sys.path.insert(0, str(Path(__file__).resolve().parent))
import slice13d_oracle as base  # noqa: E402
import slice14b_oracle as jobs  # noqa: E402

ALICE, BOB, CAROL, ERIN = (base.SUBJECTS[name] for name in ("alice", "bob", "carol", "erin"))
EMOTIONS = ["neutral", "happy", "surprise", "sad", "angry"]
COLUMNS = [f"{emotion}_seconds" for emotion in EMOTIONS]
DAY = datetime(2026, 3, 1, 10, 0, 0)
ERIN_SESSIONS = int(os.environ.get("S14F_ERIN_SESSIONS", "800"))


def shares(*values):
    return dict(zip(EMOTIONS, values))


def counts(*values):
    return dict(zip(EMOTIONS, values))


def doc(user, n, finalized, **fields):
    created = (finalized or DAY) - timedelta(minutes=10)
    result = {"user_id": user, "video_id": f"bbbbbbbb-0000-4000-8000-{n:012d}", "video_view_log_id": f"log-{user[:4]}-{n:04d}",
              "created_at": created, "emotion_percentages": shares(.2, .4, .1, .2, .1), "completion_rate": .5,
              "emotion_score_timeline": {"0": shares(50, 10, 10, 10, 20)}}
    if finalized is not None:
        result["finalized_at"] = finalized
    result.update(fields)
    return result


def sessions():
    at = lambda days, hours=0, seconds=0: DAY + timedelta(days=days, hours=hours, seconds=seconds)  # noqa: E731
    most = {"0": "happy", "50": "sad", "99": "angry", "100": "neutral", "150": "neutral", "200": "happy", "260": "bogus", "300": "surprise", "x": "sad"}
    docs = [
        doc(ALICE, 1, at(0), emotion_seconds=counts(5, 20, 3, 2, 1), most_emotion_timeline={"0": "happy"}, duration=100, completion_rate=.4),
        doc(ALICE, 2, at(0), emotion_seconds=counts(1, 1, 1, 1, 1), most_emotion_timeline={"0": "sad", "100": "sad"}),
        doc(ALICE, 3, at(1), most_emotion_timeline=most, duration=100, completion_rate=.5),
        doc(ALICE, 4, at(1, 1), duration=100, completion_rate=.5, frame_count=9),
        doc(ALICE, 5, at(2), frame_count=61, completion_rate=0.0),
        doc(ALICE, 6, at(2, 1), timeline_len=40, frame_count=10, completion_rate=0.0),
        doc(ALICE, 7, at(3), emotion_percentages=None, completion_rate=0.0),
        doc(ALICE, 8, at(3, 1), duration=40, completion_rate=1.5),
        doc(ALICE, 9, at(4), duration="abc", completion_rate=.5, frame_count=21),
        doc(ALICE, 10, None, emotion_seconds=counts(9, 9, 9, 9, 9), emotion_percentages={}, completion_rate=0.0),
        doc(ALICE, 11, at(5), emotion_seconds={"neutral": "7", "happy": "x", "sad": 2.9}, most_emotion_timeline={"0": "angry"}),
        doc(ALICE, 12, at(6), emotion_seconds=counts(0, 0, 0, 0, 0), most_emotion_timeline={}, duration=30, completion_rate=.3),
        # A member with many sessions: the totals come from stored seconds, so no session timeline reaches the extension.
        doc(CAROL, 1, at(0), emotion_seconds=counts(3, 4, 5, 6, 7)),
        # Checkpoints are stored in a DATETIME column without fractions: the original rounds, so does the port (equal rows below).
        doc(CAROL, 2, at(1) + timedelta(milliseconds=400), emotion_seconds=counts(1, 0, 0, 0, 0)),
        doc(CAROL, 3, at(1) + timedelta(milliseconds=600), emotion_seconds=counts(0, 1, 0, 0, 0)),
        doc(BOB, 1, at(0), emotion_seconds=counts(100, 100, 100, 100, 100)),
    ]
    docs += [doc(ERIN, n, at(0, 0, n), emotion_seconds=counts(n % 7, n % 5, n % 3, n % 11, n % 2), most_emotion_timeline={str(i * 100): EMOTIONS[(i + n) % 5] for i in range(30)},
                 duration=60, completion_rate=.5) for n in range(1, ERIN_SESSIONS + 1)]
    return docs


def seed_mongo(mongo_url):
    client = pymongo.MongoClient(mongo_url)
    client.drop_database(base.MONGO_DB)
    client[base.MONGO_DB]["youtube_watching_data"].insert_many(sessions())
    client.close()


def connect(admin_url):
    parsed = urllib.parse.urlparse(admin_url)
    return pymysql.connect(host=parsed.hostname, port=parsed.port, user=urllib.parse.unquote(parsed.username),
                           password=urllib.parse.unquote(parsed.password or ""), autocommit=True, charset="utf8mb4", database=base.SOURCE_DB)


def set_row(connection, user, state):
    cursor = connection.cursor()
    cursor.execute("DELETE FROM user_emotion_summary WHERE user_id = %s", (user,))
    if state is not None:
        cursor.execute(
            "INSERT INTO user_emotion_summary (user_id, neutral_seconds, happy_seconds, surprise_seconds, sad_seconds, angry_seconds, "
            "last_finalized_at, last_session_id, lock_version, created_at, updated_at) VALUES (%s,%s,%s,%s,%s,%s,%s,%s,%s,'2026-01-01 00:00:00','2026-01-01 00:00:00')",
            (user, *state["seconds"], state.get("at"), state.get("session"), state.get("version", 0)))


def get_row(connection, user):
    cursor = connection.cursor()
    cursor.execute("SELECT neutral_seconds, happy_seconds, surprise_seconds, sad_seconds, angry_seconds, last_finalized_at, last_session_id, lock_version, "
                   "created_at, updated_at FROM user_emotion_summary WHERE user_id = %s", (user,))
    row = cursor.fetchone()
    if row is None:
        return None
    return {"seconds": [int(value) for value in row[:5]], "at": row[5], "session": row[6], "version": int(row[7]), "created": row[8], "updated": row[9]}


def settled(row):
    """The row without the clock: a checkpoint of 'now' only has to be recent (the first call stamps the time of the request)."""
    if row is None:
        return None
    result = {key: row[key] for key in ("seconds", "session", "version")}
    now = datetime.utcnow()
    if row["at"] is not None and abs((now - row["at"]).total_seconds()) < 120:
        result["at"] = "recent"
    else:
        result["at"] = row["at"].isoformat() if row["at"] else None
    result["created"] = "recent" if row["created"] and abs((now - row["created"]).total_seconds()) < 120 else str(row["created"])
    result["updated"] = "touched" if row["updated"] and row["updated"] != datetime(2026, 1, 1) else "untouched"
    return result


def main():
    admin_url, mongo_url = base.need("S14F_MYSQL_ADMIN_URL"), base.need("S14F_MONGO_URL")
    base.check_endpoint(admin_url, "S14F_MYSQL_ADMIN_URL")
    base.check_endpoint(mongo_url, "S14F_MONGO_URL")
    work = Path(base.need("S14F_WORK_DIR"))
    if any(work.iterdir()):
        sys.exit("S14F_WORK_DIR must be empty")
    base.seed_mysql(admin_url)
    seed_mongo(mongo_url)
    app, _, _, _ = base.load_original(base.need("S14F_ORIGINAL_ROOT"), admin_url, mongo_url)
    from app.services.mypage_service import MypageService  # noqa: E402

    deployment = jobs.deploy(work, admin_url, mongo_url, "http://127.0.0.1:9/youtube/v3/videos", base.need("S14F_AIP_BIN"), enable_writes=True)
    connection = connect(admin_url)
    failures, count, golden = [], 0, []

    def port_call(subject):
        return deployment.request("/apply", {"key": str(uuid.uuid4()), "request": {"extension": "User.myEmotionSummary", "input": {}}}, subject)

    def case(name, subject, actor, state):
        nonlocal count
        count += 1
        set_row(connection, actor, state)
        with app.app_context():
            want = json.loads(json.dumps(MypageService.get_emotion_summary(actor)))
        want_row = settled(get_row(connection, actor))
        set_row(connection, actor, state)
        got = port_call(subject)
        port = json.loads(got["output"]["body"]) if got.get("ok") else None
        got_row = settled(get_row(connection, actor))
        ok = port == want and got_row == want_row
        golden.append({"name": name, "subject": subject, "state": None if state is None else {**state, "at": state["at"].isoformat() if state.get("at") else None},
                       "expected": want, "row": {**want_row, "at": want_row["at"]}})
        print(f"{'PASS' if ok else 'FAIL'} {name}")
        if not ok:
            print("   original:", json.dumps(want, ensure_ascii=False), want_row)
            print("   aip     :", json.dumps(port if port is not None else got, ensure_ascii=False, default=str), got_row)
            failures.append(name)
        return want_row

    try:
        d0 = DAY
        case("alice: no row, first call sums the timelines of every session", "alice", ALICE, None)
        case("alice: row without a checkpoint", "alice", ALICE, {"seconds": [10, 20, 30, 40, 50], "version": 4})
        case("alice: checkpoint at the first tie, the second session and later ones", "alice", ALICE,
             {"seconds": [10, 20, 30, 40, 50], "at": d0, "session": "log-1111-0001", "version": 7})
        case("alice: checkpoint before everything", "alice", ALICE, {"seconds": [1, 2, 3, 4, 5], "at": d0 - timedelta(days=1), "session": "", "version": 0})
        case("alice: checkpoint with a missing session id", "alice", ALICE, {"seconds": [1, 2, 3, 4, 5], "at": d0 - timedelta(seconds=1), "session": None, "version": 2})
        case("alice: checkpoint in the middle", "alice", ALICE, {"seconds": [0, 0, 0, 0, 0], "at": d0 + timedelta(days=2, hours=1), "session": "", "version": 9})
        case("alice: checkpoint after every session changes nothing", "alice", ALICE,
             {"seconds": [3, 1, 4, 1, 5], "at": d0 + timedelta(days=30), "session": "log", "version": 11})
        case("alice: checkpoint on the last session changes nothing", "alice", ALICE,
             {"seconds": [3, 1, 4, 1, 5], "at": d0 + timedelta(days=6), "session": "log-1111-0012", "version": 12})
        case("bob: no row and one session", "bob", BOB, None)
        case("carol: no row", "carol", CAROL, None)
        case("carol: checkpoint before sessions finalized at fractions of a second", "carol", CAROL,
             {"seconds": [0, 0, 0, 0, 0], "at": d0 + timedelta(hours=-1), "session": "", "version": 0})
        case("carol: checkpoint on the first fraction", "carol", CAROL,
             {"seconds": [0, 0, 0, 0, 0], "at": d0 + timedelta(days=1, milliseconds=400), "session": "log-3333-0002", "version": 0})
        case("erin: many sessions, no row", "erin", ERIN, None)
        half = ERIN_SESSIONS // 2
        case("erin: many sessions after a checkpoint in the middle", "erin", ERIN,
             {"seconds": [5, 5, 5, 5, 5], "at": d0 + timedelta(seconds=half), "session": f"log-5555-{half:04d}", "version": 1})
        # A member with no session at all gets a zero row.
        case("dave: no sessions", "dave", base.SUBJECTS["dave"], None)

        # A second call right after the first changes nothing (the checkpoint is "now" and no session is newer).
        set_row(connection, ALICE, None)
        first = port_call("alice")
        row1 = get_row(connection, ALICE)
        second = port_call("alice")
        row2 = get_row(connection, ALICE)
        count += 1
        if not (first.get("ok") and second.get("ok") and first["output"] == second["output"] and row1["version"] == 1 and row2["version"] == 1):
            failures.append("repeat")
            print("FAIL a repeated call changed the row", first, second, row1, row2)
        else:
            print("PASS a repeated call changes nothing")

        # A new session finalized after the checkpoint is added once, and only once.
        late = doc(ALICE, 50, datetime.utcnow() + timedelta(seconds=3), emotion_seconds=counts(10, 0, 0, 0, 0))
        pymongo.MongoClient(mongo_url)[base.MONGO_DB]["youtube_watching_data"].insert_one(late)
        before = json.loads(second["output"]["body"])["emotion_seconds"]
        third = port_call("alice")
        fourth = port_call("alice")
        count += 1
        after = json.loads(third["output"]["body"])["emotion_seconds"] if third.get("ok") else None
        if not (after and after["neutral"] == before["neutral"] + 10 and fourth.get("ok") and fourth["output"] == third["output"]):
            failures.append("late")
            print("FAIL a late session was not added exactly once", before, third, fourth)
        else:
            print("PASS a late session is added exactly once")

        # Two concurrent calls with new sessions waiting: one wins the lock_version, the other is refused and nothing is added twice.
        set_row(connection, BOB, {"seconds": [0, 0, 0, 0, 0], "at": d0 - timedelta(days=1), "session": "", "version": 0})
        results = [None, None]

        def worker(i):
            results[i] = port_call("bob")

        threads = [threading.Thread(target=worker, args=(i,)) for i in range(2)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        row = get_row(connection, BOB)
        wins = [r for r in results if r.get("ok")]
        count += 1
        if row["version"] not in (1, 2) or row["seconds"] != [100, 100, 100, 100, 100] or not wins:
            failures.append("concurrent")
            print("FAIL concurrent calls", results, row)
        else:
            print(f"PASS concurrent calls: {len(wins)} answered, lock_version {row['version']}, totals added once")
            print("   refused:", [r.get("error") or r.get("code") for r in results if not r.get("ok")])

        # A guest has no summary.
        guest = deployment.request("/apply", {"key": str(uuid.uuid4()), "request": {"extension": "User.myEmotionSummary", "input": {}}}, None)
        count += 1
        if guest.get("ok"):
            failures.append("guest")
            print("FAIL a guest got a summary")
        else:
            print("PASS a guest is refused")
    finally:
        deployment.close()
        connection.close()
    pymongo.MongoClient(mongo_url).drop_database(base.MONGO_DB)
    if failures:
        sys.exit(f"FAILED {len(failures)} of {count}: " + ", ".join(failures))
    if os.environ.get("S14F_WRITE_GOLDEN"):
        def extended(value):
            if isinstance(value, datetime):
                delta = value - datetime(1970, 1, 1)
                return {"$date": {"$numberLong": str(delta.days * 86_400_000 + delta.seconds * 1000 + delta.microseconds // 1000)}}
            raise TypeError(type(value))
        (base.FIXTURE / "slice14f-golden.json").write_text(json.dumps(golden, ensure_ascii=False, indent=1) + "\n")
        (base.FIXTURE / "slice14f-sessions.json").write_text(json.dumps(sessions(), default=extended, ensure_ascii=False, indent=1) + "\n")
    print(f"the emotion summary matched the original in {count} cases")


if __name__ == "__main__":
    main()
