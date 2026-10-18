"""WBS 7.2.3 Slice13-B: FaceReview home/search, home/video/all and home/bookmark through AIP read extensions versus the ORIGINAL HomeService.

Evidence levels in the output (never merged):
  ORACLE  the pinned original's own HomeService.get_search_videos / get_all_videos / get_bookmark_videos, imported from the
          original checkout and run against the same MySQL and MongoDB the AIP side reads (not a copy of the algorithm).
  HTTP    POST /extension to the real worker-backed AIP server (slice13b_host.rs test binary), real MySQL adapter, real
          MongoDB driver, Python worker process.

Everything is explicit: this script has no default endpoint, password or path. Required environment:
  S13B_MYSQL_ADMIN_URL     mysql://root:...@127.0.0.1:<high port>/   (run-owned MySQL 8.4 container)
  S13B_MONGO_URL           mongodb://127.0.0.1:<high port>            (run-owned MongoDB container, no auth)
  S13B_AIP_BIN             built `aip` binary (cargo build -p aip-cli --features aip-cli/mysql)
  S13B_HOST_BIN            built slice13b_host test executable (cargo test -p aip-service --features mysql --test slice13b_host --no-run)
  S13B_ORIGINAL_ROOT       checkout of new-facereview at 7948b7e721837fdb73c427cb54e12cad4ec2d5af (read only; revision is verified)
  S13B_WORK_DIR            empty scratch directory owned by this run
Run with the Python that has flask, flask_sqlalchemy, pymysql, pymongo, cryptography (the original's requirements).
"""
import base64
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timedelta
from pathlib import Path

import pymongo
import pymysql

HERE = Path(__file__).resolve().parent
FIXTURE = HERE.parent
REPO = FIXTURE.parents[3]
PINNED = "7948b7e721837fdb73c427cb54e12cad4ec2d5af"
SOURCE_DB, META_DB, MONGO_DB = "s13b_src", "aip_s13b_meta", "s13b_fr"
ISSUER = "https://issuer.example"
ALICE = "11111111-1111-4111-8111-111111111111"
BOB = "22222222-2222-4222-8222-222222222222"
CAROL = "33333333-3333-4333-8333-333333333333"
DAVE = "44444444-4444-4444-8444-444444444444"
ERIN = "55555555-5555-4555-8555-555555555555"
SUBJECTS = {"alice": ALICE, "bob": BOB, "carol": CAROL, "dave": DAVE, "erin": ERIN}
RUNTIME_USER, RUNTIME_PASSWORD = "s13b_rt", "s13b_runtime_only"


def need(name):
    value = os.environ.get(name)
    if not value:
        sys.exit(f"set {name}")
    return value


def check_endpoint(url, what):
    parsed = urllib.parse.urlparse(url)
    port = parsed.port or 0
    if parsed.hostname not in ("127.0.0.1", "localhost", "::1") or port <= 1024 or port in (3306, 5432, 27017):
        sys.exit(f"{what}: loopback host and an explicit non-default high port are required")
    return parsed


# ---- definition: canonical facereview.aip + only what the three extensions need ---------------------------------------------

EXTENSIONS = '''
  extension read homeSearch {
    input { page: Int(1..100000); size: Int(1..100); keyword_type: SearchKeywordType; keyword: Text; emotions: Text }
    output { videos: Text; total: Int(0..1000000000); page: Int(1..100000); size: Int(1..100); has_next: Bool }
    effect none
    deadline 20s
    implementation "slice13b_lists.homeSearch"
  }
  extension read homeVideoAll {
    input { page: Int(1..100000); size: Int(1..100); emotion: ListEmotion }
    output { videos: Text; total: Int(0..1000000000); page: Int(1..100000); size: Int(1..100); has_next: Bool }
    effect none
    deadline 20s
    implementation "slice13b_lists.homeVideoAll"
  }
  extension read homeBookmark {
    input { page: Int(1..100000); size: Int(1..100); emotion: ListEmotion }
    output { videos: Text; total: Int(0..1000000000); page: Int(1..100000); size: Int(1..100); has_next: Bool }
    effect none
    deadline 20s
    implementation "slice13b_lists.homeBookmark"
  }
'''


def patched_definition():
    text = (FIXTURE / "facereview.aip").read_text()

    def once(old, new):
        nonlocal text
        if text.count(old) != 1:
            sys.exit(f"canonical definition changed; `{old}` is not unique")
        text = text.replace(old, new, 1)

    once("enum RequestStatus { PENDING, ACCEPTED, REJECTED }\n",
         "enum RequestStatus { PENDING, ACCEPTED, REJECTED }\n"
         "enum SearchKeywordType { all, title, channel_name }\nenum ListEmotion { all, happy, sad, neutral, surprise, angry }\n")
    once("    filter id.eq, category.eq, title.like, channel_name.like, youtube_url.eq\n",
         "    filter id.eq, id.in, category.eq, title.like, channel_name.like, youtube_url.eq\n")
    once("  expose aggregate totalCount\n", "  expose aggregate totalCount\n" + EXTENSIONS)
    start = text.index("resource VideoBookmark {")
    end = text.index("resource VideoRequest {")
    block = text[start:end]
    if block.count("    filter video.eq\n") != 1:
        sys.exit("VideoBookmark filter changed")
    text = text[:start] + block.replace("    filter video.eq\n", "    filter video.eq, video.in\n") + text[end:]
    return text


# ---- data -------------------------------------------------------------------------------------------------------------------

def uuid_for(prefix, n):
    return f"{prefix}-0000-4000-8000-{n:012d}"


def video_rows():
    """(video_id, youtube_url, title, channel, category, is_deleted, created_at) beyond the canonical seed."""
    rows = []
    base = datetime(2026, 9, 1, 0, 0, 0)
    channels = ["냥채널", "쿡채널", "여행채널", "게임 채널", None]
    for n in range(1, 131):
        title = f"고양이 영상 {n}" if n % 3 == 0 else (f"요리 레시피 {n}" if n % 3 == 1 else f"브이로그 {n} 100%_라이브")
        deleted = 1 if n in (7, 61) else 0
        rows.append((uuid_for("bbbbbbbb", n), f"yt{n:09d}", title, channels[n % 5], ["animal", "cook", "vlog", "game", "travel"][n % 5], deleted,
                     base + timedelta(minutes=n * 7)))
    return rows


def mongo_docs(video_ids_with_created):
    """One distribution per video except a few; created_at is the sort key of the Mongo-first lists."""
    emotions = ["neutral", "happy", "surprise", "sad", "angry"]
    docs = []
    for index, (video_id, created) in enumerate(video_ids_with_created):
        if index % 17 == 5:
            continue  # a video without a distribution document
        dominant = emotions[index % 5]
        averages = {e: 0.05 for e in emotions}
        averages[dominant] = round(0.4 + (index % 7) * 0.07, 4)
        doc = {"video_id": video_id, "average_completion_rate": 0.5, "emotion_averages": averages,
               "recommendation_scores": {e: 0.0 for e in emotions}, "dominant_emotion": dominant, "created_at": created,
               "updated_at": created, "total_frames": 100 + index}
        if index % 19 == 3:
            doc["dominant_emotion"] = None  # not enough frames yet: null stays null
        if index % 23 == 11:
            del doc["dominant_emotion"]  # key missing: from_dict / .get default 'neutral'
        docs.append(doc)
    return docs


def seed_mysql(admin_url):
    parsed = urllib.parse.urlparse(admin_url)
    conn = pymysql.connect(host=parsed.hostname, port=parsed.port, user=urllib.parse.unquote(parsed.username),
                           password=urllib.parse.unquote(parsed.password or ""), autocommit=True, charset="utf8mb4")
    cur = conn.cursor()
    for db in (SOURCE_DB, META_DB):
        cur.execute(f"DROP DATABASE IF EXISTS `{db}`")
    cur.execute(f"CREATE DATABASE `{SOURCE_DB}` DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_0900_ai_ci")
    cur.execute(f"USE `{SOURCE_DB}`")
    cur.execute("SET FOREIGN_KEY_CHECKS=0")

    def script(text):
        for chunk in text.split(";\n"):
            statement = "\n".join(line for line in chunk.splitlines() if not line.lstrip().startswith("--"))
            if statement.strip():
                cur.execute(statement)

    script((FIXTURE / "schema.sql").read_text())
    script((FIXTURE / "seed.sql").read_text())
    cur.execute("SET FOREIGN_KEY_CHECKS=1")
    script((FIXTURE / "db-defaults.sql").read_text())
    videos = video_rows()
    cur.executemany("INSERT INTO video (video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted, created_at, updated_at)"
                    " VALUES (%s,%s,%s,%s,%s,10,0,%s,%s,%s)", [(v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[6]) for v in videos])
    # alice bookmarks 101 videos (more than one Mongo $in), bob a few including a deleted one, dave none
    bookmark_ids = [uuid_for("bbbbbbbb", n) for n in range(1, 102)]
    bookmarks = [(uuid_for("eeeeeeee", 100 + i), vid, ALICE, datetime(2026, 10, 1) + timedelta(minutes=i)) for i, vid in enumerate(bookmark_ids)]
    bookmarks += [(uuid_for("eeeeeeee", 300 + i), vid, BOB, datetime(2026, 10, 2) + timedelta(minutes=i))
                  for i, vid in enumerate([uuid_for("bbbbbbbb", 3), uuid_for("bbbbbbbb", 7), uuid_for("bbbbbbbb", 9), "aaaaaaaa-0000-4000-8000-000000000003"])]
    cur.executemany("INSERT INTO video_bookmark (video_bookmark_id, video_id, user_id, created_at) VALUES (%s,%s,%s,%s)", bookmarks)
    cur.execute("DROP USER IF EXISTS %s@'%%'", (RUNTIME_USER,))
    cur.execute(f"CREATE USER %s@'%%' IDENTIFIED BY %s", (RUNTIME_USER, RUNTIME_PASSWORD))
    for db in (SOURCE_DB, META_DB):
        cur.execute(f"GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{db}`.* TO %s@'%%'", (RUNTIME_USER,))
    cur.execute("SELECT video_id, created_at FROM video")
    created = dict(cur.fetchall())
    conn.close()
    return videos, created


def seed_mongo(mongo_url, created):
    client = pymongo.MongoClient(mongo_url)
    client.drop_database(MONGO_DB)
    coll = client[MONGO_DB]["video_distribution"]
    coll.create_index("video_id", unique=True)
    ordered = sorted((item for item in created.items() if item[0].startswith("bbbbbbbb")), key=lambda item: item[0])
    # S13B_TIE_PROBE=1 makes ten documents share one created_at: the known single-key sort limitation of the host shows up there
    tie = datetime(2026, 9, 30, 12, 0, 0)
    tied = os.environ.get("S13B_TIE_PROBE") == "1"
    pairs = [(vid, tie if tied and 20 <= i < 30 else ts) for i, (vid, ts) in enumerate(ordered)]
    docs = mongo_docs(pairs)
    # the canonical deleted video 0003 keeps a distribution: the original shows it in video/all, AIP must not
    docs.append({"video_id": "aaaaaaaa-0000-4000-8000-000000000003", "emotion_averages": {"happy": 0.9, "neutral": 0.1, "surprise": 0, "sad": 0, "angry": 0},
                 "dominant_emotion": "happy", "created_at": datetime(2026, 10, 20), "total_frames": 50})
    docs.append({"video_id": "aaaaaaaa-0000-4000-8000-000000000001", "emotion_averages": {"happy": 0.7, "neutral": 0.1, "surprise": 0.1, "sad": 0.05, "angry": 0.05},
                 "dominant_emotion": "happy", "created_at": datetime(2026, 10, 21), "total_frames": 90})
    docs.append({"video_id": "aaaaaaaa-0000-4000-8000-000000000002", "emotion_averages": {"happy": 0.1, "neutral": 0.6, "surprise": 0.1, "sad": 0.1, "angry": 0.1},
                 "dominant_emotion": "neutral", "created_at": datetime(2026, 10, 22), "total_frames": 90})
    # a distribution for a video id that has no RDB row at all (original quirk: total counts it, the list cannot show it)
    docs.append({"video_id": "ffffffff-0000-4000-8000-0000000000aa", "emotion_averages": {"happy": 1.0}, "dominant_emotion": "happy",
                 "created_at": datetime(2026, 10, 23), "total_frames": 99})
    for doc in docs:
        coll.insert_one(doc)
    client.close()
    return len(docs)


# ---- AIP deployment --------------------------------------------------------------------------------------------------------

def adopt(work, admin_url, bin_path):
    parsed = urllib.parse.urlparse(admin_url)
    host = f"{parsed.hostname}:{parsed.port}"
    definition = work / "facereview.aip"
    definition.write_text(patched_definition())
    mapping = json.loads((FIXTURE / "mapping.json").read_text())
    mapping["schema"] = SOURCE_DB
    (work / "mapping.json").write_text(json.dumps(mapping))
    from cryptography.hazmat.primitives.asymmetric import rsa
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    numbers = key.public_key().public_numbers()
    b64 = lambda value: base64.urlsafe_b64encode(value.to_bytes((value.bit_length() + 7) // 8, "big")).rstrip(b"=").decode()
    (work / "jwks.json").write_text(json.dumps({"keys": [{"kty": "RSA", "kid": "current", "alg": "RS256", "use": "sig", "n": b64(numbers.n), "e": b64(numbers.e)}]}))
    config = work / "service.json"
    config.write_text(json.dumps({
        "schema": META_DB, "external_mapping": "mapping.json", "database_adapter": "mysql",
        "database_url_env": "S13B_RUNTIME_URL", "migrate_database_url_env": "S13B_ADOPT_URL", "listen": "127.0.0.1:0", "wire": "decimal",
        "rate_limit": None, "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}}}))
    env = dict(os.environ, S13B_ADOPT_URL=admin_url.rstrip("/") + f"/{SOURCE_DB}",
               S13B_RUNTIME_URL=f"mysql://{RUNTIME_USER}:{RUNTIME_PASSWORD}@{host}/{SOURCE_DB}")

    def cli(*args):
        out = subprocess.run([bin_path, "service", *args], env=env, capture_output=True, text=True, cwd=work)
        if out.returncode != 0:
            sys.exit(f"aip {' '.join(args)} failed: {out.stderr or out.stdout}")
        return json.loads(out.stdout)

    # Without the DBA defaults adoption refuses; db-defaults.sql was applied in seed_mysql, so the plan is clean.
    plan = cli("adopt-external", str(definition), "--config", str(config))
    cli("adopt-external", str(definition), "--config", str(config), "--apply", plan["digest"])
    for subject, actor in SUBJECTS.items():
        cli("principal", "--config", str(config), "bind", "--subject", subject, "--actor", actor)
    return env, definition


class Host:
    def __init__(self, work, env, host_bin, mongo_url, definition):
        self.ready, self.stop = work / "ready.json", work / "stop"
        host_env = dict(env, AIP_S13B_DEFINITION=str(definition), AIP_S13B_MAPPING=str(work / "mapping.json"), AIP_S13B_META=META_DB,
                        AIP_S13B_RUNTIME_URL=env["S13B_RUNTIME_URL"], AIP_S13B_MONGO_URL=mongo_url, AIP_S13B_MONGO_DB=MONGO_DB,
                        AIP_S13B_EXT_DIR=str(FIXTURE / "extensions"), AIP_S13B_READY_FILE=str(self.ready), AIP_S13B_STOP_FILE=str(self.stop),
                        RUST_BACKTRACE="1")
        self.log = open(work / "host.log", "w")
        self.proc = subprocess.Popen([host_bin, "--ignored", "--nocapture", "--test-threads=1"], env=host_env, stdout=self.log, stderr=subprocess.STDOUT, cwd=REPO)
        for _ in range(150):
            if self.ready.exists():
                info = json.loads(self.ready.read_text())
                self.address, self.fingerprint = info["address"], info["fingerprint"]
                return
            if self.proc.poll() is not None:
                break
            time.sleep(0.2)
        self.log.flush()
        sys.exit("host did not start:\n" + (work / "host.log").read_text()[-3000:])

    def post(self, extension, input, subject=None):
        request = urllib.request.Request(f"http://{self.address}/extension", data=json.dumps({"extension": extension, "input": input}).encode(),
                                         headers={"content-type": "application/json", "x-aip-contract": self.fingerprint})
        if subject:
            request.add_header("authorization", f"Bearer {subject}")
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                return json.loads(response.read())
        except urllib.error.HTTPError as error:
            return json.loads(error.read() or b"{}") | {"_http": error.code}

    def close(self):
        self.stop.write_text("stop")
        try:
            self.proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            self.proc.kill()


# ---- oracle ----------------------------------------------------------------------------------------------------------------

def load_original(root, mysql_admin_url, mongo_url):
    head = subprocess.run(["git", "-C", root, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    dirty = subprocess.run(["git", "-C", root, "status", "--porcelain", "--", "app", "common"], capture_output=True, text=True).stdout.strip()
    if head != PINNED or dirty:
        sys.exit(f"original checkout must be clean at {PINNED}; head={head} dirty={bool(dirty)}")
    sys.dont_write_bytecode = True
    sys.path.insert(0, root)
    from flask import Flask
    import common.extensions as extensions
    parsed = urllib.parse.urlparse(mysql_admin_url)
    app = Flask("slice13b-oracle")
    app.config["SQLALCHEMY_DATABASE_URI"] = (f"mysql+pymysql://{urllib.parse.unquote(parsed.username)}:{urllib.parse.quote(urllib.parse.unquote(parsed.password or ''))}"
                                             f"@{parsed.hostname}:{parsed.port}/{SOURCE_DB}?charset=utf8mb4")
    app.config["SQLALCHEMY_TRACK_MODIFICATIONS"] = False
    extensions.db.init_app(app)
    extensions.mongo_db = pymongo.MongoClient(mongo_url)[MONGO_DB]  # must exist before home_service is imported
    from app.services.home_service import HomeService
    return app, HomeService


def run_original(app, service, kind, subject_id, **args):
    with app.app_context():
        if kind == "search":
            return service.get_search_videos(args["page"], args["size"], args["keyword_type"], args["keyword"], args["emotions"], subject_id)
        if kind == "all":
            result = service.get_all_videos(args["page"], args["size"], args["emotion"], subject_id)
        else:
            result = service.get_bookmark_videos(subject_id, args["page"], args["size"], args["emotion"])
        return result.to_dict()


def normal(original):
    return {"videos": [v.to_dict() if hasattr(v, "to_dict") else v for v in original["videos"]], "total": original["total"], "page": original["page"],
            "size": original["size"], "has_next": original["has_next"]}


def port_result(response):
    if not response.get("ok"):
        return {"error": response}
    out = response["output"]
    return {"videos": json.loads(out["videos"]), "total": out["total"], "page": out["page"], "size": out["size"], "has_next": out["has_next"]}


def by_id(videos):
    return sorted(videos, key=lambda v: v["video_id"])


def main():
    admin_url = need("S13B_MYSQL_ADMIN_URL")
    mongo_url = need("S13B_MONGO_URL")
    check_endpoint(admin_url, "S13B_MYSQL_ADMIN_URL")
    check_endpoint(mongo_url, "S13B_MONGO_URL")
    work = Path(need("S13B_WORK_DIR"))
    work.mkdir(parents=True, exist_ok=True)
    if any(work.iterdir()):
        sys.exit("S13B_WORK_DIR must be empty")
    print("== seeding run-owned MySQL and MongoDB")
    videos, created = seed_mysql(admin_url)
    print("mongo documents:", seed_mongo(mongo_url, created))
    print("== adopting the patched FaceReview definition with the real aip binary")
    env, definition = adopt(work, admin_url, need("S13B_AIP_BIN"))
    app, service = load_original(need("S13B_ORIGINAL_ROOT"), admin_url, mongo_url)
    host = Host(work, env, need("S13B_HOST_BIN"), mongo_url, definition)
    if os.environ.get("S13B_HOLD"):  # debugging aid: keep the host up for manual requests until <work>/hold-done exists
        print(json.dumps({"address": host.address, "fingerprint": host.fingerprint}), flush=True)
        while not (work / "hold-done").exists():
            time.sleep(0.5)
        host.close()
        return
    failures, count = [], 0
    deleted_ids = {v[0] for v in videos if v[5]} | {"aaaaaaaa-0000-4000-8000-000000000003"}

    def case(name, kind, extension, subject, args, ordered=False, expect_hidden_deleted=False, expect_error=None):
        nonlocal count
        count += 1
        actor = SUBJECTS.get(subject)
        original = normal(run_original(app, service, kind, actor, **args))
        ext_input = dict(args)
        if kind == "search":
            ext_input["emotions"] = ",".join(args["emotions"])
        response = host.post(extension, ext_input, subject)
        port = port_result(response)
        if expect_error:
            ok = "error" in port and expect_error in json.dumps(port["error"])
            print(f"{'PASS' if ok else 'FAIL'} {name}: expected error {expect_error}: {json.dumps(port)[:200]}")
            if not ok:
                failures.append(name)
            return
        if "error" in port:
            print(f"FAIL {name}: AIP error {json.dumps(port['error'], ensure_ascii=False)[:300]}")
            failures.append(name)
            return
        want = original["videos"]
        if expect_hidden_deleted:
            hidden = [v for v in want if v["video_id"] in deleted_ids]
            want = [v for v in want if v["video_id"] not in deleted_ids]
        else:
            hidden = []
        meta_ok = all(original[k] == port[k] for k in ("total", "page", "size", "has_next"))
        same = (want == port["videos"]) if ordered else (by_id(want) == by_id(port["videos"]))
        ok = meta_ok and same
        note = f" (original showed {len(hidden)} deleted video(s) that AIP policy hides)" if hidden else ""
        print(f"{'PASS' if ok else 'FAIL'} {name}: total={port['total']} len={len(port['videos'])} has_next={port['has_next']}{note}")
        if not ok:
            print("   original:", json.dumps({k: original[k] for k in ('total', 'page', 'size', 'has_next')}), [v['video_id'][-4:] for v in want][:12])
            print("   aip     :", json.dumps({k: port[k] for k in ('total', 'page', 'size', 'has_next')}), [v['video_id'][-4:] for v in port['videos']][:12])
            failures.append(name)

    try:
        for subject in ("alice",):
            who = subject or "guest"
            for emotion in ("all", "happy", "neutral", "sad"):
                case(f"video/all {who} {emotion} p1 size10", "all", "Video.homeVideoAll", subject, {"page": 1, "size": 10, "emotion": emotion}, expect_hidden_deleted=True)
            case(f"video/all {who} all p3 size20", "all", "Video.homeVideoAll", subject, {"page": 3, "size": 20, "emotion": "all"}, expect_hidden_deleted=True)
            case(f"video/all {who} beyond last page", "all", "Video.homeVideoAll", subject, {"page": 99, "size": 100, "emotion": "all"}, expect_hidden_deleted=True)
            for kt, kw in (("title", "고양이"), ("channel_name", "쿡"), ("all", "냥"), ("all", "100%"), ("title", "없는제목")):
                case(f"search {who} {kt}:{kw} no-emotion p1", "search", "Video.homeSearch", subject,
                     {"page": 1, "size": 10, "keyword_type": kt, "keyword": kw, "emotions": []})
            case(f"search {who} all:영상 p2 size 7", "search", "Video.homeSearch", subject,
                 {"page": 2, "size": 7, "keyword_type": "all", "keyword": "영상", "emotions": []})
            case(f"search {who} title:고양이 multi-emotion", "search", "Video.homeSearch", subject,
                 {"page": 1, "size": 10, "keyword_type": "title", "keyword": "고양이", "emotions": ["happy", "sad"]})
            case(f"search {who} all:브이로그 five emotions p2", "search", "Video.homeSearch", subject,
                 {"page": 2, "size": 12, "keyword_type": "all", "keyword": "브이로그", "emotions": ["happy", "sad", "neutral", "surprise", "angry"]})
            case(f"search {who} emotion filter empty result", "search", "Video.homeSearch", subject,
                 {"page": 1, "size": 10, "keyword_type": "title", "keyword": "없는제목", "emotions": ["happy"]})
        # more than one Mongo $in (126 visible candidates) in the emotion-filtered search: exact client-side (created_at, _id) merge
        case("search alice all:'' (all videos) happy+sad p1 size 20", "search", "Video.homeSearch", "alice",
             {"page": 1, "size": 20, "keyword_type": "all", "keyword": "", "emotions": ["happy", "sad"]})
        case("search alice all:'' (all videos) every emotion p2 size 50", "search", "Video.homeSearch", "alice",
             {"page": 2, "size": 50, "keyword_type": "all", "keyword": "", "emotions": ["happy", "sad", "neutral", "surprise", "angry"]})
        case("search alice title:_라이브 (LIKE wildcard kept as in the original)", "search", "Video.homeSearch", "alice",
             {"page": 1, "size": 10, "keyword_type": "title", "keyword": "_라이브", "emotions": []})
        case("video/all alice all p2 size 100", "all", "Video.homeVideoAll", "alice", {"page": 2, "size": 100, "emotion": "all"}, expect_hidden_deleted=True)
        # Inputs the original route schemas reject (422) are refused at the extension boundary; none of them reach the data stores.
        for name, extension, ext_input in (
            ("unknown emotion", "Video.homeVideoAll", {"page": 1, "size": 10, "emotion": "joy"}),
            ("page 0", "Video.homeVideoAll", {"page": 0, "size": 10, "emotion": "all"}),
            ("size 101", "Video.homeVideoAll", {"page": 1, "size": 101, "emotion": "all"}),
            ("keyword_type other", "Video.homeSearch", {"page": 1, "size": 10, "keyword_type": "tag", "keyword": "a", "emotions": ""}),
            ("bookmark unknown emotion", "Video.homeBookmark", {"page": 1, "size": 10, "emotion": "joy"}),
        ):
            response = host.post(extension, ext_input, "alice")
            ok = response.get("ok") is False and response.get("code") in ("WIRE.BAD_VALUE", "WIRE.VALUE_TOO_LONG", "WIRE.BAD_REQUEST")
            print(f"{'PASS' if ok else 'FAIL'} reject {name}: {response.get('code')}")
            count += 1
            if not ok:
                failures.append(f"reject {name}")
        response = host.post("Video.homeVideoAll", {"page": 1, "size": 10, "emotion": "all"}, "nobody")
        print(f"{'PASS' if response.get('code') == 'WIRE.UNAUTHENTICATED' else 'FAIL'} unknown principal subject: {response.get('code')}")
        count += 1
        # Skip beyond the host's Mongo skip cap is refused instead of answered from a truncated window.
        response = host.post("Video.homeVideoAll", {"page": 1002, "size": 100, "emotion": "all"}, "alice")
        refused = response.get("ok") is False
        print(f"{'PASS' if refused else 'FAIL'} page 1002 x size 100 beyond the Mongo skip cap: {response.get('code')}")
        count += 1
        if not refused:
            failures.append("skip cap")
        # login_optional endpoints of the original serve guests; the host's extension route requires a signed session (core gap).
        for extension, ext_input in (("Video.homeVideoAll", {"page": 1, "size": 10, "emotion": "all"}),
                                     ("Video.homeSearch", {"page": 1, "size": 10, "keyword_type": "all", "keyword": "냥", "emotions": ""})):
            response = host.post(extension, ext_input, None)
            known = response.get("code") == "WIRE.UNAUTHENTICATED"
            print(f"{'KNOWN-GAP' if known else 'FAIL'} guest {extension}: {response.get('code')} (original answers guests; core dependency)")
            if not known:
                failures.append(f"guest {extension}")
        for subject in ("alice", "bob", "dave"):
            for emotion in ("all", "happy"):
                case(f"bookmark {subject} {emotion} p1", "bookmark", "Video.homeBookmark", subject, {"page": 1, "size": 20, "emotion": emotion}, expect_hidden_deleted=True)
            case(f"bookmark {subject} all p2 size 50", "bookmark", "Video.homeBookmark", subject, {"page": 2, "size": 50, "emotion": "all"}, expect_hidden_deleted=True)
        # An admin has no special reading rights over somebody else's private bookmark list.
        case("bookmark erin (admin, no bookmarks)", "bookmark", "Video.homeBookmark", "erin", {"page": 1, "size": 20, "emotion": "all"})
        print(f"\n{count} oracle cases, {len(failures)} failure(s)")
    finally:
        host.close()
    if failures:
        print("FAILED:", ", ".join(failures))
        sys.exit(1)


if __name__ == "__main__":
    main()
