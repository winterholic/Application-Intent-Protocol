"""Per-request latency of the adopted service: every request runs the deployment fence (catalog fingerprint, no cache)."""
import json
import os
import statistics
import subprocess
import sys
import time
import urllib.request

import jwt

BIN = "/private/tmp/aip-721-target/debug/aip"
RUN = "/private/tmp/fr-port/run"
DEF = "/private/tmp/fr-port/facereview.aip"
ENV = dict(os.environ, AIP_FR_ADOPT_URL="mysql://root:aip_fixture_pw_721@127.0.0.1:34391/fr_src",
           AIP_FR_RUNTIME_URL="mysql://aip_fr_rt:runtime_test_only@127.0.0.1:34391/fr_src")
subprocess.run([BIN, "service", "principal", "--config", RUN + "/service.json", "bind", "--subject", "alice", "--actor", "11111111-1111-4111-8111-111111111111"],
               env=ENV, capture_output=True, cwd=RUN)
server = subprocess.Popen([BIN, "service", "serve", DEF, "--config", RUN + "/service.json"], env=ENV, cwd=RUN, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True)
listening = json.loads(server.stdout.readline())
ADDRESS, FINGERPRINT = listening["address"], listening["fingerprint"]
PRIVATE = open(RUN + "/private.pem").read()
now = int(time.time())
TOKEN = jwt.encode({"iss": "https://issuer.example", "aud": "aip-api", "sub": "alice", "iat": now, "nbf": now, "exp": now + 600}, PRIVATE,
                   algorithm="RS256", headers={"kid": "current", "typ": "at+jwt"})


def read(query):
    request = urllib.request.Request(f"http://{ADDRESS}/read", data=json.dumps({"query": query}).encode(), method="POST",
                                     headers={"Content-Type": "application/json", "x-aip-contract": FINGERPRINT, "Authorization": "Bearer " + TOKEN})
    start = time.perf_counter()
    with urllib.request.urlopen(request, timeout=10) as response:
        json.load(response)
    return (time.perf_counter() - start) * 1000


for label, query in [("Video list (public, 1 table)", {"read": "Video", "select": ["id", "title"], "sort": [{"field": "created_at", "dir": "desc"}], "limit": 20}),
                     ("Comment list with user traverse (2 tables)", {"read": "Comment", "select": ["id", "content", "user"], "limit": 20})]:
    for _ in range(20):
        read(query)
    samples = sorted(read(query) for _ in range(300))
    print(f"{label}: median {statistics.median(samples):.2f} ms, p95 {samples[int(len(samples) * .95)]:.2f} ms, max {samples[-1]:.2f} ms (n=300)")
server.terminate()
