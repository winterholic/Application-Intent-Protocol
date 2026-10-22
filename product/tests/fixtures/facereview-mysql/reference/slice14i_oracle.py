"""WBS 7.2.9: the system status of the AIP server versus the ORIGINAL AdminService.get_system_status.

  ORACLE  the pinned original's AdminService.get_system_status() on a real Redis (the request figures it reads from facereview:metrics:*)
          and without Redis, on the same MySQL and MongoDB.
  HTTP    the administrator-only read extension User.systemStatus of `aip service serve` (workers.mongo with the derived collection
          server_status, workers.redis with the prefix facereview:metrics:, or no Redis at all).

The connections and the request figures are compared field by field. The host figures (CPU, memory, disk of the machine) change every
moment and are Linux only in the port: they are printed side by side, not compared; the port's formulas equal psutil on a Linux capture in
the unit test host_figures_equal_psutil_on_a_linux_capture (spikes/spike-v4-worker/src/capability.rs).

Required environment: S14I_MYSQL_ADMIN_URL, S14I_MONGO_URL, S14I_REDIS_URL (redis://:<password>@127.0.0.1:<high port>, databases 3 and 4
are used), S14I_AIP_BIN, S14I_ORIGINAL_ROOT, S14I_WORK_DIR (empty). Run with the Python that has flask, flask_sqlalchemy, pymysql, pymongo,
redis, isodate, requests, psutil, cryptography and PyJWT, and not from a directory that holds a numbers.py.
"""
import json
import sys
import urllib.parse
from pathlib import Path

import pymongo
import redis as redis_lib

sys.path.insert(0, str(Path(__file__).resolve().parent))
import slice13d_oracle as base  # noqa: E402
import slice14b_oracle as jobs  # noqa: E402

ADMIN, MEMBER = "bob", "alice"


def with_database(url, number):
    parsed = urllib.parse.urlparse(url)
    return parsed._replace(path=f"/{number}").geturl()


def main():
    admin_url, mongo_url, redis_url = base.need("S14I_MYSQL_ADMIN_URL"), base.need("S14I_MONGO_URL"), base.need("S14I_REDIS_URL")
    for url, what in ((admin_url, "S14I_MYSQL_ADMIN_URL"), (mongo_url, "S14I_MONGO_URL"), (redis_url, "S14I_REDIS_URL")):
        base.check_endpoint(url, what)
    work = Path(base.need("S14I_WORK_DIR"))
    if any(work.iterdir()):
        sys.exit("S14I_WORK_DIR must be empty")
    base.seed_mysql(admin_url)
    base.seed_mongo(mongo_url)
    app, _, _, _ = base.load_original(base.need("S14I_ORIGINAL_ROOT"), admin_url, mongo_url)
    import app.services.admin_service as admin_module  # noqa: E402
    failures, count = [], 0

    def status_of_original(client):
        admin_module.redis_client = client
        with app.app_context():
            return json.loads(json.dumps(admin_module.AdminService.get_system_status()))

    def compare(name, want, got):
        nonlocal count
        count += 1
        ok = want == got
        print(f"{'PASS' if ok else 'FAIL'} {name}")
        if not ok:
            print("   original:", json.dumps(want))
            print("   aip     :", json.dumps(got))
            failures.append(name)

    def port_status(deployment, subject=ADMIN):
        answer = deployment.post("User.systemStatus", {}, subject)
        return json.loads(answer["output"]["body"]) if answer.get("ok") else answer

    # --- Redis configured, the request figures written ---
    metrics_db = with_database(redis_url, 3)
    client = redis_lib.Redis.from_url(metrics_db, decode_responses=True)
    client.flushdb()
    client.set("facereview:metrics:requests:1h", 200)
    client.set("facereview:metrics:errors:1h", 7)
    client.rpush("facereview:metrics:response_times", 12.5, 30, 7.25)
    first = work / "with-redis"
    first.mkdir()
    deployment = jobs.deploy(first, admin_url, mongo_url, "http://127.0.0.1:9/youtube/v3/videos", base.need("S14I_AIP_BIN"), enable_writes=True,
                             redis_url=metrics_db)
    try:
        want, got = status_of_original(client), port_status(deployment)
        compare("with Redis: the request figures", want["api"], got["api"])
        compare("with Redis: the connections", want["connections"], got["connections"])
        compare("with Redis: the fields", sorted(want), sorted(got))
        compare("with Redis: the host figure names", sorted(want["server"]), sorted(got["server"]))
        print("   host figures, original (this machine):", want["server"])
        print("   host figures, aip (Linux only, null elsewhere):", got["server"])
        # No requests were counted: the error rate is 0.0 and the average 0.0, as in the original.
        client.flushdb()
        want, got = status_of_original(client), port_status(deployment)
        compare("with Redis, empty counters: the request figures", want["api"], got["api"])
        # A broken value stops the reading where it breaks and keeps what was read before.
        client.set("facereview:metrics:requests:1h", 50)
        client.set("facereview:metrics:errors:1h", "oops")
        want, got = status_of_original(client), port_status(deployment)
        compare("with Redis, a broken error counter: the request figures", want["api"], got["api"])
        for subject in (MEMBER, None):
            count += 1
            answer = deployment.post("User.systemStatus", {}, subject)
            if answer.get("ok"):
                failures.append(f"denied {subject}")
                print("FAIL answered", subject, answer)
            else:
                print(f"PASS refused for {subject or 'a guest'}: {answer.get('code')}")
    finally:
        deployment.close()
    client.flushdb()
    # --- no Redis at all ---
    second = work / "no-redis"
    second.mkdir()
    deployment = jobs.deploy(second, admin_url, mongo_url, "http://127.0.0.1:9/youtube/v3/videos", base.need("S14I_AIP_BIN"), enable_writes=True)
    try:
        want, got = status_of_original(None), port_status(deployment)
        compare("without Redis: the request figures", want["api"], got["api"])
        compare("without Redis: the connections (redis is an error)", want["connections"], got["connections"])
    finally:
        deployment.close()
    pymongo.MongoClient(mongo_url).drop_database(base.MONGO_DB)
    if failures:
        sys.exit(f"FAILED {len(failures)} of {count}: " + ", ".join(failures))
    print(f"the system status matched the original in {count} cases")


if __name__ == "__main__":
    main()
