
import json, os, re, shutil, socket, subprocess as sp, tempfile, time, urllib.request, urllib.error, uuid
from pathlib import Path

core = Path.cwd()
original = Path(__file__).resolve().parent / "apps"
work = Path(tempfile.mkdtemp(prefix="aip-consumer-rc-"))
print("Evidence directory:", work, flush=True)
common = {key: val for key, val in os.environ.items() if not key.startswith("PG")}
common.update(npm_config_cache=str(work / "npm-cache"), PIP_NO_CACHE_DIR="1", PYTHONDONTWRITEBYTECODE="1")

def call(argv, cwd=core, env=common, *, show=False, input=None, check=True):
    result = sp.run(argv, cwd=cwd, env=env, input=input, capture_output=True, text=True)
    if show or (check and result.returncode):
        print(result.stdout.rstrip(), flush=True)
        print(result.stderr.rstrip(), flush=True)
    if check:
        result.check_returncode()
    return result

revision = call(["git","rev-parse","HEAD"]).stdout.strip()
assert revision == "4e1325644b267f2b3177a327258c0882ee5711c2"
assert not call(["git", "status", "--porcelain", "--untracked-files=no"]).stdout
aip = core / "target/release/aip"
version = call([aip, "--version"]).stdout.strip()
assert version == "aip 1.0.0", version
print(f"Core {revision} / {version}", flush=True)

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

def get_contract(url, token, fp, read_resource):
    request = urllib.request.Request(url + "/read",
        data=json.dumps({"query": {"read": read_resource, "select": ["id"]}}).encode(),
        headers={"authorization": f"Bearer {token}", "x-aip-contract": fp, "content-type": "application/json"})
    try:
        response = urllib.request.urlopen(request, timeout=3)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        return json.load(response)

container = "aip-consumer-rc-" + uuid.uuid4().hex[:10]
call(["docker", "run", "-d", "--rm", "--name", container, "-e", "POSTGRES_HOST_AUTH_METHOD=trust",
      "-e", "POSTGRES_DB=aip_test_consumer_rc", "-p", "127.0.0.1::5432", "postgres:17"])
failures = []
try:
    host = call(["docker", "port", container, "5432/tcp"]).stdout.strip()
    assert host.startswith("127.0.0.1:") and not host.endswith(":5432"), host
    for _ in range(80):
        if call(["docker", "exec", container, "pg_isready", "-q", "-h", "127.0.0.1"], check=False).returncode == 0:
            break
        time.sleep(.1)
    else:
        raise RuntimeError("PostgreSQL did not become ready")
    dsn = "postgres://postgres@" + host + "/aip_test_consumer_rc"
    generic = work / "generic"
    call(["node", "product/tools/package.mjs", "--out", generic, "--profile", "release"])
    assert call([generic / "bin/aip", "--version"]).stdout.strip() == version
    for app, names, resource, table, expected_checks in [
        ("taskshare", ("alice","bob","carol"), "TodoList", "todo_item", 22),
        ("stockdesk", ("mgr1","clerk1","mgr2"), "Product", "product", 23),
    ]:
        server = None
        root = work / app
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
            call(["psql", dsn, "-v", "ON_ERROR_STOP=1", "-c", seed_sql(app, schema)], env=env)
            for index, subject in enumerate(names, 1):
                call([installed / "bin/aip", "service", "principal", "--config", cfg, "bind", "--subject", subject, "--actor", str(index)], cwd=root, env=env)
            client = root / "client"
            client.mkdir()
            old_fp = fingerprint(incoming)
            if app == "taskshare":
                shutil.copyfile(original / app / "client/e2e.ts", client / "e2e.ts")
                shutil.copyfile(installed / "bindings.ts", client / "binding.ts")
                (client / "package.json").write_text('{"name":"taskshare-consumer","private":true,"type":"module"}')
                call(["npm", "install", "--offline", "--ignore-scripts", installed / "sdk/aip-sdk-1.0.0.tgz"], cwd=client, env=env)
                command = ["node", "e2e.ts"]
            else:
                shutil.copyfile(original / app / "client/e2e.py", client / "e2e.py")
                call([installed / "bin/aip", "service", "gen", installed / "app/app.aip",
                      "--lang", "python", "--wire", "decimal", "--out", client / "stock_binding.py"], cwd=root, env=env)
                python_src = root / "python-sdk-src"
                shutil.copytree(core / "product/python", python_src)
                call(["python3", "-m", "venv", client / ".venv"], cwd=client)
                python = client / ".venv/bin/python"
                wheels = client / "wheels"
                call([python, "-m", "pip", "--disable-pip-version-check", "wheel", "--no-deps", "--wheel-dir", wheels, python_src], cwd=client, env=env)
                wheel = next(wheels.glob("application_intent_protocol_sdk-1.0.0-*.whl"))
                call([python, "-m", "pip", "--disable-pip-version-check", "install", "--no-index", wheel], cwd=client, env=env)
                command = [python, "e2e.py"]
            server = launch(installed, cfg, base, env)
            result = call(command, cwd=client, env=env, check=False)
            (root / "e2e.log").write_text(result.stdout + result.stderr)
            result.check_returncode()
            assert "ALL PASS" in result.stdout and len(re.findall(r"(?m)^PASS ", result.stdout)) == expected_checks
            print(f"{app}: {expected_checks} SDK assertions, installed package, ALL PASS", flush=True)
            stop(server)
            server = launch(installed, cfg, base, env)
            token = env[f"TOKEN_{names[0].upper()}"]
            assert get_contract(base, token, old_fp, resource)["ok"]
            print(f"{app}: normal restart accepts existing principal and binding", flush=True)
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
            print(f"{app}: migrate preserved row/idempotency counts {before}; prior SDK contract rejected", flush=True)
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
            print(f"{app}: package reinstall and rollback restored active original binding and counts {before}", flush=True)
        except Exception as error:
            failures.append(app)
            print(f"{app}: FAIL {error}", flush=True)
            for path in (root / "e2e.log", root / "installed-1.serve.log"):
                if path.exists():
                    print(f"{path.name} tail:\n" + "\n".join(path.read_text().splitlines()[-9:]), flush=True)
        finally:
            stop(server)
finally:
    call(["docker", "stop", container])
assert not failures, failures
print(f"PASS two consumer paths on {revision}", flush=True)
