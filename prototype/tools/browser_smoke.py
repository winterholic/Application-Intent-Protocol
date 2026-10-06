"""Exercise the installed SDK in headless Chrome against the real local server.

Setup is intentionally isolated from the project and global Python environment:

    python3 -m venv /tmp/aip-browser-tools/venv
    /tmp/aip-browser-tools/venv/bin/python -m pip install playwright
    /tmp/aip-browser-tools/venv/bin/python tools/browser_smoke.py

The script uses an already-installed Google Chrome via Playwright's
``channel="chrome"`` and never downloads a browser or npm dependency.
"""
import argparse
import json
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import urllib.error
import urllib.request
import uuid

from playwright.sync_api import sync_playwright

from smoke import ROOT, listening, psql, run, stop


def modes(selected, values):
    return values if selected == "both" else (selected,)


def install_sdk(directory):
    package = directory / "sdk-package"
    run(["node", str(ROOT / "tools/build-sdk.mjs"), "--out", str(package)])
    packed = json.loads(run([
        "npm", "pack", "--offline", "--ignore-scripts", "--json",
        "--pack-destination", str(directory), str(package),
    ]))
    run(["npm", "install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund",
         "--prefix", str(directory), str(directory / packed[0]["filename"])])


def seed_base(db_url, schema):
    psql(db_url, f"""INSERT INTO {schema}.member(id) VALUES(1),(2);
        INSERT INTO {schema}.event(id,member_id,checked,title,link,phase,date) VALUES
        (11,1,false,'first','https://example.test/1','READY','2026-10-04Z'),
        (12,1,false,'second','https://example.test/2','READY','2026-10-04Z'),
        (99,2,false,'private','https://example.test/3','DONE','2026-10-04Z')""")


def seed_extensions(db_url, schema):
    psql(db_url, f"""INSERT INTO {schema}.school (id, name) VALUES (1, 'A대');
        INSERT INTO {schema}.member (id, school_id) VALUES (1, 1), (2, 1), (3, 1);
        INSERT INTO {schema}.club (id, name, logo, school_id) VALUES
          (10, 'A동아리', NULL, 1), (11, 'B동아리', NULL, 1);
        INSERT INTO {schema}.club_member (club_id, member_id, role) VALUES
          (10, 1, 'MANAGER'), (10, 2, 'MEMBER'), (11, 3, 'ADMIN');
        INSERT INTO {schema}.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
          (100, 'A 모집', '2099-10-10T00:00:00Z', 'PUBLISHED', 0, 10, NULL),
          (101, 'B 모집', '2099-10-12T00:00:00Z', 'PUBLISHED', 0, 11, NULL);
        INSERT INTO {schema}.apply (id, recruitment_id, status) VALUES
          (200, 100, 'APPROVE'), (201, 100, 'PENDING'),
          (202, 101, 'APPROVE'), (203, 101, 'APPROVE')""")


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, *_):
        pass


def start_static(directory):
    server = ThreadingHTTPServer(("127.0.0.1", 0), partial(QuietHandler, directory=str(directory)))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread


def js_literal(value):
    return json.dumps(value, ensure_ascii=False).replace("</", "<\\/")


def browser_page(directory, sdk_path):
    (directory / "package.json").write_text('{"type":"module","private":true}\n')
    (directory / "index.html").write_text(f"""<!doctype html>
<meta charset="utf-8">
<link rel="icon" href="data:,">
<script type="importmap">{{"imports":{{"@aip/prototype-sdk":"{sdk_path}"}}}}</script>
<pre id="result">pending</pre>
<script type="module" src="./browser.js"></script>
""")


BASE_BROWSER = r"""
import {connect,WriteUnsettled} from '@aip/prototype-sdk';
import {contract} from './out/contract.js';
const cfg = CONFIG;
const output = document.querySelector('#result');
const report = (value) => { window.__result = value; output.textContent = JSON.stringify(value); };
const codeOf = (error) => error && typeof error.code === 'string' ? error.code : null;
try {
  let dropNext=false;
  const droppingFetch=async(...args)=>{
    const response=await fetch(...args);
    if(dropNext && String(args[0]).endsWith('/apply')){dropNext=false;await response.text();throw new TypeError('committed WRITE response discarded');}
    return response;
  };
  const client = connect(cfg.api, cfg.token, contract,droppingFetch);
  const query = {read:'Event', select:['id','checked'], sort:[{field:'id',dir:'asc'}]};
  const before = await client.read(query);
  const cached = await client.read(query);
  const expectedIds = cfg.wire === 'safe' ? [11,12] : ['11','12'];
  if (JSON.stringify(before.rows.map(row => row.id)) !== JSON.stringify(expectedIds) || before.cached || !cached.cached) throw new Error('read/cache mismatch ' + JSON.stringify({ids:before.rows.map(row => row.id), firstCached:before.cached, secondCached:cached.cached, expectedIds}));
  const id = cfg.wire === 'safe' ? 11 : '11';
  const applied = await client.apply({apply:'Event.mark',target:{ids:[id]}},{key:'browser-smoke-' + cfg.wire});
  if (!applied.ok || JSON.stringify(applied.changed) !== JSON.stringify([id])) throw new Error('apply mismatch');
  const after = await client.read(query);
  if (after.cached || after.rows[0].checked !== true) throw new Error('apply cache invalidation mismatch');
  let authCode = null;
  try { await connect(cfg.api, 'invalid-token', contract).read(query); }
  catch (error) { authCode = codeOf(error); }
  if (authCode !== 'UNAUTHENTICATED') throw new Error('auth error code was not readable: ' + authCode);
  let contractCode = null;
  try { await connect(cfg.api, cfg.token, {...contract, fingerprint:'0'.repeat(64)}).read(query); }
  catch (error) { contractCode = codeOf(error); }
  if (contractCode !== 'CONTRACT_MISMATCH') throw new Error('contract error code was not readable: ' + contractCode);
  const writeId=cfg.wire==='safe'?12:'12';
  const writeKey='browser-write-'+cfg.wire+'-'+cfg.lang;
  dropNext=true;
  try {await client.writeExtension('Event.confirm',{id:writeId},{key:writeKey,retries:0});throw new Error('lost response settled');}
  catch(error){if(!(error instanceof WriteUnsettled))throw error;}
  if(JSON.stringify(client.pending())!==JSON.stringify([writeKey]))throw new Error('WRITE pending missing');
  const recovered=(await client.retryPending())[0];
  if(!recovered?.ok || recovered.output.id!==writeId || recovered.output.count!==1 || !recovered.replayed)throw new Error('WRITE recovery mismatch');
  const replay=await client.writeExtension('Event.confirm',{id:writeId},{key:writeKey});
  if(!replay.ok || !replay.replayed || replay.output.id!==writeId || !Object.isFrozen(replay.output))throw new Error('WRITE replay mismatch');
  const written=await client.read(query);
  if(written.cached || written.rows.some(row=>!row.checked))throw new Error('WRITE cache invalidation mismatch');
  const foreign=await client.writeExtension('Event.confirm',{id:cfg.wire==='safe'?99:'99'},{key:'browser-foreign'});
  if(foreign.ok || foreign.code!=='EXTENSION_ERROR')throw new Error('foreign WRITE denial mismatch');
  let writeAuthCode=null;
  try {await connect(cfg.api,'invalid-token',contract).writeExtension('Event.confirm',{id:writeId},{key:'browser-write-unauth'});}
  catch(error){writeAuthCode=codeOf(error);}
  if(writeAuthCode!=='UNAUTHENTICATED')throw new Error('WRITE auth response hidden by CORS');
  const stale=await connect(cfg.api,cfg.token,{...contract,fingerprint:'0'.repeat(64)}).writeExtension('Event.confirm',{id:writeId},{key:'browser-write-stale'});
  if(stale.ok || stale.code!=='CONTRACT_MISMATCH' || client.pending().length)throw new Error('WRITE contract/pending mismatch');
  report({ok:true, wire:cfg.wire, lang:cfg.lang,flow:'read-cache-apply-write-recovery', authCode, contractCode,writeRecovered:recovered.replayed,writeDenied:foreign.code});
} catch (error) {
  report({ok:false, error:String(error), code:codeOf(error)});
}
"""


EXTENSION_BROWSER = r"""
import {connect} from '@aip/prototype-sdk';
import {contract} from './out/contract.js';
const cfg = CONFIG;
const output = document.querySelector('#result');
const report = (value) => { window.__result = value; output.textContent = JSON.stringify(value); };
const codeOf = (error) => error && typeof error.code === 'string' ? error.code : null;
try {
  const client = connect(cfg.api, cfg.token, contract);
  const query = {read:'Recruitment', select:['id'], sort:[{field:'id',dir:'asc'}]};
  const before = await client.read(query);
  const expectedIds = cfg.wire === 'safe' ? [100,101] : ['100','101'];
  if (JSON.stringify(before.rows.map(row => row.id)) !== JSON.stringify(expectedIds)) throw new Error('read mismatch ' + JSON.stringify({ids:before.rows.map(row => row.id), expectedIds}));
  const clubId = cfg.wire === 'safe' ? 10 : '10';
  const cacheSize = client.cache.size;
  const pending = JSON.stringify(client.pending());
  const first = await client.extension('Recruitment.stats', {clubId});
  const second = await client.extension('Recruitment.stats', {clubId});
  if (first.approvedApplicants !== 1 || second.approvedApplicants !== 1) throw new Error('extension output mismatch');
  if (client.cache.size !== cacheSize || JSON.stringify(client.pending()) !== pending) throw new Error('extension changed read/write state');
  let authCode = null;
  try { await connect(cfg.api, 'invalid-token', contract).read(query); }
  catch (error) { authCode = codeOf(error); }
  if (authCode !== 'UNAUTHENTICATED') throw new Error('auth error code was not readable: ' + authCode);
  let contractCode = null;
  try { await connect(cfg.api, cfg.token, {...contract, fingerprint:'0'.repeat(64)}).read(query); }
  catch (error) { contractCode = codeOf(error); }
  if (contractCode !== 'CONTRACT_MISMATCH') throw new Error('contract error code was not readable: ' + contractCode);
  report({ok:true, wire:cfg.wire, lang:cfg.lang, flow:'read-cache-extension', approvedApplicants:first.approvedApplicants, authCode, contractCode});
} catch (error) {
  report({ok:false, error:String(error), code:codeOf(error)});
}
"""


def compile_for_wire(tsc, directory, source, wire):
    generated = directory / "contract.ts"
    output = directory / "out"
    result = json.loads(run([
        str(ROOT / "target/debug/aip-prototype"), "gen", str(source), "--wire", wire,
        "--out", str(generated), "--sdk-import", "@aip/prototype-sdk",
    ]))
    if not result.get("ok") or not result.get("fingerprint"):
        raise RuntimeError("contract generation did not return a fingerprint")
    run([tsc, "--strict", "--target", "es2022", "--module", "nodenext", "--moduleResolution", "nodenext",
         "--lib", "es2022,dom", "--rewriteRelativeImportExtensions", "--rootDir", str(directory),
         "--outDir", str(output), "--noEmitOnError", str(generated)])
    return output / "contract.js", result["fingerprint"]


def post_denied_origin(api, denied_origin, token, fingerprint, wire, extension_app):
    if extension_app:
        value = "100" if wire == "decimal" else 100
        request = {"request": {"apply": "Recruitment.close", "target": {"ids": [value]}}, "key": "browser-denied-" + uuid.uuid4().hex}
    else:
        value = "11" if wire == "decimal" else 11
        request = {"request": {"extension": "Event.confirm", "input": {"id": value}}, "key": "browser-denied-" + uuid.uuid4().hex}
    req = urllib.request.Request(
        api + "/apply", data=json.dumps(request).encode(), method="POST",
        headers={"Origin": denied_origin, "Authorization": "Bearer " + token,
                 "Content-Type": "application/json", "X-AIP-Contract": fingerprint},
    )
    try:
        with urllib.request.urlopen(req, timeout=5) as response:
            status, body = response.status, response.read()
    except urllib.error.HTTPError as error:
        status, body = error.code, error.read()
    try:
        payload = json.loads(body)
    except ValueError:
        payload = {}
    if payload.get("ok") is not False or payload.get("code") != "ORIGIN_NOT_ALLOWED":
        raise RuntimeError("unauthorized raw origin POST was not rejected")
    return {"status": status, "code": payload.get("code")}


def db_snapshot(db_url, schema, extension_app):
    if extension_app:
        query = f"SELECT status FROM {schema}.recruitment WHERE id=100"
    else:
        query = f"SELECT checked FROM {schema}.event WHERE id=11"
    return psql(db_url, query).strip()


def browser_run(origin, denied_origin, api, token, fingerprint, wire, lang, extension_app, directory):
    sdk_path = "/node_modules/@aip/prototype-sdk/prototype/sdk/index.js"
    browser_page(directory, sdk_path)
    config = {"api": api, "token": token, "wire": wire, "lang": lang}
    logic = EXTENSION_BROWSER if extension_app else BASE_BROWSER
    (directory / "browser.js").write_text(logic.replace("CONFIG", js_literal(config)))
    diagnostics = {"console": [], "requestfailed": [], "requests": [], "responses": [], "pageErrors": []}
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True, channel="chrome")
        try:
            page = browser.new_page()
            page.on("console", lambda msg: diagnostics["console"].append(msg.text) if msg.type == "error" else None)
            page.on("requestfailed", lambda req: diagnostics["requestfailed"].append({"method": req.method, "url": req.url, "failure": req.failure}) if req.url.startswith(api) else None)
            page.on("request", lambda req: diagnostics["requests"].append({"method": req.method, "url": req.url}))
            page.on("response", lambda response: diagnostics["responses"].append({"status": response.status, "url": response.url}))
            page.on("pageerror", lambda error: diagnostics["pageErrors"].append(str(error)))

            page.goto(denied_origin, wait_until="load")
            denied_result = page.evaluate("""async ({api,token,fingerprint,wire,extensionApp}) => {
              const id=wire==='decimal' ? (extensionApp ? '100' : '11') : (extensionApp ? 100 : 11);
              const request=extensionApp ? {apply:'Recruitment.close',target:{ids:[id]}} : {extension:'Event.confirm',input:{id}};
              try {
                const response=await fetch(api+'/apply',{method:'POST',headers:{'content-type':'application/json','authorization':'Bearer '+token,'x-aip-contract':fingerprint},body:JSON.stringify({request,key:'browser-preflight-denied'})});
                return {resolved:true,status:response.status,body:await response.text()};
              } catch (error) { return {resolved:false,error:String(error)}; }
            }""", {"api": api, "token": token, "fingerprint": fingerprint, "wire": wire, "extensionApp": extension_app})
            if denied_result.get("resolved"):
                raise RuntimeError("denied-origin browser preflight unexpectedly resolved")
            if not diagnostics["requestfailed"]:
                raise RuntimeError("denied-origin browser request had no failure event")

            denied_console_count = len(diagnostics["console"])
            page.goto(origin, wait_until="load")
            page.wait_for_function("window.__result !== undefined", timeout=25_000)
            result = page.evaluate("window.__result")
            if result.get("ok") is not True:
                raise RuntimeError("browser SDK scenario failed: " + json.dumps(result))
            allowed_console = [item for item in diagnostics["console"][denied_console_count:] if "favicon.ico" not in item]
            if allowed_console:
                raise RuntimeError("allowed-origin browser console errors: " + json.dumps(allowed_console))
            return result, denied_result, diagnostics
        except Exception as error:
            raise RuntimeError(f"browser scenario failed: {error}; diagnostics={json.dumps(diagnostics)}") from error
        finally:
            browser.close()


def exercise(db_url, binary, tsc, shared, wire, lang, extension_app):
    schema = "aip_browser_" + uuid.uuid4().hex
    owned = False
    server = None
    origins = []
    try:
        with tempfile.TemporaryDirectory(prefix="case-", dir=shared) as temp:
            directory = Path(temp)
            source = directory / "app.aip"
            original = ROOT.parent / ("spikes/spike-v1-fixture/fixture/recruitment.aip" if extension_app else "prototype/example/app.aip")
            source.write_text(original.read_text())
            (directory / "node_modules").symlink_to(shared / "node_modules", target_is_directory=True)
            init = json.loads(run([binary, "init", str(source), "--schema", schema, "--db-url", db_url]))
            if init.get("ok") is not True or init.get("schema") != schema:
                raise RuntimeError("schema init did not confirm ownership")
            owned = True
            if extension_app:
                seed_extensions(db_url, schema)
            else:
                seed_base(db_url, schema)
            (directory / "package.json").write_text('{"type":"module","private":true}\n')

            static, thread = start_static(directory)
            origins.append((static, thread))
            allowed_origin = f"http://127.0.0.1:{static.server_port}"
            denied, denied_thread = start_static(directory)
            origins.append((denied, denied_thread))
            denied_origin = f"http://127.0.0.1:{denied.server_port}/"
            origin = allowed_origin + "/"

            contract_js, fingerprint = compile_for_wire(tsc, directory, source, wire)
            if not contract_js.is_file() or not (directory / "browser.js").parent.is_dir():
                raise RuntimeError("compiled browser modules are missing")
            extensions = ROOT.parent / "spikes/spike-v4-worker/extensions"
            command = [binary, "serve", str(source), "--schema", schema, "--db-url", db_url,
                       "--listen", "127.0.0.1:0", "--wire", wire, "--dev-actor", "1", "--dev-token-ttl", "60",
                       "--allow-origin", allowed_origin]
            if not extension_app:
                extensions=ROOT / "example/extensions"
            command.extend(["--worker-dir", str(extensions), "--worker-lang", lang])
            if not extension_app:
                command.append("--enable-write-extensions")
            server = subprocess.Popen(command, cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            event = listening(server)
            if event.get("fingerprint") != fingerprint:
                raise RuntimeError("generated and running contract fingerprints differ")
            api = "http://" + event["address"]
            before = db_snapshot(db_url, schema, extension_app)
            raw_result = post_denied_origin(api, denied_origin.rstrip("/"), event["devToken"], fingerprint, wire, extension_app)
            after = db_snapshot(db_url, schema, extension_app)
            if before != after:
                raise RuntimeError("denied raw HTTP POST changed database state")

            result, denied_result, diagnostics = browser_run(
                origin, denied_origin, api, event["devToken"], fingerprint, wire, lang,
                extension_app, directory,
            )
            stop(server)
            server = None
            print(json.dumps({"ok": True, "flow": "extension" if extension_app else "base",
                              "wire": wire, "lang": lang,
                              "result": result, "deniedBrowser": denied_result,
                              "deniedRawHttp": raw_result, "databaseUnchanged": before == after}))
    finally:
        if server is not None:
            if server.poll() is None:
                server.kill()
                server.communicate(timeout=10)
        for static, thread in origins:
            static.shutdown()
            static.server_close()
            thread.join(timeout=5)
        if owned:
            psql(db_url, f"DROP SCHEMA {schema} CASCADE")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db-url", default="host=localhost dbname=postgres")
    parser.add_argument("--wire", choices=("safe", "decimal", "both"), default="both")
    parser.add_argument("--lang", choices=("node", "python", "both"), default="both")
    args = parser.parse_args()

    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    run([cargo, "build", "--offline"])
    binary = str(ROOT / "target/debug/aip-prototype")
    tsc = str(ROOT.parent / "spikes/spike-0-ts/node_modules/.bin/tsc")
    with tempfile.TemporaryDirectory(prefix="aip-browser-smoke-") as temp:
        shared = Path(temp)
        install_sdk(shared)
        for wire in modes(args.wire, ("safe", "decimal")):
            for lang in modes(args.lang, ("node", "python")):
                exercise(args.db_url, binary, tsc, shared, wire, lang, False)
        for wire in modes(args.wire, ("safe", "decimal")):
            for lang in modes(args.lang, ("node", "python")):
                exercise(args.db_url, binary, tsc, shared, wire, lang, True)


if __name__ == "__main__":
    main()
