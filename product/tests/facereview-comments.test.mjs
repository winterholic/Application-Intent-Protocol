import assert from 'node:assert/strict';
import {test} from 'node:test';
import {spawn, spawnSync} from 'node:child_process';
import {generateKeyPairSync, sign, randomUUID, createHash} from 'node:crypto';
import {mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {pathToFileURL} from 'node:url';
import {requireTestDatabase, withDatabase, withUser} from '../test-support/postgres-guard.mjs';

const root = resolve(import.meta.dirname, '../..');
const binary = join(root, 'target/debug/aip');
const file = join(root, 'product/tests/fixtures/facereview-comments/comments.aip');
const adminDb = requireTestDatabase({admin: true});

test('FaceReview comments: existing PG schema, installed SDKs and real authorization', {timeout: 180000}, async t => {
  const temp = mkdtempSync(join(tmpdir(), 'aip-facereview-'));
  const suffix = `${process.pid}_${randomUUID().replaceAll('-', '').slice(0, 8)}`;
  const database = `aip_facereview_${suffix}`;
  const role = `aip_face_runtime_${suffix}`;
  const schema = 'aip_comments';
  const source = 'facereview_copy';
  const owner = randomUUID(), other = randomUUID(), video = randomUUID(), deletedVideo = randomUUID();
  const adminUrl = withDatabase(adminDb.raw, database);
  const runtimeUrl = withUser(adminUrl, role);
  const env = {PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: process.env.TMPDIR,
    AIP_FACE_DATABASE: runtimeUrl, AIP_FACE_MIGRATION: adminUrl, npm_config_cache: join(temp, 'npm-cache')};
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
    sql(`CREATE DATABASE ${database};`, adminDb.raw);
    databaseCreated = true;
    sql(`CREATE ROLE ${role} LOGIN;`);
    roleCreated = true;
    sql(`CREATE SCHEMA ${source};
CREATE TABLE ${source}."user" (user_id uuid PRIMARY KEY DEFAULT gen_random_uuid(), name varchar(50) NOT NULL);
CREATE TABLE ${source}.video (video_id uuid PRIMARY KEY DEFAULT gen_random_uuid(), title varchar(255) NOT NULL, is_deleted boolean NOT NULL DEFAULT false);
CREATE TABLE ${source}.comment (
 comment_id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 video_id uuid NOT NULL REFERENCES ${source}.video(video_id),
 user_id uuid NOT NULL REFERENCES ${source}."user"(user_id),
 content text NOT NULL, is_modified boolean NOT NULL DEFAULT false, is_deleted boolean NOT NULL DEFAULT false,
 version integer NOT NULL DEFAULT 0,
 created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now());
CREATE INDEX idx_video_comments ON ${source}.comment(video_id, is_deleted, created_at);
INSERT INTO ${source}."user" VALUES ('${owner}','작성자'),('${other}','다른 사용자');
INSERT INTO ${source}.video(video_id,title,is_deleted) VALUES ('${video}','공개 영상',false),('${deletedVideo}','삭제된 영상',true);
GRANT CONNECT ON DATABASE ${database} TO ${role};
GRANT USAGE ON SCHEMA ${source} TO ${role};
GRANT SELECT ON ${source}."user",${source}.video TO ${role};
GRANT SELECT,INSERT,UPDATE ON ${source}.comment TO ${role};`);

    const {publicKey, privateKey} = generateKeyPairSync('rsa', {modulusLength: 2048});
    const jwk = {...publicKey.export({format: 'jwk'}), kid: 'facereview-test', alg: 'RS256', use: 'sig', key_ops: ['verify']};
    const jwks = join(temp, 'jwks.json');
    writeFileSync(jwks, JSON.stringify({keys: [jwk]}));
    writeFileSync(config, JSON.stringify({schema, database_url_env: 'AIP_FACE_DATABASE', migrate_database_url_env: 'AIP_FACE_MIGRATION',
      listen: '127.0.0.1:0', wire: 'decimal', auth: {issuer: 'https://facereview.test', audience: 'aip', jwks: {kind: 'file', path: jwks}}}));
    const mapping = join(temp, 'mapping.json');
    const introspected = join(temp, 'draft.aip');
    const scan = cli(['introspect', '--source-schema', source, '--out', introspected, '--mapping-out', mapping, '--config', config]);
    await t.test('read-only introspection discovers UUID references and database defaults', () => {
      assert.equal(scan.ok, true);
      assert.match(readFileSync(introspected, 'utf8'), /user: User/);
      assert.match(readFileSync(introspected, 'utf8'), /created_at: Time default database/);
      assert.match(readFileSync(introspected, 'utf8'), /rows read when false/);
      assert.equal(cli(['check', file]).ok, true);
    });
    const settings = JSON.parse(readFileSync(config, 'utf8'));
    settings.external_mapping = mapping;
    writeFileSync(config, JSON.stringify(settings));
    const plan = cli(['adopt-external', file, '--config', config]);
    await t.test('external adoption creates metadata/views without business table DDL', () => {
      assert.deepEqual(plan.businessDDL, []);
      assert.equal(cli(['adopt-external', file, '--apply', plan.digest, '--config', config]).ok, true);
      assert.equal(cli(['adopt-external', file, '--apply', plan.digest, '--config', config]).alreadyApplied, true);
      assert.equal(sql(`SELECT count(*) FROM ${source}.video;`), '2');
    });
    assert.equal(cli(['provision', '--config', config]).ok, true);
    const infrastructure = `aip_infra_${createHash('sha256').update(schema).digest('hex').slice(0, 52)}`;
    sql(`GRANT USAGE ON SCHEMA ${schema},${infrastructure} TO ${role};
GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA ${schema},${infrastructure} TO ${role};
GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA ${schema},${infrastructure} TO ${role};`);
    cli(['principal', '--config', config, 'bind', '--subject', 'owner', '--actor', owner]);
    cli(['principal', '--config', config, 'bind', '--subject', 'other', '--actor', other]);
    const consumer = join(temp, 'consumer'), sdk = join(temp, 'sdk');
    mkdirSync(consumer);
    writeFileSync(join(consumer, 'package.json'), '{"private":true,"type":"module"}');
    run(process.execPath, ['product/tools/build-sdk.mjs', '--out', sdk]);
    const tarball = JSON.parse(run('npm', ['pack', '--offline', '--json'], {cwd: sdk}))[0].filename;
    run('npm', ['install', '--offline', '--ignore-scripts', '--no-audit', '--no-fund', join(sdk, tarball)], {cwd: consumer});
    const binding = join(consumer, 'binding.ts');
    const python = join(temp, 'comments_binding.py');
    const tsBinding = cli(['gen', file, '--out', binding]);
    const pyBinding = cli(['gen', file, '--lang', 'python', '--out', python]);
    await t.test('TS/Python bindings share contract and TS rejects protected fields', () => {
      assert.equal(tsBinding.fingerprint, pyBinding.fingerprint);
      writeFileSync(join(consumer, 'types.ts'), `import {connect} from '@aip/sdk'; import {contract} from './binding.ts';
const c=connect('http://localhost:1',null,contract);
c.apply({apply:'Comment.edit',target:{ids:['00000000-0000-0000-0000-000000000001']},values:{content:'새 댓글'},expect:{version:0}});
// @ts-expect-error the author is never patchable
c.apply({apply:'Comment.edit',target:{ids:['00000000-0000-0000-0000-000000000001']},values:{user:'other'},expect:{version:0}});
// @ts-expect-error a version is required
c.apply({apply:'Comment.edit',target:{ids:['00000000-0000-0000-0000-000000000001']},values:{content:'새 댓글'}});
`);
      const compiler = join(root, 'spikes/spike-0-ts/node_modules/.bin/tsc');
      const flags = ['--strict', '--noEmit', '--module', 'nodenext', '--moduleResolution', 'nodenext', '--target', 'esnext', '--allowImportingTsExtensions', '--skipLibCheck'];
      run(compiler, [...flags, 'types.ts'], {cwd: consumer});
      writeFileSync(join(consumer, 'negative.ts'), readFileSync(join(consumer, 'types.ts'), 'utf8').replaceAll('@ts-expect-error', 'negative control'));
      const negative = spawnSync(compiler, [...flags, 'negative.ts'], {cwd: consumer, env, encoding: 'utf8'});
      assert.ok([1, 2].includes(negative.status), negative.stdout + negative.stderr);
      assert.match(negative.stdout, /negative.ts\(5,\d+\): error TS2353/);
      assert.match(negative.stdout, /negative.ts\(7,\d+\): error TS2345/);
    });
    const b64 = value => Buffer.from(JSON.stringify(value)).toString('base64url');
    const token = subject => {
      const now = Math.floor(Date.now() / 1000);
      const unsigned = `${b64({alg: 'RS256', typ: 'at+jwt', kid: jwk.kid})}.${b64({iss: 'https://facereview.test', aud: 'aip', sub: subject, iat: now, nbf: now, exp: now + 600})}`;
      return `${unsigned}.${sign('RSA-SHA256', Buffer.from(unsigned), privateKey).toString('base64url')}`;
    };
    server = spawn(binary, ['service', 'serve', file, '--config', config], {cwd: temp, env, stdio: ['ignore', 'pipe', 'pipe']});
    let stderr = '';
    server.stderr.on('data', data => {stderr += data;});
    lines = createInterface({input: server.stdout});
    const started = await Promise.race([
      once(lines, 'line').then(([line]) => JSON.parse(line)),
      once(server, 'exit').then(([code]) => {throw new Error(`serve exit ${code}: ${stderr}`);}),
      new Promise((_, reject) => setTimeout(() => reject(new Error(`serve timeout: ${stderr}`)), 15000).unref())
    ]);
    assert.equal(started.ok, true);
    const base = `http://${started.address}`;
    const {connect, WriteUnsettled} = await import(pathToFileURL(join(consumer, 'node_modules/@aip/sdk/product/sdk/index.js')).href);
    const {contract} = await import(pathToFileURL(binding).href);
    let lastWrite;
    const client = connect(base, token('owner'), contract, async (input, init) => {
      const response = await fetch(input, init);
      if (String(input).endsWith('/apply')) lastWrite = await response.clone().json();
      return response;
    });
    const stranger = connect(base, token('other'), contract);
    const request = content => ({atomic: [{create: 'Comment', values: {video, user: owner, content}}]});
    // Query the real transport for invalid inputs so SDK-side checks cannot mask missing server validation.
    const rawBundle = (body, key) => fetch(`${base}/apply`, {method: 'POST', headers: {Authorization: `Bearer ${token('owner')}`,
      'Content-Type': 'application/json', 'x-aip-contract': contract.fingerprint}, body: JSON.stringify({key, request: body})}).then(r => r.json());
    const query = {read: 'Comment', select: ['id', 'user', 'video', 'content', 'is_modified', 'version', 'created_at', 'updated_at'],
      filter: [{field: 'video', op: 'eq', value: video}], sort: [{field: 'id', dir: 'asc'}], limit: 100};
    const read = async () => (await client.read(query)).rows;
    let id;
    await t.test('SELECT-only source roles cannot lock authorization rows; column grants suffice', async () => {
      const denied = await rawBundle(request('권한 점검'), 'missing-lock-grants');
      assert.equal(denied.code, 'WIRE.INTERNAL', JSON.stringify(denied));
      assert.match(denied.msg, /42501/);
      assert.equal(sql(`SELECT count(*) FROM ${source}.comment;`), '0');
      // PostgreSQL FOR SHARE needs UPDATE on at least one column of each locked table.
      sql(`GRANT UPDATE(user_id) ON ${source}."user" TO ${role};
GRANT UPDATE(video_id) ON ${source}.video TO ${role};`);
      assert.equal(sql(`SELECT has_column_privilege('${role}','${source}."user"','name','UPDATE'),has_column_privilege('${role}','${source}.video','is_deleted','UPDATE');`), 'f|f');
    });
    await t.test('UUID comment creation applies trusted defaults and persists in the source table', async () => {
      const created = await client.bundle(request('첫 댓글'), {key: 'create'}).catch(error => {
        throw new Error(`${error.message}: ${JSON.stringify(lastWrite)}\n${stderr}`);
      });
      assert.equal(created.ok, true, JSON.stringify(created));
      id = created.created[0].id;
      assert.match(id, /^[a-f0-9-]{36}$/);
      const [row] = await read();
      assert.equal(row.id, id);
      assert.equal(row.version, 0);
      assert.equal(row.is_modified, false);
      assert.equal(typeof row.created_at, 'string');
      assert.equal(sql(`SELECT content,version,is_modified,is_deleted FROM ${source}.comment WHERE comment_id='${id}';`), '첫 댓글|0|f|f');
      assert.equal(sql(`SELECT has_schema_privilege('${role}','${source}','CREATE');`), 'f');
    });
    assert.ok(id, 'Stop dependent scenarios if comment creation fails');
    await t.test('the comment list traverses the public author name with its UUID', async () => {
      const result = await client.read({...query, select: ['id', 'content', {user: {select: ['id', 'name']}}]});
      assert.deepEqual(result.rows[0].user, {id: owner, name: '작성자'});
    });
    await t.test('a replay does not create another comment; changed payload with the same key is refused', async () => {
      assert.equal((await client.bundle(request('첫 댓글'), {key: 'create'})).replayed, true);
      assert.equal((await client.bundle(request('다른 댓글'), {key: 'create'})).code, 'WIRE.IDEMPOTENCY_MISMATCH');
      assert.equal((await read()).length, 1);
    });
    await t.test('another user can read a public comment but cannot edit it', async () => {
      assert.equal((await stranger.read(query)).rows[0].content, '첫 댓글');
      const denied = await stranger.apply({apply: 'Comment.edit', target: {ids: [id]}, values: {content: '변조'}, expect: {version: 0}}, {key: 'foreign-edit'});
      assert.equal(denied.code, 'WIRE.FORBIDDEN');
      assert.equal((await read())[0].content, '첫 댓글');
    });
    await t.test('guests can read comments but cannot create them', async () => {
      const guest = connect(base, null, contract);
      assert.equal((await guest.read(query)).rows[0].content, '첫 댓글');
      assert.equal((await guest.bundle(request('게스트 쓰기'), {key: 'guest'})).ok, false);
      assert.equal(sql(`SELECT count(*) FROM ${source}.comment;`), '1');
    });
    await t.test('creation refuses author spoofing and references to a deleted video', async () => {
      const spoof = request('위장'); spoof.atomic[0].values.user = other;
      assert.equal((await rawBundle(spoof, 'spoof')).code, 'WIRE.FORBIDDEN');
      const hidden = request('삭제 영상'); hidden.atomic[0].values.video = deletedVideo;
      assert.equal((await rawBundle(hidden, 'hidden-video')).code, 'WIRE.MISSING_TARGET');
      assert.equal((await read()).length, 1);
    });
    await t.test('server enforces 1..500 Unicode code points and protects defaults', async () => {
      for (const content of ['', '😀'.repeat(501), null, 123]) {
        const result = await rawBundle(request(content), `invalid-${String(content).length}-${typeof content}`);
        assert.equal(result.code, 'WIRE.BAD_VALUE', JSON.stringify(result));
      }
      const forged = request('관리 필드'); forged.atomic[0].values.version = 7;
      assert.equal((await rawBundle(forged, 'forged-default')).code, 'WIRE.FIELD_NOT_EXPOSED');
      assert.equal((await client.bundle(request('😀'.repeat(500)), {key: 'unicode-limit'})).ok, true);
    });
    await t.test('editing updates content, modified flag, timestamp and version in one transaction', async () => {
      const result = await client.apply({apply: 'Comment.edit', target: {ids: [id]}, values: {content: '수정 댓글'}, expect: {version: 0}}, {key: 'edit'});
      assert.equal(result.ok, true, JSON.stringify(result));
      const row = (await read()).find(row => row.id === id);
      assert.equal(row.content, '수정 댓글'); assert.equal(row.version, 1); assert.equal(row.is_modified, true);
      assert.ok(Date.parse(row.updated_at) >= Date.parse(row.created_at));
      const audit = cli(['audit', '--config', config, 'list', '--resource', 'Comment', '--target-id', id]);
      assert.equal(audit.audit[0].targetId, id);
    });
    await t.test('two concurrent editors with the same version have exactly one winner', async () => {
      const results = await Promise.all(['편집 A', '편집 B'].map((content, index) => client.apply({apply: 'Comment.edit', target: {ids: [id]},
        values: {content}, expect: {version: 1}}, {key: `race-${index}`})));
      assert.equal(results.filter(result => result.ok).length, 1, JSON.stringify(results));
      assert.equal(results.find(result => !result.ok).code, 'WIRE.CONFLICT');
      assert.equal((await read()).find(row => row.id === id).version, 2);
    });
    await t.test('a denied second write rolls back the first write in an atomic bundle', async () => {
      const others = await stranger.bundle({atomic: [{create: 'Comment', values: {video, user: other, content: '다른 사용자 댓글'}}]}, {key: 'other-create'});
      assert.equal(others.ok, true, JSON.stringify(others));
      const otherId = others.created[0].id;
      const before = (await read()).find(row => row.id === id);
      const result = await client.bundle({atomic: [
        {apply: 'Comment.edit', target: {ids: [id]}, values: {content: '취소될 수정'}, expect: {version: 2}},
        {apply: 'Comment.edit', target: {ids: [otherId]}, values: {content: '권한 없음'}, expect: {version: 0}}
      ]}, {key: 'rollback'});
      assert.equal(result.code, 'WIRE.FORBIDDEN');
      assert.deepEqual((await read()).find(row => row.id === id), before);
    });
    await t.test('comments of a video deleted after creation become unreadable', async () => {
      sql(`UPDATE ${source}.video SET is_deleted=true WHERE video_id='${video}';`);
      try {
        const fresh = connect(base, token('owner'), contract);
        assert.deepEqual((await fresh.read(query)).rows, []);
        assert.deepEqual((await fresh.read({read: 'Comment', select: ['id', 'content'], limit: 100})).rows, []);
      } finally {
        sql(`UPDATE ${source}.video SET is_deleted=false WHERE video_id='${video}';`);
      }
    });
    await t.test('a lost commit response recovers the same key without applying the edit twice', async () => {
      let lose = true;
      const unstable = connect(base, token('owner'), contract, async (input, init) => {
        const response = await fetch(input, init);
        if (lose && String(input).endsWith('/apply')) {await response.arrayBuffer(); throw new Error('lost committed response');}
        return response;
      });
      await assert.rejects(unstable.apply({apply: 'Comment.edit', target: {ids: [id]}, values: {content: '응답 유실 복구'}, expect: {version: 2}},
        {key: 'lost', retries: 0}), error => error instanceof WriteUnsettled);
      lose = false;
      const recovered = await unstable.retryPending();
      assert.equal(recovered.length, 1); assert.equal(recovered[0].ok, true);
      const fresh = connect(base, token('owner'), contract);
      assert.equal((await fresh.read(query)).rows.find(row => row.id === id).version, 3);
      assert.equal(sql(`SELECT content,version FROM ${source}.comment WHERE comment_id='${id}';`), '응답 유실 복구|3');
    });
    await t.test('Python generated helper edits the same UUID comment', () => {
      const script = join(temp, 'consumer.py');
      writeFileSync(script, `import sys\nfrom aip_sdk import connect\nfrom comments_binding import CONTRACT_FINGERPRINT, apply_comment_edit\nc=connect(sys.argv[1],sys.stdin.read(),CONTRACT_FINGERPRINT)\nr=apply_comment_edit(c,{"ids":["${id}"]},{"content":"Python 수정"},{"version":3},key="python")\nassert r["ok"],r\nrows=c.read({"read":"Comment","select":["id","content","version"],"filter":[{"field":"id","op":"eq","value":"${id}"}]})\nassert rows[0]["content"]=="Python 수정" and rows[0]["version"]==4,rows\nprint("Python FaceReview generated helper passed")\n`);
      assert.match(run('python3', [script, base], {cwd: temp, env: {...env, PYTHONPATH: join(root, 'product/python')}, input: token('owner')}), /passed/);
    });
    await t.test('soft delete hides the comment, retains the row and prevents later edits', async () => {
      assert.equal((await stranger.apply({apply: 'Comment.delete', target: {ids: [id]}}, {key: 'foreign-delete'})).code, 'WIRE.FORBIDDEN');
      const result = await client.apply({apply: 'Comment.delete', target: {ids: [id]}}, {key: 'delete'});
      assert.equal(result.ok, true, JSON.stringify(result));
      assert.ok(!(await read()).some(row => row.id === id));
      assert.equal(sql(`SELECT is_deleted,version FROM ${source}.comment WHERE comment_id='${id}';`), 't|5');
      assert.equal((await client.apply({apply: 'Comment.edit', target: {ids: [id]}, values: {content: '부활 시도'}, expect: {version: 5}}, {key: 'after-delete'})).code, 'WIRE.MISSING_TARGET');
    });
    await t.test('source schema drift closes readiness and blocks writes', async () => {
      sql(`ALTER TABLE ${source}.comment ALTER COLUMN version SET DEFAULT 1;`);
      const rejected = await rawBundle(request('drift 이후'), 'drift');
      assert.equal(rejected.code, 'WIRE.DEPLOYMENT_CHANGED');
      assert.equal((await fetch(`${base}/ready`)).status, 503);
    });
  } finally {
    if (server && server.exitCode === null && server.signalCode === null) {
      const ended = once(server, 'exit'); server.kill('SIGTERM'); await ended;
    }
    lines?.close();
    if (databaseCreated) sql(`DROP DATABASE ${database};`, adminDb.raw);
    if (roleCreated) sql(`DROP ROLE ${role};`, adminDb.raw);
    rmSync(temp, {recursive: true, force: true});
  }
});
