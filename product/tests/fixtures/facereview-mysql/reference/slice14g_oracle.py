"""WBS 7.2.9: POST home/video/recommend of the AIP server versus the ORIGINAL HomeService.create_user_video_recommend.

  ORACLE  the pinned original's HomeService.create_user_video_recommend(user_id, youtube_url_list): rejects the whole list (BusinessError
          V004) when any url was requested before by anyone, otherwise inserts one PENDING video_request per url with the full YouTube url.
  HTTP    POST /apply of the write extension VideoRequest.recommend to `aip service serve` (the exposed create VideoRequest, the database
          computed youtube_full_url and the unique youtube_url of db-defaults.sql).

Every case sets the video_request table to a given state, runs the original, restores the state, runs the port and compares the outcome
(created or refused as a duplicate) and the table afterwards (user, id, full url, status, comment; the clocks only for being recent).
The differences the port documents on purpose (the same id twice in one list, more than 20 ids, an id longer than the column) are run
on both sides and printed as DIFF, not counted as failures.

Required environment: S14G_MYSQL_ADMIN_URL, S14G_MONGO_URL, S14G_AIP_BIN, S14G_ORIGINAL_ROOT, S14G_WORK_DIR (empty). Optional:
S14G_WRITE_GOLDEN=1 writes slice14g-golden.json. Run with the Python that has flask, flask_sqlalchemy, pymysql, pymongo, isodate, requests,
psutil, cryptography and PyJWT, and not from a directory that holds a numbers.py.
"""
import json
import os
import sys
import threading
import urllib.parse
import uuid
from datetime import datetime
from pathlib import Path

import pymongo
import pymysql

sys.path.insert(0, str(Path(__file__).resolve().parent))
import slice13d_oracle as base  # noqa: E402
import slice14b_oracle as jobs  # noqa: E402

ALICE, BOB, CAROL = (base.SUBJECTS[name] for name in ("alice", "bob", "carol"))
FULL = "https://www.youtube.com/watch?v="
INDEX = "uk_video_request_youtube_url"


def connect(admin_url):
    parsed = urllib.parse.urlparse(admin_url)
    return pymysql.connect(host=parsed.hostname, port=parsed.port, user=urllib.parse.unquote(parsed.username),
                           password=urllib.parse.unquote(parsed.password or ""), autocommit=True, charset="utf8mb4", database=base.SOURCE_DB)


def set_rows(connection, rows):
    """The video_request table holds exactly these (user, youtube_url, status) rows."""
    cursor = connection.cursor()
    cursor.execute("DELETE FROM video_request")
    for n, (user, url, status) in enumerate(rows):
        cursor.execute(
            "INSERT INTO video_request (video_request_id, user_id, youtube_url, youtube_full_url, status, admin_comment, created_at, updated_at) "
            "VALUES (%s,%s,%s,%s,%s,NULL,'2026-01-01 00:00:00','2026-01-01 00:00:00')",
            (f"00000000-0000-4000-8000-{n:012d}", user, url, FULL + url, status))


def table(connection):
    """The rows without ids; a row made by the call has a created_at that is recent and equals its updated_at ('new')."""
    cursor = connection.cursor()
    cursor.execute("SELECT user_id, youtube_url, youtube_full_url, status, admin_comment, created_at = updated_at "
                   "AND ABS(TIMESTAMPDIFF(SECOND, created_at, UTC_TIMESTAMP())) < 120 FROM video_request ORDER BY youtube_url, user_id")
    return [[user, url, full, status, comment, "new" if fresh else "old"] for user, url, full, status, comment, fresh in cursor.fetchall()]


def main():
    admin_url, mongo_url = base.need("S14G_MYSQL_ADMIN_URL"), base.need("S14G_MONGO_URL")
    base.check_endpoint(admin_url, "S14G_MYSQL_ADMIN_URL")
    base.check_endpoint(mongo_url, "S14G_MONGO_URL")
    work = Path(base.need("S14G_WORK_DIR"))
    if any(work.iterdir()):
        sys.exit("S14G_WORK_DIR must be empty")
    base.seed_mysql(admin_url)
    base.seed_mongo(mongo_url)
    app, home, _, business_error = base.load_original(base.need("S14G_ORIGINAL_ROOT"), admin_url, mongo_url)
    deployment = jobs.deploy(work, admin_url, mongo_url, "http://127.0.0.1:9/youtube/v3/videos", base.need("S14G_AIP_BIN"), enable_writes=True)
    connection = connect(admin_url)
    failures, golden, count = [], [], 0

    def original(actor, urls):
        with app.app_context():
            try:
                home.create_user_video_recommend(actor, urls)
                return "created"
            except business_error as error:
                return f"refused:{error}"
            except Exception as error:  # noqa: BLE001 the original's own database error, e.g. a value longer than the column
                from common.extensions import db
                db.session.rollback()
                return f"error:{type(error).__name__}"

    def port(subject, actor, urls, key=None):
        answer = deployment.request("/apply", {"key": key or str(uuid.uuid4()), "request": {
            "extension": "VideoRequest.recommend", "input": {"user": actor, "youtube_url_list": json.dumps(urls)}}}, subject)
        if answer.get("ok"):
            return "created", answer
        return ("refused:duplicate" if answer.get("code") == "WIRE.ALREADY_EXISTS" else f"refused:{answer.get('code')}"), answer

    def case(name, subject, before, urls):
        nonlocal count
        count += 1
        actor = base.SUBJECTS[subject]
        set_rows(connection, before)
        want_outcome = original(actor, urls)
        want_table = table(connection)
        set_rows(connection, before)
        got_outcome, answer = port(subject, actor, urls)
        got_table = table(connection)
        # The original's V004 and the port's ALREADY_EXISTS are the same refusal.
        same_outcome = want_outcome == got_outcome or (want_outcome.startswith("refused:") and got_outcome == "refused:duplicate")
        ok = same_outcome and want_table == got_table
        golden.append({"name": name, "subject": subject, "before": [list(row) for row in before], "urls": urls,
                       "expected": {"created": want_outcome == "created", "table": want_table}})
        print(f"{'PASS' if ok else 'FAIL'} {name}")
        if not ok:
            print("   original:", want_outcome, want_table)
            print("   aip     :", got_outcome, got_table, answer)
            failures.append(name)

    try:
        case("one new id", "alice", [], ["abc12345678"])
        case("three new ids keep the list order out of the table", "alice", [], ["zzz", "aaa", "mmm"])
        case("an id the member requested before is refused", "alice", [(ALICE, "dupe0000001", "PENDING")], ["dupe0000001"])
        case("an id another member requested before is refused", "alice", [(BOB, "dupe0000002", "PENDING")], ["dupe0000002"])
        case("an id whose request was accepted is refused", "alice", [(BOB, "dupe0000003", "ACCEPTED")], ["dupe0000003"])
        case("an id whose request was rejected is refused", "alice", [(CAROL, "dupe0000004", "REJECTED")], ["dupe0000004"])
        case("new ids before a duplicate are not kept", "alice", [(BOB, "taken000001", "PENDING")], ["fresh000001", "fresh000002", "taken000001"])
        case("a duplicate before new ids refuses them too", "alice", [(BOB, "taken000002", "PENDING")], ["taken000002", "fresh000003"])
        case("unrelated requests are left alone", "bob", [(ALICE, "other000001", "PENDING"), (CAROL, "other000002", "REJECTED")], ["mine0000001"])
        case("an id of an existing video that nobody requested is accepted", "alice", [], ["vid00000001"])
        case("a text with spaces and symbols is kept verbatim", "alice", [], ["a b&c=d?e#f"])
        case("an id of the column length (50)", "carol", [], ["x" * 50])
        case("an id differing only by case is another id", "alice", [(BOB, "CaseId00001", "PENDING")], ["caseid00001"])
        case("twenty ids", "bob", [], [f"batch{n:06d}" for n in range(20)])

        # --- differences kept on purpose: both sides run, the result is printed ---
        def diff(name, subject, before, urls):
            actor = base.SUBJECTS[subject]
            set_rows(connection, before)
            cursor = connection.cursor()
            cursor.execute(f"ALTER TABLE video_request DROP INDEX {INDEX}")  # the original's own schema has no unique key
            try:
                want = original(actor, urls)
                want_rows = len(table(connection))
            finally:
                cursor.execute("DELETE FROM video_request")
                cursor.execute(f"ALTER TABLE video_request ADD UNIQUE KEY {INDEX} (youtube_url)")
            set_rows(connection, before)
            got, _ = port(subject, actor, urls)
            print(f"DIFF {name}: original {want} with {want_rows} rows, aip {got} with {len(table(connection))} rows")

        diff("the same id twice in one list", "alice", [], ["twice000001", "twice000001"])
        diff("twenty-one ids", "bob", [], [f"batch{n:06d}" for n in range(21)])
        diff("an id longer than the column (51)", "carol", [], ["y" * 51])

        # --- the port's own guards (the original has no such input) ---
        for name, payload in (("an empty list", "[]"), ("text that is not JSON", "abc"), ("an object", "{}"), ("a number in the list", "[1]"),
                              ("an empty id", "[\"\"]")):
            count += 1
            set_rows(connection, [])
            answer = deployment.request("/apply", {"key": str(uuid.uuid4()), "request": {
                "extension": "VideoRequest.recommend", "input": {"user": ALICE, "youtube_url_list": payload}}}, "alice")
            if answer.get("ok") or table(connection):
                failures.append(name)
                print("FAIL", name, answer)
            else:
                print(f"PASS refused: {name}")

        # --- owner and caller ---
        count += 1
        set_rows(connection, [])
        wrong = port("alice", BOB, ["owner000001"])
        if wrong[0] == "created" or table(connection):
            failures.append("owner")
            print("FAIL a member created a request for another member", wrong)
        else:
            print("PASS another member's id is refused:", wrong[1].get("code"))
        count += 1
        guest = deployment.request("/apply", {"key": str(uuid.uuid4()), "request": {
            "extension": "VideoRequest.recommend", "input": {"user": ALICE, "youtube_url_list": "[\"guest000001\"]"}}}, None)
        if guest.get("ok") or table(connection):
            failures.append("guest")
            print("FAIL a guest created a request", guest)
        else:
            print("PASS a guest is refused")

        # --- the same key twice: the second call replays the first and creates nothing ---
        count += 1
        set_rows(connection, [])
        key = str(uuid.uuid4())
        first, second = port("alice", ALICE, ["retry000001", "retry000002"], key), port("alice", ALICE, ["retry000001", "retry000002"], key)
        if first[0] != "created" or second[0] != "created" or first[1]["output"] != second[1]["output"] or len(table(connection)) != 2:
            failures.append("retry")
            print("FAIL a retry with the same key", first, second, table(connection))
        else:
            print("PASS a retry with the same key creates nothing more")

        # --- two members ask for overlapping lists at the same moment: every id has exactly one request ---
        count += 1
        outcomes = []
        for round_ in range(10):
            set_rows(connection, [])
            results = [None, None]
            lists = [[f"r{round_}-a", f"r{round_}-b", f"r{round_}-shared"], [f"r{round_}-shared", f"r{round_}-c", f"r{round_}-d"]]

            def run(i, subject):
                results[i] = port(subject, base.SUBJECTS[subject], lists[i])[0]

            threads = [threading.Thread(target=run, args=(0, "alice")), threading.Thread(target=run, args=(1, "bob"))]
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join()
            rows = table(connection)
            urls = sorted(row[1] for row in rows)
            winners = [i for i, outcome in enumerate(results) if outcome == "created"]
            if len(winners) != 1 or urls != sorted(lists[winners[0]]) or len(set(urls)) != len(urls):
                failures.append(f"concurrent-{round_}")
                print("FAIL concurrent lists", round_, results, rows)
                break
            outcomes.append(tuple(results))
        else:
            print(f"PASS 10 rounds of overlapping lists at the same moment: one list won each time, the loser left nothing ({sorted(set(outcomes))})")
    finally:
        deployment.close()
        connection.close()
    pymongo.MongoClient(mongo_url).drop_database(base.MONGO_DB)
    if failures:
        sys.exit(f"FAILED {len(failures)} of {count}: " + ", ".join(failures))
    if os.environ.get("S14G_WRITE_GOLDEN"):
        (base.FIXTURE / "slice14g-golden.json").write_text(json.dumps(golden, ensure_ascii=False, indent=1) + "\n")
    print(f"the video request batch matched the original in {count} cases")


if __name__ == "__main__":
    main()
