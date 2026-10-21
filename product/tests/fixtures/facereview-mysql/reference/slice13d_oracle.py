"""WBS 7.2.3 Slice13-D: FaceReview home/category, home/personalized and watch/recommended through the real `aip service serve` binary
versus the ORIGINAL HomeService / WatchService.

Evidence levels in the output (never merged):
  ORACLE  the pinned original's own HomeService.get_videos_by_category_emotions / get_personalized_videos and
          WatchService.get_recommended_videos, imported from the original checkout and run against the same MySQL and MongoDB the
          AIP side reads (not a copy of the algorithm).
  HTTP    POST /extension to `aip service serve` with workers.mongo, the Python worker sandbox and a JWKS-verified bearer token.

When S13D_GOLDEN_OUT names a file, the original's answers are written there after every case matched. The CI test
crates/aip-cli/tests/mysql_facereview_recommend.rs replays the same requests on a fresh deployment and compares with that golden file,
so it needs neither the original checkout nor Flask.

Nothing here has a default endpoint, password or path. Required environment:
  S13D_MYSQL_ADMIN_URL   mysql://root:...@127.0.0.1:<high port>/   (run-owned MySQL 8.4 container)
  S13D_MONGO_URL         mongodb://127.0.0.1:<high port>            (run-owned MongoDB container, no auth)
  S13D_AIP_BIN           built `aip` binary (cargo build -p aip-cli --features aip-cli/mysql)
  S13D_ORIGINAL_ROOT     checkout of new-facereview at 7948b7e721837fdb73c427cb54e12cad4ec2d5af (read only; revision is verified)
  S13D_WORK_DIR          empty scratch directory owned by this run
  S13D_GOLDEN_OUT        optional: where to write the golden answers
Run with the Python that has flask, flask_sqlalchemy, pymysql, pymongo, cryptography and PyJWT.
"""
import base64
import json
import os
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

import pymongo
import pymysql

HERE = Path(__file__).resolve().parent
FIXTURE = HERE.parent
PINNED = "7948b7e721837fdb73c427cb54e12cad4ec2d5af"
SOURCE_DB, META_DB, MONGO_DB = "s13d_src", "aip_s13d_meta", "s13d_fr"
ISSUER = "https://issuer.example"
SUBJECTS = {
    "alice": "11111111-1111-4111-8111-111111111111",
    "bob": "22222222-2222-4222-8222-222222222222",
    "carol": "33333333-3333-4333-8333-333333333333",
    "dave": "44444444-4444-4444-8444-444444444444",
    "erin": "55555555-5555-4555-8555-555555555555",
}
RUNTIME_USER, RUNTIME_PASSWORD = "s13d_rt", "s13d_runtime_only"


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


def video_id(n):
    return f"bbbbbbbb-0000-4000-8000-{n:012d}"


# ---- data ------------------------------------------------------------------------------------------------------------------

def seed_mysql(admin_url, extra_scripts=()):
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
    script((FIXTURE / "slice13d-seed.sql").read_text())
    for name in extra_scripts:
        script((FIXTURE / name).read_text())
    cur.execute("SET FOREIGN_KEY_CHECKS=1")
    script((FIXTURE / "db-defaults.sql").read_text())
    cur.execute("DROP USER IF EXISTS %s@'%%'", (RUNTIME_USER,))
    cur.execute("CREATE USER %s@'%%' IDENTIFIED BY %s", (RUNTIME_USER, RUNTIME_PASSWORD))
    for db in (SOURCE_DB, META_DB):
        cur.execute(f"GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{db}`.* TO %s@'%%'", (RUNTIME_USER,))
    conn.close()


def seed_mongo(mongo_url):
    client = pymongo.MongoClient(mongo_url)
    client.drop_database(MONGO_DB)
    database = client[MONGO_DB]
    database["video_distribution"].create_index("video_id", unique=True)
    for name in ("video_distribution", "youtube_watching_data"):
        # insertion order is the original's natural order; ids are generated in that order
        for doc in json.loads((FIXTURE / f"slice13d-{name}.json").read_text()):
            database[name].insert_one(doc)
    client.close()


# ---- AIP deployment --------------------------------------------------------------------------------------------------------

def b64(value):
    return base64.urlsafe_b64encode(value.to_bytes((value.bit_length() + 7) // 8, "big")).rstrip(b"=").decode()


class Deployment:
    def __init__(self, work, admin_url, mongo_url, bin_path):
        from cryptography.hazmat.primitives import serialization
        from cryptography.hazmat.primitives.asymmetric import rsa
        parsed = urllib.parse.urlparse(admin_url)
        host = f"{parsed.hostname}:{parsed.port}"
        self.work, self.bin = work, bin_path
        shutil.copytree(FIXTURE / "extensions", work / "extensions", ignore=shutil.ignore_patterns("__pycache__", "README.md"))
        (work / "facereview.aip").write_text((FIXTURE / "facereview.aip").read_text())
        mapping = json.loads((FIXTURE / "mapping.json").read_text())
        mapping["schema"] = SOURCE_DB
        (work / "mapping.json").write_text(json.dumps(mapping))
        key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        self.pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.TraditionalOpenSSL, serialization.NoEncryption())
        numbers = key.public_key().public_numbers()
        (work / "jwks.json").write_text(json.dumps({"keys": [{"kty": "RSA", "kid": "current", "alg": "RS256", "use": "sig",
                                                              "n": b64(numbers.n), "e": b64(numbers.e)}]}))
        self.config = work / "service.json"
        self.config.write_text(json.dumps({
            "schema": META_DB, "external_mapping": "mapping.json", "database_adapter": "mysql",
            "database_url_env": "S13D_RUNTIME_URL", "migrate_database_url_env": "S13D_ADOPT_URL", "listen": "127.0.0.1:0", "wire": "decimal",
            "rate_limit": None, "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
            "workers": {"directory": "extensions", "language": "python",
                        "mongo": {"uri_env": "S13D_MONGO_URL", "database": MONGO_DB, "max_pool_size": 4, "timeout_ms": 3000}}}))
        self.env = dict(os.environ, S13D_ADOPT_URL=admin_url.rstrip("/") + f"/{SOURCE_DB}", S13D_MONGO_URL=mongo_url,
                        S13D_RUNTIME_URL=f"mysql://{RUNTIME_USER}:{RUNTIME_PASSWORD}@{host}/{SOURCE_DB}")
        definition = str(work / "facereview.aip")
        plan = self.cli("adopt-external", definition, "--config", str(self.config))
        self.cli("adopt-external", definition, "--config", str(self.config), "--apply", plan["digest"])
        for subject, actor in SUBJECTS.items():
            self.cli("principal", "--config", str(self.config), "bind", "--subject", subject, "--actor", actor)
        self.log = open(work / "serve.log", "w")
        self.proc = subprocess.Popen([bin_path, "service", "serve", definition, "--config", str(self.config)], env=self.env, cwd=work,
                                     stdout=subprocess.PIPE, stderr=self.log, text=True)
        line = self.proc.stdout.readline()
        if not line:
            self.log.flush()
            sys.exit("serve did not start:\n" + (work / "serve.log").read_text()[-3000:])
        listening = json.loads(line)
        self.address, self.fingerprint = listening["address"], listening["fingerprint"]

    def cli(self, *args):
        out = subprocess.run([self.bin, "service", *args], env=self.env, capture_output=True, text=True, cwd=self.work)
        if out.returncode != 0:
            sys.exit(f"aip {' '.join(args)} failed: {out.stderr or out.stdout}")
        return json.loads(out.stdout)

    def token(self, subject):
        import jwt
        issued = int(time.time())
        return jwt.encode({"iss": ISSUER, "aud": "aip-api", "sub": subject, "iat": issued, "nbf": issued, "exp": issued + 300}, self.pem,
                          algorithm="RS256", headers={"kid": "current", "typ": "at+jwt"})

    def request(self, path, body, subject):
        headers = {"content-type": "application/json", "x-aip-contract": self.fingerprint}
        if subject:
            headers["authorization"] = f"Bearer {self.token(subject)}"
        request = urllib.request.Request(f"http://{self.address}{path}", data=json.dumps(body).encode(), headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=90) as response:
                return json.loads(response.read())
        except urllib.error.HTTPError as error:
            return json.loads(error.read() or b"{}") | {"_http": error.code}

    def post(self, extension, input, subject):
        return self.request("/extension", {"extension": extension, "input": input}, subject)

    def read(self, query, subject):
        return self.request("/read", {"query": query}, subject)

    def aggregate(self, name, query_filter, subject):
        return self.request("/read", {"query": {"aggregate": name, "input": {}, "filter": query_filter}}, subject)

    def close(self):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            self.proc.kill()


# ---- oracle ----------------------------------------------------------------------------------------------------------------

def load_original(root, mysql_admin_url, mongo_url):
    head = subprocess.run(["git", "-C", root, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    # A checkout transferred with CRLF line endings shows every file as modified; only a content change counts as dirty.
    changed = subprocess.run(["git", "-C", root, "diff", "--ignore-cr-at-eol", "--stat", "--", "app", "common"], capture_output=True, text=True).stdout.strip()
    untracked = subprocess.run(["git", "-C", root, "ls-files", "--others", "--exclude-standard", "--", "app", "common"], capture_output=True,
                               text=True).stdout.replace("__pycache__", "").strip()
    dirty = changed or "\n".join(line for line in untracked.splitlines() if "__pycache__" not in line and not line.endswith(".pyc"))
    if head != PINNED or dirty:
        sys.exit(f"original checkout must be clean at {PINNED}; head={head} dirty={bool(dirty)}")
    sys.path.insert(0, root)
    from flask import Flask
    import common.extensions as extensions
    parsed = urllib.parse.urlparse(mysql_admin_url)
    app = Flask("slice13d-oracle")
    app.config["SQLALCHEMY_DATABASE_URI"] = (f"mysql+pymysql://{urllib.parse.unquote(parsed.username)}:{urllib.parse.quote(urllib.parse.unquote(parsed.password or ''))}"
                                             f"@{parsed.hostname}:{parsed.port}/{SOURCE_DB}?charset=utf8mb4")
    app.config["SQLALCHEMY_TRACK_MODIFICATIONS"] = False
    extensions.db.init_app(app)
    extensions.mongo_db = pymongo.MongoClient(mongo_url)[MONGO_DB]  # must exist before the services are imported
    from app.services.home_service import HomeService
    from app.services.watch_service import WatchService
    from common.exception.exceptions import BusinessError
    return app, HomeService, WatchService, BusinessError


def to_plain(value):
    if hasattr(value, "to_dict"):
        return value.to_dict()
    if isinstance(value, list):
        return [to_plain(item) for item in value]
    return value


def main():
    admin_url, mongo_url = need("S13D_MYSQL_ADMIN_URL"), need("S13D_MONGO_URL")
    check_endpoint(admin_url, "S13D_MYSQL_ADMIN_URL")
    check_endpoint(mongo_url, "S13D_MONGO_URL")
    work = Path(need("S13D_WORK_DIR"))
    if any(work.iterdir()):
        sys.exit("S13D_WORK_DIR must be empty")
    seed_mysql(admin_url)
    seed_mongo(mongo_url)
    app, home, watch, business_error = load_original(need("S13D_ORIGINAL_ROOT"), admin_url, mongo_url)
    deployment = Deployment(work, admin_url, mongo_url, need("S13D_AIP_BIN"))
    failures, golden, count = [], [], 0

    def run_original(call):
        with app.app_context():
            try:
                return {"output": to_plain(call())}
            except business_error as error:
                return {"error": str(error)}

    def case(name, extension, subject, ext_input, original_call, shape):
        nonlocal count
        count += 1
        want = run_original(original_call)
        got = deployment.post(extension, ext_input, subject)
        if "error" in want:
            ok = got.get("ok") is False
            print(f"{'PASS' if ok else 'FAIL'} {name}: the original refuses ({want['error']}), AIP answered {got.get('code')}")
            golden.append({"name": name, "extension": extension, "subject": subject, "input": ext_input, "expect_error": True})
        else:
            port = shape(got)
            ok = port is not None and port == want["output"]
            size = len(want["output"]) if isinstance(want["output"], list) else len(want["output"].get("videos", []))
            print(f"{'PASS' if ok else 'FAIL'} {name}: {size} item(s)")
            if not ok:
                print("   original:", json.dumps(want["output"], ensure_ascii=False)[:1500])
                print("   aip     :", json.dumps(port if port is not None else got, ensure_ascii=False)[:1500])
            golden.append({"name": name, "extension": extension, "subject": subject, "input": ext_input, "expected": want["output"]})
        if not ok:
            failures.append(name)

    def categories(got):
        return json.loads(got["output"]["categories"]) if got.get("ok") else None

    def videos(got):
        return json.loads(got["output"]["videos"]) if got.get("ok") else None

    def page(got):
        if not got.get("ok"):
            return None
        out = got["output"]
        return {"videos": json.loads(out["videos"]), "total": out["total"], "page": out["page"], "size": out["size"], "has_next": out["has_next"]}

    try:
        for subject in ("alice", "bob", "dave", "erin"):
            actor = SUBJECTS[subject]
            case(f"home/category {subject}", "Video.homeCategory", subject, {},
                 lambda actor=actor: home.get_videos_by_category_emotions(actor).video_data, categories)
        for subject in ("alice", "bob", "carol", "dave", "erin"):
            actor = SUBJECTS[subject]
            case(f"home/personalized {subject}", "Video.homePersonalized", subject, {},
                 lambda actor=actor: home.get_personalized_videos(actor, limit=20), videos)
        recommended = [
            ("animal with emotion", 1, 1, 10), ("animal happy tie order", 2, 1, 10), ("animal happy 4", 4, 1, 10), ("cook", 9, 1, 10),
            ("cook angry", 8, 1, 10), ("vlog", 14, 1, 10), ("vlog null dominant", 16, 1, 10),
            ("travel without distribution", 22, 1, 10), ("music alone", 24, 1, 10), ("game", 19, 1, 10),
            ("animal happy page 1 size 2", 2, 1, 2), ("animal happy page 2 size 2", 2, 2, 2), ("animal happy page 3 size 2", 2, 3, 2),
            ("animal happy beyond last", 2, 9, 2),
        ]
        for subject in ("alice", "dave"):
            actor = SUBJECTS[subject]
            for label, n, page_no, size in recommended:
                vid = video_id(n)
                case(f"watch/recommended {subject} {label}", "Video.watchRecommended", subject, {"video_id": vid, "page": page_no, "size": size},
                     lambda actor=actor, vid=vid, page_no=page_no, size=size: watch.get_recommended_videos(vid, page_no, size, actor), page)
        canonical = "aaaaaaaa-0000-4000-8000-000000000001"
        case("watch/recommended alice canonical video without distribution", "Video.watchRecommended", "alice",
             {"video_id": canonical, "page": 1, "size": 10},
             lambda: watch.get_recommended_videos(canonical, 1, 10, SUBJECTS["alice"]), page)
        for label, vid in (("soft-deleted video", video_id(6)), ("NULL is_deleted video", video_id(13)),
                           ("unknown video", "cccccccc-0000-4000-8000-0000000000ff")):
            case(f"watch/recommended alice {label}", "Video.watchRecommended", "alice", {"video_id": vid, "page": 1, "size": 10},
                 lambda vid=vid: watch.get_recommended_videos(vid, 1, 10, SUBJECTS["alice"]), page)
    finally:
        deployment.close()
    print(f"{count - len(failures)}/{count} cases matched the original")
    if failures:
        sys.exit("FAILED: " + ", ".join(failures))
    out = os.environ.get("S13D_GOLDEN_OUT")
    if out:
        Path(out).write_text(json.dumps(golden, ensure_ascii=False, indent=1) + "\n")
        print(f"golden answers written to {out}")


if __name__ == "__main__":
    main()
