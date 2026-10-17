import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn, spawnSync } from "node:child_process";
import { generateKeyPairSync, sign } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, cpSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { once } from "node:events";
import { requireTestDatabase } from "../test-support/postgres-guard.mjs";

const root = resolve(import.meta.dirname, "../..");
const binary = join(root,"target/debug/aip");
const testDb = requireTestDatabase();
const env = {PATH:process.env.PATH,HOME:process.env.HOME,TMPDIR:process.env.TMPDIR,AIP_TEST_DATABASE:testDb.raw,npm_config_cache:"/tmp/aip-update-npm-cache"};
function run(cmd,args,options={}) {
  const result=spawnSync(cmd,args,{cwd:root,env,encoding:"utf8",timeout:120000,...options});
  assert.equal(result.status,0,`${cmd} failed:\n${result.stdout}\n${result.stderr}`);
  return result.stdout.trim();
}
function cli(args) { return JSON.parse(run(binary,["service",...args])); }
async function serve(file,config) {
  const child=spawn(binary,["service","serve",file,"--config",config],{cwd:root,env,stdio:["ignore","pipe","pipe"]});
  let output="",stderr="";
  child.stderr.on("data",chunk=>{stderr+=chunk;});
  const address=await new Promise((accept,reject)=>{
    const timer=setTimeout(()=>{child.kill();reject(new Error(`serve timeout: ${stderr}`));},15000);
    child.once("exit",code=>{clearTimeout(timer);reject(new Error(`serve exited ${code}: ${output} ${stderr}`));});
    child.stdout.on("data",chunk=>{output+=chunk;if(!output.includes("\n"))return;clearTimeout(timer);try {const line=JSON.parse(output.split("\n")[0]);assert.equal(line.ok,true);accept(line.address);}catch(e){child.kill();reject(e);}});
  });
  return {child,address};
}

test("memoir copy edits through installed TS/Python SDK, types, replay, loss recovery and stale contract",{timeout:180000},async()=>{
  const dir=mkdtempSync(join(tmpdir(),"aip-general-update-"));
  const schema=`aip_general_update_${process.pid}`;
  const file=join(dir,"notes.aip"), config=join(dir,"service.json"), sdk=join(dir,"sdk"), consumer=join(dir,"consumer");
  cpSync(join(root,"product/tests/fixtures/memoir-notes/notes.aip"),file);
  mkdirSync(consumer);
  const pythonBinding=join(dir,"notes_binding.py");
  // Before implementation the CLI rejects --lang; a Python binding must share the TS fingerprint.
  const py=cli(["gen",file,"--lang","python","--out",pythonBinding]);
  const ts=cli(["gen",file,"--out",join(consumer,"binding.ts")]);
  assert.equal(py.fingerprint,ts.fingerprint);
  run(process.execPath,["product/tools/build-sdk.mjs","--out",sdk]);
  const tarball=JSON.parse(run("npm",["pack","--offline","--json"],{cwd:sdk}))[0].filename;
  writeFileSync(join(consumer,"package.json"),'{"private":true,"type":"module"}');
  run("npm",["install","--offline","--ignore-scripts","--no-audit","--no-fund",join(sdk,tarball)],{cwd:consumer});
  writeFileSync(join(consumer,"types.ts"),`import {connect} from '@aip/sdk'; import {contract} from './binding.ts';
const c=connect('http://localhost:1',null,contract);
c.apply({apply:'Note.edit',target:{ids:['10']},values:{title:'new'},expect:{version:0}});
// @ts-expect-error title is nullable string, never number
c.apply({apply:'Note.edit',target:{ids:['10']},values:{title:3},expect:{version:0}});
// @ts-expect-error missing expected version
c.apply({apply:'Note.edit',target:{ids:['10']},values:{content:null}});
// @ts-expect-error owner is not patchable
c.apply({apply:'Note.edit',target:{ids:['10']},values:{owner:'2'},expect:{version:0}});
// @ts-expect-error old transitions do not accept values
c.apply({apply:'Note.favorite',target:{ids:['10']},values:{title:'x'}});
`);
  const compiler=join(root,"spikes/spike-0-ts/node_modules/.bin/tsc");
  const flags=["--strict","--noEmit","--module","nodenext","--moduleResolution","nodenext","--target","esnext","--allowImportingTsExtensions","--skipLibCheck"];
  run(compiler,[...flags,"types.ts"],{cwd:consumer});
  writeFileSync(join(consumer,"negative.ts"),readFileSync(join(consumer,"types.ts"),"utf8").replaceAll("@ts-expect-error","negative control"));
  assert.notEqual(spawnSync(compiler,[...flags,"negative.ts"],{cwd:consumer,env}).status,0);
  const {publicKey,privateKey}=generateKeyPairSync("rsa",{modulusLength:2048});
  const jwk={...publicKey.export({format:"jwk"}),kid:"update-test",alg:"RS256",use:"sig",key_ops:["verify"]};
  const jwks=join(dir,"jwks.json");writeFileSync(jwks,JSON.stringify({keys:[jwk]}));
  writeFileSync(config,JSON.stringify({schema,database_url_env:"AIP_TEST_DATABASE",listen:"127.0.0.1:0",wire:"decimal",allowed_origins:[],auth:{issuer:"https://update.example",audience:"aip-api",jwks:{kind:"file",path:jwks}}}));
  let server, initialized=false;
  try {
    cli(["init",file,"--config",config]); initialized=true;
    const sql=join(dir,"seed.sql");writeFileSync(sql,`INSERT INTO ${schema}.member(id) VALUES(1),(2); INSERT INTO ${schema}.note(id,owner_id,note_type,title,content,version,favorite,deleted) VALUES(10,1,'GENERAL','old','old body',0,false,false),(20,2,'GENERAL','hidden','secret',0,false,false);`);
    run("psql",["-X","-v","ON_ERROR_STOP=1","-d",env.AIP_TEST_DATABASE,"-f",sql]);
    cli(["principal","--config",config,"bind","--subject","editor","--actor","1"]);
    const encode=v=>Buffer.from(JSON.stringify(v)).toString("base64url");const now=Math.floor(Date.now()/1000);
    const unsigned=`${encode({alg:"RS256",typ:"at+jwt",kid:jwk.kid})}.${encode({iss:"https://update.example",aud:"aip-api",sub:"editor",iat:now,nbf:now,exp:now+600})}`;
    const token=`${unsigned}.${sign("RSA-SHA256",Buffer.from(unsigned),privateKey).toString("base64url")}`;
    server=await serve(file,config);
    const consumerScript=join(consumer,"consumer.ts");
    writeFileSync(consumerScript,`import assert from 'node:assert/strict'; import {connect,WriteUnsettled} from '@aip/sdk'; import {contract} from './binding.ts';
const token=await new Promise<string>(r=>{let s='';process.stdin.on('data',d=>s+=d);process.stdin.on('end',()=>r(s));});
const c=connect(process.argv[2],token,contract);
const request={apply:'Note.edit',target:{ids:['10']},values:{title:'SDK 제목',content:'SDK 본문'},expect:{version:0}} as const;
const one=await c.apply(request,{key:'edit-1'});assert.equal(one.ok,true);
const replay=await c.apply(request,{key:'edit-1'});assert.equal(replay.ok,true);assert.equal(replay.replayed,true);
const partial=await c.apply({apply:'Note.edit',target:{ids:['10']},values:{title:'부분 제목'},expect:{version:1}},{key:'partial'});assert.equal(partial.ok,true);
let rows=(await c.read({read:'Note',select:['id','title','content','version']})).rows;assert.equal(rows[0].content,'SDK 본문');assert.equal(rows[0].version,2);
const stale=await c.apply(request,{key:'stale'});assert.equal(stale.ok,false);if(!stale.ok){assert.equal(stale.code,'WIRE.CONFLICT');assert.equal(stale.retry,'later');}
const denied=await c.apply({...request,target:{ids:['20']}},{key:'denied'});assert.equal(denied.ok,false);if(!denied.ok)assert.equal(denied.code,'WIRE.MISSING_TARGET');
let lost=true;const flaky=connect(process.argv[2],token,contract,async(input,init)=>{const response=await fetch(input,init);if(lost&&String(input).endsWith('/apply')){await response.arrayBuffer();throw new Error('lost committed response');}return response;});
try{await flaky.apply({apply:'Note.edit',target:{ids:['10']},values:{content:null},expect:{version:2}},{key:'lost',retries:0});assert.fail('expected unsettled');}catch(e){assert.ok(e instanceof WriteUnsettled);}lost=false;
const recovered=await flaky.retryPending();assert.equal(recovered.length,1);assert.equal(recovered[0].ok,true);
const mismatch=await c.apply({...request,values:{title:'different'}},{key:'edit-1'});assert.equal(mismatch.ok,false);if(!mismatch.ok)assert.equal(mismatch.code,'WIRE.IDEMPOTENCY_MISMATCH');
const old=connect(process.argv[2],token,{...contract,fingerprint:'0'.repeat(64)});const rejected=await old.apply(request,{key:'contract'});assert.equal(rejected.ok,false);if(!rejected.ok)assert.equal(rejected.code,'WIRE.CONTRACT_MISMATCH');
console.log('memoir TS installed SDK: full/partial/null/denial/conflict/replay/recovery/contract passed');
`);
    const tsOutput=run(process.execPath,["--experimental-strip-types",consumerScript,`http://${server.address}`],{cwd:consumer,input:token});
    assert.match(tsOutput,/passed/);
    const pyScript=join(dir,"consumer.py");
    writeFileSync(pyScript,`import sys\nfrom aip_sdk import connect\nfrom notes_binding import apply_note_edit, CONTRACT_FINGERPRINT\nc=connect(sys.argv[1],sys.stdin.read(),CONTRACT_FINGERPRINT)\nr=apply_note_edit(c,{"ids":["10"]},{"title":"Python 제목","content":"Python 본문"},{"version":3},key="python-edit")\nassert r["ok"],r\nrows=c.read({"read":"Note","select":["id","title","content","version"]})\nassert rows[0]["title"]=="Python 제목" and rows[0]["content"]=="Python 본문" and rows[0]["version"]==4,rows\nprint("memoir Python generated binding: title/content passed")\n`);
    const pyOutput=run("python3",[pyScript,`http://${server.address}`],{cwd:dir,env:{...env,PYTHONPATH:join(root,"product/python")},input:token});
    assert.match(pyOutput,/passed/);
    const state=run("psql",["-X","-At","-d",env.AIP_TEST_DATABASE,"-c",`SELECT version,(SELECT count(*) FROM ${schema}.aip_outbox) FROM ${schema}.note WHERE id=10`]);
    assert.equal(state,"4|4");
    const changed=join(dir,"changed.aip");
    writeFileSync(changed,readFileSync(file,"utf8").replace("patch title, content","patch title"));
    const migration=cli(["migrate",changed,"--config",config]);
    assert.equal(migration.requiresReview,true,"patch allowlist change requires review");
    const newBinding=cli(["gen",changed,"--out",join(consumer,"changed.binding.ts")]);
    assert.notEqual(newBinding.fingerprint,ts.fingerprint,"patch allowlist changes public contract");
    console.log(tsOutput); console.log(pyOutput); console.log(`PostgreSQL version/outbox = ${state}`);
  } finally {
    if(server){server.child.kill("SIGTERM");await once(server.child,"exit");}
    if(initialized){const sql=join(dir,"cleanup.sql");writeFileSync(sql,`DROP SCHEMA ${schema} CASCADE;`);run("psql",["-X","-v","ON_ERROR_STOP=1","-d",env.AIP_TEST_DATABASE,"-f",sql]);}
    rmSync(dir,{recursive:true,force:true});
  }
});
