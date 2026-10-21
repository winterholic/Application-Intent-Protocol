"""WBS 7.2.7: the recommendation pool the AIP job writes to Redis versus what the ORIGINAL's Celery task writes.

  ORACLE  HomeService._build_and_cache_ranked_pool() of the pinned original (the body of the beat task rebuild_recommendation_pool),
          imported from the original checkout and run against the same MySQL and MongoDB, with its own redis_client on a Redis
          database of this run. Its keys are read back after the run.
  HTTP    POST /jobs/enqueue of `rebuildRecommendationPool` to `aip service serve` (workers.mongo, workers.redis, Python worker
          sandbox), as the administrator the service principal is bound to. The keys it wrote are read back the same way.

Compared: the pool JSON of `facereview:reco:pool` (entry for entry, in order), every field of the hash `facereview:reco:category`
(entry for entry) and the order of the categories (the order of the original's own category list against the order key the job adds),
plus the expiry. The original's cached hash comes back in Redis's internal order, so that is not what is compared.
The seed (slice13d) leaves the original no randomness: each category has at most one video that can only be padding.

Nothing here has a default endpoint, password or path. Required environment:
  S14A_MYSQL_ADMIN_URL   mysql://root:...@127.0.0.1:<high port>/   (run-owned MySQL 8.4 container)
  S14A_MONGO_URL         mongodb://127.0.0.1:<high port>            (run-owned MongoDB container, no auth)
  S14A_REDIS_URL         redis://:<password>@127.0.0.1:<high port>/ (run-owned Redis container; databases 1 and 2 are used)
  S14A_AIP_BIN           built `aip` binary (cargo build -p aip-cli --features aip-cli/mysql)
  S14A_ORIGINAL_ROOT     checkout of new-facereview at 7948b7e721837fdb73c427cb54e12cad4ec2d5af (read only; revision is verified)
  S14A_WORK_DIR          empty scratch directory owned by this run
Run with the Python that has flask, flask_sqlalchemy, pymysql, pymongo, redis, cryptography and PyJWT, and not from a directory that
holds a numbers.py (it would shadow the standard library).
"""
import json
import os
import shutil
import subprocess
import sys
import time
import urllib.parse
from pathlib import Path

import pymongo
import redis as redis_lib

sys.path.insert(0, str(Path(__file__).resolve().parent))
import slice13d_oracle as base  # noqa: E402

KEYS = ("facereview:reco:pool", "facereview:reco:category")


def need(name):
    value = os.environ.get(name)
    if not value:
        sys.exit(f"set {name}")
    return value


def with_database(url, database):
    parsed = urllib.parse.urlparse(url)
    return parsed._replace(path=f"/{database}").geturl()


class JobDeployment(base.Deployment):
    """The 7.2.3 deployment plus Redis, a job call limit and the service principal."""

    def start_server(self):
        definition = str(self.work / "facereview.aip")
        self.log = open(self.work / "serve.log", "w")
        self.proc = subprocess.Popen([self.bin, "service", "serve", definition, "--config", str(self.config)], env=self.env, cwd=self.work,
                                     stdout=subprocess.PIPE, stderr=self.log, text=True)
        line = self.proc.stdout.readline()
        if not line:
            self.log.flush()
            sys.exit("serve did not start:\n" + (self.work / "serve.log").read_text()[-3000:])
        listening = json.loads(line)
        self.address, self.fingerprint = listening["address"], listening["fingerprint"]


def deploy(work, admin_url, mongo_url, redis_url, bin_path):
    """Prepare the work directory like base.Deployment, then serve with workers.redis added."""
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
        "limits": {"job_calls": 400},
        "workers": {"directory": "extensions", "language": "python",
                    "mongo": {"uri_env": "S13D_MONGO_URL", "database": base.MONGO_DB, "max_pool_size": 4, "timeout_ms": 3000},
                    "redis": {"uri_env": "S14A_REDIS_URL", "prefixes": ["facereview:reco:"], "max_in_flight": 4, "timeout_ms": 3000}}}))
    env = dict(os.environ, S13D_ADOPT_URL=admin_url.rstrip("/") + f"/{base.SOURCE_DB}", S13D_MONGO_URL=mongo_url,
               S14A_REDIS_URL=with_database(redis_url, 2),
               S13D_RUNTIME_URL=f"mysql://{base.RUNTIME_USER}:{base.RUNTIME_PASSWORD}@{host}/{base.SOURCE_DB}")
    deployment = object.__new__(JobDeployment)
    deployment.work, deployment.bin, deployment.env, deployment.config = work, bin_path, env, config
    deployment.pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.TraditionalOpenSSL, serialization.NoEncryption())
    definition = str(work / "facereview.aip")
    plan = deployment.cli("adopt-external", definition, "--config", str(config))
    deployment.cli("adopt-external", definition, "--config", str(config), "--apply", plan["digest"])
    for subject, actor in base.SUBJECTS.items():
        deployment.cli("principal", "--config", str(config), "bind", "--subject", subject, "--actor", actor)
    deployment.start_server()
    return deployment


def dump(client):
    """What a Redis database holds under the original's two keys: parsed JSON, hash order, expiry."""
    pool = json.loads(client.get("facereview:reco:pool"))
    hash_ = client.hgetall("facereview:reco:category")
    return {
        "pool": pool,
        "categories": {name: json.loads(text) for name, text in hash_.items()},
        "order": list(hash_),
        "ttl": {key: client.ttl(key) for key in KEYS},
    }


def main():
    admin_url, mongo_url, redis_url = need("S14A_MYSQL_ADMIN_URL"), need("S14A_MONGO_URL"), need("S14A_REDIS_URL")
    base.check_endpoint(admin_url, "S14A_MYSQL_ADMIN_URL")
    base.check_endpoint(mongo_url, "S14A_MONGO_URL")
    base.check_endpoint(redis_url, "S14A_REDIS_URL")
    work = Path(need("S14A_WORK_DIR"))
    if any(work.iterdir()):
        sys.exit("S14A_WORK_DIR must be empty")
    base.seed_mysql(admin_url)
    base.seed_mongo(mongo_url)
    original = redis_lib.Redis.from_url(with_database(redis_url, 1), decode_responses=True)
    aip = redis_lib.Redis.from_url(with_database(redis_url, 2), decode_responses=True)
    original.delete(*KEYS)
    aip.delete(*KEYS, "facereview:reco:category_order")

    app, home, _, _ = base.load_original(need("S14A_ORIGINAL_ROOT"), admin_url, mongo_url)
    import common.extensions as extensions
    extensions.redis_client = original  # the beat task reads it as `from common.extensions import redis_client`
    with app.app_context():
        pool, category_dtos = home._build_and_cache_ranked_pool()
    want = dump(original)
    # What the original's own request path returns when its cache is cold is this order. Its cached hash comes back in Redis's internal
    # order instead (a value over 64 bytes makes the hash a hash table), which is why the job also stores the order.
    want["order"] = [dto.category_name for dto in category_dtos]
    assert len(want["pool"]) == len(pool), "the original wrote the pool it returned"

    deployment = deploy(work, admin_url, mongo_url, redis_url, need("S14A_AIP_BIN"))
    failures = []
    try:
        accepted = deployment.request("/jobs/enqueue", {"operation": "rebuildRecommendationPool", "input": {}, "key": "oracle-1", "delayMs": 0}, "bob")
        if not accepted.get("ok"):
            sys.exit(f"enqueue failed: {accepted}")
        run_id = accepted["run"]["id"]
        for _ in range(300):
            status = deployment.request("/jobs/status", {"id": run_id}, "bob")["run"]
            if status["state"] in ("SUCCEEDED", "FAILED", "CANCELLED"):
                break
            time.sleep(0.2)
        if status["state"] != "SUCCEEDED":
            sys.exit(f"the job did not succeed: {status}")
    finally:
        deployment.close()
    got = dump(aip)
    got_order = json.loads(aip.get("facereview:reco:category_order"))

    def check(name, ok, detail=""):
        print(f"{'PASS' if ok else 'FAIL'} {name} {detail}")
        if not ok:
            failures.append(name)

    check("pool entries", got["pool"] == want["pool"], f"{len(want['pool'])} entries")
    if got["pool"] != want["pool"]:
        print("   original:", json.dumps(want["pool"], ensure_ascii=False)[:800])
        print("   aip     :", json.dumps(got["pool"], ensure_ascii=False)[:800])
    check("category fields", got["categories"] == want["categories"], f"{len(want['categories'])} categories")
    check("category order", got_order == want["order"], f"{want['order']}")
    if got_order != want["order"]:
        print("   original:", want["order"])
        print("   aip     :", got_order)
    check("pool expiry", 7000 < got["ttl"]["facereview:reco:pool"] <= 7200 and 7000 < want["ttl"]["facereview:reco:pool"] <= 7200)
    check("category expiry", 7000 < got["ttl"]["facereview:reco:category"] <= 7200 and 7000 < want["ttl"]["facereview:reco:category"] <= 7200)
    original.delete(*KEYS)
    aip.delete(*KEYS, "facereview:reco:category_order")
    pymongo.MongoClient(mongo_url).drop_database(base.MONGO_DB)
    if failures:
        sys.exit("FAILED: " + ", ".join(failures))
    print(f"{len(want['pool'])} pool entries and {len(want['categories'])} categories matched the original")


if __name__ == "__main__":
    main()
