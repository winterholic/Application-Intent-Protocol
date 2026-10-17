"""Read parity: the adopted AIP service over the real FaceReview MySQL schema versus the original app's SQLAlchemy queries.

Requires fr_src adopted (adopt.sh) and the principal bindings below. Starts `aip service serve`, asks both sides the same
questions, and prints one line per case. Exit code 1 when any case differs.
"""
import json
import os
import subprocess
import sys
import time
import urllib.request

import jwt

BIN = "/private/tmp/aip-721-target/debug/aip"
RUN = "/private/tmp/fr-port/run"
DEF = "/private/tmp/fr-port/facereview.aip"
ENV = dict(os.environ, AIP_FR_ADOPT_URL="mysql://root:aip_fixture_pw_721@127.0.0.1:34391/fr_src",
           AIP_FR_RUNTIME_URL="mysql://aip_fr_rt:runtime_test_only@127.0.0.1:34391/fr_src")
ALICE, BOB = "11111111-1111-4111-8111-111111111111", "22222222-2222-4222-8222-222222222222"


def cli(*args):
    out = subprocess.run([BIN, "service", *args, "--config", RUN + "/service.json"], env=ENV, capture_output=True, text=True, cwd=RUN)
    return out


for subject, actor in [("alice", ALICE), ("bob", BOB)]:
    r = subprocess.run([BIN, "service", "principal", "--config", RUN + "/service.json", "bind", "--subject", subject, "--actor", actor],
                       env=ENV, capture_output=True, text=True, cwd=RUN)
    if r.returncode != 0 and "already" not in r.stderr and "PRINCIPAL_CONFLICT" not in r.stderr:
        print("bind failed", r.stderr)
        sys.exit(2)

server = subprocess.Popen([BIN, "service", "serve", DEF, "--config", RUN + "/service.json"], env=ENV, cwd=RUN,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
listening = json.loads(server.stdout.readline() or "{}")
if not listening:
    print("server did not start:", server.stderr.read())
    sys.exit(2)
ADDRESS, FINGERPRINT = listening["address"], listening["fingerprint"]
PRIVATE = open(RUN + "/private.pem").read()


def token(subject):
    now = int(time.time())
    return jwt.encode({"iss": "https://issuer.example", "aud": "aip-api", "sub": subject, "iat": now, "nbf": now, "exp": now + 300},
                      PRIVATE, algorithm="RS256", headers={"kid": "current", "typ": "at+jwt"})


def post(path, body, subject=None):
    headers = {"Content-Type": "application/json", "x-aip-contract": FINGERPRINT}
    if subject:
        headers["Authorization"] = "Bearer " + token(subject)
    request = urllib.request.Request(f"http://{ADDRESS}{path}", data=json.dumps(body).encode(), headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        return json.load(error)


def read(resource, select, subject=None, **extra):
    query = {"read": resource, "select": select, **extra}
    return post("/read", {"query": query}, subject)


# --- the original app's queries, through its own SQLAlchemy models -------------------------------------------------------------
os.environ.update(DB_USERNAME="x", DB_PASSWORD="x", DB_HOST="127.0.0.1", DB_PORT="1", DB_NAME="x", MONGO_HOST="127.0.0.1", MONGO_PORT="1",
                  MONGO_DB_NAME="x", REDIS_URL="redis://127.0.0.1:1/0", SECRET_KEY="x", JWT_SECRET_KEY="x")
sys.path.insert(0, "/private/tmp/fr-port")
import importlib  # noqa: E402

from sqlalchemy import create_engine, or_  # noqa: E402
from sqlalchemy.orm import Session  # noqa: E402

from common.extensions import db  # noqa: E402

for module in ["user", "video", "comment", "video_like", "video_bookmark", "video_request", "video_view_log", "user_favorite_genre",
               "user_point_history"]:
    importlib.import_module(f"app.models.{module}")
from app.models.comment import Comment  # noqa: E402
from app.models.user import User  # noqa: E402
from app.models.user_favorite_genre import UserFavoriteGenre  # noqa: E402
from app.models.user_point_history import UserPointHistory  # noqa: E402
from app.models.video import Video  # noqa: E402
from app.models.video_bookmark import VideoBookmark  # noqa: E402
from app.models.video_like import VideoLike  # noqa: E402
from app.models.video_request import VideoRequest  # noqa: E402
from app.models.video_view_log import VideoViewLog  # noqa: E402

engine = create_engine("mysql+pymysql://root:aip_fixture_pw_721@127.0.0.1:34391/fr_src?charset=utf8mb4")
session = Session(engine)
failures = []


def case(name, aip_ids, original_ids):
    ok = aip_ids == original_ids
    print(("OK   " if ok else "FAIL ") + name + ("" if ok else f"\n     aip      {aip_ids}\n     original {original_ids}"))
    if not ok:
        failures.append(name)


def ids(response):
    if response.get("ok") is not True:
        return ["ERR", response.get("code"), response.get("message")]
    return [row["id"] for row in response["rows"]]


# Search videos as HomeService.get_search_videos does (title / channel / both), newest first.
for label, kind, keyword in [("title", "title", "고양이"), ("channel", "channel", "냥"), ("both", "all", "고양이")]:
    if kind == "title":
        original = session.query(Video).filter_by(is_deleted=0).filter(Video.title.like(f"%{keyword}%"))
        aip = read("Video", ["id"], filter=[{"field": "title", "op": "contains", "value": keyword}], sort=[{"field": "created_at", "dir": "desc"}])
    elif kind == "channel":
        original = session.query(Video).filter_by(is_deleted=0).filter(Video.channel_name.like(f"%{keyword}%"))
        aip = read("Video", ["id"], filter=[{"field": "channel_name", "op": "contains", "value": keyword}], sort=[{"field": "created_at", "dir": "desc"}])
    else:
        original = session.query(Video).filter_by(is_deleted=0).filter(or_(Video.title.like(f"%{keyword}%"), Video.channel_name.like(f"%{keyword}%")))
        aip = None
    original = [v.video_id for v in original.order_by(Video.created_at.desc()).all()]
    if aip is not None:
        case(f"video search by {label}", ids(aip), original)
# The or-search has no `or` filter in the read grammar; record the gap instead of pretending parity.
print("GAP  video search over title OR channel: the read filter list is AND only")

case("video list newest first (is_deleted=0 only, NULL flag excluded)",
     ids(read("Video", ["id"], sort=[{"field": "created_at", "dir": "desc"}])),
     [v.video_id for v in session.query(Video).filter_by(is_deleted=0).order_by(Video.created_at.desc()).all()])
case("video by category", ids(read("Video", ["id"], filter=[{"field": "category", "op": "eq", "value": "cook"}])),
     [v.video_id for v in session.query(Video).filter_by(is_deleted=0, category="cook").order_by(Video.video_id).all()])
case("comments of a video (not deleted, video not deleted)",
     ids(read("Comment", ["id"], filter=[{"field": "video", "op": "eq", "value": "aaaaaaaa-0000-4000-8000-000000000001"}], sort=[{"field": "created_at", "dir": "asc"}])),
     [c.comment_id for c in session.query(Comment).filter_by(video_id="aaaaaaaa-0000-4000-8000-000000000001", is_deleted=0).order_by(Comment.created_at.asc()).all()])
case("alice sees only her own likes", ids(read("VideoLike", ["id"], ALICE and "alice", sort=[{"field": "created_at", "dir": "asc"}])),
     [x.video_like_id for x in session.query(VideoLike).filter_by(user_id=ALICE).order_by(VideoLike.created_at).all()])
case("bob sees only his own likes", ids(read("VideoLike", ["id"], "bob", sort=[{"field": "created_at", "dir": "asc"}])),
     [x.video_like_id for x in session.query(VideoLike).filter_by(user_id=BOB).order_by(VideoLike.created_at).all()])
case("anonymous sees no likes", ids(read("VideoLike", ["id"])), [])
case("alice bookmarks", ids(read("VideoBookmark", ["id"], "alice")), [x.video_bookmark_id for x in session.query(VideoBookmark).filter_by(user_id=ALICE).all()])
case("alice requests", ids(read("VideoRequest", ["id"], "alice")), [x.video_request_id for x in session.query(VideoRequest).filter_by(user_id=ALICE).all()])
case("alice view logs", ids(read("VideoViewLog", ["id"], "alice")), [x.video_view_log_id for x in session.query(VideoViewLog).filter_by(user_id=ALICE).all()])
case("alice favorite genres", ids(read("UserFavoriteGenre", ["id"], "alice")), [x.user_favorite_genre_id for x in session.query(UserFavoriteGenre).filter_by(user_id=ALICE).all()])
case("alice point history (negative amounts)",
     ids(read("UserPointHistory", ["id"], "alice", sort=[{"field": "created_at", "dir": "asc"}])),
     [x.user_point_history_id for x in session.query(UserPointHistory).filter_by(user_id=ALICE).order_by(UserPointHistory.created_at).all()])
case("alice reads herself only", ids(read("User", ["id"], "alice")), [ALICE])

# Paging as HomeService does it: page/size become offset/limit over the same newest-first order. Seed more videos first so pages differ.
extra = [("aaaaaaaa-1000-4000-8000-%012d" % i, "pg%09d" % i, "페이지 영상 %d" % i, "2026-11-%02d 10:00:00" % (i + 1)) for i in range(1, 8)]
with engine.begin() as conn:
    from sqlalchemy import text
    for video_id, url, title, created in extra:
        conn.execute(text("INSERT INTO video (video_id, youtube_url, title, category, created_at, updated_at) VALUES (:i, :u, :t, 'etc', :c, :c) "
                          "ON DUPLICATE KEY UPDATE title = VALUES(title)"), {"i": video_id, "u": url, "t": title, "c": created})
session.rollback()
for page, size in [(1, 3), (2, 3), (3, 3), (4, 3)]:
    original = [v.video_id for v in session.query(Video).filter_by(is_deleted=0).order_by(Video.created_at.desc()).offset((page - 1) * size).limit(size).all()]
    case(f"video page {page} size {size}", ids(read("Video", ["id"], sort=[{"field": "created_at", "dir": "desc"}], limit=size, offset=(page - 1) * size)), original)
too_deep = read("Video", ["id"], limit=3, offset=10**7)
print("     offset beyond the declared bound:", too_deep.get("code"))
print("GAP  list total (count of all matches): the original returns it with every page; the read response has no total")

# Field values, not only ids: enum case, nullable booleans, UTC timestamps, nullable ints.
detail = read("Video", ["id", "title", "channel_name", "category", "duration", "view_count", "created_at"],
              filter=[{"field": "id", "op": "eq", "value": "aaaaaaaa-0000-4000-8000-000000000001"}])
row = detail["rows"][0] if detail.get("ok") else {}
original = session.get(Video, "aaaaaaaa-0000-4000-8000-000000000001")
expected = {"id": original.video_id, "title": original.title, "channel_name": original.channel_name, "category": original.category.value,
            "duration": original.duration, "view_count": original.view_count}
case("video detail scalar fields", {k: row.get(k) for k in expected}, expected)
print("     created_at over the wire:", row.get("created_at"), "| original:", original.created_at)
user = read("User", ["id", "email", "name", "role", "profile_image_id", "is_tutorial_done", "is_verify_email_done"], "alice")["rows"][0]
mine = session.get(User, ALICE)
case("user scalar fields", {k: user.get(k) for k in ["email", "name", "role", "profile_image_id", "is_tutorial_done", "is_verify_email_done"]},
     {"email": mine.email, "name": mine.name, "role": mine.role.value if hasattr(mine.role, "value") else mine.role,
      "profile_image_id": mine.profile_image_id, "is_tutorial_done": bool(mine.is_tutorial_done), "is_verify_email_done": bool(mine.is_verify_email_done)})
password_leak = read("User", ["id", "password"], "alice")
case("password column is not readable", password_leak.get("ok"), False)

server.terminate()
print("\n%d failing case(s)" % len(failures))
sys.exit(1 if failures else 0)
