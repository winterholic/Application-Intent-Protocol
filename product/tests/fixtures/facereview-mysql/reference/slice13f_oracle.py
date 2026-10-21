"""WBS 7.2.5: FaceReview `GET watch/video` (video detail with the all-viewer emotion timeline) and its `view_count += 1` through the
real `aip service serve` binary versus the ORIGINAL WatchService.get_video_detail.

Evidence levels in the output (never merged):
  ORACLE  the pinned original's own `WatchService.get_video_detail`, imported from the original checkout and run against the same MySQL
          and MongoDB the AIP side reads. Its `view_count += 1` is flushed inside the oracle's session and rolled back, so the data both
          sides read is the same for every case.
  HTTP    POST /extension `Video.watchDetail` (RDB reads through the row policies, the timeline through the server-side Mongo aggregate
          `video_emotion_timeline`) and POST /apply `Video.view` for the increment.

It reuses the deployment and original-loading code of slice13d_oracle.py, with S13F_* in place of S13D_*:
  S13F_MYSQL_ADMIN_URL, S13F_MONGO_URL, S13F_AIP_BIN, S13F_ORIGINAL_ROOT, S13F_WORK_DIR, optional S13F_GOLDEN_OUT
"""
import importlib.util
import json
import os
import sys
import time
import urllib.parse
from pathlib import Path

import pymongo
import pymysql

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("slice13d_oracle", HERE / "slice13d_oracle.py")
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)

EMOTIONS = ("happy", "neutral", "surprise", "sad", "angry")


def need(name):
    value = os.environ.get(name)
    if not value:
        sys.exit(f"set {name}")
    return value


def vid(n):
    return f"eeeeeeee-0000-4000-8000-{n:012d}"


def seed_timelines(mongo_url):
    client = pymongo.MongoClient(mongo_url)
    # insertion order is the original's natural order
    client[base.MONGO_DB]["youtube_watching_data"].insert_many(json.loads((base.FIXTURE / "slice13f-youtube_watching_data.json").read_text()))
    client.close()


def main():
    admin_url, mongo_url = need("S13F_MYSQL_ADMIN_URL"), need("S13F_MONGO_URL")
    base.check_endpoint(admin_url, "S13F_MYSQL_ADMIN_URL")
    base.check_endpoint(mongo_url, "S13F_MONGO_URL")
    work = Path(need("S13F_WORK_DIR"))
    if any(work.iterdir()):
        sys.exit("S13F_WORK_DIR must be empty")
    base.seed_mysql(admin_url, ["slice13f-seed.sql"])
    base.seed_mongo(mongo_url)
    seed_timelines(mongo_url)
    app, _home, watch, business_error = base.load_original(need("S13F_ORIGINAL_ROOT"), admin_url, mongo_url)
    deployment = base.Deployment(work, admin_url, mongo_url, need("S13F_AIP_BIN"))
    failures, golden, timelines, count = [], [], {}, 0
    parsed = urllib.parse.urlparse(admin_url)
    conn = pymysql.connect(host=parsed.hostname, port=parsed.port, user=urllib.parse.unquote(parsed.username),
                           password=urllib.parse.unquote(parsed.password or ""), database=base.SOURCE_DB, charset="utf8mb4", autocommit=True)
    cur = conn.cursor()

    def original(video, actor):
        # The original's get_video_detail is @transactional and commits its own `view_count += 1`. Record that increment, then put the
        # row back so every case (and the AIP side) reads the same data.
        cur.execute("SELECT view_count, updated_at FROM video WHERE video_id = %s", (video,))
        before = cur.fetchone()
        with app.app_context():
            try:
                result = {"output": watch.get_video_detail(video, actor).to_dict()}
            except business_error as error:
                result = {"error": str(error)}
        if before is not None:
            cur.execute("SELECT view_count FROM video WHERE video_id = %s", (video,))
            after = cur.fetchone()[0]
            result["increment"] = after - before[0]
            cur.execute("UPDATE video SET view_count = %s, updated_at = %s WHERE video_id = %s", (before[0], before[1], video))
        return result

    def detail(got):
        if not got.get("ok"):
            return None
        out = dict(got["output"])
        out["timeline_data"] = json.loads(out["timeline_data"])
        return out

    def case(name, video, subject):
        nonlocal count
        count += 1
        actor = base.SUBJECTS[subject]
        want = original(video, actor)
        got = deployment.post("Video.watchDetail", {"video_id": video}, subject)
        if "error" in want:
            ok = got.get("ok") is False
            print(f"{'PASS' if ok else 'FAIL'} {name}: the original refuses ({want['error']}), AIP answered {got.get('code')}")
            golden.append({"name": name, "subject": subject, "video": video, "expect_error": True})
        else:
            port = detail(got)
            ok = port == want["output"] and want.get("increment") == 1
            points = sum(len(v) for v in want["output"]["timeline_data"].values())
            print(f"{'PASS' if ok else 'FAIL'} {name}: {points} timeline point(s), view_count={want['output']['view_count']}")
            if not ok:
                print("   original increment:", want.get("increment"))
                if port is None:
                    print("   aip     :", json.dumps(got, ensure_ascii=False)[:600])
                else:
                    for key in want["output"]:
                        if port.get(key) != want["output"][key]:
                            if key == "timeline_data":
                                for emotion, points in want["output"][key].items():
                                    for index, point in enumerate(points):
                                        if port[key].get(emotion, [None] * (index + 1))[index] != point:
                                            print(f"   timeline {emotion}[{index}] original={point} aip={port[key].get(emotion, [])[index:index + 1]}")
                                            break
                            else:
                                print(f"   {key}: original={want['output'][key]!r} aip={port.get(key)!r}")
            # The 500-point timeline is a function of the video only, so it is stored once per video, not once per caller.
            expected = dict(want["output"])
            timelines[video] = expected.pop("timeline_data")
            golden.append({"name": name, "subject": subject, "video": video, "expected": expected})
        if not ok:
            failures.append(name)

    def row(video):
        cur.execute("SELECT view_count, updated_at FROM video WHERE video_id = %s", (video,))
        return cur.fetchone() or (None, None)

    def view_case(name, video, subject, expect):
        nonlocal count
        count += 1
        before = row(video)
        reply = deployment.request("/apply", {"key": f"view-{int(time.time() * 1000)}-{count}", "request": {"apply": "Video.view", "target": {"ids": [video]}}}, subject)
        after = row(video)
        if expect == "increment":
            ok = reply.get("ok") is True and after[0] == before[0] + 1 and after[1] >= before[1]
        elif expect == "null stays null":
            ok = reply.get("ok") is True and after[0] is None
        else:
            ok = reply.get("ok") is False and after == before
        print(f"{'PASS' if ok else 'FAIL'} {name}: {before[0]} -> {after[0]} ({reply.get('ok')}, {reply.get('code')})")
        if not ok:
            failures.append(name)

    try:
        videos = [("timeline 60s", vid(1)), ("duration NULL", vid(2)), ("duration 0", vid(3)), ("no sessions", vid(4)), ("view_count NULL before backfill", vid(5)),
                  ("3s video", vid(6)), ("seeded video with one session", base.video_id(2)), ("seeded video without sessions", base.video_id(4)),
                  ("canonical video", "aaaaaaaa-0000-4000-8000-000000000001"), ("soft-deleted", base.video_id(6)),
                  ("NULL is_deleted", base.video_id(13)), ("unknown", "cccccccc-0000-4000-8000-0000000000ff")]
        for subject in ("alice", "bob", "dave"):
            for label, video in videos:
                case(f"watch/video {subject} {label}", video, subject)
        view_case("view increments the count (alice)", vid(1), "alice", "increment")
        view_case("view increments again (bob)", vid(1), "bob", "increment")
        view_case("view of a count that was NULL before the DBA backfill", vid(5), "alice", "increment")
        view_case("view of a soft-deleted video is refused", base.video_id(6), "alice", "refused")
        view_case("view of an unknown video is refused", "cccccccc-0000-4000-8000-0000000000ff", "alice", "refused")
        after_view = deployment.post("Video.watchDetail", {"video_id": vid(1)}, "alice")
        count += 1
        ok = after_view.get("ok") and after_view["output"]["view_count"] == 12  # seeded 10, then the two views above
        print(f"{'PASS' if ok else 'FAIL'} the detail read after two views carries the new count: {after_view.get('output', {}).get('view_count')}")
        if not ok:
            failures.append("count after views")
        # Privacy: the timeline collection cannot be read as raw documents through the generic Mongo path, and the extension only
        # carries buckets. The raw collection stays closed to Video.watchDetail (it holds only the derived grant).
    finally:
        conn.close()
        deployment.close()
    print(f"{count - len(failures)}/{count} cases matched")
    if failures:
        sys.exit("FAILED: " + ", ".join(failures))
    out = os.environ.get("S13F_GOLDEN_OUT")
    if out:
        Path(out).write_text(json.dumps({"timelines": timelines, "cases": golden}, ensure_ascii=False, separators=(",", ":")) + "\n")
        print(f"golden answers written to {out}")


if __name__ == "__main__":
    main()
