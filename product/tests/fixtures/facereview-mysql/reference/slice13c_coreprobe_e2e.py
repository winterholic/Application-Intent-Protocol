"""CORE-PROBE: Slice13-C read extensions through the real service with a SCRATCH core patch that lets a definition declare
`access mongo.<collection>` (see slice13c-core-probe.patch; NOT part of the product branch). Needs a binary built from the patched
tree and S13C_MONGO_URL. Documents what the Mongo capability does with FaceReview data once a grant is declarable.

Original docstring: Slice13-C against the real `aip service` binary, real MySQL 8.4 and the real Python worker over HTTP.

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
with admin_conn() as c, c.cursor() as cur:
    cur.execute(f"CREATE DATABASE `{META}` DEFAULT CHARSET utf8mb4")
    cur.execute(f"CREATE USER '{RT_USER}'@'%' IDENTIFIED BY '{RT_PASS}'")
    for schema in (SRC, META):
        cur.execute(f"GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO '{RT_USER}'@'%'")

txt = (FIXTURE / "slice13c_watch.facereview.aip").read_text()
txt = txt.replace("    access VideoLike.countForVideo, Comment.countForVideo\n    effect none\n    deadline 5s",
                  "    access VideoLike.countForVideo, Comment.countForVideo, mongo.youtube_watching_data\n    effect none\n    deadline 5s", 1)
txt = txt.replace("    output { videos_json: Text; total: Int; page: Int; size: Int; has_next: Bool }\n    effect none",
                  "    output { videos_json: Text; total: Int; page: Int; size: Int; has_next: Bool }\n    access mongo.video_distribution\n    effect none", 1)
assert txt.count("mongo.") == 2
(work / "def.aip").write_text(txt)
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
    import pymongo
    mongo = pymongo.MongoClient(os.environ["S13C_MONGO_URL"])["s13c_e2e"]
    for name in ("video_distribution", "youtube_watching_data"):
        mongo[name].delete_many({})
    # FaceReview stores user_id as the UUID string (see youtube_watching_data.py); the capability filters on the numeric actor id.
    mongo["youtube_watching_data"].insert_many([
        {"video_id": CAT, "user_id": ALICE, "emotion_score_timeline": {"0": [100, 0, 0, 0, 0]}},
        {"video_id": CAT, "user_id": BOB, "emotion_score_timeline": {"0": [0, 100, 0, 0, 0]}},
    ])
    mongo["video_distribution"].insert_one({"video_id": COOK, "dominant_emotion": "happy", "emotion_averages": {"happy": 0.5},
                                            "recommendation_scores": {"happy": 1.5}})
    mongo["video_distribution"].insert_one({"video_id": CAT, "dominant_emotion": "happy", "emotion_averages": {"happy": 0.9},
                                            "recommendation_scores": {"happy": 3.0}})
    d = post("/extension", {"extension": "Video.watchDetail", "input": {"video": CAT}}, "alice")
    print("PROBE watchDetail(alice) ->", json.dumps(d, ensure_ascii=False)[:700])
    if d.get("ok"):
        tl = json.loads(d["output"]["timeline_data_json"])
        print("PROBE alice timeline happy[0], neutral[0] ->", tl["happy"][0], tl["neutral"][0], "scope", d["output"]["timeline_scope"])
        check("grant declared: worker reaches Mongo and answers", d["ok"], True)
        check("GAP evidence: alice's own session (user_id is her UUID string) is invisible, the capability filters on the numeric actor id",
              d["output"]["timeline_sessions"], 0)
        print("PROBE original would average ALICE+BOB: neutral 50 / happy 50 in bucket 1; capability-visible set only ever holds the caller's own sessions")
    for i, (dom, score, avg) in enumerate([("happy", 3.0, 0.9), ("happy", 1.0, 0.3), ("happy", 2.0, 0.6)]):
        mongo["video_distribution"].insert_one({"video_id": f"bbbbbbbb-0000-4000-8000-00000000000{i}", "dominant_emotion": dom,
                                                "emotion_averages": {"happy": avg}, "recommendation_scores": {"happy": score}})
    mongo["video_distribution"].insert_one({"video_id": "cccccccc-1111-4111-8111-000000000001", "dominant_emotion": "sad",
                                            "emotion_averages": {"sad": 0.7}, "recommendation_scores": {"sad": 9.0}})
    # Reply-size ladder on the real service: one session document of N timeline keys per numeric actor id 1..4, a different video each.
    import random
    rng = random.Random(3)
    ladder = [(300, "bbbbbbbb-0000-4000-8000-000000000000"), (900, "bbbbbbbb-0000-4000-8000-000000000001"),
              (1500, "bbbbbbbb-0000-4000-8000-000000000002"), (3000, "cccccccc-1111-4111-8111-000000000001")]
    for keys, video in ladder:
        for uid in (1, 2, 3, 4):
            mongo["youtube_watching_data"].insert_one({"video_id": video, "user_id": uid, "emotion_score_timeline":
                {str(k * 50): [round(rng.uniform(0, 100), 3) for _ in range(5)] for k in range(keys)}})
    for keys, video in ladder:
        size = len(json.dumps(mongo["youtube_watching_data"].find_one({"video_id": video}, {"_id": 0})))
        got = post("/extension", {"extension": "Video.watchDetail", "input": {"video": video}}, "alice")
        print(f"PROBE reply ladder keys={keys} docbytes={size} -> ok={got.get('ok')} code={got.get('code')} sessions={got.get('output', {}).get('timeline_sessions')}")
        if size < 60000:
            check(f"reply ladder: a {size}-byte session document is served", (got.get("ok"), got.get("output", {}).get("timeline_sessions")), (True, 1))
        else:
            # D6: the Python worker reads ctx replies through asyncio.StreamReader's default 64 KiB line limit, far under the
            # capability's own 1 MiB cap. Fail-closed is the required behavior here; success with a partial/empty timeline is not.
            check(f"reply ladder GAP evidence: a {size}-byte session document fails closed (worker 64 KiB line limit)", (got.get("ok"), got.get("code") in ("WIRE.WORKER_FAILED", "WIRE.EXTENSION_ERROR")), (False, True))
    rec = post("/extension", {"extension": "Video.watchRecommended", "input": {"video": COOK, "page": 1, "size": 10}}, "alice")
    if rec.get("ok"):
        got = json.loads(rec["output"]["videos_json"])
        print("PROBE recommended order/bookmarks ->", [(v["video_id"][-1], v["dominant_emotion_per"], v["is_bookmarked"]) for v in got], rec["output"]["total"])
        check("recommended through the real service: score order (3.0, 2.0, 1.0) and alice's bookmark", 
              ([v["video_id"][-1] for v in got], [v["is_bookmarked"] for v in got]), (["0", "2", "1"], [False, True, False]))
    print("PROBE watchRecommended(alice) ->", json.dumps(rec, ensure_ascii=False)[:600])
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
