"""Authorization parity on the real FaceReview schema: the original admin_required / super_admin_required rules and the
AdminService effects (deactivate, role change, reject request, soft deletes) versus the adopted AIP service. Run after adopt.sh."""
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
PEOPLE = {"alice": "11111111-1111-4111-8111-111111111111", "bob": "22222222-2222-4222-8222-222222222222",
          "carol": "33333333-3333-4333-8333-333333333333", "dave": "44444444-4444-4444-8444-444444444444",
          "erin": "55555555-5555-4555-8555-555555555555"}
CAT = "aaaaaaaa-0000-4000-8000-000000000001"
REQUEST = "ffffffff-0000-4000-8000-000000000001"

for subject, actor in PEOPLE.items():
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


tag = str(int(time.time()))
counter = [0]


def apply(name, target, subject, values=None):
    counter[0] += 1
    request = {"apply": name, "target": {"ids": [target]}}
    if values:
        request["values"] = values
    return post("/apply", {"key": f"adm-{tag}-{counter[0]}", "request": request}, subject)


def read(resource, select, subject, **extra):
    return post("/read", {"query": {"read": resource, "select": select, **extra}}, subject)


def row(sql, *args):
    cur.execute(sql, args)
    return cur.fetchone()


failures = []


def case(name, got, want):
    ok = got == want
    print(("OK   " if ok else "FAIL ") + name + ("" if ok else f"\n     got  {got}\n     want {want}"))
    if not ok:
        failures.append(name)


def refused(response):
    return response.get("ok") is not True


# Reads: members see themselves, admins see everyone (admin_required), is_deleted visible to the admin filter.
mine = read("User", ["id"], "alice")
case("a general member lists only herself", [r["id"] for r in mine["rows"]], [PEOPLE["alice"]])
every = read("User", ["id", "role", "is_deleted"], "bob", sort=[{"field": "created_at", "dir": "asc"}, {"field": "id", "dir": "asc"}])
cur.execute("SELECT user_id FROM user ORDER BY created_at, user_id")
case("an admin lists every user", [r["id"] for r in every["rows"]], [r["user_id"] for r in cur.fetchall()])
case("keyword over name", [r["id"] for r in read("User", ["id"], "bob", filter=[{"field": "name", "op": "contains", "value": "앨리"}])["rows"]], [PEOPLE["alice"]])
case("anonymous lists no user", read("User", ["id"], None).get("rows"), [])

# Deactivate: SUPER_ADMIN anyone, ADMIN only GENERAL, GENERAL nobody. Comments of the member are hidden in the same transaction.
case("a general member cannot deactivate", refused(apply("User.deactivate", PEOPLE["dave"], "alice")), True)
case("a refused deactivate changed nothing", row("SELECT is_deleted FROM user WHERE user_id = %s", PEOPLE["dave"])["is_deleted"], 0)
case("an ADMIN cannot deactivate another ADMIN", refused(apply("User.deactivate", PEOPLE["erin"], "bob")), True)
case("...and the admin row is untouched", row("SELECT is_deleted FROM user WHERE user_id = %s", PEOPLE["erin"])["is_deleted"], 0)
done = apply("User.deactivate", PEOPLE["dave"], "bob")
case("an ADMIN deactivates a GENERAL member", done.get("ok"), True)
case("the member is flagged, not removed", row("SELECT is_deleted FROM user WHERE user_id = %s", PEOPLE["dave"])["is_deleted"], 1)
case("the member's comments are hidden with it", row("SELECT is_deleted FROM comment WHERE user_id = %s", PEOPLE["dave"])["is_deleted"], 1)
case("a SUPER_ADMIN deactivates an ADMIN", apply("User.deactivate", PEOPLE["erin"], "carol").get("ok"), True)

# Role change: SUPER_ADMIN only, and only for an active member.
case("an ADMIN cannot change a role", refused(apply("User.changeRole", PEOPLE["alice"], "bob", {"role": "ADMIN"})), True)
case("a deactivated member's role cannot be changed (original: not found)", refused(apply("User.changeRole", PEOPLE["dave"], "carol", {"role": "ADMIN"})), True)
case("a SUPER_ADMIN changes a role", apply("User.changeRole", PEOPLE["alice"], "carol", {"role": "ADMIN"}).get("ok"), True)
case("...stored", row("SELECT role FROM user WHERE user_id = %s", PEOPLE["alice"])["role"], "ADMIN")
case("a role outside the enum is refused", refused(apply("User.changeRole", PEOPLE["alice"], "carol", {"role": "ROOT"})), True)
apply("User.changeRole", PEOPLE["alice"], "carol", {"role": "GENERAL"})

# Video requests: admins read all and reject a pending one once.
case("a member cannot reject", refused(apply("VideoRequest.reject", REQUEST, "alice", {"admin_comment": "x"})), True)
case("a member cannot read other users' requests", [r["id"] for r in read("VideoRequest", ["id"], "dave")["rows"]], [])
case("an admin reads the request", [r["id"] for r in read("VideoRequest", ["id", "status"], "bob")["rows"]], [REQUEST])
rejected = apply("VideoRequest.reject", REQUEST, "bob", {"admin_comment": "중복 요청"})
case("an admin rejects a pending request", rejected.get("ok"), True)
stored = row("SELECT status, admin_comment FROM video_request WHERE video_request_id = %s", REQUEST)
case("...status and comment stored", (stored["status"], stored["admin_comment"]), ("REJECTED", "중복 요청"))
case("a processed request cannot be rejected again", refused(apply("VideoRequest.reject", REQUEST, "bob", {"admin_comment": "또"})), True)

# Soft deletes by an admin: the row stays, the public reads stop showing it, the admin still sees comments with the filter.
case("a member cannot remove a comment", refused(apply("Comment.remove", "cccccccc-0000-4000-8000-000000000001", "alice")), True)
case("an admin removes a comment", apply("Comment.remove", "cccccccc-0000-4000-8000-000000000001", "bob").get("ok"), True)
case("...the row stays flagged", row("SELECT is_deleted FROM comment WHERE comment_id = 'cccccccc-0000-4000-8000-000000000001'")["is_deleted"], 1)
case("members no longer see it", "cccccccc-0000-4000-8000-000000000001" in [r["id"] for r in read("Comment", ["id"], "alice", filter=[{"field": "video", "op": "eq", "value": CAT}])["rows"]], False)
case("an admin finds it with the is_deleted filter", [r["id"] for r in read("Comment", ["id"], "bob", filter=[{"field": "is_deleted", "op": "eq", "value": True}])["rows"]].count("cccccccc-0000-4000-8000-000000000001"), 1)
case("a member cannot remove a video", refused(apply("Video.remove", CAT, "alice")), True)
case("an admin removes a video", apply("Video.remove", CAT, "bob").get("ok"), True)
case("...it leaves the public list", CAT in [r["id"] for r in read("Video", ["id"], None)["rows"]], False)
case("...and its row stays", row("SELECT is_deleted FROM video WHERE video_id = %s", CAT)["is_deleted"], 1)

# A deactivated member loses access at once (the original's login_required filters is_deleted=False).
dave_read = read("User", ["id"], "dave")
print("     deactivated member's /read ->", json.dumps(dave_read, ensure_ascii=False)[:160])
case("a deactivated member is no longer authenticated", dave_read.get("ok") is not True or dave_read.get("rows") == [], True)
dave_write = post("/apply", {"key": f"adm-{tag}-dave", "request": {"atomic": [{"create": "Comment", "values": {"video": "aaaaaaaa-0000-4000-8000-000000000002", "user": PEOPLE["dave"], "content": "복귀 시도"}}]}}, "dave")
case("...and cannot write", refused(dave_write), True)
case("...no row was written", row("SELECT COUNT(*) AS n FROM comment WHERE content = '복귀 시도'")["n"], 0)

server.terminate()
print("\n%d failing case(s)" % len(failures))
sys.exit(1 if failures else 0)
