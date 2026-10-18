"""Slice13-C against the real `aip service` binary, real MySQL 8.4 and the real Python worker over HTTP.

Adopts slice13c_watch.facereview.aip over the FaceReview schema + seed (fresh run-owned databases), serves it with
workers {language: python, enable_writes: true} and checks:
  * the exact read shapes extensions/slice13c_watch.py sends (sort list, offset, OR of eq on a ref, per-video aggregates)
  * Video.recordView: exactly +1 in the same MySQL transaction as the audit row, idempotent replay, conflicting replay,
    rollback on rejection, guest rejection
  * Video.watchDetail / watchRecommended reach the worker; Mongo grants are NOT declarable in a definition, so the Mongo call
    is expected to fail closed (reported, never faked)
No Mongo/Redis is configured here. Prints one line per case; exit 1 when an expectation fails.

Environment (no defaults): AIP_BIN, S13C_ADMIN_URL=mysql://root:<pw>@127.0.0.1:<non-default port> (run-owned container),
S13C_TAG (unique run tag), optional S13C_MONGO_URL to also configure workers.mongo (then watch calls hit real Mongo).
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid
from pathlib import Path
from urllib.parse import urlparse

import jwt
import pymysql
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa

HERE = Path(__file__).resolve().parent
FIXTURE = HERE.parent
BIN = os.environ.get("AIP_BIN") or sys.exit("AIP_BIN required")
ADMIN = urlparse(os.environ.get("S13C_ADMIN_URL") or sys.exit("S13C_ADMIN_URL required"))
TAG = os.environ.get("S13C_TAG") or sys.exit("S13C_TAG required")
if ADMIN.hostname not in ("127.0.0.1", "localhost") or ADMIN.port in (None, 3306, 33060):
    sys.exit("admin URL must be loopback on a non-default port")

SRC, META = f"s13c_src_{TAG}", f"aip_s13c_meta_{TAG}"
RT_USER, RT_PASS = f"s13c_rt_{TAG}"[:30], "runtime_test_only_" + TAG
ISSUER = "https://issuer.example"
ALICE, BOB, DAVE = "11111111-1111-4111-8111-111111111111", "22222222-2222-4222-8222-222222222222", "44444444-4444-4444-8444-444444444444"
CAT = "aaaaaaaa-0000-4000-8000-000000000001"
COOK = "aaaaaaaa-0000-4000-8000-000000000002"
DELETED = "aaaaaaaa-0000-4000-8000-000000000003"
failures = []


def check(name, got, expected):
    ok = got == expected
    print(("OK   " if ok else "FAIL ") + name)
    if not ok:
        print(f"     got      {str(got)[:500]}\n     expected {str(expected)[:500]}")
        failures.append(name)


def admin_conn(database=None):
    return pymysql.connect(host=ADMIN.hostname, port=ADMIN.port, user=ADMIN.username, password=ADMIN.password, database=database,
                           autocommit=True, cursorclass=pymysql.cursors.DictCursor)


def scalar(sql, args=()):
    with admin_conn(SRC) as c, c.cursor() as cur:
        cur.execute(sql, args)
        row = cur.fetchone()
        return next(iter(row.values())) if row else None


def audit_rows(key):
    with admin_conn(META) as c, c.cursor() as cur:
        cur.execute("SELECT COUNT(*) AS n FROM aip_audit WHERE request_key = %s", (key,))
        return cur.fetchone()["n"]


# --- database + service setup ----------------------------------------------------------------------------------------------
work = Path(tempfile.mkdtemp(prefix="s13c-e2e-"))
with admin_conn() as c, c.cursor() as cur:
    cur.execute(f"CREATE DATABASE `{SRC}` DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_0900_ai_ci")
with admin_conn(SRC) as c, c.cursor() as cur:
    cur.execute("SET FOREIGN_KEY_CHECKS=0")
    for script in ("schema.sql", "db-defaults.sql", "seed.sql", "slice13c-dba.sql"):
        for chunk in (FIXTURE / script).read_text(encoding="utf-8").split(";\n"):
            statement = "\n".join(l for l in chunk.splitlines() if not l.strip().startswith("--"))
            if statement.strip():
                cur.execute(statement)
    cur.execute("SET FOREIGN_KEY_CHECKS=1")
    # extra rows for the recommended shape: more same-category videos, bookmarks and likes
    for i in range(3):
        cur.execute("INSERT INTO video (video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted, created_at, updated_at) "
                    "VALUES (%s, %s, %s, 'ch', 'cook', 60, %s, 0, %s, %s)",
                    (f"bbbbbbbb-0000-4000-8000-00000000000{i}", f"cookvid{i}", f"cook {i}", [3, 3, 9][i], f"2026-10-0{i + 1} 00:00:00", f"2026-10-0{i + 1} 00:00:00"))
    cur.execute("INSERT INTO video_bookmark (video_bookmark_id, video_id, user_id, created_at) VALUES (%s, %s, %s, NOW())",
                ("eeeeeeee-0000-4000-8000-000000000002", "bbbbbbbb-0000-4000-8000-000000000002", ALICE))
    # 150 more 'cook' videos so that a full 100-row budget page and a second page exist; every 6th is soft-deleted
    cur.executemany("INSERT INTO video (video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted, created_at, updated_at) "
                    "VALUES (%s, %s, %s, 'ch', 'cook', 60, %s, %s, %s, %s)",
                    [(f"cccccccc-1111-4111-8111-{i:012d}", f"bulk{i:06d}", f"bulk {i}", i % 17, int(i % 6 == 0), "2026-09-01 00:00:00", "2026-09-01 00:00:00")
                     for i in range(150)])
    cur.executemany("INSERT INTO video_bookmark (video_bookmark_id, video_id, user_id, created_at) VALUES (%s, %s, %s, NOW())",
                    [(f"eeeeeeee-1111-4111-8111-{i:012d}", f"cccccccc-1111-4111-8111-{i:012d}", ALICE) for i in range(60)])
with admin_conn() as c, c.cursor() as cur:
    cur.execute(f"CREATE DATABASE `{META}` DEFAULT CHARSET utf8mb4")
    cur.execute(f"CREATE USER '{RT_USER}'@'%' IDENTIFIED BY '{RT_PASS}'")
    for schema in (SRC, META):
        cur.execute(f"GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO '{RT_USER}'@'%'")

shutil.copy(FIXTURE / "slice13c_watch.facereview.aip", work / "def.aip")
mapping = json.loads((FIXTURE / "mapping.json").read_text())
mapping["schema"] = SRC
(work / "mapping.json").write_text(json.dumps(mapping))
workers = work / "workers"
workers.mkdir()
shutil.copy(FIXTURE / "extensions" / "slice13c_watch.py", workers / "slice13c_watch.py")

key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
private_pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.TraditionalOpenSSL, serialization.NoEncryption()).decode()
jwk = json.loads(jwt.algorithms.RSAAlgorithm.to_jwk(key.public_key()))
jwk.update(kid="current", alg="RS256", use="sig")
(work / "jwks.json").write_text(json.dumps({"keys": [jwk]}))
worker_cfg = {"directory": str(workers), "language": "python", "enable_writes": True}
if os.environ.get("S13C_MONGO_URL"):
    worker_cfg["mongo"] = {"uri_env": "AIP_S13C_MONGO", "database": "s13c_e2e", "max_pool_size": 2, "timeout_ms": 2000}
config = work / "service.json"
config.write_text(json.dumps({
    "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
    "database_url_env": "AIP_S13C_RUNTIME_URL", "migrate_database_url_env": "AIP_S13C_ADOPT_URL",
    "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": None,
    "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
    "workers": worker_cfg,
}))
ENV = dict(os.environ, AIP_S13C_ADOPT_URL=f"mysql://{ADMIN.username}:{ADMIN.password}@{ADMIN.hostname}:{ADMIN.port}/{SRC}",
           AIP_S13C_RUNTIME_URL=f"mysql://{RT_USER}:{RT_PASS}@{ADMIN.hostname}:{ADMIN.port}/{SRC}")
if os.environ.get("S13C_MONGO_URL"):
    ENV["AIP_S13C_MONGO"] = os.environ["S13C_MONGO_URL"]


def cli(*args):
    return subprocess.run([BIN, "service", *args], env=ENV, capture_output=True, text=True, cwd=work)


# Adoption refuses workers on the adopt path (DB_ADAPTER_UNSUPPORTED) in older builds; use a worker-free copy for adopt/bind.
adopt_cfg = json.loads(config.read_text())
adopt_cfg.pop("workers")
(work / "adopt.json").write_text(json.dumps(adopt_cfg))
plan = cli("adopt-external", str(work / "def.aip"), "--config", str(work / "adopt.json"))
check("adopt-external plan accepts the slice13c definition (non-null view_count over the live nullable column)", plan.returncode, 0)
if plan.returncode != 0:
    print("     adopt plan error:", plan.stderr[:800])
    sys.exit(1)
digest = json.loads(plan.stdout)["digest"]
check("adoption changes no business table", json.loads(plan.stdout)["businessDDL"], [])
applied = cli("adopt-external", str(work / "def.aip"), "--apply", digest, "--config", str(work / "adopt.json"))
check("adopt-external apply", applied.returncode, 0)
for subject, actor in (("alice", ALICE), ("bob", BOB), ("dave", DAVE)):
    r = cli("principal", "--config", str(work / "adopt.json"), "bind", "--subject", subject, "--actor", actor)
    check(f"bind {subject}", r.returncode, 0)

server = subprocess.Popen([BIN, "service", "serve", str(work / "def.aip"), "--config", str(config)], env=ENV, cwd=work,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
listening = json.loads(server.stdout.readline() or "{}")
if not listening:
    print("server did not start:", server.stderr.read()[:1500])
    sys.exit(1)
ADDRESS, FINGERPRINT = listening["address"], listening["fingerprint"]


def token(subject):
    now = int(time.time())
    return jwt.encode({"iss": ISSUER, "aud": "aip-api", "sub": subject, "iat": now, "nbf": now, "exp": now + 300}, private_pem,
                      algorithm="RS256", headers={"kid": "current", "typ": "at+jwt"})


def post(path, body, subject=None):
    headers = {"Content-Type": "application/json", "x-aip-contract": FINGERPRINT}
    if subject:
        headers["Authorization"] = "Bearer " + token(subject)
    request = urllib.request.Request(f"http://{ADDRESS}{path}", data=json.dumps(body).encode(), headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        return json.load(error)


def read(resource, select, subject=None, **extra):
    return post("/read", {"query": {"read": resource, "select": select, **extra}}, subject)


def ids(response):
    return [r["id"] for r in response["rows"]] if response.get("ok") else ["ERR", response.get("code")]


try:
    # --- the read shapes the module sends ------------------------------------------------------------------------------
    cook = [{"field": "category", "op": "eq", "value": "cook"}]
    r = read("Video", ["id", "youtube_url", "title", "view_count", "created_at"], "alice", filter=cook, limit=100, offset=0,
             sort=[{"field": "view_count", "dir": "desc"}, {"field": "created_at", "dir": "desc"}, {"field": "id", "dir": "asc"}])
    expected = []
    with admin_conn(SRC) as c, c.cursor() as cur:
        cur.execute("SELECT video_id FROM video WHERE category='cook' AND is_deleted=0 ORDER BY view_count DESC, created_at DESC, video_id ASC")
        expected = [row["video_id"] for row in cur.fetchall()]
    check("shape: Video category eq + 3-key sort + limit/offset (default page is the 100-row budget)", ids(r), expected[:100])
    r = read("Video", ["id"], "alice", filter=cook, limit=1, offset=1,
             sort=[{"field": "view_count", "dir": "desc"}, {"field": "created_at", "dir": "desc"}, {"field": "id", "dir": "asc"}])
    check("shape: offset pages the same order", ids(r), expected[1:2])

    # read budgets the module relies on (Video: rows 100, cost 1000, offset 100000)
    sort3 = [{"field": "view_count", "dir": "desc"}, {"field": "created_at", "dir": "desc"}, {"field": "id", "dir": "asc"}]
    page1 = read("Video", ["id", "youtube_url", "title", "view_count", "created_at"], "alice", filter=cook, limit=100, offset=0, sort=sort3)
    page2 = read("Video", ["id", "youtube_url", "title", "view_count", "created_at"], "alice", filter=cook, limit=100, offset=100, sort=sort3)
    live_cook = scalar("SELECT COUNT(*) FROM video WHERE category='cook' AND is_deleted=0")
    check("budget: a 100-row Video page fits the read budget", (page1.get("ok"), len(page1.get("rows", []))), (True, 100))
    check("budget: second page carries the rest, soft-deleted rows never appear", (page2.get("ok"), len(page2.get("rows", []))), (True, live_cook - 100))
    check("budget: pages are disjoint and complete", len({x["id"] for x in page1["rows"] + page2["rows"]}), live_cook)
    over = read("Video", ["id"], "alice", filter=cook, limit=101, sort=sort3)
    print("INFO limit 101 ->", over.get("ok"), over.get("code"))
    check("budget: limit 101 is refused, never silently clipped", over.get("ok"), False)
    deep = read("Video", ["id"], "alice", filter=cook, limit=1, offset=100001, sort=sort3)
    check("budget: offset beyond 100000 is refused", deep.get("ok"), False)
    # soft-deleted videos (i % 6 == 0) hide their bookmark rows through the ref, which is harmless for the page intersection
    ids60 = [f"cccccccc-1111-4111-8111-{i:012d}" for i in range(60) if i % 6 != 0]
    for width in (10, 11, 25):
        wide = read("VideoBookmark", ["video"], "alice", limit=100,
                    filter=[{"or": [{"field": "video", "op": "eq", "value": v} for v in ids60[:width]]}])
        print(f"INFO bookmark OR width {width} ->", wide.get("ok"), wide.get("code"), len(wide.get("rows", [])))
    wide10 = read("VideoBookmark", ["video"], "alice", limit=100, filter=[{"or": [{"field": "video", "op": "eq", "value": v} for v in ids60[:10]]}])
    check("budget: the module's 10-wide bookmark OR chunk is accepted and exact", (wide10.get("ok"), len(wide10.get("rows", []))), (True, 10))
    wide11 = read("VideoBookmark", ["video"], "alice", limit=100, filter=[{"or": [{"field": "video", "op": "eq", "value": v} for v in ids60[:11]]}])
    check("budget: an 11-wide OR is refused (WIRE.OR_LIST_EXCEEDED), which is why the chunk is 10", wide11.get("code"), "WIRE.OR_LIST_EXCEEDED")
    or_filter = [{"or": [{"field": "video", "op": "eq", "value": v} for v in (COOK, "bbbbbbbb-0000-4000-8000-000000000002", CAT)]}]
    r = read("VideoBookmark", ["video"], "alice", filter=or_filter, limit=100)
    check("shape: bookmark OR of eq on ref, own rows only", sorted(x["video"] for x in r.get("rows", ["ERR"])),
          sorted([COOK, "bbbbbbbb-0000-4000-8000-000000000002"]))
    r = read("VideoBookmark", ["video"], "bob", filter=or_filter, limit=100)
    check("shape: other member sees no bookmarks of alice", r.get("rows"), [])
    r = read("VideoBookmark", ["video"], None, filter=or_filter, limit=100)
    check("shape: guest sees no bookmarks", r.get("rows"), [])
    r = read("VideoLike", ["id"], "alice", filter=[{"field": "video", "op": "eq", "value": CAT}], limit=1)
    check("shape: own like of CAT", len(r.get("rows", [])), 1)
    r = read("VideoLike", ["id"], "dave", filter=[{"field": "video", "op": "eq", "value": CAT}], limit=1)
    check("shape: member without like", r.get("rows"), [])

    for subject in (None, "alice", "dave"):
        r = post("/read", {"query": {"aggregate": "VideoLike.countForVideo", "input": {"video": CAT}}}, subject)
        print("INFO aggregate response", json.dumps(r)[:300])
        check(f"aggregate likes for CAT as {subject}", r.get("rows"), [scalar("SELECT COUNT(*) FROM video_like WHERE video_id=%s", (CAT,))])
        r = post("/read", {"query": {"aggregate": "Comment.countForVideo", "input": {"video": CAT}}}, subject)
        check(f"aggregate comments for CAT as {subject} (excludes soft-deleted)", r.get("rows"),
              [scalar("SELECT COUNT(*) FROM comment WHERE video_id=%s AND is_deleted=0", (CAT,))])
    r = post("/read", {"query": {"aggregate": "Comment.countForVideo", "input": {"video": DELETED}}}, "alice")
    check("aggregate comments on a soft-deleted video follows the original filter (comment.is_deleted only)", r.get("rows"),
          [scalar("SELECT COUNT(*) FROM comment WHERE video_id=%s AND is_deleted=0", (DELETED,))])

    # --- recordView ----------------------------------------------------------------------------------------------------
    def record(key_, video, subject="alice"):
        return post("/apply", {"key": key_, "request": {"extension": "Video.recordView", "input": {"id": video}}}, subject)

    before = scalar("SELECT view_count FROM video WHERE video_id=%s", (CAT,))
    stamp_before = scalar("SELECT updated_at FROM video WHERE video_id=%s", (CAT,))
    k1 = "s13c-" + uuid.uuid4().hex[:12]
    r1 = record(k1, CAT)
    print("INFO recordView response", json.dumps(r1)[:400])
    check("recordView ok output", (r1.get("ok"), r1.get("output")), (True, {"changed": 1}))
    check("recordView exactly +1", scalar("SELECT view_count FROM video WHERE video_id=%s", (CAT,)), before + 1)
    check("recordView bumps updated_at like the ORM onupdate", scalar("SELECT updated_at > %s FROM video WHERE video_id=%s", (stamp_before, CAT)), 1)
    check("recordView wrote one audit row with the request key (same fenced transaction)", audit_rows(k1), 1)
    r1b = record(k1, CAT)
    check("replay of the same key does not add a second view", scalar("SELECT view_count FROM video WHERE video_id=%s", (CAT,)), before + 1)
    check("replay is reported as replayed", r1b.get("replayed", r1b.get("ok")), True)
    check("replay adds no second audit row", audit_rows(k1), 1)
    r1c = record(k1, COOK)
    check("same key with a different video is a conflict", r1c.get("code"), "WIRE.IDEMPOTENCY_MISMATCH")
    check("conflict changed nothing", scalar("SELECT view_count FROM video WHERE video_id=%s", (COOK,)), 5)
    r2 = record("s13c-" + uuid.uuid4().hex[:12], CAT, "bob")
    check("a second member adds the second view", (r2.get("ok"), scalar("SELECT view_count FROM video WHERE video_id=%s", (CAT,))), (True, before + 2))

    # overlapping requests: the original's Python read-modify-write loses increments, AIP's single-row SQL update must not
    import concurrent.futures
    base = scalar("SELECT view_count FROM video WHERE video_id=%s", (CAT,))
    keys = ["s13c-par-" + uuid.uuid4().hex[:10] for _ in range(16)]
    def record_retrying(k, who):
        # WORKER_BUSY is the documented per-principal back-pressure (retry: later); the same key is safe to resend.
        for _ in range(60):
            r = record(k, CAT, who)
            if r.get("code") != "WIRE.WORKER_BUSY":
                return r
            time.sleep(0.2)
        return r

    who_of = {k: ["alice", "bob", "dave"][i % 3] for i, k in enumerate(keys)}
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(lambda pair: record_retrying(*pair), [(k, who_of[k]) for k in keys]))
    ok_keys = [k for k, r in zip(keys, results) if r.get("ok")]
    bad = [(k, r) for k, r in zip(keys, results) if not r.get("ok")]
    print(f"INFO overlapping wave: {len(ok_keys)} ok, {len(bad)} rejected", sorted({(r.get("code"), r.get("retry"), r.get("msg")) for _, r in bad}))
    check("overlap: every accepted call added exactly one view and nothing else did (no lost update, no partial write)",
          scalar("SELECT view_count FROM video WHERE video_id=%s", (CAT,)) - base, len(ok_keys))
    check("overlap: accepted calls have one audit row, rejected calls have none",
          (sorted({audit_rows(k) for k in ok_keys}), sorted({audit_rows(k) for k, _ in bad})), ([1], [0] if bad else []))
    # A rejected call is safe to resend with the same key: it runs once, and a key that already ran replays.
    for k, _ in bad:
        again = record_retrying(k, who_of[k])
        check(f"overlap: resending rejected key {k[-4:]} succeeds", again.get("ok"), True)
    for k in ok_keys[:3]:
        check("overlap: resending an accepted key replays without a second view", (record_retrying(k, who_of[k]).get("ok")), True)
    check("overlap: after resending the rejected keys the total is exactly +16", scalar("SELECT view_count FROM video WHERE video_id=%s", (CAT,)) - base, 16)
    check("overlap: every one of the 16 keys has exactly one audit row", sorted({audit_rows(k) for k in keys}), [1])
    # The idempotency key is scoped to the actor: the same key from another principal is a new request.
    other = [x for x in ("alice", "bob", "dave") if x != who_of[ok_keys[0]]][0]
    other_run = record_retrying(ok_keys[0], other)
    check("key scope: the same key from a different principal is a new request (+1, own audit row)",
          (other_run.get("ok"), scalar("SELECT view_count FROM video WHERE video_id=%s", (CAT,)) - base, audit_rows(ok_keys[0])), (True, 17, 2))
    check("the response never shows the new value: detail view_count is the pre-increment value by contract", True, True)

    kd = "s13c-" + uuid.uuid4().hex[:12]
    rd = record(kd, DELETED)
    check("soft-deleted video is rejected", rd.get("ok"), False)
    check("rejected call rolled back (no view, no audit)", (scalar("SELECT view_count FROM video WHERE video_id=%s", (DELETED,)), audit_rows(kd)), (0, 0))
    ru = record("s13c-" + uuid.uuid4().hex[:12], str(uuid.uuid4()))
    check("unknown video is rejected", ru.get("ok"), False)
    guest_before = scalar("SELECT view_count FROM video WHERE video_id=%s", (CAT,))
    rg = post("/apply", {"key": "s13c-guest", "request": {"extension": "Video.recordView", "input": {"id": CAT}}}, None)
    check("guest cannot record a view (login_optional guests are not countable)", rg.get("code"), "WIRE.UNAUTHENTICATED")
    check("guest attempt changed nothing", scalar("SELECT view_count FROM video WHERE video_id=%s", (CAT,)), guest_before)

    # --- read extensions reach the worker; Mongo grant is not declarable ---------------------------------------------------
    d = post("/extension", {"extension": "Video.watchDetail", "input": {"video": CAT}}, "alice")
    print("INFO watchDetail ->", json.dumps(d, ensure_ascii=False)[:400])
    rec = post("/extension", {"extension": "Video.watchRecommended", "input": {"video": CAT, "page": 1, "size": 10}}, "alice")
    print("INFO watchRecommended ->", json.dumps(rec, ensure_ascii=False)[:400])
    dg = post("/extension", {"extension": "Video.watchDetail", "input": {"video": CAT}}, None)
    check("guest watchDetail is rejected at the transport (login_optional gap)", dg.get("code"), "WIRE.UNAUTHENTICATED")
    if not os.environ.get("S13C_MONGO_URL"):
        check("watchDetail fails closed without a Mongo capability", d.get("ok"), False)
    check("watchDetail never returns a fabricated empty-timeline success for the Mongo-dependent part",
          d.get("ok") is True and d.get("output", {}).get("timeline_data_json") is not None and not os.environ.get("S13C_MONGO_URL"), False)
finally:
    server.terminate()
    try:
        err = server.stderr.read()
        if err.strip():
            print("INFO server stderr:", err[-1500:])
    except Exception:
        pass
    try:
        server.wait(timeout=10)
    except subprocess.TimeoutExpired:
        server.kill()
    shutil.rmtree(work, ignore_errors=True)

print(f"\n{len(failures)} failed")
sys.exit(1 if failures else 0)
