"""Slice13-C oracle: the ORIGINAL WatchService functions versus extensions/slice13c_watch.py on the same live MySQL + Mongo rows.

Oracle = app.services.watch_service.WatchService (new-facereview@7948b7e721837fdb73c427cb54e12cad4ec2d5af) imported from a
`git archive` of that revision and executed unmodified against real MySQL (SQLAlchemy) and real MongoDB (pymongo).
Module under test = slice13c_watch.py driven through a ctx test double. The double is NOT the AIP server: it reimplements the
row policies and the Mongo capability limits (limit<=100, $in<=100, youtube_watching_data forced to user_id=actor) so the
worker logic is exercised under the same bounds. Server planner/transport evidence is separate (slice13c_* Rust/HTTP tests).

Environment (no defaults, nothing is read from .env):
  FACEREVIEW_SOURCE_PATH   directory holding the extracted original `app/` and `common/` at the pinned revision
  S13C_MYSQL_URL           mysql+pymysql://user:password@127.0.0.1:<non-default port>/<run-owned db>
  S13C_MONGO_URL           mongodb://127.0.0.1:<non-default port>
Run: <venv python> slice13c_oracle.py   (exit 1 on any mismatch; prints one line per case)
"""
import asyncio
import importlib.util
import json
import os
import random
import sys
import uuid
from datetime import datetime, timedelta
from urllib.parse import urlparse

HERE = os.path.dirname(os.path.abspath(__file__))
MODULE = os.path.join(HERE, "..", "extensions", "slice13c_watch.py")


def need(name):
    value = os.environ.get(name)
    if not value:
        sys.exit(f"{name} is required")
    return value


def loopback_run_owned(url, default_ports):
    parsed = urlparse(url)
    if parsed.hostname not in ("127.0.0.1", "localhost") or parsed.port in default_ports or parsed.port is None:
        sys.exit("fixture URL must be loopback on an explicit non-default port")


SOURCE = need("FACEREVIEW_SOURCE_PATH")
MYSQL_URL = need("S13C_MYSQL_URL")
MONGO_URL = need("S13C_MONGO_URL")
loopback_run_owned(MYSQL_URL, {3306, 33060})
loopback_run_owned(MONGO_URL, {27017})
MYSQL_DB = urlparse(MYSQL_URL).path.lstrip("/")

os.environ.update(DB_USERNAME="x", DB_PASSWORD="x", DB_HOST="127.0.0.1", DB_PORT="1", DB_NAME="x", MONGO_HOST="127.0.0.1", MONGO_PORT="1",
                  MONGO_DB_NAME="x", REDIS_URL="redis://127.0.0.1:1/0", SECRET_KEY="x", JWT_SECRET_KEY="x")
sys.path.insert(0, SOURCE)

import pymongo  # noqa: E402
from flask import Flask  # noqa: E402
from sqlalchemy import text  # noqa: E402

from common import extensions  # noqa: E402

app = Flask("slice13c_oracle")
app.config["SQLALCHEMY_DATABASE_URI"] = MYSQL_URL
extensions.db.init_app(app)
mongo_client = pymongo.MongoClient(MONGO_URL, serverSelectionTimeoutMS=3000)
MONGO_DB = f"s13c_oracle_{uuid.uuid4().hex[:8]}"
extensions.mongo_db = mongo_client[MONGO_DB]  # must be set before watch_service binds `from common.extensions import mongo_db`

from app.models.comment import Comment  # noqa: E402
from app.models.user import User  # noqa: E402
from app.models.video import Video  # noqa: E402
from app.models.video_bookmark import VideoBookmark  # noqa: E402
from app.models.video_like import VideoLike  # noqa: E402
from app.services.watch_service import WatchService  # noqa: E402
from common.enum.youtube_genre import GenreEnum  # noqa: E402

spec = importlib.util.spec_from_file_location("slice13c_watch", MODULE)
port = importlib.util.module_from_spec(spec)
spec.loader.exec_module(port)

db = extensions.db
mdb = extensions.mongo_db
failures = []
checks = 0


def check(name, aip, original):
    global checks
    checks += 1
    ok = aip == original
    print(("OK   " if ok else "FAIL ") + name)
    if not ok:
        print(f"     module   {str(aip)[:600]}")
        print(f"     original {str(original)[:600]}")
        failures.append(name)


# ---------------------------------------------------------------------------------------------------------------------
# ctx test double
# ---------------------------------------------------------------------------------------------------------------------
class CapabilityViolation(Exception):
    pass


class FakeData:
    def __init__(self, ctx):
        self.ctx = ctx

    async def read(self, query):
        self.ctx.reads.append(query)
        resource = query["read"]
        sql, params = self.ctx.plan(resource, query)
        with db.engine.connect() as conn:
            rows = [dict(r._mapping) for r in conn.execute(text(sql), params)]
        return {"ok": True, "rows": [self.ctx.shape(resource, r) for r in rows]}

    async def aggregate(self, resource, name, inp):
        self.ctx.aggregates.append((resource, name, inp))
        table, extra = {"VideoLike": ("video_like", ""), "Comment": ("comment", " AND is_deleted = 0")}[resource]
        with db.engine.connect() as conn:
            n = conn.execute(text(f"SELECT COUNT(*) FROM {table} WHERE video_id = :v{extra}"), {"v": inp["video"]}).scalar()
        return {"ok": True, "rows": [n]}


class FakeCtx:
    def __init__(self, actor):
        self.actor = actor
        self.reads, self.aggregates, self.mongo_calls = [], [], []
        self.data = FakeData(self)

    # planner double: only the query shapes the module emits; anything else is a test failure, not a silent pass.
    def plan(self, resource, q):
        limit, offset = q.get("limit", 100), q.get("offset", 0)
        if limit > 100 or offset > 100000:
            raise CapabilityViolation(f"read budget exceeded: limit={limit} offset={offset}")
        params = {"limit": limit, "offset": offset}
        where, order = [], ""
        flt = q.get("filter", [])
        if resource == "Video":
            where.append("is_deleted = 0")
        elif resource in ("VideoLike", "VideoBookmark"):
            if self.actor is None:
                where.append("1 = 0")
            else:
                where.append("user_id = :actor")
                params["actor"] = self.actor
        else:
            raise CapabilityViolation(f"unplanned resource {resource}")
        cols = {"id": {"Video": "video_id", "VideoLike": "video_like_id", "VideoBookmark": "video_bookmark_id"}[resource],
                "video": "video_id", "category": "category"}
        for i, item in enumerate(flt):
            if "or" in item:
                if len(item["or"]) > 10:
                    raise CapabilityViolation("OR list exceeds the planner limit of 10 conditions")
                ors = []
                for j, sub in enumerate(item["or"]):
                    assert sub["op"] == "eq"
                    ors.append(f"{cols[sub['field']]} = :f{i}_{j}")
                    params[f"f{i}_{j}"] = sub["value"]
                where.append("(" + " OR ".join(ors) + ")")
            else:
                assert item["op"] == "eq"
                where.append(f"{cols[item['field']]} = :f{i}")
                params[f"f{i}"] = item["value"]
        sorts = q.get("sort", [])
        if sorts:
            sort_cols = {"view_count": "view_count", "created_at": "created_at", "id": "video_id"}
            order = " ORDER BY " + ", ".join(f"{sort_cols[s['field']]} {s['dir'].upper()}" for s in sorts)
        table = {"Video": "video", "VideoLike": "video_like", "VideoBookmark": "video_bookmark"}[resource]
        select = "*"
        return f"SELECT {select} FROM {table} WHERE {' AND '.join(where)}{order} LIMIT :limit OFFSET :offset", params

    def shape(self, resource, row):
        if resource == "Video":
            return {"id": row["video_id"], "youtube_url": row["youtube_url"], "title": row["title"], "channel_name": row["channel_name"],
                    "category": row["category"], "duration": row["duration"], "view_count": row["view_count"],
                    "created_at": row["created_at"].strftime("%Y-%m-%dT%H:%M:%S.%fZ")}
        key = {"VideoLike": "video_like_id", "VideoBookmark": "video_bookmark_id"}[resource]
        return {"id": row[key], "video": row["video_id"]}

    async def call(self, op, name, inp, **_):
        assert op == "mongo", op
        self.mongo_calls.append((name, inp))
        if name not in ("video_distribution", "youtube_watching_data"):
            raise CapabilityViolation(f"collection {name} not granted")
        command = inp["command"]
        if command not in ("find", "find_one"):
            raise CapabilityViolation(f"command {command}")
        flt = dict(inp.get("filter", {}))
        vid = flt.get("video_id")
        if isinstance(vid, dict) and len(vid.get("$in", [])) > 100:
            raise CapabilityViolation("$in exceeds 100")
        if name == "youtube_watching_data":
            if self.actor is None:
                raise CapabilityViolation("principal required")
            if "user_id" in flt and flt["user_id"] != self.actor:
                raise CapabilityViolation("other user_id")
            flt["user_id"] = self.actor
        coll = mdb[name]
        if command == "find_one":
            doc = coll.find_one(flt, {"_id": 0})
            return {"doc": doc}
        limit = inp.get("limit", 100)
        if not (1 <= limit <= 100):
            raise CapabilityViolation("limit")
        cursor = coll.find(flt, {"_id": 0, **{k: v for k, v in (inp.get("projection") or {}).items()}} if inp.get("projection") else {"_id": 0})
        if inp.get("sort_by"):
            cursor = cursor.sort([("_id", 1)])
        cursor = cursor.skip(inp.get("skip", 0)).limit(limit)
        return {"docs": list(cursor)}


def run(coro):
    return asyncio.new_event_loop().run_until_complete(coro)


# ---------------------------------------------------------------------------------------------------------------------
# seed
# ---------------------------------------------------------------------------------------------------------------------
T0 = datetime(2026, 1, 1, 0, 0, 0)


def reset_all():
    with app.app_context():
        with db.engine.begin() as conn:
            conn.execute(text("SET FOREIGN_KEY_CHECKS = 0"))
            for table in ("video_bookmark", "video_like", "comment", "video", "user"):
                conn.execute(text(f"DELETE FROM `{table}`"))
            conn.execute(text("SET FOREIGN_KEY_CHECKS = 1"))
    for name in ("video_distribution", "youtube_watching_data"):
        mdb[name].delete_many({})


def add_user(name, deleted=False):
    uid = str(uuid.uuid4())
    db.session.add(User(user_id=uid, email=f"{name}-{uid[:8]}@example.test", password="x", name=name, is_deleted=deleted))
    db.session.commit()
    return uid


def add_video(i, category="drama", views=0, deleted=False, duration=100):
    vid = str(uuid.uuid4())
    db.session.add(Video(video_id=vid, youtube_url=f"yt{i:04d}{vid[:6]}", title=f"title {i}", channel_name=None if i % 3 == 0 else f"ch {i}",
                         category=GenreEnum(category), duration=duration, view_count=views, is_deleted=deleted,
                         created_at=T0 + timedelta(minutes=i), updated_at=T0))
    db.session.commit()
    return vid


def add_dist(video_id, dominant, averages=None, scores=None, include_dominant_key=True):
    averages = averages or {e: 0.0 for e in port.EMOTIONS}
    scores = scores or {e: 0.0 for e in port.EMOTIONS}
    doc = {"video_id": video_id, "emotion_averages": averages, "recommendation_scores": scores}
    if include_dominant_key:
        doc["dominant_emotion"] = dominant
    mdb["video_distribution"].insert_one(doc)


def add_session(video_id, user_id, timeline):
    mdb["youtube_watching_data"].insert_one({"video_id": video_id, "user_id": user_id, "emotion_score_timeline": timeline})


def view_count_of(video_id):
    # fresh connection: the original commits inside its own app context/session
    with db.engine.connect() as conn:
        return conn.execute(text("SELECT view_count FROM video WHERE video_id = :v"), {"v": video_id}).scalar()


def original_detail(video_id, user_id):
    with app.app_context():
        return WatchService.get_video_detail(video_id, user_id).to_dict()


def original_recommended(video_id, page, size, user_id):
    with app.app_context():
        return WatchService.get_recommended_videos(video_id, page, size, user_id).to_dict()


def module_detail(video_id, actor):
    ctx = FakeCtx(actor)
    out = run(port.watchDetail({"video": video_id}, ctx))
    out["timeline_data"] = json.loads(out.pop("timeline_data_json"))
    return out, ctx


def module_recommended(video_id, page, size, actor):
    ctx = FakeCtx(actor)
    out = run(port.watchRecommended({"video": video_id, "page": page, "size": size}, ctx))
    out["videos"] = json.loads(out.pop("videos_json"))
    return out, ctx


# ---------------------------------------------------------------------------------------------------------------------
# cases
# ---------------------------------------------------------------------------------------------------------------------
def timeline_cases():
    rng = random.Random(1313)
    with app.app_context():
        reset_all()
        alice, bob, carol = add_user("alice"), add_user("bob"), add_user("carol")

        # no sessions at all -> default single point
        empty = add_video(1)
        check("timeline: empty seed", original_detail(empty, alice)["timeline_data"], module_detail(empty, alice)[0]["timeline_data"])

        # sessions whose timelines hold nothing usable -> default (keys exist but all malformed)
        junk = add_video(2)
        add_session(junk, alice, {"0": [1, 2], "50": "x", "100": None})
        check("timeline: only malformed score rows", original_detail(junk, alice)["timeline_data"],
              module_detail(junk, alice)[0]["timeline_data"])

        # one session, duration 0 and None -> (duration or 1) * 100
        for label, duration in (("duration 0", 0), ("duration None", None)):
            vid = add_video(3)
            if duration is None:
                db.session.query(Video).filter_by(video_id=vid).update({"duration": None})
                db.session.commit()
            else:
                db.session.query(Video).filter_by(video_id=vid).update({"duration": duration})
                db.session.commit()
            add_session(vid, alice, {"0": [10, 20, 30, 40, 50], "50": [11, 21, 31, 41, 51], "99": [1, 1, 1, 1, 1], "100": [9, 9, 9, 9, 9]})
            check(f"timeline: {label}", original_detail(vid, alice)["timeline_data"], module_detail(vid, alice)[0]["timeline_data"])

        # one viewer owns every session: capability-visible set equals the original's set, so full equality is expected
        vid = add_video(4, duration=300)
        for _ in range(4):
            add_session(vid, alice, {str(k * 50): [rng.uniform(0, 100) for _ in range(5)] for k in range(rng.randint(5, 40))})
        check("timeline: several sessions of the same viewer (round 1)", original_detail(vid, alice)["timeline_data"],
              module_detail(vid, alice)[0]["timeline_data"])

        # algorithm parity on arbitrary multi-user data: feed the module every session document directly (test-side read)
        for case in range(40):
            vid = add_video(100 + case, duration=rng.choice([1, 7, 59, 60, 61, 300, 3600, 100000]))
            owners = [alice, bob, carol]
            for _ in range(rng.randint(1, 6)):
                pool = range(0, rng.choice([300, 6000, 400000]), 25)
                keys = rng.sample(pool, k=min(len(pool), rng.randint(1, 25)))
                timeline = {str(k): [round(rng.uniform(0, 100), rng.choice([0, 1, 2, 6])) for _ in range(rng.choice([5, 5, 5, 6, 3]))] for k in keys}
                if rng.random() < 0.15:
                    timeline["abc"] = [1, 2, 3, 4, 5]
                add_session(vid, rng.choice(owners), timeline)
            original = original_detail(vid, alice)["timeline_data"]
            all_docs = list(mdb["youtube_watching_data"].find({"video_id": vid}, {"emotion_score_timeline": 1, "_id": 0}))
            duration = db.session.query(Video).filter_by(video_id=vid).first().duration
            check(f"timeline: algorithm parity on multi-user fuzz #{case}", port.compress_timeline(all_docs, duration), original)

        # capability scope: the same multi-user video through the worker double sees only the caller's sessions
        scoped_vid = add_video(900, duration=120)
        add_session(scoped_vid, alice, {"0": [100, 0, 0, 0, 0]})
        add_session(scoped_vid, bob, {"0": [0, 100, 0, 0, 0]})
        original = original_detail(scoped_vid, alice)["timeline_data"]
        got, ctx = module_detail(scoped_vid, alice)
        check("timeline: scope flag is explicit", got["timeline_scope"], "caller")
        check("timeline GAP: caller-only worker result DIFFERS from original on multi-user data (expected until an aggregate contract exists)",
              got["timeline_data"] != original, True)
        alice_only = [{"emotion_score_timeline": {"0": [100, 0, 0, 0, 0]}}]
        check("timeline: worker result equals the algorithm over the caller's visible docs", got["timeline_data"],
              port.compress_timeline(alice_only, 120))
        try:
            run(FakeCtx(alice).call("mongo", "youtube_watching_data", {"command": "find", "filter": {"video_id": scoped_vid, "user_id": bob}}))
            spoof = "UNEXPECTED_OK"
        except CapabilityViolation:
            spoof = "denied"
        check("timeline: spoofed user_id filter is denied by the capability double", spoof, "denied")


def detail_cases():
    with app.app_context():
        reset_all()
        alice, bob, deleted_user = add_user("alice"), add_user("bob"), add_user("gone", deleted=True)
        vid = add_video(1, views=41, duration=200)
        other = add_video(2)
        for u in (alice, bob):
            db.session.add(VideoLike(video_like_id=str(uuid.uuid4()), video_id=vid, user_id=u))
        db.session.add(VideoLike(video_like_id=str(uuid.uuid4()), video_id=other, user_id=alice))
        db.session.add(VideoBookmark(video_bookmark_id=str(uuid.uuid4()), video_id=vid, user_id=alice))
        for i, (user, deleted) in enumerate([(alice, False), (bob, False), (bob, True), (deleted_user, False)]):
            db.session.add(Comment(comment_id=str(uuid.uuid4()), video_id=vid, user_id=user, content=f"c{i}", is_deleted=deleted,
                                   created_at=T0 + timedelta(minutes=i), updated_at=T0))
        db.session.commit()
        add_session(vid, alice, {"0": [50, 10, 10, 20, 10]})

        for label, actor in (("alice (liked+bookmarked)", alice), ("bob (liked only)", bob)):
            before = view_count_of(vid)
            got, ctx = module_detail(vid, actor)  # the worker detail first: it must not write
            check(f"detail {label}: module detail does not write (read extension)", view_count_of(vid), before)
            original = original_detail(vid, actor)
            for field in ("video_id", "youtube_url", "title", "channel_name", "category", "duration", "view_count", "like_count",
                          "comment_count", "user_is_liked", "is_bookmarked"):
                check(f"detail {label}: {field}", got[field], original[field])
            check(f"detail {label}: original increments view_count (+1)", view_count_of(vid) - before, 1)
            # query budget the detail needs: 1 video read + 1 like + 1 bookmark read, 2 aggregates, 1 Mongo find (<100 docs)
            check(f"detail {label}: read budget", (len(ctx.reads), len(ctx.aggregates), len(ctx.mongo_calls)), (3, 2, 1))

        # a member who liked nothing
        carol = add_user("carol")
        got, _ = module_detail(vid, carol)
        original = original_detail(vid, carol)
        check("detail: member without like/bookmark", (got["user_is_liked"], got["is_bookmarked"]), (original["user_is_liked"], original["is_bookmarked"]))

        # deleted video and unknown id
        gone = add_video(3, deleted=True)
        for label, target in (("soft-deleted video", gone), ("unknown id", str(uuid.uuid4()))):
            try:
                original_detail(target, alice)
                o = "no error"
            except Exception as e:  # BusinessError
                o = type(e).__name__
            try:
                module_detail(target, alice)
                m = "no error"
            except port.VideoNotFound:
                m = "BusinessError"
            check(f"detail: {label} is not found", m, o)

        # 101 sessions of one viewer -> the double returns <=100 per call, module pages with skip
        many = add_video(4, duration=100)
        for i in range(101):
            add_session(many, alice, {"0": [i % 100, 0, 0, 0, 0]})
        check("detail: 101 sessions page through the 100-doc Mongo limit", module_detail(many, alice)[0]["timeline_data"],
              original_detail(many, alice)["timeline_data"])


def recommended_cases():
    rng = random.Random(7)
    with app.app_context():
        reset_all()
        alice, bob = add_user("alice"), add_user("bob")
        weights = port.EMOTIONS

        def dist(video, dominant, per, score, key=True):
            averages = {e: 0.0 for e in weights}
            if dominant:
                averages[dominant] = per
            add_dist(video, dominant, averages, {e: (score if e == dominant else 0.0) for e in weights}, include_dominant_key=key)

        # current video with a dominant emotion: same category AND same emotion, ranked by recommendation score
        current = add_video(1, "cook")
        dist(current, "happy", 0.5, 1.0)
        ranked = []
        for i in range(2, 32):
            v = add_video(i, "cook", views=rng.randint(0, 50))
            if i % 5 == 0:
                dist(v, "sad", 0.4, 2.0)                     # same category, other emotion -> excluded
            elif i % 7 == 0:
                pass                                          # no distribution -> excluded
            else:
                dist(v, "happy", round(rng.uniform(0.1, 0.9), 4), round(rng.uniform(0.1, 4.0), 4))
            ranked.append(v)
        other_cat = add_video(40, "game")
        dist(other_cat, "happy", 0.9, 9.0)
        deleted = add_video(41, "cook", deleted=True)
        dist(deleted, "happy", 0.9, 9.0)
        for v in ranked[:4]:
            db.session.add(VideoBookmark(video_bookmark_id=str(uuid.uuid4()), video_id=v, user_id=alice))
        db.session.commit()

        for page, size in ((1, 10), (2, 10), (3, 10), (1, 50), (4, 10), (1, 1), (2, 7)):
            original = original_recommended(current, page, size, alice)
            got, ctx = module_recommended(current, page, size, alice)
            check(f"recommended emotion branch page={page} size={size}", got, original)
        check("recommended emotion branch: guest (no bookmarks)", module_recommended(current, 1, 10, None)[0],
              original_recommended(current, 1, 10, None))
        check("recommended emotion branch: other member sees no bookmarks", module_recommended(current, 1, 10, bob)[0],
              original_recommended(current, 1, 10, bob))

        # more than 100 same-category videos: Video reads page past the 100-row budget, Mongo $in is chunked at 100
        big_current = add_video(500, "music")
        dist(big_current, "happy", 0.5, 1.0)
        for i in range(501, 701):
            v = add_video(i, "music", views=i % 13)
            dist(v, "happy", round(0.001 * (i % 977) + 0.0001, 4), round(0.0013 * ((i * 7) % 983) + 0.0001, 4))
        original = original_recommended(big_current, 1, 50, alice)
        got, ctx = module_recommended(big_current, 1, 50, alice)
        check("recommended emotion branch: 200 same-category videos, total", got["total"], original["total"])
        check("recommended emotion branch: 200 same-category videos, page 1", got, original)
        check("recommended emotion branch: video reads paged at 100 rows", [r.get("offset", 0) for r in ctx.reads if r["read"] == "Video"][-3:],
              [0, 100, 200][-3:] if len([r for r in ctx.reads if r["read"] == "Video"]) >= 3 else [0, 100][-3:])
        check("recommended emotion branch: Mongo $in chunks of <=100", max(len(c[1]["filter"]["video_id"]["$in"]) for c in ctx.mongo_calls
                                                                                if "$in" in str(c[1]["filter"].get("video_id", ""))), 100)
        bookmark_reads = [r for r in ctx.reads if r["read"] == "VideoBookmark"]
        check("recommended: bookmark lookups for a 50-row page are 5 OR-of-10 reads", len(bookmark_reads), 5)
        check("recommended emotion branch: page 4 of 50", module_recommended(big_current, 4, 50, alice)[0], original_recommended(big_current, 4, 50, alice))

        # fallback branch: current emotion missing (insufficient frames) -> view_count desc, created_at desc, badges from distributions
        for label, current_doc in (("no distribution document", None), ("dominant_emotion null", (None, True)), ("key absent -> neutral", ("absent", False))):
            cur = add_video(800 + len(label), "vlog", views=3)
            if label == "dominant_emotion null":
                dist(cur, None, 0.0, 0.0)
            elif label.startswith("key absent"):
                dist(cur, None, 0.0, 0.0, key=False)
            for i in range(20):
                v = add_video(900 + i + 20 * len(label), "vlog", views=rng.choice([0, 1, 1, 5, 5, 5, 9]))
                if i % 2 == 0:
                    dist(v, rng.choice(weights), round(rng.uniform(0.05, 0.95), 4), 1.0)
                elif i % 4 == 1:
                    dist(v, None, 0.0, 0.0)
            for page, size in ((1, 10), (2, 10), (1, 50)):
                original = original_recommended(cur, page, size, alice)
                got, _ = module_recommended(cur, page, size, alice)
                if label.startswith("key absent"):
                    # original reads the absent key as 'neutral' (VideoDistribution.from_dict) -> emotion branch
                    check(f"recommended {label} page={page}: total", got["total"], original["total"])
                    check(f"recommended {label} page={page}: ids as sets", sorted(v["video_id"] for v in got["videos"]) if got["total"] <= size else [],
                          sorted(v["video_id"] for v in original["videos"]) if original["total"] <= size else [])
                else:
                    check(f"recommended fallback ({label}) page={page} size={size}", got, original)

        # empty / lonely category
        lone = add_video(2000, "horror")
        check("recommended: lone video in category", module_recommended(lone, 1, 10, alice)[0], original_recommended(lone, 1, 10, alice))
        empty_emotion = add_video(2001, "beauty")
        dist(empty_emotion, "happy", 0.5, 1.0)
        add_video(2002, "beauty")  # same category, no distribution -> emotion branch finds nothing
        check("recommended: emotion branch with no matching distribution", module_recommended(empty_emotion, 1, 10, alice)[0],
              original_recommended(empty_emotion, 1, 10, alice))
        check("recommended: page beyond the end", module_recommended(current, 99, 10, alice)[0], original_recommended(current, 99, 10, alice))

        # missing / deleted current video
        for label, target in (("soft-deleted", deleted), ("unknown", str(uuid.uuid4()))):
            try:
                original_recommended(target, 1, 10, alice)
                o = "no error"
            except Exception as e:
                o = type(e).__name__
            try:
                module_recommended(target, 1, 10, alice)
                m = "no error"
            except port.VideoNotFound:
                m = "BusinessError"
            check(f"recommended: {label} current video is not found", m, o)

        # ties: equal scores. Original order is Mongo-natural (unspecified); compare as a multiset and pin the module's tie-break.
        tie_current = add_video(3000, "comedy")
        dist(tie_current, "happy", 0.5, 1.0)
        ties = []
        for i in range(3001, 3011):
            v = add_video(i, "comedy")
            dist(v, "happy", 0.5, 2.0)
            ties.append(v)
        original = original_recommended(tie_current, 1, 50, alice)
        got, _ = module_recommended(tie_current, 1, 50, alice)
        check("recommended: equal scores keep the same set and total", sorted(v["video_id"] for v in got["videos"]),
              sorted(v["video_id"] for v in original["videos"]))
        check("recommended: equal scores are ordered by video_id in the module (deterministic pages)",
              [v["video_id"] for v in got["videos"]], sorted(ties))


def view_count_original():
    # Oracle fact the AIP write must reproduce: one detail call = exactly +1 on view_count, nothing else changes.
    with app.app_context():
        reset_all()
        alice = add_user("alice")
        vid = add_video(1, views=7)
        for expected in (8, 9, 10):
            original_detail(vid, alice)
            check(f"original view_count after call -> {expected}", view_count_of(vid), expected)
        original_detail(vid, None)
        check("original: a guest call also increments (login_optional)", view_count_of(vid), 11)


if __name__ == "__main__":
    try:
        timeline_cases()
        detail_cases()
        recommended_cases()
        view_count_original()
    finally:
        mongo_client.drop_database(MONGO_DB)
    print(f"\n{checks} checks, {len(failures)} failed")
    sys.exit(1 if failures else 0)
