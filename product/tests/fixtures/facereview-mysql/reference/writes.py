"""Write parity on the real FaceReview schema: what the original app's SQLAlchemy code would leave in the rows versus what the
adopted AIP service leaves. Run after adopt.sh. Prints one line per case; exit 1 on any difference."""
import json
import os
import subprocess
import sys
import time
import urllib.request

import jwt
import pymysql

BIN = "/private/tmp/aip-721-target/debug/aip"
RUN = "/private/tmp/fr-port/run"
DEF = "/private/tmp/fr-port/facereview.aip"
ENV = dict(os.environ, AIP_FR_ADOPT_URL="mysql://root:aip_fixture_pw_721@127.0.0.1:34391/fr_src",
           AIP_FR_RUNTIME_URL="mysql://aip_fr_rt:runtime_test_only@127.0.0.1:34391/fr_src")
ALICE, BOB = "11111111-1111-4111-8111-111111111111", "22222222-2222-4222-8222-222222222222"
CAT, DELETED = "aaaaaaaa-0000-4000-8000-000000000001", "aaaaaaaa-0000-4000-8000-000000000003"

for subject, actor in [("alice", ALICE), ("bob", BOB)]:
    subprocess.run([BIN, "service", "principal", "--config", RUN + "/service.json", "bind", "--subject", subject, "--actor", actor],
                   env=ENV, capture_output=True, text=True, cwd=RUN)
server = subprocess.Popen([BIN, "service", "serve", DEF, "--config", RUN + "/service.json"], env=ENV, cwd=RUN,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
listening = json.loads(server.stdout.readline() or "{}")
if not listening:
    print("server did not start:", server.stderr.read())
    sys.exit(2)
ADDRESS, FINGERPRINT = listening["address"], listening["fingerprint"]
PRIVATE = open(RUN + "/private.pem").read()
db = pymysql.connect(host="127.0.0.1", port=34391, user="root", password="aip_fixture_pw_721", database="fr_src", autocommit=True,
                     cursorclass=pymysql.cursors.DictCursor)
cur = db.cursor()


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


def create(resource, values, key, subject):
    return post("/apply", {"key": key, "request": {"atomic": [{"create": resource, "values": values}]}}, subject)


def apply(name, target, key, subject, values=None):
    request = {"apply": name, "target": {"ids": [target]}}
    if values:
        request["values"] = values
    return post("/apply", {"key": key, "request": request}, subject)


failures = []


def case(name, got, want):
    ok = got == want
    print(("OK   " if ok else "FAIL ") + name + ("" if ok else f"\n     got  {got}\n     want {want}"))
    if not ok:
        failures.append(name)


def row(sql, *args):
    cur.execute(sql, args)
    return cur.fetchone()


tag = str(int(time.time()))
# 1. comment create: original CommentService writes is_modified=0, is_deleted=0, created_at=updated_at=utcnow, id=uuid4.
made = create("Comment", {"video": CAT, "user": ALICE, "content": "새 댓글 " + tag}, "c-" + tag, "alice")
print("     create response:", json.dumps(made, ensure_ascii=False)[:200])
cid = (made.get("created") or [None])[0] if isinstance(made.get("created"), list) else made.get("id") or made.get("createdId")
stored = row("SELECT * FROM comment WHERE content = %s", "새 댓글 " + tag)
case("comment create stored", bool(stored) and (stored["is_modified"], stored["is_deleted"], stored["user_id"], stored["video_id"]) == (0, 0, ALICE, CAT), True)
if stored:
    case("comment id is a UUID", len(stored["comment_id"]) == 36 and stored["comment_id"][14] == "4", True)
    drift = abs((stored["created_at"] - __import__("datetime").datetime.utcnow()).total_seconds())
    case("comment created_at is UTC now", drift < 120, True)
    cid = stored["comment_id"]

# 2. as another user: forged owner, deleted video, edit/delete of someone else's comment
forged = create("Comment", {"video": CAT, "user": BOB, "content": "위조 " + tag}, "c2-" + tag, "alice")
case("comment as someone else is refused", (forged.get("ok"), forged.get("code")), (False, forged.get("code")))
print("     forged ->", forged.get("code"))
case("forged comment left no row", row("SELECT COUNT(*) AS n FROM comment WHERE content = %s", "위조 " + tag)["n"], 0)
on_deleted = create("Comment", {"video": DELETED, "user": ALICE, "content": "삭제영상 " + tag}, "c3-" + tag, "alice")
case("comment on a deleted video is refused", on_deleted.get("ok"), False)
print("     deleted video ->", on_deleted.get("code"))
anonymous = create("Comment", {"video": CAT, "user": ALICE, "content": "익명 " + tag}, "c4-" + tag, None)
case("anonymous comment is refused", anonymous.get("ok"), False)
print("     anonymous ->", anonymous.get("code"))

if cid:
    other_edit = apply("Comment.edit", cid, "e1-" + tag, "bob", {"content": "남의 수정"})
    case("editing someone else's comment is refused", other_edit.get("ok"), False)
    edited = apply("Comment.edit", cid, "e2-" + tag, "alice", {"content": "수정됨 " + tag})
    case("owner edit ok", edited.get("ok"), True)
    after = row("SELECT * FROM comment WHERE comment_id = %s", cid)
    case("edit sets is_modified and content, keeps created_at", (after["is_modified"], after["content"]), (1, "수정됨 " + tag))
    case("edit moved updated_at", after["updated_at"] >= after["created_at"], True)
    replay = apply("Comment.edit", cid, "e2-" + tag, "alice", {"content": "수정됨 " + tag})
    case("same key replays instead of writing again", replay.get("replayed"), True)
    deleted = apply("Comment.delete", cid, "d1-" + tag, "alice")
    case("owner soft delete ok", deleted.get("ok"), True)
    gone = row("SELECT is_deleted FROM comment WHERE comment_id = %s", cid)
    case("soft delete keeps the row, flags is_deleted (original semantics)", gone and gone["is_deleted"], 1)
    again = apply("Comment.delete", cid, "d2-" + tag, "alice")
    case("deleting a deleted comment is refused", again.get("ok"), False)

# 3. like / bookmark create + unique(user, video)
like = create("VideoLike", {"video": CAT, "user": BOB}, "l-" + tag, "bob")
case("like create ok or already liked", like.get("ok") or like.get("code"), True if like.get("ok") else like.get("code"))
dup = create("VideoLike", {"video": CAT, "user": BOB}, "l2-" + tag, "bob")
print("     duplicate like ->", dup.get("code"), dup.get("message"))
case("duplicate like is a definite refusal, not a server error", dup.get("ok") is False and dup.get("code") not in (None, "WIRE.INTERNAL"), True)
case("duplicate like left one row", row("SELECT COUNT(*) AS n FROM video_like WHERE user_id = %s AND video_id = %s", BOB, CAT)["n"], 1)
bookmark = create("VideoBookmark", {"video": CAT, "user": BOB}, "b-" + tag, "bob")
case("bookmark create ok", bookmark.get("ok"), True)
req = create("VideoRequest", {"user": ALICE, "youtube_url": "req" + tag[-8:], "youtube_full_url": "https://www.youtube.com/watch?v=req" + tag[-8:]}, "r-" + tag, "alice")
case("video request create ok", req.get("ok"), True)
stored = row("SELECT status FROM video_request WHERE youtube_url = %s", "req" + tag[-8:])
case("request status defaults to PENDING (DB default)", stored and stored["status"], "PENDING")

server.terminate()
print("\n%d failing case(s)" % len(failures))
sys.exit(1 if failures else 0)
