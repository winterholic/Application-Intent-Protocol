"""Exercise the actual CLI and generated SDK on a fresh, owned local schema."""
import argparse
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import subprocess
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]


def run(args, *, env=None, timeout=60):
    result = subprocess.run(args, cwd=ROOT, env=env, text=True, capture_output=True, timeout=timeout)
    if result.returncode:
        # Arguments can contain a DB credential; never include them in diagnostics.
        name = Path(args[0]).name
        detail = ""
        if name in ("node", "tsc"):
            detail = result.stderr or result.stdout
            for key in ("AIP_SMOKE_TOKEN", "AIP_SMOKE_OLD_TOKEN"):
                value = (env or {}).get(key)
                if value:
                    detail = detail.replace(value, "[redacted]")
        else:
            try:
                detail = json.loads(result.stderr).get("code", "")
            except (ValueError, AttributeError):
                pass
        raise RuntimeError(f"{name} failed (exit {result.returncode}) {detail[-2000:]}")
    return result.stdout


def psql(db_url, sql):
    return run(["psql", "-X", "-q", "-v", "ON_ERROR_STOP=1", "-d", db_url, "-Atc", sql], timeout=10)


def listening(process):
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        if not selector.select(10):
            raise RuntimeError("server readiness timed out")
        line = process.stdout.readline()
    if not line:
        raise RuntimeError("server exited before readiness")
    event = json.loads(line)
    if event.get("event") != "listening" or not event.get("ok"):
        raise RuntimeError("unexpected server readiness event")
    return event


def stop(process):
    if process.poll() is None:
        process.send_signal(signal.SIGTERM)
    stdout, _ = process.communicate(timeout=10)
    if process.returncode or not any(json.loads(line).get("event") == "stopped" for line in stdout.splitlines()):
        raise RuntimeError("server did not stop cleanly")


SCRIPT = """
import {connect} from SDK_IMPORT;
import {contract as original} from './original.ts';
import {contract as current} from './current.ts';
declare const process: {env: Record<string,string|undefined>};
function expect(condition: boolean, msg: string) { if (!condition) throw new Error(msg); }
function equal(a: unknown,b: unknown) {return JSON.stringify(a)===JSON.stringify(b);}
const url=process.env.AIP_SMOKE_URL!;
const token=process.env.AIP_SMOKE_TOKEN!;
const phase=process.env.AIP_SMOKE_PHASE;
const row11=ID11;
const row12=ID12;
const aip=connect(url,token,current);
const query={read:'Event',select:['id','checked'],sort:[{field:'id',dir:'asc'}]} as const;
if (phase==='before') {
  const result=await aip.read(query);
  expect(equal(result.rows.map(row=>[row.id,row.checked]),[[row11,false],[row12,false]]),'row policy and selected Id wire');
  expect((await aip.read(query)).cached===true,'read cache');
  const changed=await aip.apply({apply:'Event.mark',target:{ids:[row11]}},{key:'smoke-write'});
  expect(changed.ok && equal(changed.changed,[row11]),'caller direct apply commits');
  const fresh=await aip.read(query);
  expect(fresh.cached===false && fresh.rows[0].checked===true,'write invalidates read cache');
  const replay=await aip.apply({apply:'Event.mark',target:{ids:[row11]}},{key:'smoke-write'});
  expect(replay.ok && replay.replayed===true,'same-key replay');
  expect(equal(aip.pending(),[]),'settled write has no pending');
} else {
  expect(original.fingerprint!==current.fingerprint,'public definition changed contract');
  const old=connect(url,token,original);
  const rejected=await old.apply({apply:'Event.mark',target:{ids:[row12]}});
  expect(!rejected.ok && rejected.code==='CONTRACT_MISMATCH','stale first write rejected before commit');
  expect(equal(old.pending(),[]),'contract mismatch is settled rejection');
  const before=await aip.read(query);
  expect(equal(before.rows.map(row=>[row.id,row.checked]),[[row11,true],[row12,false]]),'restart preserves committed data and stale write does not mutate');
  const result=await aip.apply({apply:'Event.mark',target:{ids:[row12]}});
  expect(result.ok && equal(result.changed,[row12]),'regenerated caller applies');
  const staleSession=connect(url,process.env.AIP_SMOKE_OLD_TOKEN!,current);
  expect((await staleSession.post('/session',{})).code==='UNAUTHENTICATED','restart invalidates old demo credentials');
  const recovered=await aip.post('/status',{key:'smoke-write',request:{apply:'Event.mark',target:{ids:[row11]}}});
  expect(recovered.ok && equal(recovered.changed,[row11]),'committed same-key status survives server restart');
}
console.log(JSON.stringify({ok:true,phase}));
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db-url", default="host=localhost dbname=postgres")
    parser.add_argument("--wire", choices=("decimal", "safe", "both"), default="both")
    parser.add_argument("--sdk", choices=("source", "package", "both"), default="both")
    args = parser.parse_args()
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    run([cargo, "build", "--offline"])
    binary = str(ROOT / "target/debug/aip-prototype")
    sdk = str(ROOT / "sdk/index.ts")
    tsc = str(ROOT.parent / "spikes/spike-0-ts/node_modules/.bin/tsc")
    modes = ("source", "package") if args.sdk == "both" else (args.sdk,)
    for sdk_mode in modes:
        for wire in (("decimal", "safe") if args.wire == "both" else (args.wire,)):
            exercise(args.db_url, binary, sdk, tsc, wire, sdk_mode)


def exercise(db_url, binary, sdk, tsc, wire, sdk_mode):
    schema = "aip_smoke_" + uuid.uuid4().hex
    owned = False
    servers = []
    try:
        with tempfile.TemporaryDirectory(prefix="aip-prototype-smoke-") as directory:
            directory = Path(directory)
            sdk_import = sdk
            if sdk_mode == "package":
                package = directory / "sdk-package"
                run(["node", str(ROOT / "tools/build-sdk.mjs"), "--out", str(package)])
                packed = json.loads(run(["npm", "pack", "--offline", "--ignore-scripts", "--json", "--pack-destination", str(directory), str(package)]))
                (directory / "package.json").write_text(json.dumps({"type": "module", "private": True}))
                run(["npm", "install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", "--prefix", str(directory), str(directory / packed[0]["filename"])])
                sdk_import = "@aip/prototype-sdk"
            source = directory / "app.aip"
            original = (ROOT / "example/app.aip").read_text()
            source.write_text(original)
            run([binary, "init", str(source), "--schema", schema, "--db-url", db_url])
            owned = True
            psql(db_url, f"""INSERT INTO {schema}.member(id) VALUES(1),(2);
                INSERT INTO {schema}.event(id,member_id,checked,title,link,phase,date) VALUES
                (11,1,false,'first','https://example.test/1','READY','2026-10-04Z'),
                (12,1,false,'second','https://example.test/2','READY','2026-10-04Z'),
                (99,2,false,'private','https://example.test/3','DONE','2026-10-04Z')""")

            def generate(name):
                return json.loads(run([binary, "gen", str(source), "--wire", wire, "--out", str(directory / name), "--sdk-import", sdk_import]))

            first = generate("original.ts")
            generate("current.ts")
            script = directory / "caller.mts"
            script.write_text(SCRIPT.replace("ID11", "'11'" if wire == "decimal" else "11").replace("ID12", "'12'" if wire == "decimal" else "12").replace("SDK_IMPORT", json.dumps(sdk_import)))

            def call(phase, old_token=""):
                executable = script
                if sdk_mode == "package":
                    run([tsc, "--strict", "--target", "es2022", "--module", "nodenext", "--moduleResolution", "nodenext", "--rewriteRelativeImportExtensions", "--rootDir", str(directory), "--outDir", str(directory / "js"), "--noEmitOnError", str(script)])
                    executable = directory / "js/caller.mjs"
                else:
                    run([tsc, "--noEmit", "--strict", "--target", "esnext", "--module", "nodenext", "--moduleResolution", "nodenext", "--allowImportingTsExtensions", str(script)])
                process = subprocess.Popen([binary, "serve", str(source), "--schema", schema, "--db-url", db_url,
                    "--listen", "127.0.0.1:0", "--wire", wire, "--dev-actor", "1", "--dev-token-ttl", "60"],
                    cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                servers.append(process)
                event = listening(process)
                env = os.environ.copy()
                env.update(AIP_SMOKE_URL="http://" + event["address"], AIP_SMOKE_TOKEN=event["devToken"], AIP_SMOKE_PHASE=phase, AIP_SMOKE_OLD_TOKEN=old_token)
                response = json.loads(run(["node", str(executable)], env=env, timeout=20))
                if response != {"ok": True, "phase": phase}:
                    raise RuntimeError("caller smoke result mismatch")
                stop(process)
                return event

            initial = call("before")
            if original.count("bulk maxRows 20") != 1:
                raise RuntimeError("canonical bulk mutation target missing")
            source.write_text(original.replace("bulk maxRows 20", "bulk maxRows 10"))
            changed = generate("current.ts")
            if first["fingerprint"] == changed["fingerprint"]:
                raise RuntimeError("public change did not change contract")
            second = call("after", initial["devToken"])
            if second["fingerprint"] != changed["fingerprint"]:
                raise RuntimeError("generated and running contracts differ")
            rows = psql(db_url, f"SELECT id,checked FROM {schema}.event ORDER BY id").strip().splitlines()
            if rows != ["11|t", "12|t", "99|f"]:
                raise RuntimeError("unexpected committed rows")
            print(json.dumps({"ok": True, "sdk": sdk_mode, "wire": wire, "phases": 2, "definitionChange": "bulk 20 to 10", "dataPreserved": True, "staleFirstWrite": "CONTRACT_MISMATCH"}))
    finally:
        for process in servers:
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=10)
        if owned:
            psql(db_url, f"DROP SCHEMA {schema} CASCADE")


if __name__ == "__main__":
    main()
