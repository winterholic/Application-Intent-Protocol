import assert from 'node:assert/strict';
import {test} from 'node:test';
import {generateKeyPairSync, sign, randomUUID, createHash} from 'node:crypto';
import {mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {pathToFileURL} from 'node:url';
import {spawn, spawnSync} from 'node:child_process';

const track = resolve(new URL('..', import.meta.url).pathname);
const root = resolve(track, '../..');
const binary = join(root, 'target/debug/aip');
const file = join(track, 'app.aip');
const b64 = value => Buffer.from(JSON.stringify(value)).toString('base64url');

test('AIP server enforces forum group visibility and unique link relations with the SDK', {timeout: 60000}, async () => {
  const temp = mkdtempSync(join(tmpdir(), 'aip-flaskbb-read-'));
  const suffix = randomUUID().replaceAll('-', '').slice(0, 10);
  const schema = `aip_flaskbb_${suffix}`;
  const database = 'aip_flaskbb';
  const dbUrl = `postgresql://aip_flaskbb:aip_flaskbb_fixture_password@127.0.0.1:55439/${database}`;
  const env = {...process.env, AIP_FLASKBB_DATABASE: dbUrl};
  const run = (command, args, options = {}) => {
    const result = spawnSync(command, args, {cwd: root, env, encoding: 'utf8', timeout: 20000, maxBuffer: 4 << 20, ...options});
    assert.equal(result.status, 0, `${command} ${args.join(' ')}\n${result.stdout}\n${result.stderr}`);
    return result.stdout.trim();
  };
  const sql = statement => run('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-At', '-d', dbUrl, '-c', statement]);
  const config = join(temp, 'service.json');
  const sdkRoot = join(temp, 'sdk');
  const jwksPath = join(temp, 'jwks.json');
  const bindingPath = join(temp, 'binding.ts');
  let server;
  try {
    const {publicKey, privateKey} = generateKeyPairSync('rsa', {modulusLength: 2048});
    const jwk = {...publicKey.export({format: 'jwk'}), kid: 'flaskbb-fixture', alg: 'RS256', use: 'sig', key_ops: ['verify']};
    writeFileSync(jwksPath, JSON.stringify({keys: [jwk]}));
    writeFileSync(config, JSON.stringify({schema, database_url_env: 'AIP_FLASKBB_DATABASE', migrate_database_url_env: 'AIP_FLASKBB_DATABASE',
      listen: '127.0.0.1:0', wire: 'decimal', allowed_origins: ['http://127.0.0.1:58080'],
      auth: {issuer: 'https://flaskbb.test', audience: 'aip-flaskbb', jwks: {kind: 'file', path: jwksPath}}}));
    const init = JSON.parse(run(binary, ['service', 'init', file, '--config', config]));
    assert.equal(init.ok, true);
    run(process.execPath, ['product/tools/build-sdk.mjs', '--out', sdkRoot]);
    const q = `"${schema}"`;
    sql(`INSERT INTO ${q}."group" (id,name,guest,admin,super_mod,mod,banned,editpost,deletepost,deletetopic,posttopic,postreply,viewhidden,makehidden) VALUES
      (1,'Guest',true,false,false,false,false,false,false,false,false,false,false,false),
      (2,'Member',false,false,false,false,false,true,false,false,true,true,false,false);`);
    sql(`INSERT INTO ${q}.user (id,username,primary_group_id,activated) VALUES (1,'guest-fixture',1,true),(2,'member-fixture',2,true);`);
    sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','Fixture',1);`);
    sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,external,post_count,topic_count) VALUES
      (1,1,'Public forum','Visible to guest group',1,false,NULL,0,0),
      (2,1,'Private fixture','Member group required',2,false,NULL,0,0);`);
    sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,post_count,hidden) VALUES
      (1,1,'Public topic',1,'guest-fixture',now(),now(),false,false,2,1,false),
      (2,2,'Private topic',2,'member-fixture',now(),now(),false,false,3,1,false),
      (3,2,'Hidden topic',2,'member-fixture',now(),now(),false,false,0,1,true);`);
    sql(`INSERT INTO ${q}.post (id,topic_id,user_id,username,content,date_created,hidden) VALUES
      (1,1,1,'guest-fixture','Public post',now(),false),
      (2,2,2,'member-fixture','Private post',now(),false),
      (3,3,2,'member-fixture','Hidden post',now(),true);`);
    sql(`UPDATE ${q}.topic SET first_post_id=id, last_post_id=id WHERE id IN (1,2,3);`);
    sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,2,2);`);
    sql(`INSERT INTO ${q}.user_group (id,user_id,group_id) VALUES (1,1,1),(2,2,2);`);
    const duplicate = spawnSync('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-d', dbUrl, '-c', `INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (3,2,2)`], {encoding: 'utf8', env});
    assert.notEqual(duplicate.status, 0, 'duplicate forum/group access must be rejected');
    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'member-fixture', '--actor', '2']);
    const generated = JSON.parse(run(binary, ['service', 'gen', file, '--out', bindingPath]));
    const bindingSource = readFileSync(bindingPath, 'utf8');
    const descriptors = JSON.parse(bindingSource.match(/export const readDescriptors = (.*) as const;/)[1]);
    const contract = {fingerprint: generated.fingerprint, idWire: 'decimal-string-v13', extensions: {}, readDescriptors: descriptors};
    server = spawn(binary, ['service', 'serve', file, '--config', config], {cwd: temp, env, stdio: ['ignore', 'pipe', 'pipe']});
    let stderr = '';
    server.stderr.on('data', data => {stderr += data;});
    const lines = createInterface({input: server.stdout});
    const started = await Promise.race([
      once(lines, 'line').then(([line]) => JSON.parse(line)),
      once(server, 'exit').then(([code]) => {throw new Error(`serve exit ${code}: ${stderr}`);}),
      new Promise((_, reject) => setTimeout(() => reject(new Error(`serve timeout: ${stderr}`)), 15000).unref())
    ]);
    assert.equal(started.ok, true, JSON.stringify(started));
    const {connect} = await import(pathToFileURL(join(sdkRoot, 'product/sdk/index.js')).href);
    const token = subject => {
      const now = Math.floor(Date.now() / 1000);
      const unsigned = `${b64({alg: 'RS256', typ: 'at+jwt', kid: jwk.kid})}.${b64({iss: 'https://flaskbb.test', aud: 'aip-flaskbb', sub: subject, iat: now, nbf: now, exp: now + 600})}`;
      return `${unsigned}.${sign('RSA-SHA256', Buffer.from(unsigned), privateKey).toString('base64url')}`;
    };
    const member = connect(`http://${started.address}`, token('member-fixture'), contract);
    const guest = connect(`http://${started.address}`, null, contract);
    const query = {read: 'Forum', select: ['id', 'title', 'category'], sort: [{field: 'id', dir: 'asc'}], limit: 100};
    const memberRows = (await member.read(query)).rows;
    const guestRows = (await guest.read(query)).rows;
    assert.deepEqual(memberRows.map(row => row.title), ['Private fixture']);
    assert.deepEqual(guestRows.map(row => row.title), ['Public forum']);
    const topicQuery = {read: 'Topic', select: ['id', 'title', 'forum', 'first_post'], sort: [{field: 'id', dir: 'asc'}], limit: 100};
    const postQuery = {read: 'Post', select: ['id', 'topic', 'username', 'content'], sort: [{field: 'id', dir: 'asc'}], limit: 100};
    assert.deepEqual((await member.read(topicQuery)).rows.map(row => row.title), ['Private topic']);
    assert.deepEqual((await guest.read(topicQuery)).rows.map(row => row.title), ['Public topic']);
    assert.deepEqual((await member.read(postQuery)).rows.map(row => row.content), ['Private post']);
    assert.deepEqual((await guest.read(postQuery)).rows.map(row => row.content), ['Public post']);
    assert.equal(sql(`SELECT count(*) FROM ${q}.forum_group_access`), '2');
    console.log(JSON.stringify({schema, ...init, memberVisible: memberRows.map(row => row.title), guestVisible: guestRows.map(row => row.title), duplicateLinkRejected: true}));
  } finally {
    if (server && server.exitCode === null) {
      server.kill('SIGTERM');
      await Promise.race([once(server, 'exit'), new Promise(resolve => setTimeout(resolve, 3000))]);
    }
    if (/^aip_flaskbb_[a-f0-9]{10}$/.test(schema)) {
      const adminUrl = dbUrl;
      spawnSync('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-d', adminUrl, '-c', `DROP SCHEMA IF EXISTS "${schema}" CASCADE`], {encoding: 'utf8', env});
    }
    rmSync(temp, {recursive: true, force: true});
  }
});
