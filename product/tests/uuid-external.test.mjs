import assert from 'node:assert/strict';
import {test} from 'node:test';
import {spawn,spawnSync} from 'node:child_process';
import {mkdtempSync,writeFileSync,readFileSync,mkdirSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {createInterface} from 'node:readline';
import {once} from 'node:events';
import {generateKeyPairSync,sign,randomUUID,createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';
const root=resolve(import.meta.dirname,'../..');
const cli=join(root,'target/debug/aip');
function run(bin,args,env={},cwd=root) {const p=spawnSync(bin,args,{cwd,env:{...process.env,...env},encoding:'utf8',timeout:120000,maxBuffer:8<<20});assert.equal(p.status,0,`${bin} ${args.join(' ')}\n${p.stdout}\n${p.stderr}`);return p.stdout;}

// Only a new, uniquely named database is touched. No .env files or saved credentials are used.
test('UUID owned and external PostgreSQL: introspect, adopt, restricted role, installed TS/Python SDK, drift refusal',{timeout:240000},async()=>{
 const stop=async(proc)=>{if(proc.exitCode!==null||proc.signalCode!==null)return;const ended=once(proc,'exit');proc.kill('SIGTERM');await ended;};
 const temp=mkdtempSync(join(tmpdir(),'aip-uuid-sdk-'));
 const dbname=`aip_uuid_trial_${process.pid}`; const source='memoir_existing';
 const role=`aip_uuid_runtime_${process.pid}`;
 const adopter=`aip_uuid_adopter_${process.pid}`;
 const owner=randomUUID(),other=randomUUID();
 let server;
 const sql=(text,db=dbname)=>{const file=join(temp,'fixture.sql');writeFileSync(file,text);return run('psql',['-X','-v','ON_ERROR_STOP=1','-d',`host=localhost dbname=${db}`,'-f',file]);};
 const admin=`host=localhost dbname=${dbname}`;
 const runtime=`host=localhost dbname=${dbname} user=${role}`;
 const migration=`host=localhost dbname=${dbname} user=${adopter}`;
 const cmd=(config,args,extra={})=>JSON.parse(run(cli,['service',...(['principal','audit'].includes(args[0])?[args[0],'--config',config,...args.slice(1)]:['check','gen'].includes(args[0])?args:[...args,'--config',config])],{AIP_UUID_DB:runtime,AIP_UUID_MIGRATION:migration,...extra},temp));
 try {
  sql(`CREATE DATABASE ${dbname};`, 'postgres');
  sql(`CREATE ROLE ${role} LOGIN; CREATE ROLE ${adopter} LOGIN; GRANT CONNECT,CREATE ON DATABASE ${dbname} TO ${adopter};
CREATE SCHEMA ${source};
CREATE TABLE ${source}.users(user_id uuid PRIMARY KEY DEFAULT gen_random_uuid());
CREATE TABLE ${source}.notes(note_id uuid PRIMARY KEY DEFAULT gen_random_uuid(), user_id uuid NOT NULL REFERENCES ${source}.users(user_id), title text NOT NULL UNIQUE CHECK(length(title)>0), body text, archived boolean NOT NULL DEFAULT false, created_at timestamptz NOT NULL DEFAULT now());
INSERT INTO ${source}.users VALUES('${owner}'),('${other}');
INSERT INTO ${source}.notes(user_id,title) VALUES('${owner}','existing');
GRANT USAGE ON SCHEMA ${source} TO ${role},${adopter}; GRANT SELECT ON ALL TABLES IN SCHEMA ${source} TO ${adopter}; GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA ${source} TO ${role};`);
  const {privateKey,publicKey}=generateKeyPairSync('rsa',{modulusLength:2048});
  const jwk=publicKey.export({format:'jwk'});Object.assign(jwk,{kid:'uuid-trial',use:'sig',alg:'RS256',key_ops:['verify']});
  writeFileSync(join(temp,'jwks.json'),JSON.stringify({keys:[jwk]}));
  const b64=x=>Buffer.from(JSON.stringify(x)).toString('base64url');
  const token=subject=>{const now=Math.floor(Date.now()/1000);const msg=`${b64({alg:'RS256',kid:jwk.kid,typ:'at+jwt'})}.${b64({iss:'https://issuer.uuid.test',aud:'aip',sub:subject,iat:now,nbf:now,exp:now+600})}`;return `${msg}.${sign('RSA-SHA256',Buffer.from(msg),privateKey).toString('base64url')}`;};
  const sdkDir=join(temp,'sdk');run(process.execPath,['product/tools/build-sdk.mjs','--out',sdkDir]);
  const consumer=join(temp,'consumer');mkdirSync(consumer);writeFileSync(join(consumer,'package.json'),'{"type":"module"}');
  run('npm',['pack','--offline','--pack-destination',consumer,sdkDir],{npm_config_cache:join(temp,'npm-cache')},consumer);
  run('npm',['install','--offline','--ignore-scripts','--no-audit','--no-fund',join(consumer,'aip-sdk-0.1.0.tgz')],{npm_config_cache:join(temp,'npm-cache')},consumer);
  const {connect}=await import(pathToFileURL(join(consumer,'node_modules/@aip/sdk/product/sdk/index.js')).href);
  for (const wire of ['decimal','safe']) {
   const schema=`aip_uuid_external_${wire}`;
   const config=join(temp,`${wire}.json`);const app=join(temp,`${wire}.aip`);const map=join(temp,`${wire}.mapping.json`);
   const c={schema,database_url_env:'AIP_UUID_DB',migrate_database_url_env:'AIP_UUID_MIGRATION',listen:'127.0.0.1:0',wire,auth:{issuer:'https://issuer.uuid.test',audience:'aip',jwks:{kind:'file',path:join(temp,'jwks.json')}}};
   writeFileSync(config,JSON.stringify(c));
   const scan=cmd(config,['introspect','--source-schema',source,'--out',app,'--mapping-out',map]);assert.equal(scan.ok,true);
   const draft=readFileSync(app,'utf8');assert.match(draft,/id: Uuid/);assert.match(draft,/rows read when false/);assert.match(draft,/default database/);
   // Review policy, then expose the notes use case; retain physical mapping from the catalog.
   const reviewed=draft.replace('actor principal','actor Users').replace(/resource Users \{([\s\S]*?)rows read when false/, 'resource Users {$1rows read when false').replace(/resource Notes \{([\s\S]*?)rows read when false/, `resource Notes {$1rows read when user = actor
 transition Edit {from true allow user = actor patch title, body expect body}
 expose apply Edit {target id; bulk maxRows 1}
 transition Archive { from archived = false to archived = true allow user = actor repeat unchanged notify user "notes.archived" }
 expose read {select id, user, title, body, archived, created_at; refIds user; filter id.eq, id.gt; sort id; budget {rows 50; depth 1; cost 1000; deadline 2s; cursor}}
 expose apply Archive {target id; bulk maxRows 10}
 expose create {fields user, title, body allow user = actor}`);
   writeFileSync(app,reviewed);c.external_mapping=map;writeFileSync(config,JSON.stringify(c));
   assert.equal(cmd(config,['check',app]).ok,true);
   const plan=cmd(config,['adopt-external',app]);assert.deepEqual(plan.businessDDL,[]);
   assert.equal(cmd(config,['adopt-external',app,'--apply',plan.digest]).ok,true);assert.equal(cmd(config,['adopt-external',app,'--apply',plan.digest]).alreadyApplied,true);
   assert.equal(cmd(config,['provision']).ok,true);
   const infra=createHash('sha256').update(schema).digest('hex').slice(0,52); const infraSchema=`aip_infra_${infra}`;
   // Grant only DML to runtime, on AIP metadata/views and separate infrastructure.
   sql(`GRANT USAGE ON SCHEMA ${schema},${infraSchema} TO ${role}; GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA ${schema},${infraSchema} TO ${role}; GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA ${schema},${infraSchema} TO ${role};`);
   assert.equal(cmd(config,['principal','bind','--subject','owner','--actor',owner]).ok,true);
   assert.equal(cmd(config,['principal','bind','--subject','other','--actor',other]).ok,true);
   const binding=join(consumer,`${wire}.ts`);const py=join(temp,`${wire}_binding.py`);
   assert.equal(cmd(config,['gen',app,'--wire',wire,'--out',binding]).ok,true);assert.equal(cmd(config,['gen',app,'--wire',wire,'--lang','python','--out',py]).ok,true);
   assert.match(readFileSync(py,'utf8'),/NotesId = NewType\("NotesId", str\)/);
   run(join(root,'spikes/spike-0-ts/node_modules/.bin/tsc'),['--strict','--skipLibCheck','--target','ES2022','--module','NodeNext','--moduleResolution','NodeNext',binding],{},consumer);
   const {contract}=await import(pathToFileURL(binding.replace(/\.ts$/,'.js')).href);
   server=spawn(cli,['service','serve',app,'--config',config],{cwd:temp,env:{...process.env,AIP_UUID_DB:runtime,AIP_UUID_MIGRATION:migration},stdio:['ignore','pipe','pipe']});
   let stderr='';server.stderr.on('data',d=>stderr+=d);const lines=createInterface({input:server.stdout});
   const started=await Promise.race([once(lines,'line').then(([line])=>JSON.parse(line)),once(server,'exit').then(([code])=>{throw new Error(`serve exit ${code}: ${stderr}`);}),new Promise((_,reject)=>setTimeout(()=>reject(new Error(`serve timeout: ${stderr}`)),20000).unref())]);
   const base=`http://${started.address}`;
   const client=connect(base,token('owner'),contract);const hidden=connect(base,token('other'),contract);
   const query={read:'Notes',select:['id','title','user','archived','created_at'],sort:[{field:'id',dir:'asc'}],limit:50};
   const before=await client.read(query);assert.ok(before.rows.length>=1);assert.equal((await hidden.read(query)).rows.length,0);
   const uniqueTitle=`sdk ${wire}`;
   const request={atomic:[{create:'Notes',values:{user:owner,title:uniqueTitle}}]};
   const created=await client.bundle(request,{key:`uuid-create-${wire}`});assert.equal(created.ok,true,JSON.stringify(created));const id=created.created[0].id;assert.match(id,/^[a-f0-9-]{36}$/);
   assert.equal((await client.bundle(request,{key:`uuid-create-${wire}`})).replayed,true);
   const mismatch=await client.bundle({atomic:[{create:'Notes',values:{user:owner,title:'different'}}]},{key:`uuid-create-${wire}`});assert.equal(mismatch.code,'WIRE.IDEMPOTENCY_MISMATCH');
   const changed=await client.apply({apply:'Notes.Archive',target:{ids:[id]}},{key:`uuid-change-${wire}`});assert.deepEqual(changed.changed,[id]);
   const audit=cmd(config,['audit','list','--resource','Notes','--target-id',id]);assert.equal(audit.audit[0].targetId,id);
   const denied=await hidden.apply({apply:'Notes.Archive',target:{ids:[id]}},{key:`uuid-denied-${wire}`});assert.equal(denied.ok,false);
   const after=await client.read({...query,filter:[{field:'id',op:'eq',value:id}]});assert.equal(after.rows[0].archived,true);assert.equal(typeof after.rows[0].created_at,'string');
   const edit=await client.apply({apply:'Notes.Edit',target:{ids:[id]},values:{title:`updated ${wire}`,body:'written'},expect:{body:null}},{key:`uuid-edit-${wire}`});assert.equal(edit.ok,true,JSON.stringify(edit));
   const stale=await client.apply({apply:'Notes.Edit',target:{ids:[id]},values:{title:'stale'},expect:{body:null}},{key:`uuid-stale-${wire}`});assert.equal(stale.code,'WIRE.CONFLICT');
   const edited=await client.read({...query,select:['id','title','body'],filter:[{field:'id',op:'eq',value:id}]});assert.equal(edited.rows[0].title,`updated ${wire}`);assert.equal(edited.rows[0].body,'written');
   const clientId=await client.bundle({atomic:[{create:'Notes',values:{id:randomUUID(),user:owner,title:'client id'}}]},{key:`forbid-id-${wire}`});assert.equal(clientId.code,'WIRE.FIELD_NOT_EXPOSED');
   const sorted=await client.read({...query,limit:1});const next=await client.read({...query,limit:1,after:{id:sorted.rows[0].id}});assert.notEqual(sorted.rows[0]?.id,next.rows[0]?.id);
   const duplicates=await client.bundle({atomic:[{create:'Notes',values:{user:owner,title:`updated ${wire}`}}]},{key:`unique-${wire}`});assert.equal(duplicates.code,'WIRE.ALREADY_EXISTS');
   const pyScript=join(temp,'call.py');writeFileSync(pyScript,`import json\nfrom aip_sdk import connect\nc=connect(${JSON.stringify(base)},${JSON.stringify(token('owner'))},${JSON.stringify(contract.fingerprint)})\nr=c.read({'read':'Notes','select':['id','title'],'filter':[{'field':'id','op':'eq','value':${JSON.stringify(id)}}]})\nassert r[0]['id']==${JSON.stringify(id)}\nprint('Python UUID SDK read passed')\n`);run('python3',[pyScript],{PYTHONPATH:join(root,'product/python')},temp);
   const row=sql(`SELECT title,archived,created_at IS NOT NULL AS default_respected FROM ${source}.notes WHERE note_id='${id}';`);assert.match(row,/updated/);assert.match(row,/t/);
   // Drift in existing defaults is caught even after startup, before a write executes.
   sql(`ALTER TABLE ${source}.notes ALTER COLUMN archived SET DEFAULT true;`);
   const drift=await fetch(`${base}/apply`,{method:'POST',headers:{Authorization:`Bearer ${token('owner')}`,'Content-Type':'application/json','x-aip-contract':contract.fingerprint},body:JSON.stringify({key:'drift',request:{apply:'Notes.Archive',target:{ids:[id]}}})}).then(r=>r.json());assert.equal(drift.code,'WIRE.DEPLOYMENT_CHANGED');assert.equal((await fetch(`${base}/ready`)).status,503);
   await stop(server);server=undefined;lines.close();
   const refused=spawnSync(cli,['service','serve',app,'--config',config],{env:{...process.env,AIP_UUID_DB:runtime},cwd:temp,encoding:'utf8',timeout:15000});assert.notEqual(refused.status,0);assert.match(refused.stdout+refused.stderr,/SCHEMA_MISMATCH/);
   sql(`ALTER TABLE ${source}.notes ALTER COLUMN archived SET DEFAULT false;`);
   writeFileSync(app,reviewed.replace('rows read when user = actor','rows read when false'));const policies=cmd(config,['adopt-external',app]);assert.deepEqual(policies.businessDDL,[]);assert.equal(cmd(config,['adopt-external',app,'--apply',policies.digest]).ok,true);
   assert.equal(cmd(config,['adopt-external',app,'--apply',policies.digest]).alreadyApplied,true);
   assert.notEqual(policies.oldFactsDigest,policies.factsDigest);
   writeFileSync(app,reviewed);
   console.log(`${wire}: external UUID SDK create/read/apply/replay/cursor/Python, restricted DML role and drift refusal passed`);
   // The default AIP-owned storage path uses the same UUID contract and installed SDK.
   const owned=`aip_uuid_owned_${wire}`,ownedConfig=join(temp,`owned-${wire}.json`),ownedApp=join(temp,`owned-${wire}.aip`);
   const ownedDefinition=`actor Users
resource Users {fields {id: Uuid} rows read when id = actor}
resource Notes {fields {id: Uuid; user: Users; title: Text; archived: Bool}
 rows read when user = actor
 transition Archive {from archived = false to archived = true allow user = actor repeat unchanged}
 expose read {select id, title, archived; filter id.eq; budget {rows 10; depth 1; cost 1000; deadline 2s}}
 expose create {fields user, title, archived allow user = actor}
 expose apply Archive {target id; bulk maxRows 10}}
`;
   writeFileSync(ownedApp,ownedDefinition);writeFileSync(ownedConfig,JSON.stringify({...c,schema:owned,external_mapping:undefined}));
   assert.equal(cmd(ownedConfig,['init',ownedApp]).ok,true);assert.equal(cmd(ownedConfig,['provision']).ok,true);
   const oi=`aip_infra_${createHash('sha256').update(owned).digest('hex').slice(0,52)}`;
   sql(`INSERT INTO ${owned}.users VALUES('${owner}'); GRANT USAGE ON SCHEMA ${owned},${oi} TO ${role}; GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA ${owned},${oi} TO ${role}; GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA ${owned},${oi} TO ${role};`);
   assert.equal(cmd(ownedConfig,['principal','bind','--subject','owner','--actor',owner]).ok,true);
   const ob=join(consumer,`owned-${wire}.ts`);cmd(ownedConfig,['gen',ownedApp,'--wire',wire,'--out',ob]);run(join(root,'spikes/spike-0-ts/node_modules/.bin/tsc'),['--strict','--skipLibCheck','--target','ES2022','--module','NodeNext','--moduleResolution','NodeNext',ob],{},consumer);
   const ownedBinding=(await import(pathToFileURL(ob.replace(/\.ts$/,'.js')).href)).contract;
   server=spawn(cli,['service','serve',ownedApp,'--config',ownedConfig],{cwd:temp,env:{...process.env,AIP_UUID_DB:runtime},stdio:['ignore','pipe','pipe']});
   const ownedLines=createInterface({input:server.stdout});let ownedError='';server.stderr.on('data',d=>ownedError+=d);const listening=await Promise.race([once(ownedLines,'line').then(([line])=>JSON.parse(line)),once(server,'exit').then(([code])=>{throw new Error(`owned serve exit ${code}: ${ownedError}`);})]);
   const oc=connect(`http://${listening.address}`,token('owner'),ownedBinding);
   const cr=await oc.bundle({atomic:[{create:'Notes',values:{user:owner,title:'owned SDK',archived:false}}]},{key:`owned-${wire}`});assert.equal(cr.ok,true,JSON.stringify(cr));const oid=cr.created[0].id;assert.match(oid,/^[a-f0-9-]{36}$/);
   assert.deepEqual((await oc.apply({apply:'Notes.Archive',target:{ids:[oid]}},{key:`owned-archive-${wire}`})).changed,[oid]);
   const rr=await oc.read({read:'Notes',select:['id','title','archived'],filter:[{field:'id',op:'eq',value:oid}]});assert.equal(rr.rows[0].archived,true);
   await stop(server);server=undefined;ownedLines.close();
   console.log(`${wire}: AIP-owned native UUID create/read/apply via installed SDK passed`);

  }
 } finally {
  if(server){await stop(server);}
  sql(`DROP DATABASE IF EXISTS ${dbname} WITH (FORCE); DROP ROLE IF EXISTS ${role}; DROP ROLE IF EXISTS ${adopter};`,'postgres');rmSync(temp,{recursive:true,force:true});
 }
});
