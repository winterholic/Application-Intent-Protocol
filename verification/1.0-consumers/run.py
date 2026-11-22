"""독립 소비자(taskshare, stockdesk) 실행 검증.

실행 대상은 이 checkout의 HEAD다. 대상 SHA는 `--core-sha`(또는 AIP_CONSUMER_CORE_SHA, CI는 inputs.core_sha 또는 github.sha)로 받고
HEAD가 다르면 실패한다. 결과는 `--result`(기본: 증거 디렉터리의 independent-consumers.json)에 소스 SHA, 빌드 엔진(`aip --version`·바이너리 sha256·
패키지 BUILD.json), SDK 아티팩트(이름·버전·sha256·출처)와 앱별 결과로 남기고, 어긋나면 실패한다.
업그레이드 시험의 baseline 고정(aip-release-gate.yml의 --baseline)은 이 실행의 대상이 아니라 `upgradeBaseline`에 따로 표시한다.

  cd <Core checkout> && python3 verification/1.0-consumers/run.py [--core-sha <40자 SHA>] [--result <file>]
필요: docker(postgres:17), node, python3, cargo. 실행 파일은 이 스크립트가 `cargo build --release --locked -p aip-cli`로 직접 확인한다.
"""
import argparse, datetime, hashlib, json, os, re, shutil, socket, subprocess as sp, sys, tarfile, tempfile, time, urllib.request, urllib.error, uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import provenance as prov

parser = argparse.ArgumentParser()
parser.add_argument("--core-sha")
parser.add_argument("--result")
args = parser.parse_args()

core = Path.cwd()
original = Path(__file__).resolve().parent / "apps"
work = Path(tempfile.mkdtemp(prefix="aip-consumer-rc-"))
result_path = Path(args.result) if args.result else work / "independent-consumers.json"
print("Evidence directory:", work, flush=True)
common = {key: val for key, val in os.environ.items() if not key.startswith("PG")}
common.update(npm_config_cache=str(work / "npm-cache"), PIP_NO_CACHE_DIR="1", PYTHONDONTWRITEBYTECODE="1")

record = {
    "schema": "aip-independent-consumers/1",
    "status": "fail", "error": None,
    "startedAt": datetime.datetime.now(datetime.timezone.utc).isoformat(), "finishedAt": None,
    "target": {"expectedSha": None, "expectedShaSource": None, "headSha": None, "dirty": None},
    "engine": {"version": None, "sha256": None, "binary": None},
    "sdk": {"npm": {}, "python": {}},
    "upgradeBaseline": {"pinned": True, "usedByThisRun": False, "sha": None, "source": ".github/workflows/aip-release-gate.yml --baseline"},
    "environment": {}, "checks": [], "apps": {},
}

def finish():
    record["finishedAt"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    result_path.parent.mkdir(parents=True, exist_ok=True)
    result_path.write_text(json.dumps(record, indent=2, ensure_ascii=False) + "\n")
    print("Result JSON:", result_path, flush=True)

def note(app, message):
    print(message, flush=True)
    record["apps"].setdefault(app, {"status": "running", "steps": []})["steps"].append(message)

def verify(checks, suffix=""):
    for check in checks:
        if suffix:
            check.name += ":" + suffix
        print(("OK   " if check.ok else "FAIL ") + f"{check.name}: {check.detail}", flush=True)
    record["checks"].extend(check.as_dict() for check in checks)
    prov.require_all(checks)

def call(argv, cwd=core, env=common, *, show=False, input=None, check=True):
    result = sp.run(argv, cwd=cwd, env=env, input=input, capture_output=True, text=True)
    if show or (check and result.returncode):
        print(result.stdout.rstrip(), flush=True)
        print(result.stderr.rstrip(), flush=True)
    if check:
        result.check_returncode()
    return result

def file_sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def tree_hashes(root):
    return {str(path.relative_to(root)): file_sha256(path) for path in sorted(Path(root).rglob("*")) if path.is_file()}

def online_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]

def stop(server):
    if server is not None and server.poll() is None:
        server.terminate()
        try:
            server.wait(timeout=5)
        except sp.TimeoutExpired:
            server.kill()
            server.wait()

def launch(installed, cfg, base, env):
    stream = open(cfg.parent / (installed.name + ".serve.log"), "a")
    server = sp.Popen([installed / "bin/aip", "service", "serve", installed / "app/app.aip", "--config", cfg],
                     cwd=cfg.parent, env=env, stdout=stream, stderr=stream, text=True)
    stream.close()
    for _ in range(100):
        if server.poll() is not None:
            raise RuntimeError((cfg.parent / (installed.name + ".serve.log")).read_text())
        try:
            with urllib.request.urlopen(base + "/ready", timeout=.3) as response:
                if response.status == 200:
                    return server
        except OSError:
            time.sleep(.1)
    stop(server)
    raise RuntimeError("Service did not become ready")

def seed_sql(app, schema):
    if app == "taskshare":
        return f"""INSERT INTO {schema}.member(id,name,email) VALUES
          (1,'Alice','alice@example.test'),(2,'Bob','bob@example.test'),(3,'Carol','carol@example.test');"""
    return f"""INSERT INTO {schema}.tenant(id,name) VALUES (1,'Gangnam Store'),(2,'Another Store');
        INSERT INTO {schema}.staff(id,tenant_id,name,role) VALUES
          (1,1,'Manager','MANAGER'),(2,1,'Joo','CLERK'),(3,2,'Other Manager','MANAGER');"""

def package(source, root, version):
    incoming = root / ("incoming-" + version)
    installed = root / ("installed-" + version)
    call(["node", "product/tools/package.mjs", "--manifest", source, "--out", incoming])
    install = call(["node", incoming / "tools/app-deploy.mjs", "install", "--out", installed])
    assert json.loads(install.stdout)["ok"] and (installed / "bin/aip").is_file()
    return incoming, installed

def fingerprint(incoming):
    text = (incoming / "APP_CONTRACT.json").read_text()
    data = json.loads(text)
    if isinstance(data.get("contractFingerprint"), str):
        return data["contractFingerprint"]
    if isinstance(data.get("fingerprint"), str):
        return data["fingerprint"]
    return re.search(r'^export const contractFingerprint = "([^"]+)";', (incoming / "bindings.ts").read_text(), re.M)[1]

def post_json(url, path, token, fp, body, timeout=10):
    request = urllib.request.Request(url + path, data=json.dumps(body).encode(),
        headers={"authorization": f"Bearer {token}", "x-aip-contract": fp, "content-type": "application/json"})
    try:
        response = urllib.request.urlopen(request, timeout=timeout)
    except urllib.error.HTTPError as error:
        response = error
    except OSError as error:
        return {"ok": False, "code": "TRANSPORT", "msg": str(error)}
    with response:
        try:
            return json.load(response)
        except ValueError:
            return {"ok": False, "code": f"HTTP.{response.status}", "msg": "non-JSON response"}

def get_contract(url, token, fp, read_resource, select=("id",)):
    return post_json(url, "/read", token, fp, {"query": {"read": read_resource, "select": list(select)}}, timeout=3)

def client_runs(app, root, python):
    typescript = ("typescript", ["node", "e2e.ts"], root / "client-ts")
    py = ("python", [python, "e2e.py"], root / "client-py")
    return [(*typescript, 22), (*py, 16)] if app == "taskshare" else [(*py, 23), (*typescript, 19)]

def recover(app, root, base, env, fp, names, installed, cfg, container, server):
    """장애 후 복구. 서버가 SIGKILL로 죽거나 DB 연결이 끊긴 뒤에도 같은 멱등 key 재전송이 쓰기를 정확히 한 번만 남기는지 본다."""
    config = json.loads(cfg.read_text())
    schema, token = config["schema"], env[f"TOKEN_{names[0].upper()}"]
    summary = record["apps"][app].setdefault("recovery", {})

    def create(tag):
        if app == "taskshare":
            return ({"create": "TodoList", "values": {"owner": "1", "title": tag, "archived": False}},
                    f"SELECT count(*) FROM {schema}.todo_list WHERE title = '{tag}'")
        return ({"create": "Order", "values": {"tenant": "1", "placedBy": "1", "number": tag, "status": "OPEN", "total": "0.00"}},
                f"SELECT count(*) FROM {schema}.\"order\" WHERE number = '{tag}'")

    def count(sql):
        return int(call(["docker", "exec", container, "psql", "-U", "postgres", "-d", "aip_test_consumer_rc", "-At", "-c", sql]).stdout.strip())

    def send(operations, key):
        return post_json(base, "/apply", token, fp, {"request": {"atomic": operations}, "key": key})

    def retry_until_ok(operations, key, attempts=12):
        codes = []
        for _ in range(attempts):
            response = send(operations, key)
            if response.get("ok"):
                return response, codes
            codes.append(response.get("code"))
            time.sleep(.5)
        raise AssertionError(f"같은 key 재전송이 성공하지 못함: {codes}")

    # 1. 응답을 받은 직후 서버가 SIGKILL로 죽는다(응답 유실과 같다). 새 서버에 같은 key를 보내면 재생이고 행은 하나다.
    tag = "r1" + uuid.uuid4().hex[:8]
    operation, query = create(tag)
    assert send([operation], "recover-" + tag)["ok"]
    server.kill()
    server.wait()
    server = launch(installed, cfg, base, env)
    again = send([operation], "recover-" + tag)
    assert again.get("ok") and again.get("replayed") is True and count(query) == 1, again
    summary["sigkill-after-commit"] = "replayed=true, rows=1"
    note(app, f"{app}: SIGKILL after commit, restart, same key replayed and left exactly one row")

    # 2. 요청이 처리되는 도중 서버가 죽는다. 두 행을 한 트랜잭션으로 만들었으니 어느 시점에 죽어도 둘 다 있거나 둘 다 없어야 하고,
    #    재시작 뒤 같은 key 재전송이 성공해 각각 정확히 한 행이 된다.
    import threading
    runs = []
    for delay in (0, .005, .02, .06):
        tags = ["r2" + uuid.uuid4().hex[:8] + "a", "r2" + uuid.uuid4().hex[:8] + "b"]
        (op_a, query_a), (op_b, query_b) = create(tags[0]), create(tags[1])
        key = "recover-" + tags[0]
        sender = threading.Thread(target=lambda: send([op_a, op_b], key))
        sender.start()
        time.sleep(delay)
        server.kill()
        server.wait()
        sender.join()
        server = launch(installed, cfg, base, env)
        after_crash = (count(query_a), count(query_b))
        assert after_crash[0] == after_crash[1] and after_crash[0] in (0, 1), f"부분 커밋: {after_crash}"
        response, codes = retry_until_ok([op_a, op_b], key)
        assert (count(query_a), count(query_b)) == (1, 1), (count(query_a), count(query_b))
        runs.append({"delayS": delay, "rowsAfterCrash": after_crash[0], "retryCodesBeforeOk": codes, "replayed": response.get("replayed", False)})
    summary["sigkill-during-request"] = runs
    note(app, f"{app}: SIGKILL during an atomic 2-row write: no partial commit, same-key retry gave one row each ({runs})")

    # 3. 서버는 살아 있고 DB 연결만 모두 끊긴다. 서버 재시작 없이 같은 key 재시도가 성공해야 한다.
    tag = "r3" + uuid.uuid4().hex[:8]
    operation, query = create(tag)
    call(["docker", "exec", container, "psql", "-U", "postgres", "-d", "aip_test_consumer_rc", "-At", "-c",
          "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity WHERE datname = 'aip_test_consumer_rc' AND pid <> pg_backend_pid()"])
    first = send([operation], "recover-" + tag)
    if first.get("ok"):
        codes = []
    else:
        _, codes = retry_until_ok([operation], "recover-" + tag)
        codes.insert(0, first.get("code"))
    assert count(query) == 1
    summary["db-connections-terminated"] = {"firstAttemptOk": bool(first.get("ok")), "codesBeforeOk": codes}
    note(app, f"{app}: all DB connections terminated; same-key retry succeeded without restart (first attempt ok={bool(first.get('ok'))}, codes {codes}), one row")
    return server

def version_query(name):
    return call(["node", "product/tools/version.mjs", name]).stdout.strip()

def main():
    head = call(["git", "rev-parse", "HEAD"]).stdout.strip()
    dirty = call(["git", "status", "--porcelain", "--untracked-files=no"]).stdout.strip()
    expected, source = prov.resolve_target(args.core_sha, os.environ, head)
    record["target"].update(expectedSha=expected, expectedShaSource=source, headSha=head, dirty=bool(dirty))
    print(f"실행 대상 Core {expected} ({source})", flush=True)
    verify(prov.source_checks(expected, head, dirty))
    gate = (core / ".github/workflows/aip-release-gate.yml").read_text()
    pinned = re.search(r"--baseline\s+([0-9a-f]{40})", gate)
    record["upgradeBaseline"]["sha"] = pinned[1] if pinned else None

    workspace_version = version_query("version")
    # 소스가 같으면 cargo는 no-op이고 다르면 다시 빌드한다. 아래 바이너리는 항상 이 checkout 소스에서 나온 것이다.
    call(["cargo", "build", "--release", "--locked", "-p", "aip-cli"])
    aip = core / "target/release/aip"
    version = call([aip, "--version"]).stdout.strip()
    binary_sha = file_sha256(aip)
    record["engine"].update(version=version, sha256=binary_sha, binary=str(aip))
    print(f"Core {head} / {version} / aip sha256 {binary_sha[:16]}", flush=True)
    versions = {"workspaceVersion": workspace_version, "npmTarball": version_query("npm-tarball"),
                "wheelFile": version_query("wheel-file"), "pythonVersion": version_query("python-version")}
    record["environment"] = {"node": call(["node", "--version"]).stdout.strip(), "python": sys.version.split()[0],
                             "platform": sys.platform, **versions}

    # 대상 소스의 SDK를 이번 실행에서 새로 만들어, 설치될 npm tarball·wheel과 내용을 대조할 기준으로 쓴다.
    fresh_sdk = work / "fresh-sdk"
    call(["node", "product/tools/build-sdk.mjs", "--out", fresh_sdk])
    fresh_hashes = tree_hashes(fresh_sdk)
    python_src = work / "python-sdk-src"
    shutil.copytree(core / "product/python", python_src)
    builder = work / "wheel-builder"
    call(["python3", "-m", "venv", builder])
    wheels = work / "wheels"
    call([builder / "bin/python", "-m", "pip", "--disable-pip-version-check", "wheel", "--no-deps", "--wheel-dir", wheels, python_src])
    wheel = next(wheels.glob("*.whl"))
    wheel_version, wheel_hashes = prov.wheel_info(wheel.read_bytes())
    sdk_root = core / "product/python/aip_sdk"
    source_hashes = {f"aip_sdk/{path.relative_to(sdk_root)}": file_sha256(path)
                     for path in sorted(sdk_root.rglob("*")) if path.is_file() and "__pycache__" not in path.parts}
    record["sdk"]["python"] = {"file": wheel.name, "version": wheel_version, "sha256": file_sha256(wheel),
                               "origin": "pip wheel of product/python in this checkout"}

    container = "aip-consumer-rc-" + uuid.uuid4().hex[:10]
    call(["docker", "run", "-d", "--rm", "--name", container, "-e", "POSTGRES_HOST_AUTH_METHOD=trust",
          "-e", "POSTGRES_DB=aip_test_consumer_rc", "-p", "127.0.0.1::5432", "postgres:17"])
    failures = []
    try:
        host = call(["docker", "port", container, "5432/tcp"]).stdout.strip().splitlines()[0]
        assert host.startswith("127.0.0.1:") and not host.endswith(":5432"), host
        for _ in range(80):
            if call(["docker", "exec", container, "pg_isready", "-q", "-h", "127.0.0.1"], check=False).returncode == 0:
                break
            time.sleep(.1)
        else:
            raise RuntimeError("PostgreSQL did not become ready")
        dsn = "postgres://postgres@" + host + "/aip_test_consumer_rc"
        pg_version = call(["docker", "exec", container, "psql", "-U", "postgres", "-At", "-c", "show server_version"]).stdout.strip()
        record["environment"]["postgres"] = pg_version
        assert pg_version.startswith("17"), pg_version
        generic = work / "generic"
        call(["node", "product/tools/package.mjs", "--out", generic, "--profile", "release"])
        assert call([generic / "bin/aip", "--version"]).stdout.strip() == version
        for app, names, resource, table in [
            ("taskshare", ("alice", "bob", "carol"), "TodoList", "todo_item"),
            ("stockdesk", ("mgr1", "clerk1", "mgr2"), "Product", "product"),
        ]:
            server = None
            root = work / app
            record["apps"][app] = {"status": "running", "steps": []}
            try:
                source = root / "source"
                source.mkdir(parents=True)
                for file in ("app.aip", "aip.app.json"):
                    shutil.copyfile(original / app / file, source / file)
                shutil.copytree(original / app / "extensions", source / "extensions", ignore=shutil.ignore_patterns("__pycache__"))
                assert json.loads(call([generic / "bin/aip", "service", "check", source / "app.aip"]).stdout)["ok"]
                bad = root / "invalid.aip"
                bad.write_text((source / "app.aip").read_text() + "\nresource Broken { fields { id: ImaginaryType } }\n")
                assert call([generic / "bin/aip", "service", "check", bad], check=False).returncode
                incoming, installed = package(source, root, "1")
                build = json.loads((incoming / "BUILD.json").read_text())
                verify(prov.engine_checks(expected, workspace_version, version, binary_sha, {app: build}), app)
                assert file_sha256(installed / "bin/aip") == binary_sha, "설치된 aip가 빌드한 바이너리와 다르다"
                npm_tarballs = sorted((installed / "sdk").glob("*.tgz"))
                assert len(npm_tarballs) == 1, npm_tarballs
                npm = npm_tarballs[0]
                with tarfile.open(npm) as archive:
                    npm_version = json.loads(archive.extractfile("package/package.json").read())["version"]
                verify(prov.sdk_checks(npm.name, versions["npmTarball"], npm_version, workspace_version, prov.tar_file_hashes(npm.read_bytes()), fresh_hashes,
                                       wheel.name, versions["wheelFile"], wheel_version, versions["pythonVersion"], wheel_hashes, source_hashes), app)
                record["sdk"]["npm"][app] = {"file": npm.name, "version": npm_version, "sha256": file_sha256(npm),
                                             "origin": "installed app package sdk/ (product/tools/package.mjs → build-sdk.mjs on this checkout)"}
                config = json.loads((original / app / "service.json").read_text())
                config["workers"]["directory"] = str(installed / "app/extensions")
                port = online_port()
                base = f"http://127.0.0.1:{port}"
                config["listen"] = f"127.0.0.1:{port}"
                cfg = root / "service.json"
                cfg.write_text(json.dumps(config))
                env = dict(common, AIP_URL=base, RUN_ID="rc-initial")
                env[config["database_url_env"]] = dsn
                for index, subject in enumerate(names, 1):
                    env[f"TOKEN_{subject.upper()}"] = call(["node", generic / "tools/dev-jwt.mjs",
                        "--dir", root / "dev-idp", "--subject", subject], cwd=root).stdout.strip()
                schema = config["schema"]
                call([installed / "bin/aip", "service", "init", installed / "app/app.aip", "--config", cfg], cwd=root, env=env)
                call(["docker", "exec", "-i", container, "psql", "-U", "postgres", "-d", "aip_test_consumer_rc", "-v", "ON_ERROR_STOP=1", "-q"],
                     input=seed_sql(app, schema), env=env)
                for index, subject in enumerate(names, 1):
                    call([installed / "bin/aip", "service", "principal", "--config", cfg, "bind", "--subject", subject, "--actor", str(index)], cwd=root, env=env)
                old_fp = fingerprint(incoming)
                # 앱마다 TypeScript와 Python 클라이언트를 모두 설치해 같은 서버에 실행한다(정의에서 생성한 binding + 설치한 SDK 아티팩트).
                python_module = "todo_binding" if app == "taskshare" else "stock_binding"
                ts_dir, py_dir = root / "client-ts", root / "client-py"
                ts_dir.mkdir(parents=True)
                py_dir.mkdir(parents=True)
                shutil.copyfile(original / app / "client/e2e.ts", ts_dir / "e2e.ts")
                shutil.copyfile(installed / "bindings.ts", ts_dir / "binding.ts")
                (ts_dir / "package.json").write_text(f'{{"name":"{app}-consumer-ts","private":true,"type":"module"}}')
                call(["npm", "install", "--offline", "--ignore-scripts", npm], cwd=ts_dir, env=env)
                shutil.copyfile(original / app / "client/e2e.py", py_dir / "e2e.py")
                call([installed / "bin/aip", "service", "gen", installed / "app/app.aip",
                      "--lang", "python", "--wire", "decimal", "--out", py_dir / f"{python_module}.py"], cwd=root, env=env)
                call(["python3", "-m", "venv", py_dir / ".venv"], cwd=py_dir)
                python = py_dir / ".venv/bin/python"
                call([python, "-m", "pip", "--disable-pip-version-check", "install", "--no-index", wheel], cwd=py_dir, env=env)
                server = launch(installed, cfg, base, env)
                for language, command, directory, expected_assertions in client_runs(app, root, python):
                    result = call(command, cwd=directory, env=dict(env, RUN_ID="rc-" + language), check=False)
                    (root / f"e2e-{language}.log").write_text(result.stdout + result.stderr)
                    result.check_returncode()
                    assert "ALL PASS" in result.stdout and len(re.findall(r"(?m)^PASS ", result.stdout)) == expected_assertions, language
                    note(app, f"{app}: {language} SDK client, {expected_assertions} assertions, installed package, ALL PASS")
                    record["apps"][app].setdefault("clients", {})[language] = expected_assertions
                server = recover(app, root, base, env, old_fp, names, installed, cfg, container, server)
                stop(server)
                server = launch(installed, cfg, base, env)
                token = env[f"TOKEN_{names[0].upper()}"]
                assert get_contract(base, token, old_fp, resource)["ok"]
                note(app, f"{app}: normal restart accepts existing principal and binding")
                counts_sql = f"SELECT (SELECT count(*) FROM {schema}.{table}), (SELECT count(*) FROM {schema}.aip_idem);"
                before = call(["psql", dsn, "-At", "-c", counts_sql], env=env).stdout.strip()
                assert all(int(value) > 0 for value in before.split("|"))
                stop(server)
                server = None
                backup = root / "before-migration.dump"
                with backup.open("wb") as stream:
                    sp.run(["docker", "exec", container, "pg_dump", "--username=postgres", "--no-owner", "--format=custom", "--schema", schema,
                            "aip_test_consumer_rc"], stdout=stream, check=True)
                updated = root / "source-update"
                shutil.copytree(source, updated)
                def_file = updated / "app.aip"
                if app == "stockdesk":
                    shutil.copyfile(original / "stockdesk-1.1.0-src/app.aip", def_file)
                else:
                    def_file.write_text(def_file.read_text().replace("archived: Bool }", "archived: Bool; color: Text(1..15)? }", 1)
                                        .replace("select id, title, archived, owner, openCount", "select id, title, archived, owner, openCount, color", 1))
                manifest = updated / "aip.app.json"
                manifest.write_text(json.dumps(dict(json.loads(manifest.read_text()), version="1.1.0")))
                new_incoming, newest = package(updated, root, "2")
                verify(prov.engine_checks(expected, workspace_version, version, binary_sha,
                                          {app + "-1.1.0": json.loads((new_incoming / "BUILD.json").read_text())}), app)
                new_cfg = root / "service-next.json"
                new_config = json.loads(cfg.read_text())
                new_config["workers"]["directory"] = str(newest / "app/extensions")
                new_cfg.write_text(json.dumps(new_config))
                migrate = [newest / "bin/aip", "service", "migrate", newest / "app/app.aip", "--config", new_cfg]
                plan = json.loads(call(migrate, cwd=root, env=env).stdout)
                assert plan["ok"] and not plan["blocked"] and plan["digest"], plan
                call(migrate + ["--apply", plan["digest"]], cwd=root, env=env)
                after = call(["psql", dsn, "-At", "-c", counts_sql], env=env).stdout.strip()
                assert before == after, (before, after)
                server = launch(newest, new_cfg, base, env)
                new_fp = fingerprint(new_incoming)
                assert new_fp != old_fp
                assert get_contract(base, token, old_fp, resource)["code"] == "WIRE.CONTRACT_MISMATCH"
                assert get_contract(base, token, new_fp, resource)["ok"]
                new_field = "color" if app == "taskshare" else "barcode"
                assert get_contract(base, token, new_fp, resource, ("id", new_field))["ok"], "migrated client cannot read the new field"
                assert not get_contract(base, token, old_fp, resource, ("id", new_field))["ok"]
                note(app, f"{app}: migrate preserved row/idempotency counts {before}; prior SDK contract rejected")
                stop(server)
                server = None
                db = "aip_test_rollback_" + app
                call(["docker", "exec", container, "createdb", "--username=postgres", db])
                restored_dsn = "postgres://postgres@" + host + "/" + db
                with backup.open("rb") as stream:
                    sp.run(["docker", "exec", "-i", container, "pg_restore", "--username=postgres", "--no-owner", "--exit-on-error",
                            "--dbname", db], stdin=stream, check=True)
                reinstalled = root / "reinstalled"
                call(["node", incoming / "tools/app-deploy.mjs", "install", "--out", reinstalled])
                restored_config = json.loads(cfg.read_text())
                restored_config["workers"]["directory"] = str(reinstalled / "app/extensions")
                restore_cfg = root / "service-restored.json"
                restore_cfg.write_text(json.dumps(restored_config))
                restored_env = dict(env)
                restored_env[config["database_url_env"]] = restored_dsn
                server = launch(reinstalled, restore_cfg, base, restored_env)
                assert call(["psql", restored_dsn, "-At", "-c", counts_sql], env=env).stdout.strip() == before
                assert get_contract(base, token, old_fp, resource)["ok"]
                note(app, f"{app}: package reinstall and rollback restored active original binding and counts {before}")
                record["apps"][app]["status"] = "pass"
            except prov.ProvenanceError:
                record["apps"][app]["status"] = "provenance-fail"
                raise
            except Exception as error:
                failures.append(app)
                record["apps"][app]["status"] = "fail"
                record["apps"][app]["error"] = str(error)[-800:]
                print(f"{app}: FAIL {error}", flush=True)
                for path in (root / "e2e.log", root / "installed-1.serve.log"):
                    if path.exists():
                        print(f"{path.name} tail:\n" + "\n".join(path.read_text().splitlines()[-9:]), flush=True)
            finally:
                stop(server)
    finally:
        call(["docker", "stop", container])
    assert not failures, failures
    record["status"] = "pass"
    print(f"PASS two consumer paths on {head}", flush=True)

exit_code = 0
try:
    main()
except BaseException as error:
    record["error"] = f"{type(error).__name__}: {str(error)[-1200:]}"
    print("FAIL", record["error"], flush=True)
    exit_code = 1
finally:
    finish()
sys.exit(exit_code)
