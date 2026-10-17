"""Catalog cost: request latency against the number of bound tables (the fence reads every bound table's catalog per request)."""
import json
import os
import shutil
import statistics
import subprocess
import time
import urllib.request

import jwt
import pymysql

BIN = "/private/tmp/aip-721-target/debug/aip"
BASE = "/private/tmp/fr-port/big"
ADOPT = "mysql://root:aip_fixture_pw_721@127.0.0.1:34391/big_src"
RUNTIME = "mysql://aip_big_rt:runtime_test_only@127.0.0.1:34391/big_src"
ENV = dict(os.environ, AIP_BIG_ADOPT_URL=ADOPT, AIP_BIG_RUNTIME_URL=RUNTIME)
db = pymysql.connect(host="127.0.0.1", port=34391, user="root", password="aip_fixture_pw_721", autocommit=True)
cur = db.cursor()
UID = "11111111-1111-4111-8111-111111111111"


def measure(count):
    shutil.rmtree(BASE, ignore_errors=True)
    os.makedirs(BASE)
    cur.execute("DROP DATABASE IF EXISTS big_src")
    cur.execute("DROP DATABASE IF EXISTS aip_big_meta")
    cur.execute("CREATE DATABASE big_src")
    cur.execute("DROP USER IF EXISTS 'aip_big_rt'@'%'")
    cur.execute("CREATE USER 'aip_big_rt'@'%' IDENTIFIED BY 'runtime_test_only'")
    cur.execute("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON big_src.* TO 'aip_big_rt'@'%'")
    cur.execute("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON aip_big_meta.* TO 'aip_big_rt'@'%'")
    definition = ["resource Person { fields { id: Uuid; name: Text(1..50) } rows read when true expose read { select id, name; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }", "actor Person"]
    mapping = {"schema": "big_src", "tables": {"Person": {"table": "person", "columns": {"id": "id", "name": "name"}}}}
    cur.execute("USE big_src")
    cur.execute("CREATE TABLE person (id VARCHAR(36) NOT NULL PRIMARY KEY, name VARCHAR(50) NOT NULL) ENGINE=InnoDB")
    cur.execute("INSERT INTO person VALUES (%s, 'a')", (UID,))
    for i in range(count - 1):
        cur.execute(f"CREATE TABLE item{i} (id VARCHAR(36) NOT NULL PRIMARY KEY, name VARCHAR(50) NOT NULL, n INT NOT NULL, note VARCHAR(100) NULL, "
                    f"owner_id VARCHAR(36) NOT NULL, created_at DATETIME NOT NULL, flag TINYINT(1) NOT NULL, kind VARCHAR(20) NOT NULL, "
                    f"score DECIMAL(10,2) NOT NULL, due DATE NULL, KEY idx_owner (owner_id), CONSTRAINT fk_item{i} FOREIGN KEY (owner_id) REFERENCES person (id)) ENGINE=InnoDB")
        definition.append(f"resource Item{i} {{ fields {{ id: Uuid; name: Text(1..50); n: Int(0..1000000); note: Text(1..100)?; owner: Person; created_at: Time; flag: Bool; kind: Text(1..20); score: Decimal(10,2); due: Date? }} "
                          f"rows read when true expose read {{ select id, name; budget {{ rows 20; depth 1; deadline 2s; cost 1000 }} }} }}")
        mapping["tables"][f"Item{i}"] = {"table": f"item{i}", "columns": {"id": "id", "name": "name", "n": "n", "note": "note", "owner": "owner_id",
                                                                           "created_at": "created_at", "flag": "flag", "kind": "kind", "score": "score", "due": "due"}}
    open(BASE + "/app.aip", "w").write("\n".join(definition))
    json.dump(mapping, open(BASE + "/mapping.json", "w"))
    shutil.copy("/private/tmp/fr-port/run/jwks.json", BASE + "/jwks.json")
    json.dump({"schema": "aip_big_meta", "external_mapping": "mapping.json", "database_adapter": "mysql", "database_url_env": "AIP_BIG_RUNTIME_URL",
               "migrate_database_url_env": "AIP_BIG_ADOPT_URL", "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": None,
               "auth": {"issuer": "https://issuer.example", "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}}}, open(BASE + "/service.json", "w"))
    plan = subprocess.run([BIN, "service", "adopt-external", BASE + "/app.aip", "--config", BASE + "/service.json"], env=ENV, capture_output=True, text=True, cwd=BASE)
    digest = json.loads(plan.stdout)["digest"]
    applied = subprocess.run([BIN, "service", "adopt-external", BASE + "/app.aip", "--config", BASE + "/service.json", "--apply", digest], env=ENV, capture_output=True, text=True, cwd=BASE)
    assert applied.returncode == 0, applied.stderr
    subprocess.run([BIN, "service", "principal", "--config", BASE + "/service.json", "bind", "--subject", "p", "--actor", UID], env=ENV, capture_output=True, cwd=BASE)
    server = subprocess.Popen([BIN, "service", "serve", BASE + "/app.aip", "--config", BASE + "/service.json"], env=ENV, cwd=BASE,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    listening = json.loads(server.stdout.readline())
    private = open("/private/tmp/fr-port/run/private.pem").read()
    now = int(time.time())
    token = jwt.encode({"iss": "https://issuer.example", "aud": "aip-api", "sub": "p", "iat": now, "nbf": now, "exp": now + 600}, private, algorithm="RS256",
                       headers={"kid": "current", "typ": "at+jwt"})

    def read():
        request = urllib.request.Request(f"http://{listening['address']}/read", data=json.dumps({"query": {"read": "Person", "select": ["id", "name"]}}).encode(), method="POST",
                                         headers={"Content-Type": "application/json", "x-aip-contract": listening["fingerprint"], "Authorization": "Bearer " + token})
        start = time.perf_counter()
        with urllib.request.urlopen(request, timeout=30) as response:
            body = json.load(response)
        assert body["ok"] is True, body
        return (time.perf_counter() - start) * 1000

    for _ in range(10):
        read()
    samples = sorted(read() for _ in range(100))
    server.terminate()
    print(f"{count:4d} bound tables: median {statistics.median(samples):7.2f} ms  p95 {samples[94]:7.2f} ms")


for n in (1, 10, 30, 60, 120):
    measure(n)
cur.execute("DROP DATABASE IF EXISTS big_src")
cur.execute("DROP DATABASE IF EXISTS aip_big_meta")
cur.execute("DROP USER IF EXISTS 'aip_big_rt'@'%'")
