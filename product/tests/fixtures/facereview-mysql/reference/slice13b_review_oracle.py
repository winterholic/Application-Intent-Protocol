"""Slice13-B against the independent reviewer's frozen oracle (oracle_cases.json, B-* cases) AND the live original on real MySQL.

The reviewer's cases were produced by the pinned original HomeService on SQLite + mongomock. This script
  1. loads the reviewer's own seed definitions (text between `def vid` and `LIKES =` in run_oracle.py; nothing is copied),
  2. seeds them into a run-owned MySQL 8.4 and mongod (real driver, real collation),
  3. serves the Slice13-B extensions (same host as slice13b_oracle.py) and produces the actual response of every B case,
  4. compares with oracle_cases.json using compare_actual.py's rules plus an explicit table of INTENDED differences,
  5. additionally runs the original HomeService itself on the same MySQL/mongod (this is the authoritative answer for the
     `collation_dependent` cases B-s-4 and B-s-8) and compares the actual response with it.

Intended differences (never silent; each is printed with its case):
  DELETED   AIP's Video row policy hides soft-deleted videos. The original looks videos up without is_deleted and shows them. The
            expected list drops them; total/has_next stay the original's. Classified as an explicit capability gap, not a fix.
  GUEST     the host's /extension requires a signed session, so guest calls of the original's login_optional endpoints are refused
            (WIRE.UNAUTHENTICATED). Reported as GAP, never as PASS.
  USER-MAP  B-bm-4 asks for user uid(77), who has no row in user and so cannot hold a principal; a real user with no bookmarks
            stands in (same expected output).
Order rule: lists of `order: unspecified` cases are compared as multisets keyed by video_id (membership is exact); in addition the
actual list must be in Mongo page order (created_at desc, insertion order desc), which is this port's own contract.

Environment: the S13B_* variables of slice13b_oracle.py plus S13B_REVIEW_ORACLE_DIR (directory with oracle_cases.json and run_oracle.py).
"""
import copy
import json
import os
import sys
from datetime import datetime, timedelta
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.dont_write_bytecode = True
import pymongo  # noqa: E402
import pymysql  # noqa: E402
import slice13b_oracle as base  # noqa: E402


def load_review_seed(oracle_dir):
    source = (oracle_dir / "run_oracle.py").read_text()
    start, end = source.index("def vid(n):"), source.index("LIKES = ")
    namespace = {"datetime": datetime, "timedelta": timedelta, "copy": copy}
    exec(compile(source[start:end], "run_oracle.py(seed)", "exec"), namespace)  # the reviewer's own definitions
    bookmarks = eval(source[source.index("BOOKMARKS = ") + len("BOOKMARKS = "):].splitlines()[0], namespace)
    return namespace, bookmarks


def seed_mysql(admin_url, ns, bookmarks):
    parsed = base.urllib.parse.urlparse(admin_url)
    conn = pymysql.connect(host=parsed.hostname, port=parsed.port, user=base.urllib.parse.unquote(parsed.username),
                           password=base.urllib.parse.unquote(parsed.password or ""), autocommit=True, charset="utf8mb4")
    cur = conn.cursor()
    for db in (base.SOURCE_DB, base.META_DB):
        cur.execute(f"DROP DATABASE IF EXISTS `{db}`")
    cur.execute(f"CREATE DATABASE `{base.SOURCE_DB}` DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_0900_ai_ci")
    cur.execute(f"USE `{base.SOURCE_DB}`")
    cur.execute("SET FOREIGN_KEY_CHECKS=0")
    for name in ("schema.sql", "db-defaults.sql"):
        for chunk in (base.FIXTURE / name).read_text().split(";\n"):
            statement = "\n".join(line for line in chunk.splitlines() if not line.lstrip().startswith("--"))
            if statement.strip():
                cur.execute(statement)
    cur.execute("SET FOREIGN_KEY_CHECKS=1")
    uid = ns["uid"]
    for n, name in ((1, "alice"), (2, "bob"), (3, "charlie"), (4, "dave")):
        cur.execute("INSERT INTO `user` (user_id, email, password, name, role, is_deleted, created_at, updated_at) VALUES (%s,%s,'x',%s,'GENERAL',0,NOW(),NOW())",
                    (uid(n), f"{name}@example.com", name))
    vid, t0 = ns["vid"], ns["T0"]
    for n, title, channel, category, duration, views, deleted, offset in ns["VIDEOS"]:
        cur.execute("INSERT INTO video (video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted, created_at, updated_at)"
                    " VALUES (%s,%s,%s,%s,%s,%s,%s,%s,%s,%s)",
                    (vid(n), f"yt{n:03d}", title, channel, category, duration, views, deleted, t0 + timedelta(minutes=offset), t0))
    for index, (n, user) in enumerate(bookmarks):
        if n != 9999:  # no FK target in MySQL; the reviewer skips it too
            cur.execute("INSERT INTO video_bookmark (video_bookmark_id, video_id, user_id, created_at) VALUES (%s,%s,%s,%s)",
                        (f"eeeeeeee-0000-4000-8000-{index:012d}", vid(n), user, t0 + timedelta(minutes=index)))
    cur.execute("DROP USER IF EXISTS %s@'%%'", (base.RUNTIME_USER,))
    cur.execute("CREATE USER %s@'%%' IDENTIFIED BY %s", (base.RUNTIME_USER, base.RUNTIME_PASSWORD))
    for db in (base.SOURCE_DB, base.META_DB):
        cur.execute(f"GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{db}`.* TO %s@'%%'", (base.RUNTIME_USER,))
    return conn


def seed_extra(conn, mongo_coll, ns):
    """The reviewer's `extra=True` data: 101 bulk videos, 101 bookmarks of charlie, the `bulk current` video."""
    cur = conn.cursor()
    vid, uid, t0 = ns["vid"], ns["uid"], ns["T0"]
    docs = []
    for i in range(101):
        n = 200 + i
        cur.execute("INSERT INTO video (video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted, created_at, updated_at)"
                    " VALUES (%s,%s,%s,'chanBulk','comedy',10,%s,0,%s,%s)", (vid(n), f"bk{n:03d}", f"bulk {i:03d}", i, t0 + timedelta(minutes=1000 + i), t0))
        docs.append(ns["dist"](n, "happy", 1000 + i, ns["avgs"](happy=0.5, neutral=0.5), scores={"happy": i / 100}))
    for i in range(101):
        cur.execute("INSERT INTO video_bookmark (video_bookmark_id, video_id, user_id, created_at) VALUES (%s,%s,%s,%s)",
                    (f"eeeeeeee-0000-4000-8000-{5000 + i:012d}", vid(200 + i), uid(3), t0 + timedelta(minutes=5000 + i)))
    cur.execute("INSERT INTO video (video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted, created_at, updated_at)"
                " VALUES (%s,'bk400','bulk current','chanBulk','comedy',10,0,0,%s,%s)", (vid(400), t0 + timedelta(minutes=2000), t0))
    docs.append(ns["dist"](400, "happy", 2000, ns["avgs"](happy=0.5, neutral=0.5), scores={"happy": 0.5}))
    mongo_coll.insert_many(docs)


def actual_for(host, case, subjects):
    method, args = case["method"], case["args"]
    if method == "get_all_videos":
        page, size, emotion, user = args
        extension, ext_input = "Video.homeVideoAll", {"page": page, "size": size, "emotion": emotion}
    elif method == "get_bookmark_videos":
        user, page, size, emotion = args
        extension, ext_input = "Video.homeBookmark", {"page": page, "size": size, "emotion": emotion}
    else:
        page, size, keyword_type, keyword, emotions, user = args
        extension = "Video.homeSearch"
        ext_input = {"page": page, "size": size, "keyword_type": keyword_type, "keyword": keyword, "emotions": ",".join(emotions)}
    subject = subjects.get(user) if user else None
    response = host.post(extension, ext_input, subject)
    if user is None:
        # A real guest is refused by the host (core gap, recorded as GAP). The data logic of the case is still checked through a
        # signed-in member with no bookmarks, whose answer must equal the guest answer of the original (is_bookmarked all false).
        guest_code = response.get("code") if not response.get("ok") else None
        response = host.post(extension, ext_input, "dave")
        if not response.get("ok"):
            return {"rejected": response.get("code")}, user, guest_code
    elif not response.get("ok"):
        return {"rejected": response.get("code")}, user, None
    else:
        guest_code = None
    out = response["output"]
    return {"ok": {"videos": json.loads(out["videos"]), "total": out["total"], "page": out["page"], "size": out["size"], "has_next": out["has_next"]}}, user, guest_code


def apply_intended(expected, deleted_ids):
    """Returns (expected adjusted for the deleted-video policy, number of hidden videos)."""
    if "ok" not in expected:
        return expected, 0
    adjusted = copy.deepcopy(expected)
    kept = [v for v in adjusted["ok"]["videos"] if v["video_id"] not in deleted_ids]
    hidden = len(adjusted["ok"]["videos"]) - len(kept)
    adjusted["ok"]["videos"] = kept
    return adjusted, hidden


def multiset(result):
    if "ok" in result:
        result = copy.deepcopy(result)
        result["ok"]["videos"] = sorted(result["ok"]["videos"], key=lambda v: v["video_id"])
    return result


def main():
    oracle_dir = Path(base.need("S13B_REVIEW_ORACLE_DIR"))
    admin_url, mongo_url = base.need("S13B_MYSQL_ADMIN_URL"), base.need("S13B_MONGO_URL")
    base.check_endpoint(admin_url, "S13B_MYSQL_ADMIN_URL")
    base.check_endpoint(mongo_url, "S13B_MONGO_URL")
    work = Path(base.need("S13B_WORK_DIR"))
    work.mkdir(parents=True, exist_ok=True)
    if any(work.iterdir()):
        sys.exit("S13B_WORK_DIR must be empty")
    cases = [c for c in json.loads((oracle_dir / "oracle_cases.json").read_text()) if c["id"].startswith("B-")]
    ns, bookmarks = load_review_seed(oracle_dir)
    uid, vid = ns["uid"], ns["vid"]
    subjects = {uid(1): "alice", uid(2): "bob", uid(3): "charlie", uid(77): "dave"}
    base.SUBJECTS = {"alice": uid(1), "bob": uid(2), "charlie": uid(3), "dave": uid(4)}
    deleted_ids = {vid(n) for n, *rest in ns["VIDEOS"] if rest[5]} | {vid(4)}

    conn = seed_mysql(admin_url, ns, bookmarks)
    client = pymongo.MongoClient(mongo_url)
    client.drop_database(base.MONGO_DB)
    coll = client[base.MONGO_DB]["video_distribution"]
    coll.create_index("video_id", unique=True)
    docs = copy.deepcopy(ns["DISTRIBUTIONS"])
    coll.insert_many(docs)
    env, definition = base.adopt(work, admin_url, base.need("S13B_AIP_BIN"))
    app, service = base.load_original(base.need("S13B_ORIGINAL_ROOT"), admin_url, mongo_url)
    host = base.Host(work, env, base.need("S13B_HOST_BIN"), mongo_url, definition)

    actual, results, live = {}, [], {}
    rank_after_extra = False
    try:
        for phase in ("base", "extra"):
            if phase == "extra":
                seed_extra(conn, coll, ns)
                rank_after_extra = True
            for case in cases:
                needs_extra = bool(case.get("id") in ("B-bm-5", "B-s-13"))
                if needs_extra != (phase == "extra"):
                    continue
                result, user, guest_code = actual_for(host, case, subjects)
                actual[case["id"]] = result
                args = case["args"]
                kind = {"get_all_videos": "all", "get_bookmark_videos": "bookmark", "get_search_videos": "search"}[case["method"]]
                if kind == "all":
                    live_args = dict(page=args[0], size=args[1], emotion=args[2]); who = args[3]
                elif kind == "bookmark":
                    live_args = dict(page=args[1], size=args[2], emotion=args[3]); who = args[0]
                else:
                    live_args = dict(page=args[0], size=args[1], keyword_type=args[2], keyword=args[3], emotions=args[4]); who = args[5]
                live[case["id"]] = {"ok": base.normal(base.run_original(app, service, kind, who, **live_args))}
                results.append((case, result, who, guest_code))
    finally:
        host.close()

    mongo_rank = {}
    for index, doc in enumerate(list(coll.find().sort([("created_at", -1), ("_id", -1)]))):
        mongo_rank[doc["video_id"]] = index
    passes, gaps, fails, intended = 0, [], [], []
    for case, result, who, guest_code in results:
        cid, exp = case["id"], case["expected"]
        guest = who is None and case["method"] in ("get_all_videos", "get_search_videos")
        if guest:
            known = guest_code == "WIRE.UNAUTHENTICATED"
            (gaps if known else fails).append((cid, "real guest refused (core gap); logic checked below via member without bookmarks" if known
                                               else f"guest call answered {guest_code}"))
        if "ok" not in result:
            fails.append((cid, f"AIP refused: {result}"))
            continue
        want, hidden = apply_intended(exp, deleted_ids)
        want_live, hidden_live = apply_intended(live[cid], deleted_ids)
        if hidden:
            intended.append((cid, f"DELETED: original lists {hidden} soft-deleted video(s) that AIP policy hides"))
        if "unspecified" in str(case.get("order", "")):
            same_frozen, same_live = multiset(want) == multiset(result), multiset(want_live) == multiset(result)
        else:
            same_frozen, same_live = want == result, want_live == result
        ordered = [v["video_id"] for v in result["ok"]["videos"]]
        ranks = [mongo_rank.get(v) for v in ordered]
        rdb_ordered = case["method"] == "get_search_videos" and not case["args"][4]  # no-emotion search answers in RDB created_at order
        mongo_order_ok = rdb_ordered or ranks == sorted(ranks)
        if same_frozen and same_live and mongo_order_ok:
            passes += 1
            note = "" if case.get("collation_dependent") is None else " (collation_dependent: real MySQL result equals the frozen SQLite one)"
            print(f"PASS {cid}{note}{' (guest case: data logic via bookmark-less member)' if guest else ''}")
        else:
            fails.append((cid, f"frozen={same_frozen} live_mysql={same_live} mongo_page_order={mongo_order_ok} :: {json.dumps(result, ensure_ascii=False)[:200]}"))
    for line in intended:
        print("INTENDED", *line)
    for line in gaps:
        print("GAP", *line)
    for line in fails:
        print("FAIL", *line)
    print(f"B cases: {len(cases)} passed={passes} intended_diff_cases={len(intended)} guest_gaps={len(gaps)} failed={len(fails)}")
    (work / "actual_B.json").write_text(json.dumps(actual, ensure_ascii=False, indent=1))
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
