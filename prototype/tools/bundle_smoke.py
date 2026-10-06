"""Exercise a relocated private bundle using only its packaged runtime assets.

On a fresh build this invokes the bundle builder once. To test a durable bundle
without touching the checkout or Cargo, pass ``--bundle <bundle-directory>``.
Every run copies the input to a fresh path containing spaces, verifies manifest
hashes and links, installs only the included SDK tarball offline, and uses a
fresh UUID PostgreSQL schema. This is a current-host private experiment, not a
production deployment test.
"""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import platform
import shutil
import subprocess
import sys
import tempfile
import uuid


BASE_CALLER = r"""
import {connect,WriteUnsettled} from '@aip/prototype-sdk';
import {contract} from './contract.ts';
const cfg = JSON.parse(await new Promise(resolve => {
  let text = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', chunk => text += chunk);
  process.stdin.on('end', () => resolve(text));
}));
const wire = cfg.wire;
const id = value => wire === 'safe' ? value : String(value);
function expect(value, label) { if (!value) throw new Error(label); }
const client = connect(cfg.api,cfg.token,contract);
const query = {read:'Event',select:['id','checked'],sort:[{field:'id',dir:'asc'}]};
const first = await client.read(query);
const hit = await client.read(query);
expect(JSON.stringify(first.rows.map(row => [row.id,row.checked])) === JSON.stringify([[id(11),false],[id(12),false]]), 'initial read rows');
expect(!first.cached && hit.cached, 'initial read cache hit');
const request = {apply:'Event.mark',target:{ids:[id(11)]}};
const key = `bundle-mark-${wire}`;
const applied = await client.apply(request,{key});
expect(applied.ok && JSON.stringify(applied.changed) === JSON.stringify([id(11)]), 'apply Event.mark');
const replay = await client.apply(request,{key});
expect(replay.ok && replay.replayed === true, 'idempotent replay');
const after = await client.read(query);
expect(!after.cached && after.rows[0].checked === true, 'apply invalidates cached read');
const conflict = await client.writeExtension('Event.confirm',{id:id(11)},{key});
expect(!conflict.ok && conflict.code==='IDEMPOTENCY_MISMATCH','WRITE shares standard apply key');
let lost = false;
const droppingFetch = async (...args) => {
  const response = await fetch(...args);
  if (String(args[0]).endsWith('/apply') && !lost) {lost=true;await response.text();throw new TypeError('committed response discarded');}
  return response;
};
const writer = connect(cfg.api,cfg.token,contract,droppingFetch);
const writeKey=`bundle-confirm-${wire}-${cfg.lang}`;
try {await writer.writeExtension('Event.confirm',{id:id(12)},{key:writeKey,retries:0});throw new Error('response loss settled');}
catch(error){expect(error instanceof WriteUnsettled,'response loss pending');}
expect(JSON.stringify(writer.pending())===JSON.stringify([writeKey]),'WRITE pending key');
const recovered=(await writer.retryPending())[0];
expect(recovered?.ok && recovered.output.id===id(12) && recovered.output.count===1 && recovered.replayed,'WRITE recovery');
const repeated=await writer.writeExtension('Event.confirm',{id:id(12)},{key:writeKey});
expect(repeated.ok && repeated.replayed && repeated.output.count===1,'WRITE stable replay');
const unchanged=await writer.writeExtension('Event.confirm',{id:id(12)},{key:writeKey+'-new'});
expect(unchanged.ok && unchanged.output.count===0,'new intent observes repeat unchanged');
const denied=await writer.writeExtension('Event.confirm',{id:id(99)},{key:writeKey+'-foreign'});
expect(!denied.ok && denied.code==='EXTENSION_ERROR','WRITE foreign actor denial');
expect(JSON.stringify(writer.pending())==='[]','settled WRITE pending empty');
const stale = connect(cfg.api,cfg.token,{...contract,fingerprint:'0'.repeat(64)});
const rejected = await stale.apply({apply:'Event.mark',target:{ids:[id(12)]}},{key:`bundle-stale-${wire}`});
expect(!rejected.ok && rejected.code === 'CONTRACT_MISMATCH', 'stale first write rejected');
expect(JSON.stringify(stale.pending()) === '[]', 'stale write is not pending');
console.log(JSON.stringify({ok:true,wire,lang:cfg.lang,flow:'read-cache-apply-write-recovery',
  cacheHit:hit.cached,cacheInvalidated:!after.cached,replayed:replay.replayed,
  staleCode:rejected.code,stalePending:stale.pending().length,writeRecovered:recovered.replayed,unchangedCount:unchanged.output.count,writeDenied:denied.code}));
"""


RECRUITMENT_CALLER = r"""
import {connect} from '@aip/prototype-sdk';
import {contract} from './contract.ts';
const cfg = JSON.parse(await new Promise(resolve => {
  let text = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', chunk => text += chunk);
  process.stdin.on('end', () => resolve(text));
}));
const wire = cfg.wire;
const id = value => wire === 'safe' ? value : String(value);
function expect(value, label) { if (!value) throw new Error(label); }
const client = connect(cfg.api,cfg.token,contract);
const pendingBefore = JSON.stringify(client.pending());
const fromTen = await client.extension('Recruitment.stats',{clubId:id(10)});
expect(fromTen.approvedApplicants === 1, 'club 10 stats');
let deniedCode = null;
let deniedReturned = false;
try { await client.extension('Recruitment.stats',{clubId:id(11)}); deniedReturned = true; }
catch (error) { deniedCode = error?.code ?? null; }
expect(!deniedReturned && deniedCode === 'EXTENSION_ERROR', 'club 11 public error code: ' + deniedCode);
expect(JSON.stringify(client.pending()) === pendingBefore, 'extension denial leaves pending writes unchanged');

const query = {read:'Recruitment',select:['id','bookmarkCount',{club:{select:['id','name','logo']}}],sort:[{field:'id',dir:'asc'}]};
const before = await client.read(query);
expect(JSON.stringify(before.rows.map(row => row.id)) === JSON.stringify([id(100),id(101)]), 'nested recruitment read ids');
expect(before.cached === false && before.stored === false, 'time-dependent Recruitment read is not cached');
for (const row of before.rows) {
  expect(typeof row.bookmarkCount === 'number' && Number.isSafeInteger(row.bookmarkCount), 'bookmarkCount decoded Int');
  expect(row.club && typeof row.club.name === 'string', 'nested public club fields');
  expect(row.club.logo === null, 'nullable public logo');
}

const notExposed = await client.apply({apply:'Recruitment.close',target:{ids:[id(100)]}},{key:`bundle-not-exposed-${wire}-${cfg.lang}`});
expect(!notExposed.ok && notExposed.code === 'NOT_EXPOSED', 'unexposed close remains unavailable');
expect(JSON.stringify(client.pending()) === '[]', 'unexposed write is not pending');
const after = await client.read(query);
expect(JSON.stringify(after.rows.map(row => row.id)) === JSON.stringify([id(100),id(101)]), 'unexposed close preserves visible data');
console.log(JSON.stringify({ok:true,wire,lang:cfg.lang,flow:'stats-read-not-exposed',
  approvedApplicants:fromTen.approvedApplicants,deniedCode,nestedRows:before.rows.length,
  noCache:before.stored === false,notExposedCode:notExposed.code,pending:client.pending().length}));
"""


def run(argv, *, cwd, timeout=30, input_text=None, redactions=()):
    result = subprocess.run(
        [str(arg) for arg in argv], cwd=cwd, text=True, input=input_text,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout,
    )
    if result.returncode:
        detail = (result.stderr or result.stdout)[-3000:]
        for value in redactions:
            if value:
                detail = detail.replace(value, "[redacted]")
        raise RuntimeError(f"{Path(argv[0]).name} failed ({result.returncode}): {detail}")
    return result.stdout


def psql(db_url, *args, cwd):
    return run(["psql", "-X", "-q", "-v", "ON_ERROR_STOP=1", "-d", db_url, *args], cwd=cwd, timeout=15)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def checked_bundle_copy(source, relocated):
    if source.is_symlink() or not source.is_dir():
        raise RuntimeError("bundle input must be a real directory")
    shutil.copytree(source, relocated, symlinks=True)
    manifest = json.loads((relocated / "manifest.json").read_text())
    files = manifest.get("files")
    if not isinstance(files, dict) or "manifest.json" in files or not files:
        raise RuntimeError("bundle manifest file list is invalid")
    for path in relocated.rglob("*"):
        if path.is_symlink():
            raise RuntimeError("relocated bundle contains a symlink")
    for name, expected in files.items():
        if not isinstance(name, str) or "\\" in name or "\x00" in name:
            raise RuntimeError("manifest contains an unsafe relative path")
        relative = PurePosixPath(name)
        if relative.is_absolute() or not relative.parts or any(part in ("", ".", "..") for part in relative.parts) or relative.as_posix() != name:
            raise RuntimeError("manifest contains an unsafe relative path")
        target = relocated.joinpath(*relative.parts)
        try:
            target.resolve(strict=True).relative_to(relocated.resolve(strict=True))
        except (OSError, ValueError):
            raise RuntimeError("manifest path escapes the relocated bundle") from None
        if not target.is_file() or target.is_symlink() or sha256(target) != expected:
            raise RuntimeError("bundle manifest SHA-256 mismatch: " + name)
    required = (
        "bin/aip-prototype", "examples/app.aip", "examples/recruitment.aip",
        "examples/recruitment-seed.sql", "examples/event-seed.sql", "extensions/node/event.mjs", "extensions/python/event.py", "extensions/node/recruitment.mjs",
        "extensions/python/recruitment.py", "sdk/aip-prototype-sdk-0.0.0.tgz",
    )
    if any(name not in files for name in required):
        raise RuntimeError("bundle lacks a required CLI, fixture, extension, seed, or SDK artifact")
    return manifest


def await_listening(process):
    import selectors
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        if not selector.select(15):
            raise RuntimeError("bundled server readiness timed out")
        line = process.stdout.readline()
    if not line:
        raise RuntimeError("bundled server exited before readiness")
    event = json.loads(line)
    if event.get("event") != "listening" or event.get("ok") is not True:
        raise RuntimeError("bundled server returned an unexpected readiness event")
    return event


def stop(process):
    if process.poll() is None:
        process.terminate()
    stdout, _ = process.communicate(timeout=15)
    if process.returncode != 0 or not any(
        json.loads(line).get("event") == "stopped" for line in stdout.splitlines() if line.startswith("{")
    ):
        raise RuntimeError("bundled server did not stop cleanly")


def seed(db_url, schema, bundle_root, extension_app, consumer):
    if extension_app:
        seed_args = ["-1", "-v", "schema=" + schema, "-f", bundle_root / "examples/recruitment-seed.sql"]
    else:
        seed_args = ["-1", "-v", "schema=" + schema, "-f", bundle_root / "examples/event-seed.sql"]
    psql(db_url, *seed_args, cwd=bundle_root)
    query = (
        f"SELECT id || ':' || status FROM {schema}.recruitment ORDER BY id"
        if extension_app else f"SELECT id || ':' || checked FROM {schema}.event ORDER BY id"
    )
    return psql(db_url, "-Atc", query, cwd=bundle_root).strip().splitlines()


def compile_generated(binary, source, wire, consumer, bundle_root):
    output = consumer / "contract.ts"
    info = json.loads(run([
        binary, "gen", source, "--wire", wire, "--out", output, "--sdk-import", "@aip/prototype-sdk",
    ], cwd=bundle_root, timeout=20))
    if info.get("ok") is not True or len(info.get("fingerprint", "")) != 64:
        raise RuntimeError("bundled CLI failed to generate a contract")
    return info["fingerprint"]


def run_caller(binary, source, wire, lang, bundle_root, consumer, db_url, schema, extension_app):
    fingerprint = compile_generated(binary, source, wire, consumer, bundle_root)
    process = None
    try:
        command = [str(binary), "serve", str(source), "--schema", schema, "--db-url", db_url,
                   "--listen", "127.0.0.1:0", "--wire", wire, "--dev-actor", "1", "--dev-token-ttl", "60"]
        command.extend(["--worker-dir", str(bundle_root / "extensions" / lang), "--worker-lang", lang])
        if not extension_app:
            command.append("--enable-write-extensions")
        process = subprocess.Popen(command, cwd=bundle_root, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        event = await_listening(process)
        if event.get("fingerprint") != fingerprint:
            raise RuntimeError("bundled gen/serve fingerprints differ")
        (consumer / "caller.mjs").write_text(RECRUITMENT_CALLER if extension_app else BASE_CALLER)
        cfg = json.dumps({"api": "http://" + event["address"], "token": event["devToken"], "wire": wire, "lang": lang})
        output = run(["node", "--experimental-strip-types", "caller.mjs"], cwd=consumer, timeout=60,
                     input_text=cfg, redactions=(event["devToken"],))
        try:
            result = json.loads(output.splitlines()[-1])
        except (ValueError, IndexError):
            raise RuntimeError("bundled SDK caller did not emit its JSON result") from None
        expected = (
            {"ok": True, "wire": wire, "lang": lang, "flow": "stats-read-not-exposed",
             "approvedApplicants": 1, "deniedCode": "EXTENSION_ERROR", "nestedRows": 2,
             "noCache": True, "notExposedCode": "NOT_EXPOSED", "pending": 0}
            if extension_app else
            {"ok": True, "wire": wire, "lang": lang, "flow": "read-cache-apply-write-recovery",
             "cacheHit": True, "cacheInvalidated": True, "replayed": True,
             "staleCode": "CONTRACT_MISMATCH", "stalePending": 0,
             "writeRecovered": True, "unchangedCount": 0, "writeDenied": "EXTENSION_ERROR"}
        )
        if result != expected:
            raise RuntimeError("bundled SDK result differed from expected behavior")
        stop(process)
        process = None
        rows = psql(
            db_url, "-Atc",
            f"SELECT id || ':' || status FROM {schema}.recruitment ORDER BY id" if extension_app
            else f"SELECT id || ':' || checked FROM {schema}.event ORDER BY id",
            cwd=bundle_root,
        ).strip().splitlines()
        expected_rows = ["100:PUBLISHED", "101:PUBLISHED"] if extension_app else ["11:true", "12:true", "99:false"]
        if rows != expected_rows:
            raise RuntimeError("bundle flow database state differed from expected rows")
        return {"flow": result["flow"], "wire": wire, "lang": lang,
                "result": result, "database": rows}
    finally:
        if process is not None:
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=10)


def exercise_flow(bundle_root, temp_root, db_url, wire, lang, extension_app):
    schema = "aip_bundle_smoke_" + uuid.uuid4().hex
    source = bundle_root / ("examples/recruitment.aip" if extension_app else "examples/app.aip")
    binary = bundle_root / "bin/aip-prototype"
    consumer = temp_root / f"consumer-{'recruitment' if extension_app else 'app'}-{wire}-{lang}"
    consumer.mkdir()
    (consumer / "package.json").write_text('{"type":"module","private":true}\n')
    tarball = bundle_root / "sdk/aip-prototype-sdk-0.0.0.tgz"
    run(["npm", "install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", "--prefix", consumer, tarball], cwd=consumer, timeout=60)

    init = json.loads(run([binary, "init", source, "--schema", schema, "--db-url", db_url], cwd=bundle_root, timeout=20))
    owns_schema = init.get("ok") is True and init.get("schema") == schema
    if not owns_schema:
        raise RuntimeError("bundled CLI init did not confirm unique schema ownership")
    try:
        initial_rows = seed(db_url, schema, bundle_root, extension_app, consumer)
        expected_initial = ["100:PUBLISHED", "101:PUBLISHED"] if extension_app else ["11:false", "12:false", "99:false"]
        if initial_rows != expected_initial:
            raise RuntimeError("bundled seed produced unexpected initial rows")
        return run_caller(binary, source, wire, lang, bundle_root, consumer, db_url, schema, extension_app)
    finally:
        if owns_schema:
            psql(db_url, "-Atc", f"DROP SCHEMA {schema} CASCADE", cwd=bundle_root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", help="기존 private bundle 디렉터리. 지정하면 빌더/Cargo/checkout 없이 실행")
    parser.add_argument("--db-url", default="host=localhost dbname=postgres")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="aip-bundle-smoke-") as temp:
        temp_root = Path(temp)
        if args.bundle:
            bundle_input = Path(args.bundle).expanduser().absolute()
        else:
            prototype = Path(__file__).resolve().parents[1]
            build_output = temp_root / "builder output"
            built = json.loads(run([sys.executable, prototype / "tools/build-bundle.py", "--out", build_output], cwd=prototype, timeout=240))
            if built.get("ok") is not True or Path(built.get("out", "")) != build_output:
                raise RuntimeError("bundle builder did not report its fresh output path")
            bundle_input = build_output
        relocated = temp_root / "relocated bundle with spaces"
        manifest = checked_bundle_copy(bundle_input, relocated)
        if manifest.get("bundleKind") != "private-local-macos-development" or manifest.get("host", {}).get("architecture") != platform.machine():
            raise RuntimeError("relocated bundle manifest does not describe this host")
        for wire in ("safe", "decimal"):
            for lang in ("node", "python"):
                result = exercise_flow(relocated, temp_root, args.db_url, wire, lang, False)
                print(json.dumps({"ok": True, "bundle": relocated.name, **result}))
        for wire in ("safe", "decimal"):
            for lang in ("node", "python"):
                result = exercise_flow(relocated, temp_root, args.db_url, wire, lang, True)
                print(json.dumps({"ok": True, "bundle": relocated.name, **result}))


if __name__ == "__main__":
    main()
