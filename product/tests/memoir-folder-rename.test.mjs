import assert from 'node:assert/strict';
import {test} from 'node:test';
import {spawn, spawnSync} from 'node:child_process';
import {generateKeyPairSync, sign, randomUUID, createHash} from 'node:crypto';
import {mkdtempSync, mkdirSync, writeFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {pathToFileURL} from 'node:url';

const root = resolve(import.meta.dirname, '../..');
const targetDir = process.env.CARGO_TARGET_DIR || join(root, 'target');
const binary = join(targetDir, 'debug/aip');
const file = join(root, 'product/tests/fixtures/memoir-folder-rename/folders.aip');

test('Memoir folder rename: owner SDK read/apply and outsider denial in isolated PostgreSQL', {timeout: 180000}, async t => {
  const temp = mkdtempSync(join(tmpdir(), 'aip-memoir-folder-'));
  const suffix = `${process.pid}_${randomUUID().replaceAll('-', '').slice(0, 8)}`;
  const database = `aip_memoir_${suffix}`;
  const role = `aip_memoir_runtime_${suffix}`;
  const schema = 'aip_memoir';
  const owner = randomUUID(), outsider = randomUUID(), folder = randomUUID();
  const adminUrl = `host=localhost dbname=${database}`;
  const runtimeUrl = `host=localhost dbname=${database} user=${role}`;
  const env = {PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: process.env.TMPDIR,
    AIP_MEMOIR_DATABASE: runtimeUrl, AIP_MEMOIR_MIGRATION: adminUrl, npm_config_cache: join(temp, 'npm-cache')};
  const run = (command, args, options = {}) => {
    const result = spawnSync(command, args, {cwd: root, env, encoding: 'utf8', timeout: 120000, maxBuffer: 8 << 20, ...options});
    assert.equal(result.status, 0, `${command} ${args.join(' ')}\n${result.stdout}\n${result.stderr}`);
    return result.stdout.trim();
  };
  const sql = (text, url = adminUrl) => {
    const path = join(temp, 'fixture.sql');
    writeFileSync(path, text);
    return run('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-At', '-d', url, '-f', path]);
  };
  const config = join(temp, 'service.json');
  const cli = args => JSON.parse(run(binary, ['service', ...args]));
  let server, lines, databaseCreated = false, roleCreated = false;
  try {
    sql(`CREATE DATABASE ${database};`, 'host=localhost dbname=postgres');
    databaseCreated = true;
    sql(`CREATE ROLE ${role} LOGIN;`);
    roleCreated = true;
    sql(`CREATE SCHEMA memoir_source;
CREATE TABLE memoir_source.users (id uuid PRIMARY KEY);
CREATE TABLE memoir_source.folders (
  id uuid PRIMARY KEY, member_id uuid NOT NULL REFERENCES memoir_source.users(id), parent_id uuid REFERENCES memoir_source.folders(id),
  name varchar(255) NOT NULL, sort_order integer NOT NULL, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL, deleted_at timestamptz);
INSERT INTO memoir_source.users VALUES ('${owner}'),('${outsider}');
INSERT INTO memoir_source.folders VALUES ('${folder}','${owner}',NULL,'Research',3,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',NULL);
GRANT USAGE ON SCHEMA memoir_source TO ${role};
GRANT SELECT ON memoir_source.users,memoir_source.folders TO ${role};`);

    const {publicKey, privateKey} = generateKeyPairSync('rsa', {modulusLength: 2048});
    const jwk = {...publicKey.export({format: 'jwk'}), kid: 'memoir-favorite-test', alg: 'RS256', use: 'sig', key_ops: ['verify']};
    const jwks = join(temp, 'jwks.json');
    writeFileSync(jwks, JSON.stringify({keys: [jwk]}));
    writeFileSync(config, JSON.stringify({schema, database_url_env: 'AIP_MEMOIR_DATABASE', migrate_database_url_env: 'AIP_MEMOIR_MIGRATION',
      listen: '127.0.0.1:0', wire: 'decimal', auth: {issuer: 'https://memoir-synthetic.test', audience: 'aip', jwks: {kind: 'file', path: jwks}}}));
    assert.equal(cli(['init', file, '--config', config]).ok, true);
    const plan = cli(['migrate', file, '--config', config]);
    assert.equal(cli(['migrate', file, '--apply', plan.digest, '--config', config]).ok, true);
    assert.equal(cli(['provision', '--config', config]).ok, true);
    const infrastructure = `aip_infra_${createHash('sha256').update(schema).digest('hex').slice(0, 52)}`;
    sql(`GRANT USAGE ON SCHEMA ${schema},${infrastructure} TO ${role};
GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA ${schema},${infrastructure} TO ${role};
GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA ${schema},${infrastructure} TO ${role};`);
    sql(`INSERT INTO ${schema}.member(id) SELECT id FROM memoir_source.users;
INSERT INTO ${schema}.folder(id,owner_id,name,sort_order,deleted)
SELECT id,member_id,name,sort_order,deleted_at IS NOT NULL FROM memoir_source.folders WHERE id='${folder}';`);
    assert.equal(sql(`SELECT id,owner_id,name,sort_order,deleted FROM ${schema}.folder WHERE id='${folder}';`), `${folder}|${owner}|Research|3|f`);
    cli(['principal', '--config', config, 'bind', '--subject', 'owner', '--actor', owner]);
    cli(['principal', '--config', config, 'bind', '--subject', 'outsider', '--actor', outsider]);

    const consumer = join(temp, 'consumer'), sdk = join(temp, 'sdk');
    mkdirSync(consumer);
    writeFileSync(join(consumer, 'package.json'), '{"private":true,"type":"module"}');
    run(process.execPath, ['product/tools/build-sdk.mjs', '--out', sdk]);
    const tarball = JSON.parse(run('npm', ['pack', '--offline', '--json'], {cwd: sdk}))[0].filename;
    run('npm', ['install', '--offline', '--ignore-scripts', '--no-audit', '--no-fund', join(sdk, tarball)], {cwd: consumer});
    const binding = join(consumer, 'binding.ts');
    const generated = cli(['gen', file, '--out', binding]);
    writeFileSync(join(consumer, 'types.ts'), `import {connect} from '@aip/sdk'; import {contract} from './binding.ts';
const client=connect('http://localhost:1',null,contract);
client.apply({apply:'Folder.rename',target:{ids:['${folder}']},values:{name:'Engineering',sortOrder:7}});
// @ts-expect-error folder ownership cannot be changed by the rename operation
client.apply({apply:'Folder.rename',target:{ids:['${folder}']},values:{owner:'${outsider}',name:'Engineering',sortOrder:7}});
`);
    const compiler = process.env.AIP_TSC || join(root, 'spikes/spike-0-ts/node_modules/.bin/tsc');
    run(compiler, ['--strict','--noEmit','--module','nodenext','--moduleResolution','nodenext','--target','esnext','--allowImportingTsExtensions','--skipLibCheck','types.ts'], {cwd: consumer});

    const b64 = value => Buffer.from(JSON.stringify(value)).toString('base64url');
    const token = subject => {
      const now = Math.floor(Date.now() / 1000);
      const unsigned = `${b64({alg:'RS256',typ:'at+jwt',kid:jwk.kid})}.${b64({iss:'https://memoir-synthetic.test',aud:'aip',sub:subject,iat:now,nbf:now,exp:now+600})}`;
      return `${unsigned}.${sign('RSA-SHA256',Buffer.from(unsigned),privateKey).toString('base64url')}`;
    };
    server = spawn(binary, ['service','serve',file,'--config',config], {cwd:temp,env,stdio:['ignore','pipe','pipe']});
    let stderr = '';
    server.stderr.on('data', data => {stderr += data;});
    lines = createInterface({input:server.stdout});
    const started = await Promise.race([
      once(lines,'line').then(([line])=>JSON.parse(line)),
      once(server,'exit').then(([code])=>{throw new Error(`serve exit ${code}: ${stderr}`);}),
      new Promise((_,reject)=>setTimeout(()=>reject(new Error(`serve timeout: ${stderr}`)),15000).unref())
    ]);
    assert.equal(started.ok,true);
    const base = `http://${started.address}`;
    const {connect} = await import(pathToFileURL(join(consumer,'node_modules/@aip/sdk/product/sdk/index.js')).href);
    const {contract} = await import(pathToFileURL(binding).href);
    const clients = Object.fromEntries(['owner','outsider'].map(subject=>[subject,connect(base,token(subject),contract)]));
    const query = {read:'Folder',select:['id','owner','name','sortOrder','deleted'],filter:[{field:'id',op:'eq',value:folder}],limit:10};

    await t.test('owner reads the synthetic Memoir-shaped folder; unrelated member cannot see it',async()=>{
      assert.equal(generated.ok,true);
      const rows = await clients.owner.read(query);
      assert.equal(rows.rows.length,1);
      assert.equal(rows.rows[0].owner,owner);
      assert.equal(rows.rows[0].name,'Research');
      assert.equal((await clients.outsider.read(query)).rows.length,0);
    });
    await t.test('owner renames a folder through the installed SDK and PostgreSQL persists it',async()=>{
      const result = await clients.owner.apply({apply:'Folder.rename',target:{ids:[folder]},values:{name:'Engineering',sortOrder:7}},{key:'owner-rename'});
      assert.equal(result.ok,true,JSON.stringify(result));
      assert.deepEqual(result.changed,[folder]);
      assert.equal(sql(`SELECT name||'|'||sort_order FROM ${schema}.folder WHERE id='${folder}';`),'Engineering|7');
      assert.equal(sql(`SELECT name||'|'||sort_order FROM memoir_source.folders WHERE id='${folder}';`),'Research|3');
      assert.equal((await clients.owner.read(query)).rows[0].name,'Engineering');
    });
    await t.test('outsider apply is denied and a fresh read confirms no stored state change',async()=>{
      const denied = await clients.outsider.apply({apply:'Folder.rename',target:{ids:[folder]},values:{name:'Forbidden',sortOrder:99}},{key:'outsider-rename'});
      assert.equal(denied.ok,false,JSON.stringify(denied));
      assert.equal(denied.code,'WIRE.MISSING_TARGET',JSON.stringify(denied));
      assert.equal(sql(`SELECT name||'|'||sort_order FROM ${schema}.folder WHERE id='${folder}';`),'Engineering|7');
      assert.equal((await connect(base,token('owner'),contract).read(query)).rows[0].name,'Engineering');
    });
    assert.match(generated.fingerprint,/^[a-f0-9]{64}$/);
  } finally {
    if(server&&server.exitCode===null&&server.signalCode===null){const ended=once(server,'exit');server.kill('SIGTERM');await ended;}
    lines?.close();
    if(databaseCreated)sql(`DROP DATABASE ${database};`,'host=localhost dbname=postgres');
    if(roleCreated)sql(`DROP ROLE ${role};`,'host=localhost dbname=postgres');
    rmSync(temp,{recursive:true,force:true});
  }
});
