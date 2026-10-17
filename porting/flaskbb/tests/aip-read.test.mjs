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
  const uiHosts = [];
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
    sql(`INSERT INTO ${q}."group" (id,name,guest,admin,super_mod,mod,banned,editpost,deletepost,deletetopic,posttopic,postreply,viewhidden,makehidden,mod_edituser,mod_banuser,description) VALUES
      (1,'Guest',true,false,false,false,false,false,false,false,false,false,false,false,false,false,NULL),
      (2,'Member',false,false,false,false,false,true,false,false,true,true,false,false,false,false,NULL),
      (3,'Administrator',false,true,false,false,false,true,true,true,true,true,true,true,true,true,NULL),
      (4,'Read only',false,false,false,false,false,false,false,false,false,false,false,false,false,false,NULL);`);
    sql(`INSERT INTO ${q}.user (id,username,email,date_joined,lastseen,birthday,gender,website,location,signature,avatar,notes,primary_group_id,activated,language,theme) VALUES
      (1,'guest-fixture','guest@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,1,true,'en',NULL),
      (2,'member-fixture','member@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,2,true,'en',NULL),
      (3,'admin-fixture','admin@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,3,true,'en',NULL),
      (4,'readonly-fixture','readonly@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,4,true,'en',NULL);`);
    sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','Fixture',1);`);
    sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,external,show_moderators,last_post_id,last_post_user_id,last_post_title,last_post_username,last_post_created) VALUES
      (1,1,'Public forum','Visible to guest group',1,false,NULL,false,NULL,NULL,NULL,NULL,NULL),
      (2,1,'Private fixture','Member group required',2,false,NULL,false,NULL,NULL,NULL,NULL,NULL);`);
    sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES
      ('00000000-0000-4000-8000-000000000001',1,'Public topic',1,'guest-fixture',now(),now(),false,false,2,true,false,false),
      ('00000000-0000-4000-8000-000000000002',2,'Private topic',2,'member-fixture',now(),now(),false,false,3,true,false,false),
      ('00000000-0000-4000-8000-000000000003',2,'Hidden topic',2,'member-fixture',now(),now(),false,false,0,true,false,true);`);
    sql(`INSERT INTO ${q}.post (id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted) VALUES
      ('00000000-0000-4000-8000-000000000011','00000000-0000-4000-8000-000000000001',1,1,'guest-fixture','Public post',now(),false,true,0,false),
      ('00000000-0000-4000-8000-000000000012','00000000-0000-4000-8000-000000000002',2,2,'member-fixture','Private post',now(),false,true,0,false),
      ('00000000-0000-4000-8000-000000000013','00000000-0000-4000-8000-000000000003',2,2,'member-fixture','Hidden post',now(),true,true,0,false);`);
    sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,2,2),(3,2,3),(4,2,4);`);
    const duplicate = spawnSync('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-d', dbUrl, '-c', `INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (5,2,2)`], {encoding: 'utf8', env});
    assert.notEqual(duplicate.status, 0, 'duplicate forum/group access must be rejected');
    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'member-fixture', '--actor', '2']);
    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'admin-fixture', '--actor', '3']);
    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'readonly-fixture', '--actor', '4']);
    const generated = JSON.parse(run(binary, ['service', 'gen', file, '--out', bindingPath]));
    const bindingSource = readFileSync(bindingPath, 'utf8');
    const descriptors = JSON.parse(bindingSource.match(/export const readDescriptors = (.*) as const;/)[1]);
    const identityKinds = JSON.parse(bindingSource.match(/identityKinds: (.*), fingerprint:/)[1]);
    const contract = {fingerprint: generated.fingerprint, idWire: 'decimal-string-v13', identityKinds, extensions: {}, readDescriptors: descriptors};
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
    const admin = connect(`http://${started.address}`, token('admin-fixture'), contract);
    const readOnly = connect(`http://${started.address}`, token('readonly-fixture'), contract);
    const guest = connect(`http://${started.address}`, null, contract);

    const python = '/private/tmp/flaskbb-aip-host-venv/bin/python';
    const launchUi = async subject => {
      const uiEnv = {
        PATH: process.env.PATH,
        PYTHONPATH: [join(track, 'ui'), join(root, 'product/python')].join(':'),
        FLASKBB_TEST_INTEGRATION: '1',
        FLASKBB_TEST_ACCESS_TOKEN: token(subject),
        FLASKBB_TEST_SUBJECT: subject,
        FLASKBB_TEST_USERNAME: subject,
        FLASKBB_TEST_AIP_URL: `http://${started.address}`,
      };
      const host = spawn(python, ['tests/run_ui_host.py'], {cwd: track, env: uiEnv, stdio: ['ignore', 'pipe', 'pipe']});
      uiHosts.push(host);
      let hostStderr = '';
      host.stderr.on('data', data => {hostStderr += data;});
      const lines = createInterface({input: host.stdout});
      const listening = await Promise.race([
        once(lines, 'line').then(([line]) => JSON.parse(line)),
        once(host, 'exit').then(([code]) => {throw new Error(`UI host exit ${code}: ${hostStderr}`);}),
        new Promise((_, reject) => setTimeout(() => reject(new Error(`UI host timeout: ${hostStderr}`)), 15000).unref())
      ]);
      return {host, base: `http://127.0.0.1:${listening.port}`};
    };
    const readonlyUi = await launchUi('readonly-fixture');
    const cookie = 'aip_session=aip-integration-session';
    const csrf = 'aip-integration-csrf-token';
    const readonlyForumResponse = await fetch(`${readonlyUi.base}/forum/2`, {headers: {Cookie: cookie}});
    assert.equal(readonlyForumResponse.status, 200);
    const readonlyForumHtml = await readonlyForumResponse.text();
    assert.doesNotMatch(readonlyForumHtml, /Start a topic/, 'read-only members must not see the topic form');
    const readonlyTopicResponse = await fetch(`${readonlyUi.base}/topic/00000000-0000-4000-8000-000000000002`, {headers: {Cookie: cookie}});
    assert.equal(readonlyTopicResponse.status, 200);
    assert.doesNotMatch(await readonlyTopicResponse.text(), /<button[^>]*>Reply<\/button>/, 'read-only members must not see the reply form');
    const beforeReadonlyUiPost = sql(`SELECT count(*) FROM ${q}.topic`);
    const readonlyPostResponse = await fetch(`${readonlyUi.base}/forum/2/topic`, {
      method: 'POST', redirect: 'manual', headers: {Cookie: cookie, 'Content-Type': 'application/x-www-form-urlencoded'},
      body: new URLSearchParams({csrf_token: csrf, title: 'Denied UI topic', content: 'Must not persist'})
    });
    assert.equal(readonlyPostResponse.status, 403, 'AIP denies a forged hidden-form submission');
    assert.equal(sql(`SELECT count(*) FROM ${q}.topic`), beforeReadonlyUiPost, 'denied UI write leaves the DB unchanged');

    const query = {read: 'Forum', select: ['id', 'title', 'category'], sort: [{field: 'id', dir: 'asc'}], limit: 100};
    const memberRows = (await member.read(query)).rows;
    const guestRows = (await guest.read(query)).rows;
    assert.deepEqual(memberRows.map(row => row.title), ['Private fixture']);
    assert.deepEqual(guestRows.map(row => row.title), ['Public forum']);
    const topicQuery = {read: 'Topic', select: ['id', 'title', 'forum', 'published'], sort: [{field: 'id', dir: 'asc'}], limit: 100};
    const postQuery = {read: 'Post', select: ['id', 'topic', 'username', 'content', 'date_created', 'date_modified', 'modified_by', 'version'], sort: [{field: 'id', dir: 'asc'}], limit: 100};
    assert.deepEqual((await member.read(topicQuery)).rows.map(row => row.title), ['Private topic']);
    assert.deepEqual((await guest.read(topicQuery)).rows.map(row => row.title), ['Public topic']);
    assert.deepEqual((await admin.read(topicQuery)).rows.map(row => row.title), ['Private topic', 'Hidden topic']);
    assert.deepEqual((await member.read(postQuery)).rows.map(row => row.content), ['Private post']);
    assert.deepEqual((await guest.read(postQuery)).rows.map(row => row.content), ['Public post']);
    assert.deepEqual((await admin.read(postQuery)).rows.map(row => row.content), ['Private post', 'Hidden post']);
    assert.equal(sql(`SELECT count(*) FROM ${q}.forum_group_access`), '4');
    assert.deepEqual((await readOnly.read(query)).rows.map(row => row.title), ['Private fixture']);
    const topicId = randomUUID();
    const firstPostId = randomUUID();
    const topicCreate = {create: 'Topic', values: {id: topicId, forum: '2', title: 'SDK-created topic', user: '2', username: 'member-fixture', locked: false, important: false, views: 0, published: false, deleted: false, hidden: false, hidden_at: null, hidden_by: null}};
    const postCreate = {create: 'Post', values: {id: firstPostId, topic: topicId, forum: '2', user: '2', username: 'member-fixture', content: 'Initial body', hidden: false, published: false, version: 0, deleted: false}};
    const rejectedTopicId = randomUUID();
    const beforeDeniedCount = sql(`SELECT count(*) FROM ${q}.topic`);
    const denied = await readOnly.bundle({atomic: [{create: 'Topic', values: {...topicCreate.values, id: rejectedTopicId, user: '4', username: 'readonly-fixture'}}]}, {key: `deny-topic-${rejectedTopicId}`});
    assert.equal(denied.code, 'WIRE.FORBIDDEN', JSON.stringify(denied));
    assert.equal(sql(`SELECT count(*) FROM ${q}.topic`), beforeDeniedCount, 'denied creation leaves all topic rows unchanged');
    const rollbackTopicId = randomUUID();
    const rollbackPostId = randomUUID();
    const workflow = (topic, post) => [
      {...topicCreate, values: {...topicCreate.values, id: topic}},
      {...postCreate, values: {...postCreate.values, id: post, topic}},
      {apply: 'Post.publish', target: {ids: [post]}},
      {create: 'TopicFirstPost', values: {topic, post}},
      {apply: 'Topic.publish', target: {ids: [topic]}},
    ];
    const beforeRollback = [sql(`SELECT count(*) FROM ${q}.topic`), sql(`SELECT count(*) FROM ${q}.post`)];
    const rolledBack = await member.bundle({atomic: [...workflow(rollbackTopicId, rollbackPostId).slice(0, 3), {apply: 'Topic.publish', target: {ids: [randomUUID()]}}]}, {key: `rollback-topic-${rollbackTopicId}`});
    assert.equal(rolledBack.code, 'WIRE.MISSING_TARGET', JSON.stringify(rolledBack));
    assert.deepEqual([sql(`SELECT count(*) FROM ${q}.topic`), sql(`SELECT count(*) FROM ${q}.post`)], beforeRollback, 'late failure rolls back both new rows');
    const successfulWorkflow = workflow(topicId, firstPostId);
    const workflowKey = `create-topic-${topicId}`;
    const createdTopic = await member.bundle({atomic: successfulWorkflow}, {key: workflowKey});
    assert.equal(createdTopic.ok, true, JSON.stringify(createdTopic));
    assert.deepEqual(createdTopic.created.slice(0, 2).map(row => [row.resource, row.id]), [['Topic', topicId], ['Post', firstPostId]]);
    assert.equal(createdTopic.created[2].resource, 'TopicFirstPost');
    const replay = await member.bundle({atomic: successfulWorkflow}, {key: workflowKey});
    assert.equal(replay.replayed, true, JSON.stringify(replay));
    assert.deepEqual(replay.created, createdTopic.created);
    assert.equal(sql(`SELECT count(*) FROM ${q}.topic WHERE id='${topicId}' AND published AND last_updated IS NOT NULL`), '1');
    assert.equal(sql(`SELECT count(*) FROM ${q}.post WHERE id='${firstPostId}' AND topic_id='${topicId}' AND content='Initial body'`), '1');
    assert.equal(sql(`SELECT count(*) FROM ${q}.topic WHERE forum_id=2 AND published AND hidden=false`), '2');
    assert.equal(sql(`SELECT count(*) FROM ${q}.post WHERE topic_id='${topicId}' AND published AND date_created IS NOT NULL`), '1');
    const trackingKey = `track-${topicId}`;
    const tracking = await member.bundle({atomic: [{create: 'TopicTracker', values: {user: '2', topic: topicId, active: true}}]}, {key: trackingKey});
    assert.equal(tracking.ok, true, JSON.stringify(tracking));
    const trackerId = tracking.created[0].id;
    assert.deepEqual((await member.read({read: 'TopicTracker', select: ['id', 'user', 'topic', 'active'], filter: [{field: 'topic', op: 'eq', value: topicId}], limit: 10})).rows.map(row => [row.user, row.topic, row.active]), [['2', topicId, true]]);
    const deniedTracker = await readOnly.bundle({atomic: [{create: 'TopicTracker', values: {user: '2', topic: topicId, active: true}}]}, {key: `deny-track-${topicId}`});
    assert.equal(deniedTracker.code, 'WIRE.FORBIDDEN', JSON.stringify(deniedTracker));
    const untracked = await member.bundle({atomic: [{apply: 'TopicTracker.untrack', target: {ids: [trackerId]}}]}, {key: `untrack-${trackerId}`});
    assert.equal(untracked.ok, true, JSON.stringify(untracked));
    assert.equal((await member.read({read: 'TopicTracker', select: ['id'], filter: [{field: 'topic', op: 'eq', value: topicId}, {field: 'active', op: 'eq', value: true}], limit: 10})).rows.length, 0, 'untracked topics are absent from the active tracker list');
    const retracked = await member.bundle({atomic: [{apply: 'TopicTracker.track', target: {ids: [trackerId]}}]}, {key: `retrack-${trackerId}`});
    assert.equal(retracked.ok, true, JSON.stringify(retracked));
    assert.equal((await member.read({read: 'TopicTracker', select: ['active'], filter: [{field: 'topic', op: 'eq', value: topicId}], limit: 10})).rows[0].active, true);
    const report = await member.bundle({atomic: [{create: 'Report', values: {reporter: '2', post: firstPostId, reason: 'Fixture report reason'}}]}, {key: `report-${firstPostId}`});
    assert.equal(report.ok, true, JSON.stringify(report));
    const reportId = report.created[0].id;
    const submitted = await member.bundle({atomic: [{apply: 'Report.submit', target: {ids: [reportId]}}]}, {key: `submit-report-${reportId}`});
    assert.equal(submitted.ok, true, JSON.stringify(submitted));
    assert.equal((await member.read({read: 'Report', select: ['id'], filter: [{field: 'id', op: 'eq', value: reportId}], limit: 10})).rows.length, 0, 'reporter cannot inspect the moderator queue');
    const reported = (await admin.read({read: 'Report', select: ['id', 'reporter', 'post', 'reason', 'reported', 'zapped', 'zapped_by'], filter: [{field: 'id', op: 'eq', value: reportId}], limit: 1})).rows[0];
    assert.deepEqual([reported.reporter, reported.post, reported.reason, reported.zapped, reported.zapped_by], ['2', firstPostId, 'Fixture report reason', null, null]);
    assert.ok(Date.parse(reported.reported), 'submission timestamp is assigned by the AIP server');
    const deniedResolve = await member.bundle({atomic: [{apply: 'Report.resolve', target: {ids: [reportId]}}]}, {key: `deny-resolve-${reportId}`});
    assert.equal(deniedResolve.code, 'WIRE.MISSING_TARGET', JSON.stringify(deniedResolve), 'reporter cannot address moderator-only reports');
    const resolved = await admin.bundle({atomic: [{apply: 'Report.resolve', target: {ids: [reportId]}}]}, {key: `resolve-${reportId}`});
    assert.equal(resolved.ok, true, JSON.stringify(resolved));
    assert.deepEqual((await admin.read({read: 'Report', select: ['zapped', 'zapped_by'], filter: [{field: 'id', op: 'eq', value: reportId}], limit: 1})).rows.map(row => [Boolean(row.zapped), row.zapped_by]), [[true, '3']]);
    const locked = await admin.bundle({atomic: [{apply: 'Topic.lock', target: {ids: [topicId]}}]}, {key: `lock-${topicId}`});
    assert.equal(locked.ok, true, JSON.stringify(locked));
    assert.equal((await member.read({read: 'Topic', select: ['id', 'locked'], filter: [{field: 'id', op: 'eq', value: topicId}], limit: 1})).rows[0].locked, true);
    const lockedReply = await member.bundle({atomic: [{create: 'Post', values: {id: randomUUID(), topic: topicId, forum: '2', user: '2', username: 'member-fixture', content: 'blocked by topic lock', hidden: false, published: false, version: 0}}]}, {key: `locked-reply-${topicId}`});
    assert.equal(lockedReply.code, 'WIRE.FORBIDDEN', JSON.stringify(lockedReply));
    const highlighted = await admin.bundle({atomic: [{apply: 'Topic.highlight', target: {ids: [topicId]}}]}, {key: `highlight-${topicId}`});
    assert.equal(highlighted.ok, true, JSON.stringify(highlighted));
    const unlocked = await admin.bundle({atomic: [{apply: 'Topic.unlock', target: {ids: [topicId]}}]}, {key: `unlock-${topicId}`});
    assert.equal(unlocked.ok, true, JSON.stringify(unlocked));
    const trivialized = await admin.bundle({atomic: [{apply: 'Topic.trivialize', target: {ids: [topicId]}}]}, {key: `trivialize-${topicId}`});
    assert.equal(trivialized.ok, true, JSON.stringify(trivialized));
    const initialTopic = (await member.read({read: 'Topic', select: ['id', 'last_updated'], filter: [{field: 'id', op: 'eq', value: topicId}], limit: 1})).rows[0];
    const replyId = randomUUID();
    const replyWorkflow = {atomic: [
      {create: 'Post', values: {id: replyId, topic: topicId, forum: '2', user: '2', username: 'member-fixture', content: 'Reply body', hidden: false, published: false, version: 0, deleted: false}},
      {apply: 'Post.publish', target: {ids: [replyId]}},
      {apply: 'Topic.bump', target: {ids: [topicId]}},
    ]};
    const reply = await member.bundle(replyWorkflow, {key: `reply-${replyId}`});
    assert.equal(reply.ok, true, JSON.stringify(reply));
    const beforeDeniedReply = sql(`SELECT count(*) FROM ${q}.post`);
    const deniedReply = await readOnly.bundle({atomic: [{create: 'Post', values: {id: randomUUID(), topic: topicId, forum: '2', user: '4', username: 'readonly-fixture', content: 'forbidden', hidden: false, published: false, version: 0, deleted: false}}]}, {key: `deny-reply-${topicId}`});
    assert.equal(deniedReply.code, 'WIRE.FORBIDDEN', JSON.stringify(deniedReply));
    assert.equal(sql(`SELECT count(*) FROM ${q}.post`), beforeDeniedReply);
    const afterReplyTopic = (await member.read({read: 'Topic', select: ['id', 'last_updated'], filter: [{field: 'id', op: 'eq', value: topicId}], limit: 1})).rows[0];
    assert.ok(Date.parse(afterReplyTopic.last_updated) > Date.parse(initialTopic.last_updated));
    const edit = await member.bundle({atomic: [{apply: 'Post.edit', target: {ids: [replyId]}, values: {content: 'Reply edited'}, expect: {version: 0}}]}, {key: `edit-${replyId}-v0`});
    assert.equal(edit.ok, true, JSON.stringify(edit));
    const editedPost = (await member.read({read: 'Post', select: ['id', 'content', 'date_modified', 'modified_by', 'version'], filter: [{field: 'id', op: 'eq', value: replyId}], limit: 1})).rows[0];
    assert.deepEqual([editedPost.content, editedPost.modified_by, editedPost.version], ['Reply edited', 'member-fixture', 1]);
    assert.ok(editedPost.date_modified);
    const deniedEdit = await readOnly.bundle({atomic: [{apply: 'Post.edit', target: {ids: [replyId]}, values: {content: 'unauthorized edit'}, expect: {version: 0}}]}, {key: `deny-edit-${replyId}`});
    assert.equal(deniedEdit.code, 'WIRE.FORBIDDEN', JSON.stringify(deniedEdit));
    const conflicted = await member.bundle({atomic: [{apply: 'Post.edit', target: {ids: [replyId]}, values: {content: 'stale edit'}, expect: {version: 0}}]}, {key: `edit-${replyId}-stale`});
    assert.equal(conflicted.code, 'WIRE.CONFLICT', JSON.stringify(conflicted));
    assert.equal((await member.read({read: 'Post', select: ['id', 'content', 'version'], filter: [{field: 'id', op: 'eq', value: replyId}], limit: 1})).rows[0].content, 'Reply edited');
    const memberForum = (await member.read({read: 'Forum', select: ['id', 'title', 'topic_count', 'post_count'], filter: [{field: 'id', op: 'eq', value: '2'}], limit: 1})).rows[0];
    assert.deepEqual([memberForum.topic_count, memberForum.post_count], [2, 3]);
    const guestForum = (await guest.read({read: 'Forum', select: ['id', 'title', 'topic_count', 'post_count'], filter: [{field: 'id', op: 'eq', value: '1'}], limit: 1})).rows[0];
    assert.deepEqual([guestForum.topic_count, guestForum.post_count], [1, 1]);
    const memberStats = (await member.read({read: 'User', select: ['id', 'post_count'], filter: [{field: 'id', op: 'eq', value: '2'}], limit: 1})).rows[0];
    assert.equal(memberStats.post_count, 3);
    const firstPostHide = await admin.bundle({atomic: [{apply: 'Post.hide', target: {ids: [firstPostId]}}]}, {key: `hide-first-post-${firstPostId}`});
    assert.equal(firstPostHide.code, 'WIRE.FORBIDDEN', 'first-post hide must use the topic-level operation');
    const firstPostDelete = await member.bundle({atomic: [{apply: 'Post.delete', target: {ids: [firstPostId]}}]}, {key: `delete-first-post-${firstPostId}`});
    assert.equal(firstPostDelete.code, 'WIRE.FORBIDDEN', 'first-post delete must use the topic-level operation');
    const topicHide = await admin.bundle({atomic: [{apply: 'Topic.hide', target: {ids: [topicId]}}]}, {key: `hide-topic-${topicId}`});
    assert.equal(topicHide.ok, true, JSON.stringify(topicHide));
    assert.equal((await member.read({read: 'Topic', select: ['id'], filter: [{field: 'id', op: 'eq', value: topicId}], limit: 1})).rows.length, 0);
    assert.equal((await member.read({read: 'Post', select: ['id'], filter: [{field: 'id', op: 'eq', value: replyId}], limit: 1})).rows.length, 0);
    const hiddenCounts = (await member.read({read: 'Forum', select: ['topic_count', 'post_count'], filter: [{field: 'id', op: 'eq', value: '2'}], limit: 1})).rows[0];
    assert.deepEqual([hiddenCounts.topic_count, hiddenCounts.post_count], [1, 1], 'hidden topic and posts do not leak through aggregates');
    const topicUnhide = await admin.bundle({atomic: [{apply: 'Topic.unhide', target: {ids: [topicId]}}]}, {key: `unhide-topic-${topicId}`});
    assert.equal(topicUnhide.ok, true, JSON.stringify(topicUnhide));
    const replyHide = await admin.bundle({atomic: [{apply: 'Post.hide', target: {ids: [replyId]}}]}, {key: `hide-post-${replyId}`});
    assert.equal(replyHide.ok, true, JSON.stringify(replyHide));
    assert.equal((await member.read({read: 'Post', select: ['id'], filter: [{field: 'id', op: 'eq', value: replyId}], limit: 1})).rows.length, 0);
    const replyUnhide = await admin.bundle({atomic: [{apply: 'Post.unhide', target: {ids: [replyId]}}]}, {key: `unhide-post-${replyId}`});
    assert.equal(replyUnhide.ok, true, JSON.stringify(replyUnhide));
    assert.equal((await member.read({read: 'Topic', select: ['id', 'locked'], filter: [{field: 'id', op: 'eq', value: topicId}], limit: 1})).rows[0].locked, false);
    const replyDelete = await member.bundle({atomic: [{apply: 'Post.delete', target: {ids: [replyId]}}]}, {key: `delete-post-${replyId}`});
    assert.equal(replyDelete.ok, true, JSON.stringify(replyDelete));
    assert.equal((await member.read({read: 'Post', select: ['id'], filter: [{field: 'id', op: 'eq', value: replyId}], limit: 1})).rows.length, 0);
    assert.equal(sql(`SELECT deleted FROM ${q}.post WHERE id='${replyId}'`), 't', 'delete is a retained tombstone in the AIP model');
    const readonlyDelete = await readOnly.bundle({atomic: [{apply: 'Topic.delete', target: {ids: [topicId]}}]}, {key: `deny-delete-topic-${topicId}`});
    assert.equal(readonlyDelete.code, 'WIRE.FORBIDDEN', JSON.stringify(readonlyDelete));
    const topicDelete = await member.bundle({atomic: [
      {apply: 'Topic.delete', target: {ids: [topicId]}},
      {apply: 'Post.delete', target: {ids: [firstPostId]}},
    ]}, {key: `delete-topic-${topicId}`});
    assert.equal(topicDelete.ok, true, JSON.stringify(topicDelete));
    assert.equal(sql(`SELECT deleted FROM ${q}.topic WHERE id='${topicId}'`), 't');
    assert.equal(sql(`SELECT count(*) FROM ${q}.post WHERE topic_id='${topicId}' AND deleted`), '2', 'topic deletion tombstones its complete post set');
    assert.equal((await member.read({read: 'Topic', select: ['id'], filter: [{field: 'id', op: 'eq', value: topicId}], limit: 1})).rows.length, 0);
    assert.equal((await member.read({read: 'Post', select: ['id'], filter: [{field: 'id', op: 'eq', value: firstPostId}], limit: 1})).rows.length, 0);
    const countsAfterDelete = (await member.read({read: 'Forum', select: ['topic_count', 'post_count'], filter: [{field: 'id', op: 'eq', value: '2'}], limit: 1})).rows[0];
    assert.deepEqual([countsAfterDelete.topic_count, countsAfterDelete.post_count], [1, 1], 'deleted topic cascade is absent from member reads and aggregates');
    const memberUi = await launchUi('member-fixture');
    const memberForumResponse = await fetch(`${memberUi.base}/forum/2`, {headers: {Cookie: cookie}});
    assert.equal(memberForumResponse.status, 200);
    const memberForumHtml = await memberForumResponse.text();
    assert.match(memberForumHtml, /Start a topic/, 'authorized member sees the original topic form');
    assert.match(memberForumHtml, /maxlength="255"[^>]*required/, 'AIP title cap and required-field constraints are rendered');
    const uiTopicTitle = `UI integration ${randomUUID()}`;
    const uiTopicBody = 'Created through the Flask renderer and AIP SDK';
    const uiTopicResponse = await fetch(`${memberUi.base}/forum/2/topic`, {
      method: 'POST', redirect: 'manual', headers: {Cookie: cookie, 'Content-Type': 'application/x-www-form-urlencoded'},
      body: new URLSearchParams({csrf_token: csrf, title: uiTopicTitle, content: uiTopicBody})
    });
    assert.equal(uiTopicResponse.status, 303, `UI topic create response: ${uiTopicResponse.status}`);
    const uiTopicPath = uiTopicResponse.headers.get('location');
    const uiTopicId = uiTopicPath?.split('/').at(-1);
    assert.ok(uiTopicId);
    assert.equal(sql(`SELECT count(*) FROM ${q}.topic WHERE id='${uiTopicId}' AND title='${uiTopicTitle}' AND published`), '1');
    assert.equal(sql(`SELECT count(*) FROM ${q}.post WHERE topic_id='${uiTopicId}' AND content='${uiTopicBody}' AND published`), '1');
    const memberTopicHtml = await (await fetch(`${memberUi.base}${uiTopicPath}`, {headers: {Cookie: cookie}})).text();
    assert.match(memberTopicHtml, /Reply/);
    const uiReplyResponse = await fetch(`${memberUi.base}/topic/${uiTopicId}/reply`, {
      method: 'POST', redirect: 'manual', headers: {Cookie: cookie, 'Content-Type': 'application/x-www-form-urlencoded'},
      body: new URLSearchParams({csrf_token: csrf, content: 'Reply through AIP SDK'})
    });
    assert.equal(uiReplyResponse.status, 303, `UI reply response: ${uiReplyResponse.status}`);
    assert.equal(sql(`SELECT count(*) FROM ${q}.post WHERE topic_id='${uiTopicId}' AND content='Reply through AIP SDK' AND published`), '1');
    const uiTopicRows = (await member.read({read: 'Topic', select: ['id', 'title', 'published'], filter: [{field: 'id', op: 'eq', value: uiTopicId}], limit: 1})).rows;
    assert.deepEqual(uiTopicRows.map(row => [row.title, row.published]), [[uiTopicTitle, true]], 'official SDK sees UI-created topic');
    console.log(JSON.stringify({schema, ...init, memberVisible: memberRows.map(row => row.title), guestVisible: guestRows.map(row => row.title), duplicateLinkRejected: true, uiTopicAndReplyPersisted: true, readonlyUiWriteDenied: true}));
  } finally {
    for (const uiHost of uiHosts) {
      if (uiHost.exitCode === null) {
        uiHost.kill('SIGTERM');
        await Promise.race([once(uiHost, 'exit'), new Promise(resolve => setTimeout(resolve, 3000))]);
      }
    }
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
