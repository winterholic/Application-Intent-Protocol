"""WBS 7.2.7: the trending video collection of the AIP job versus the ORIGINAL YoutubeTrendingJob.

  ORACLE  the pinned original's YoutubeTrendingJob (common/scheduler/jobs/youtube_trending_job.py), imported from the original checkout:
            1. its _map_youtube_category and _classify_category_by_keywords on a table of videos, and the isodate duration it uses;
            2. its execute() run against a stand-in YouTube (requests.get is replaced, nothing leaves the machine) on the same MySQL and
               MongoDB the AIP side uses, then the rows and documents it left.
  HTTP    POST /jobs/enqueue of `fetchTrendingVideos` to `aip service serve` (workers.mongo with write collections, the query-string
          integration youtube_videos pointing at a local stand-in YouTube server), as the administrator the job principal is.

The rows (youtube_url, title, channel, category, duration, view_count, is_deleted) and both MongoDB documents of every new video must be
equal. Two differences are intended and printed: the original saves a video without a title (an empty string, which the Video field
refuses) and an empty channel name as an empty string (AIP stores NULL).

Required environment: S14B_MYSQL_ADMIN_URL, S14B_MONGO_URL, S14B_AIP_BIN, S14B_ORIGINAL_ROOT, S14B_WORK_DIR (empty), as in
slice14a_oracle.py. Run with the Python that has flask, flask_sqlalchemy, pymysql, pymongo, isodate, requests, cryptography and PyJWT,
and not from a directory that holds a numbers.py.
"""
import http.server
import json
import os
import shutil
import subprocess
import sys
import threading
import time
import urllib.parse
from pathlib import Path

import pymongo
import pymysql

sys.path.insert(0, str(Path(__file__).resolve().parent))
import slice13d_oracle as base  # noqa: E402


def video(id, title, channel, category, duration, tags=(), description=""):
    return {"id": id, "snippet": {"title": title, "channelTitle": channel, "categoryId": category, "tags": list(tags), "description": description},
            "contentDetails": {"duration": duration}, "statistics": {"viewCount": "12345"}}


PAGES = [
    [video("ytnew00001", "Official MV", "Music Channel", "10", "PT3M25S"),
     video("ytnew00002", "Weekend table", "Home Chef", "26", "PT1H2M3S", ["table"], "맛있는 집밥 레시피 모음"),
     video("yt000000001", "Already here", "Old", "10", "PT1M")],
    [video("ytnew00003", "맛집 먹방 mukbang", "Eater", "22", "PT10M", ["night"]),
     video("ytnew00004", "롤 게임 gaming", "Gamer", "22", "PT25M30S"),
     video("yt000000006", "Deleted before", "Old", "10", "PT2M")],
    [video("ytnew00005", "Match highlights", "Sports", "17", "P0D"),
     video("ytnew00006", "Variety clip", "", "24", "PT45S"),
     video("ytnew00001", "Official MV", "Music Channel", "10", "PT3M25S"),
     video("ytnotitle1", "", "No title", "10", "PT1M")],
]

# (categoryId, title, tags, description): the mapping rules, one case per rule and a few ties and misses.
CLASSIFY_CASES = [
    ("10", "x", [], ""), ("15", "x", [], ""), ("17", "x", [], ""), ("19", "x", [], ""), ("20", "x", [], ""), ("23", "x", [], ""),
    ("24", "x", [], ""), ("25", "x", [], ""), ("26", "x", [], ""), ("27", "x", [], ""), ("28", "x", [], ""),
    ("26", "Weekend table", ["table"], "맛있는 집밥 레시피 모음"), ("26", "Cooking recipe", [], ""), ("26", "x", ["cook"], ""),
    ("26", "x", [], "a" * 299 + "recipe"), ("26", "x", [], "a" * 300 + "recipe"),
    ("1", "mukbang asmr 먹방", [], ""), ("2", "drama series 드라마", ["시리즈"], ""), ("22", "daily vlog 브이로그", [], ""),
    ("22", "travel trip 여행 vlog", [], ""), ("99", "롤 게임 gaming pubg", [], ""), ("99", "workout gym 운동 요가", [], ""),
    ("99", "horror 공포 thriller", [], ""), ("99", "kpop mv 음악 노래", [], ""), ("99", "dog cat 강아지 고양이 펫", [], ""),
    ("99", "makeup 메이크업 fashion", [], ""), ("99", "funny comedy 개그 유머", [], ""), ("99", "sports soccer 축구 야구", [], ""),
    ("99", "nothing matches here", [], ""), ("", "", [], ""), ("22", "운동", [], ""), ("22", "variety show 예능 예능", [], ""),
    ("99", "news tutorial 뉴스 정보", [], ""),
]
DURATIONS = ["PT0S", "P0D", "PT45S", "PT3M25S", "PT1H2M3S", "PT10H", "P1DT1H", "PT1.9S", "PT0.4S", "P1W", "PT", "P", "garbage", "", "PT1H61M"]


class FakeYoutube(http.server.BaseHTTPRequestHandler):
    requests = []

    def do_GET(self):
        FakeYoutube.requests.append(self.path)
        query = dict(urllib.parse.parse_qsl(urllib.parse.urlparse(self.path).query))
        page = int(query.get("pageToken", "page0")[4:]) if query.get("pageToken") else 0
        body = {"items": PAGES[page]}
        if page + 1 < len(PAGES):
            body["nextPageToken"] = f"page{page + 1}"
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


def snapshot(admin_url, mongo_url):
    parsed = urllib.parse.urlparse(admin_url)
    conn = pymysql.connect(host=parsed.hostname, port=parsed.port, user=urllib.parse.unquote(parsed.username),
                           password=urllib.parse.unquote(parsed.password or ""), autocommit=True, charset="utf8mb4")
    cur = conn.cursor()
    cur.execute(f"SELECT video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted FROM `{base.SOURCE_DB}`.video "
                "WHERE youtube_url LIKE 'ytnew%' OR youtube_url = 'ytnotitle1' ORDER BY youtube_url")
    rows = cur.fetchall()
    conn.close()
    database = pymongo.MongoClient(mongo_url)[base.MONGO_DB]
    by_id = {row[0]: row[1] for row in rows}
    documents = {}
    for name in ("video_distribution", "video_timeline_emotion_count"):
        for doc in database[name].find({"video_id": {"$in": list(by_id)}}):
            doc.pop("_id")
            url = by_id[doc.pop("video_id")]
            doc.pop("created_at", None)
            doc.pop("updated_at", None)
            documents[(name, url)] = json.loads(json.dumps(doc, default=str))
    return [row[1:] for row in rows], documents


def main():
    admin_url, mongo_url = base.need("S14B_MYSQL_ADMIN_URL"), base.need("S14B_MONGO_URL")
    base.check_endpoint(admin_url, "S14B_MYSQL_ADMIN_URL")
    base.check_endpoint(mongo_url, "S14B_MONGO_URL")
    work = Path(base.need("S14B_WORK_DIR"))
    if any(work.iterdir()):
        sys.exit("S14B_WORK_DIR must be empty")
    base.seed_mysql(admin_url)
    base.seed_mongo(mongo_url)
    app, _, _, _ = base.load_original(base.need("S14B_ORIGINAL_ROOT"), admin_url, mongo_url)
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "extensions"))
    import importlib
    ours = importlib.import_module("slice14b_trending")
    yt_common = importlib.import_module("youtube_common")
    import common.scheduler.jobs.youtube_trending_job as original_module  # noqa: E402 (the original's `common` package)
    failures = []

    def check(name, ok, detail=""):
        print(f"{'PASS' if ok else 'FAIL'} {name} {detail}")
        if not ok:
            failures.append(name)

    # 1. The pure rules, against the original's methods.
    job = original_module.YoutubeTrendingJob()
    same = 0
    for case in CLASSIFY_CASES:
        want = job._map_youtube_category(case[0], case[1], list(case[2]), case[3]).value
        got = ours.map_youtube_category(case[0], case[1], list(case[2]), case[3])
        if want != got:
            print(f"   category differs for {case}: original {want}, aip {got}")
        else:
            same += 1
    check("category rules", same == len(CLASSIFY_CASES), f"{same}/{len(CLASSIFY_CASES)} cases")
    import isodate
    same = 0
    for text in DURATIONS:
        try:
            want = int(isodate.parse_duration(text).total_seconds())
        except (TypeError, ValueError, OverflowError, AttributeError):
            want = 0
        got = yt_common.parse_duration(text)
        if want != got:
            print(f"   duration differs for {text!r}: original {want}, aip {got}")
        else:
            same += 1
    check("duration rules", same == len(DURATIONS), f"{same}/{len(DURATIONS)} cases")

    # 2. The original's execute() against the stand-in provider, on the database and Mongo database of this run.
    class Response:
        def __init__(self, payload):
            self.payload = payload

        def raise_for_status(self):
            pass

        def json(self):
            return self.payload

    def fake_get(url, params=None, **kwargs):
        page = int(params["pageToken"][4:]) if params.get("pageToken") else 0
        body = {"items": PAGES[page]}
        if page + 1 < len(PAGES):
            body["nextPageToken"] = f"page{page + 1}"
        return Response(body)

    original_module.requests.get = fake_get
    app.config["YOUTUBE_API_KEY"] = "oracle-key"
    with app.app_context():
        original_module.YoutubeTrendingJob().execute()
    want_rows, want_docs = snapshot(admin_url, mongo_url)

    # 3. The AIP job on the same starting data.
    base.seed_mysql(admin_url)
    base.seed_mongo(mongo_url)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FakeYoutube)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    deployment = deploy(work, admin_url, mongo_url, f"http://127.0.0.1:{server.server_address[1]}/youtube/v3/videos", base.need("S14B_AIP_BIN"))
    try:
        accepted = deployment.request("/jobs/enqueue", {"operation": "fetchTrendingVideos", "input": {}, "key": "oracle-1", "delayMs": 0}, "bob")
        if not accepted.get("ok"):
            sys.exit(f"enqueue failed: {accepted}")
        for _ in range(300):
            status = deployment.request("/jobs/status", {"id": accepted["run"]["id"]}, "bob")["run"]
            if status["state"] in ("SUCCEEDED", "FAILED", "CANCELLED"):
                break
            time.sleep(0.2)
        if status["state"] != "SUCCEEDED":
            sys.exit(f"the job did not succeed: {status}")
    finally:
        deployment.close()
        server.shutdown()
    got_rows, got_docs = snapshot(admin_url, mongo_url)

    # The original saves a video without a title and an empty channel name as empty strings; the AIP fields cannot hold them.
    intended = [row for row in want_rows if row[1] == ""]
    want_rows = [row for row in want_rows if row[1] != ""]
    want_rows = [(row[0], row[1], None if row[2] == "" else row[2]) + row[3:] for row in want_rows]
    want_docs = {key: value for key, value in want_docs.items() if key[1] not in {row[0] for row in intended}}
    for row in intended:
        print(f"INTENDED DIFFERENCE the original saved {row[0]!r} with an empty title; AIP skips it")
    print("INTENDED DIFFERENCE an empty channel name is NULL in AIP and '' in the original")
    check("rows", got_rows == want_rows, f"{len(want_rows)} new videos")
    if got_rows != want_rows:
        print("   original:", want_rows)
        print("   aip     :", got_rows)
    check("documents", got_docs == want_docs, f"{len(want_docs)} documents")
    if got_docs != want_docs:
        print("   original:", json.dumps(want_docs, ensure_ascii=False)[:800])
        print("   aip     :", json.dumps(got_docs, ensure_ascii=False)[:800])
    check("provider requests", len(FakeYoutube.requests) == 3 and all("key=oracle-key" in path for path in FakeYoutube.requests),
          f"{len(FakeYoutube.requests)} requests")
    pymongo.MongoClient(mongo_url).drop_database(base.MONGO_DB)
    if failures:
        sys.exit("FAILED: " + ", ".join(failures))
    print("the trending job matched the original")


def deploy(work, admin_url, mongo_url, youtube_url, bin_path):
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric import rsa
    parsed = urllib.parse.urlparse(admin_url)
    host = f"{parsed.hostname}:{parsed.port}"
    shutil.copytree(base.FIXTURE / "extensions", work / "extensions", ignore=shutil.ignore_patterns("__pycache__", "README.md"))
    (work / "facereview.aip").write_text((base.FIXTURE / "facereview.aip").read_text())
    mapping = json.loads((base.FIXTURE / "mapping.json").read_text())
    mapping["schema"] = base.SOURCE_DB
    (work / "mapping.json").write_text(json.dumps(mapping))
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    numbers = key.public_key().public_numbers()
    (work / "jwks.json").write_text(json.dumps({"keys": [{"kty": "RSA", "kid": "current", "alg": "RS256", "use": "sig",
                                                          "n": base.b64(numbers.n), "e": base.b64(numbers.e)}]}))
    config = work / "service.json"
    config.write_text(json.dumps({
        "schema": base.META_DB, "external_mapping": "mapping.json", "database_adapter": "mysql",
        "database_url_env": "S13D_RUNTIME_URL", "migrate_database_url_env": "S13D_ADOPT_URL", "listen": "127.0.0.1:0", "wire": "decimal",
        "rate_limit": None, "auth": {"issuer": base.ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
        "workers": {"directory": "extensions", "language": "python",
                    "mongo": {"uri_env": "S13D_MONGO_URL", "database": base.MONGO_DB, "max_pool_size": 4, "timeout_ms": 3000,
                              "write_collections": ["video_distribution", "video_timeline_emotion_count"], "ensure_indexes": True},
                    "integrations": [{"name": "youtube_videos", "url": youtube_url, "method": "GET", "timeout_ms": 3000,
                                      "max_request_bytes": 4096, "max_response_bytes": 1048576, "allow_loopback": True,
                                      "query_params": ["part", "chart", "regionCode", "maxResults", "pageToken"],
                                      "secret_query_env": {"name": "key", "env": "S14B_API_KEY"}}]}}))
    env = dict(os.environ, S13D_ADOPT_URL=admin_url.rstrip("/") + f"/{base.SOURCE_DB}", S13D_MONGO_URL=mongo_url, S14B_API_KEY="oracle-key",
               S13D_RUNTIME_URL=f"mysql://{base.RUNTIME_USER}:{base.RUNTIME_PASSWORD}@{host}/{base.SOURCE_DB}")
    deployment = object.__new__(base.Deployment)
    deployment.work, deployment.bin, deployment.env, deployment.config = work, bin_path, env, config
    deployment.pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.TraditionalOpenSSL, serialization.NoEncryption())
    definition = str(work / "facereview.aip")
    plan = deployment.cli("adopt-external", definition, "--config", str(config))
    deployment.cli("adopt-external", definition, "--config", str(config), "--apply", plan["digest"])
    for subject, actor in base.SUBJECTS.items():
        deployment.cli("principal", "--config", str(config), "bind", "--subject", subject, "--actor", actor)
    deployment.log = open(work / "serve.log", "w")
    deployment.proc = subprocess.Popen([bin_path, "service", "serve", definition, "--config", str(config)], env=env, cwd=work,
                                       stdout=subprocess.PIPE, stderr=deployment.log, text=True)
    line = deployment.proc.stdout.readline()
    if not line:
        deployment.log.flush()
        sys.exit("serve did not start:\n" + (work / "serve.log").read_text()[-3000:])
    listening = json.loads(line)
    deployment.address, deployment.fingerprint = listening["address"], listening["fingerprint"]
    return deployment


if __name__ == "__main__":
    main()
