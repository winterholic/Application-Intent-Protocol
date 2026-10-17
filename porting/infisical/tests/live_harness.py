"""Live-database harness for the Infisical port: one throwaway AIP deployment per instance.

It talks only to a loopback PostgreSQL named by AIP_TEST_DATABASE_URL (never the shared 5432, never
a remote host), creates its own uniquely named schema, serves it on an ephemeral loopback port and
drops the schema again. Nothing here reads .env files or secret stores.

Environment:
  AIP_TEST_DATABASE_URL  postgresql://user:pw@127.0.0.1:<port>/<db>  (required, loopback, port != 5432)
  AIP_BIN                path to the aip binary (default: target/debug/aip of this checkout)
  INFISICAL_PYTHON       interpreter for the worker runtime (default: the interpreter running the tests)
"""

import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.parse
import uuid

HERE = pathlib.Path(__file__).resolve().parent
PORT_DIR = HERE.parent
REPO = PORT_DIR.parent.parent
sys.path.insert(0, str(PORT_DIR / "tools"))
sys.path.insert(0, str(REPO / "product" / "python"))

import jwks  # noqa: E402  (tools/jwks.py)
from aip_sdk import AipError, connect  # noqa: E402

LOOPBACK = {"127.0.0.1", "localhost", "::1"}


class HarnessUnavailable(RuntimeError):
    """The environment cannot host a live run (callers skip instead of fail)."""


def database_url():
    url = os.environ.get("AIP_TEST_DATABASE_URL", "")
    if not url:
        raise HarnessUnavailable("AIP_TEST_DATABASE_URL is not set")
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme not in ("postgres", "postgresql"):
        raise HarnessUnavailable("AIP_TEST_DATABASE_URL must be a postgresql:// URL")
    if parsed.hostname not in LOOPBACK:
        raise HarnessUnavailable(f"refusing non-loopback database host {parsed.hostname!r}")
    if (parsed.port or 5432) == 5432:
        raise HarnessUnavailable("refusing port 5432: use a dedicated isolated container")
    return url


def aip_binary():
    path = pathlib.Path(os.environ.get("AIP_BIN", REPO / "target" / "debug" / "aip"))
    if not path.is_file():
        raise HarnessUnavailable(f"aip binary not found at {path}; run cargo build -p aip-cli")
    return str(path)


def psql(url, sql, expect_rows=False):
    """Run SQL through psql; returns stdout (tuples only, unaligned) when expect_rows."""
    args = ["psql", url, "-v", "ON_ERROR_STOP=1", "-X", "-q"]
    if expect_rows:
        args += ["-At", "-F", "|"]
    result = subprocess.run(args + ["-c", sql], capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(f"psql failed: {result.stderr.strip()}")
    return result.stdout


def psql_file(url, path, schema):
    """Run a seed file with :schema substituted by the deployment schema."""
    text = pathlib.Path(path).read_text(encoding="utf-8").replace(":schema", schema)
    result = subprocess.run(["psql", url, "-v", "ON_ERROR_STOP=1", "-X", "-q", "-1", "-f", "-"],
                            input=text, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(f"seed failed: {result.stderr.strip()}")


class Deployment:
    """init -> seed -> principal bind -> serve for one app definition."""

    def __init__(self, app_path, seed_path=None, principals=(), workers=False, workers_dir=None):
        self.url = database_url()
        self.binary = aip_binary()
        self.app = str(app_path)
        self.seed = seed_path
        self.principals = list(principals)  # (subject, actor id)
        self.workers = workers
        self.workers_dir = pathlib.Path(workers_dir) if workers_dir else PORT_DIR / "workers"
        self.schema = "aip_inf_" + uuid.uuid4().hex[:12]
        self.work = pathlib.Path(tempfile.mkdtemp(prefix="infisical-live-"))
        self.process = None
        self.base = None
        self.fingerprint = None
        self._log = None

    # -- lifecycle ---------------------------------------------------------------
    def start(self):
        jwks.keygen(self.work)
        config = {
            "schema": self.schema, "database_url_env": "AIP_TEST_DATABASE_URL",
            "listen": "127.0.0.1:0", "wire": "decimal",
            "auth": {"issuer": jwks.ISSUER, "audience": jwks.AUDIENCE,
                     "jwks": {"kind": "file", "path": "jwks.json"}},
        }
        if self.workers:
            config["workers"] = {"directory": str(self.workers_dir), "language": "python",
                                 "runtime_executable": os.environ.get("INFISICAL_PYTHON", sys.executable),
                                 "enable_writes": False}
        (self.work / "service.json").write_text(json.dumps(config), encoding="utf-8")
        self.cli("init", self.app, "--config", str(self.work / "service.json"))
        if self.seed:
            psql_file(self.url, self.seed, self.schema)
        for subject, actor in self.principals:
            self.cli("principal", "--config", str(self.work / "service.json"), "bind",
                     "--subject", subject, "--actor", str(actor))
        self._log = open(self.work / "serve.log", "w", encoding="utf-8")
        self.process = subprocess.Popen(
            [self.binary, "service", "serve", self.app, "--config", str(self.work / "service.json")],
            stdout=subprocess.PIPE, stderr=self._log, text=True, env=os.environ.copy())
        deadline = time.time() + 30
        while time.time() < deadline:
            line = self.process.stdout.readline()
            if not line:
                break
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if event.get("event") == "listening":
                self.base = "http://" + event["address"]
                self.fingerprint = event["fingerprint"]
                return self
        raise RuntimeError("serve did not report a listening address: " + self.serve_log())

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
        if self._log:
            self._log.close()
        try:
            psql(self.url, f"DROP SCHEMA IF EXISTS {self.schema} CASCADE")
            infra = "aip_infra_" + __import__("hashlib").sha256(self.schema.encode()).hexdigest()[:52]
            psql(self.url, f"DROP SCHEMA IF EXISTS {infra} CASCADE")
        finally:
            shutil.rmtree(self.work, ignore_errors=True)

    def __enter__(self):
        return self.start()

    def __exit__(self, *exc):
        self.stop()

    # -- helpers -----------------------------------------------------------------
    def cli(self, *args):
        result = subprocess.run([self.binary, "service", *args], capture_output=True, text=True,
                                env=os.environ.copy())
        if result.returncode != 0:
            raise RuntimeError(f"aip service {args[0]} failed: {result.stdout.strip()} {result.stderr.strip()}")
        return json.loads(result.stdout) if result.stdout.strip().startswith("{") else result.stdout

    def serve_log(self):
        try:
            return (self.work / "serve.log").read_text(encoding="utf-8")[-2000:]
        except OSError:
            return ""

    def sql(self, statement):
        return psql(self.url, statement.replace(":schema", self.schema), expect_rows=True)

    def client(self, subject, lifetime=jwks.MAX_LIFETIME):
        return connect(self.base, jwks.mint(self.work, subject, lifetime=lifetime), self.fingerprint)

    def token(self, subject, **kwargs):
        return jwks.mint(self.work, subject, **kwargs)


def error_code(callable_, *args, **kwargs):
    """Return the AIP error code a call fails with, or None when it succeeds."""
    try:
        callable_(*args, **kwargs)
    except AipError as error:
        return error.code
    return None


def job_receipt(client, operation, payload, key, timeout=30):
    job = client.jobs.enqueue(operation, payload, key=key)
    return client.jobs.wait(job["id"], timeout=timeout)


def create(client, resource, values, key=None):
    """One exposed create through /apply; returns the response dict (ok/code), never raises on a rejection."""
    return client.bundle([{"create": resource, "values": values}], key=key)


def transition(client, name, ids, key=None, values=None):
    request = {"apply": name, "target": {"ids": [str(i) for i in ids]}}
    if values is not None:
        request["values"] = values
    return client.apply(request, key=key)
