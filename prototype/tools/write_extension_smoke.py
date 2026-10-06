"""Exercise generated WRITE bindings, cache and recovery against actual Node/Python workers."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import uuid

from smoke import ROOT, listening, psql, run, stop
from extension_smoke import install_package, modes

NODE = """import fs from 'node:fs';
const counter=new URL('./counter',import.meta.url);
export async function run(input,ctx) {
  fs.appendFileSync(counter,'1\\n');
  const result=await ctx.data.apply('Event','mark',[input.id]);
  return {id:result.changed[0]??result.unchanged[0],count:String(input.id)==='13'?9007199254740992:1};
}
"""
PYTHON = """from pathlib import Path
async def run(input,ctx):
    with (Path(__file__).parent/'counter').open('a') as f: f.write('1\\n')
    result=await ctx.data.apply('Event','mark',[input['id']])
    return {'id':(result['changed']+result['unchanged'])[0],'count':9007199254740992 if str(input['id'])=='13' else 1}
"""
SCRIPT = r"""
import {connect,WriteUnsettled} from SDK_IMPORT;
import {contract} from './contract.ts';
declare const process: {env: Record<string,string|undefined>};
function expect(value: unknown,label: string) {if(!value)throw new Error(label);}
function equal(a: unknown,b: unknown) {return JSON.stringify(a)===JSON.stringify(b);}
const url=process.env.AIP_SMOKE_URL!;const token=process.env.AIP_SMOKE_TOKEN!;
const id11=ID11;const id12=ID12;const id13=ID13;const id99=ID99;
const client=connect(url,token,contract);
const query={read:'Event',select:['id','checked'],sort:[{field:'id',dir:'asc'}]} as const;
const before=await client.read(query);expect(equal(before.rows.map(row=>[row.id,row.checked]),[[id11,false],[id12,false],[id13,false]]),'initial actor read');
expect((await client.read(query)).cached,'initial read cache hit');
const first=await client.writeExtension('Event.run',{id:id11},{key:'shared-key',retries:0});
if(!first.ok)throw new Error('WRITE failed: '+first.code);
expect(first.output.id===id11&&first.output.count===1,'WRITE decoded output');
expect(Object.isFrozen(first)&&Object.isFrozen(first.output)&&Object.isFrozen(first.tags),'WRITE result frozen');
const replay=await client.writeExtension('Event.run',{id:id11},{key:'shared-key',retries:0});expect(replay.ok&&replay.replayed,'WRITE idempotent replay');
const after=await client.read(query);expect(!after.cached&&after.rows[0].checked,'WRITE invalidates actual cached read');
const conflict=await client.apply({apply:'Event.mark',target:{ids:[id11]}},{key:'shared-key',retries:0});expect(!conflict.ok&&conflict.code==='IDEMPOTENCY_MISMATCH','standard apply and WRITE share key space');

let dropped=false;
const losingFetch: typeof fetch=async(input,init)=>{
  const response=await fetch(input,init);
  if(String(input).endsWith('/apply')&&!dropped){dropped=true;await response.text();throw new TypeError('committed WRITE response deliberately discarded');}
  return response;
};
const losing=connect(url,token,contract,losingFetch);
try {await losing.writeExtension('Event.run',{id:id12},{key:'lost-key',retries:0});throw new Error('loss did not remain unsettled');}
catch(error){expect(error instanceof WriteUnsettled,'actual committed response loss remains unsettled');}
expect(equal(losing.pending(),['lost-key']),'pending key survives loss');
const recovered=await losing.retryPending();const recoveredFirst=recovered[0];
expect(recoveredFirst?.ok&&'output' in recoveredFirst&&recoveredFirst.output.id===id12&&recoveredFirst.output.count===1&&recoveredFirst.replayed,'same-key WRITE recovery');expect(equal(losing.pending(),[]),'recovery clears pending');
const denied=await client.writeExtension('Event.run',{id:id99},{key:'foreign-key',retries:0});expect(!denied.ok&&denied.code==='EXTENSION_ERROR','foreign actor WRITE rejected');
const unsafe=await client.writeExtension('Event.run',{id:id13},{key:'unsafe-key',retries:0});expect(!unsafe.ok&&unsafe.code==='OUTPUT_INVALID','unsafe Int output rejected before commit');
const stale=connect(url,token,{...contract,fingerprint:'0'.repeat(64)});
const old=await stale.writeExtension('Event.run',{id:id13},{key:'stale-key',retries:0});expect(!old.ok&&old.code==='CONTRACT_MISMATCH','stale first WRITE rejected');expect(equal(stale.pending(),[]),'stale pre-execution rejection is settled');
expect(equal(client.pending(),[]),'confirmed rejections leave no pending');
console.log(JSON.stringify({ok:true,replayed:replay.ok&&replay.replayed,recovered:recoveredFirst?.ok,unsafeCode:unsafe.ok?null:unsafe.code}));
"""


def exercise(db_url, binary, tsc, shared, sdk_mode, wire, lang):
    schema = "aip_write_smoke_" + uuid.uuid4().hex
    owned = False
    server = None
    try:
        with tempfile.TemporaryDirectory(prefix="write-case-", dir=shared) as temp:
            directory = Path(temp)
            source = ROOT.parent / "spikes/spike-v6-transport/tests/fixtures/write-extension.aip"
            worker_dir = directory / "workers"
            worker_dir.mkdir()
            (worker_dir / "write.mjs").write_text(NODE)
            (worker_dir / "write.py").write_text(PYTHON)
            (directory / "package.json").write_text(json.dumps({"private": True, "type": "module"}))
            if sdk_mode == "package":
                (directory / "node_modules").symlink_to(shared / "node_modules", target_is_directory=True)
            sdk_import = str(ROOT / "sdk/index.ts") if sdk_mode == "source" else "@aip/prototype-sdk"
            initialized = json.loads(run([binary, "init", str(source), "--schema", schema, "--db-url", db_url]))
            if initialized.get("ok") is not True or initialized.get("schema") != schema:
                raise RuntimeError("init did not confirm fresh schema ownership")
            owned = True
            psql(db_url, f"INSERT INTO {schema}.member(id) VALUES(1),(2); INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false),(12,1,false),(13,1,false),(99,2,false)")
            generated = json.loads(run([binary, "gen", str(source), "--wire", wire, "--out", str(directory / "contract.ts"), "--sdk-import", sdk_import]))
            caller = SCRIPT.replace("SDK_IMPORT", json.dumps(sdk_import))
            for value in (11, 12, 13, 99):
                caller = caller.replace(f"ID{value}", str(value) if wire == "safe" else f"'{value}'")
            script = directory / "caller.mts"
            script.write_text(caller)
            if sdk_mode == "source":
                run([tsc, "--noEmit", "--strict", "--target", "esnext", "--module", "nodenext", "--moduleResolution", "nodenext", "--allowImportingTsExtensions", str(script)])
                executable = ["node", "--experimental-strip-types", str(script)]
            else:
                run([tsc, "--strict", "--target", "es2022", "--module", "nodenext", "--moduleResolution", "nodenext", "--rewriteRelativeImportExtensions", "--rootDir", str(directory), "--outDir", str(directory / "js"), "--noEmitOnError", str(script)])
                executable = ["node", str(directory / "js/caller.mjs")]
            server = subprocess.Popen([binary, "serve", str(source), "--schema", schema, "--db-url", db_url, "--listen", "127.0.0.1:0", "--wire", wire, "--dev-actor", "1", "--dev-token-ttl", "60", "--worker-dir", str(worker_dir), "--worker-lang", lang, "--enable-write-extensions"], cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            event = listening(server)
            if event.get("fingerprint") != generated.get("fingerprint"):
                raise RuntimeError("WRITE gen/serve contract fingerprint mismatch")
            env = os.environ.copy()
            env.update(AIP_SMOKE_URL="http://" + event["address"], AIP_SMOKE_TOKEN=event["devToken"])
            result = json.loads(run(executable, env=env, timeout=30))
            rows = psql(db_url, f"SELECT id||':'||checked FROM {schema}.event ORDER BY id").splitlines()
            records = psql(db_url, f"SELECT count(*) FROM {schema}.aip_idem").strip()
            counter = (worker_dir / "counter").read_text().splitlines()
            if rows != ["11:true", "12:true", "13:false", "99:false"] or records != "2" or len(counter) != 4:
                raise RuntimeError("WRITE smoke effects/idempotent records/invocations differ")
            stop(server)
            server = None
            print(json.dumps({"ok": True, "sdk": sdk_mode, "wire": wire, "lang": lang, "result": result, "records": 2, "workerInvocations": 4, "data": rows}))
    finally:
        if server is not None:
            if server.poll() is None:
                server.kill()
            server.communicate(timeout=10)
        if owned:
            psql(db_url, f"DROP SCHEMA {schema} CASCADE")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db-url", default="host=localhost dbname=postgres")
    parser.add_argument("--sdk", choices=("source", "package", "both"), default="both")
    parser.add_argument("--wire", choices=("safe", "decimal", "both"), default="both")
    parser.add_argument("--lang", choices=("node", "python", "both"), default="both")
    args = parser.parse_args()
    run([shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo"), "build", "--offline"])
    binary = str(ROOT / "target/debug/aip-prototype")
    tsc = str(ROOT.parent / "spikes/spike-0-ts/node_modules/.bin/tsc")
    with tempfile.TemporaryDirectory(prefix="aip-write-smoke-") as temp:
        shared = Path(temp)
        sdk_modes = modes(args.sdk, ("source", "package"))
        if "package" in sdk_modes:
            install_package(shared)
        for sdk_mode in sdk_modes:
            for wire in modes(args.wire, ("safe", "decimal")):
                for lang in modes(args.lang, ("node", "python")):
                    exercise(args.db_url, binary, tsc, shared, sdk_mode, wire, lang)


if __name__ == "__main__":
    main()
