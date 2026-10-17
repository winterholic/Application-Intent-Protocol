import assert from 'node:assert/strict';
import {test} from 'node:test';
import {generateKeyPairSync, sign, randomUUID} from 'node:crypto';
import {mkdtempSync, readFileSync, writeFileSync, rmSync} from 'node:fs';
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

test('FlaskBB ported features: search, profile, conversations, tracker, admin CRUD in real PostgreSQL', {timeout: 60000}, async () => {
  const temp = mkdtempSync(join(tmpdir(), 'aip-flaskbb-feat-'));
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
    const jwk = {...publicKey.export({format: 'jwk'}), kid: 'flaskbb-feat-fixture', alg: 'RS256', use: 'sig', key_ops: ['verify']};
    writeFileSync(jwksPath, JSON.stringify({keys: [jwk]}));
    writeFileSync(config, JSON.stringify({
      schema,
      database_url_env: 'AIP_FLASKBB_DATABASE',
      migrate_database_url_env: 'AIP_FLASKBB_DATABASE',
      listen: '127.0.0.1:0',
      wire: 'decimal',
      allowed_origins: ['http://127.0.0.1:58080'],
      auth: {
        issuer: 'https://flaskbb.test',
        audience: 'aip-flaskbb',
        jwks: {kind: 'file', path: jwksPath},
      },
    }));

    const init = JSON.parse(run(binary, ['service', 'init', file, '--config', config]));
    assert.equal(init.ok, true);
    run(process.execPath, ['product/tools/build-sdk.mjs', '--out', sdkRoot]);

    const q = `"${schema}"`;
    sql(`INSERT INTO ${q}."group" (id,name,guest,admin,super_mod,mod,banned,editpost,deletepost,deletetopic,posttopic,postreply,viewhidden,makehidden,mod_edituser,mod_banuser,description) VALUES
      (1,'Guest',true,false,false,false,false,false,false,false,false,false,false,false,false,false,NULL),
      (2,'Member',false,false,false,false,false,true,false,false,true,true,false,false,false,false,NULL),
      (3,'Administrator',false,true,false,false,false,true,true,true,true,true,true,true,true,true,NULL);`);

    sql(`INSERT INTO ${q}.user (id,username,email,date_joined,lastseen,birthday,gender,website,location,signature,avatar,notes,primary_group_id,activated,language,theme) VALUES
      (1,'guest-fixture','guest@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,1,true,'en',NULL),
      (2,'member-alice','alice@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,2,true,'en',NULL),
      (3,'admin-bob','bob@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,3,true,'en',NULL);`);

    sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General Category','Test category',1);`);
    sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,external,show_moderators,last_post_id,last_post_user_id,last_post_title,last_post_username,last_post_created) VALUES
      (1,1,'Public Forum','Visible to all',1,false,NULL,false,NULL,NULL,NULL,NULL,NULL);`);

    sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,1,2),(3,1,3);`);

    sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES
      ('00000000-0000-4000-8000-000000000101',1,'First Discussion Topic',2,'member-alice',now(),now(),false,false,5,true,false,false);`);

    sql(`INSERT INTO ${q}.post (id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted) VALUES
      ('00000000-0000-4000-8000-000000000201','00000000-0000-4000-8000-000000000101',1,2,'member-alice','Hello world! First post content',now(),false,true,0,false);`);

    sql(`SELECT setval(pg_get_serial_sequence('${q}.category', 'id'), 100);`);
    sql(`SELECT setval(pg_get_serial_sequence('${q}.forum', 'id'), 100);`);
    sql(`SELECT setval(pg_get_serial_sequence('${q}.user', 'id'), 100);`);

    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'alice-sub', '--actor', '2']);
    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'bob-sub', '--actor', '3']);

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

    const alice = connect(`http://${started.address}`, token('alice-sub'), contract);
    const bob = connect(`http://${started.address}`, token('bob-sub'), contract);

    // 1. F07 / F15 검색 (Search) 검증
    const topicSearch = (await alice.read({
      read: 'Topic',
      select: ['id', 'title'],
      filter: [{field: 'title', op: 'contains', value: 'Discussion'}],
    })).rows;
    assert.equal(topicSearch.length, 1);
    assert.equal(topicSearch[0].title, 'First Discussion Topic');

    const postSearch = (await alice.read({
      read: 'Post',
      select: ['id', 'content'],
      filter: [{field: 'content', op: 'contains', value: 'First post'}],
    })).rows;
    assert.equal(postSearch.length, 1);
    assert.equal(postSearch[0].content, 'Hello world! First post content');

    const userSearch = (await alice.read({
      read: 'User',
      select: ['id', 'username'],
      filter: [{field: 'username', op: 'contains', value: 'alice'}],
    })).rows;
    assert.equal(userSearch.length, 1);
    assert.equal(userSearch[0].username, 'member-alice');

    const forumSearch = (await alice.read({
      read: 'Forum',
      select: ['id', 'title'],
      filter: [{field: 'title', op: 'contains', value: 'Public'}],
    })).rows;
    assert.equal(forumSearch.length, 1);
    assert.equal(forumSearch[0].title, 'Public Forum');

    // 2. F03 프로필 수정 (Profile update) 검증
    const updateResult = await alice.bundle({
      atomic: [
        {
          apply: 'User.updateProfile',
          target: {ids: ['2']},
          values: {
            location: 'Seoul, Korea',
            signature: 'Alice In Wonderland',
            website: 'https://alice.example.com',
          },
        },
      ],
    }, {key: 'alice-profile-update'});
    assert.equal(updateResult.ok, true, JSON.stringify(updateResult));

    const aliceProfile = (await alice.read({
      read: 'User',
      select: ['id', 'username', 'location', 'signature', 'website'],
      filter: [{field: 'id', op: 'eq', value: '2'}],
    })).rows;
    assert.equal(aliceProfile[0].location, 'Seoul, Korea');
    assert.equal(aliceProfile[0].signature, 'Alice In Wonderland');
    assert.equal(aliceProfile[0].website, 'https://alice.example.com');

    // 타인이 alice의 프로필을 수정하려 하면 거부
    const bobMaliciousUpdate = await bob.bundle({
      atomic: [
        {
          apply: 'User.updateProfile',
          target: {ids: ['2']},
          values: {signature: 'Hacked by Bob'},
        },
      ],
    }, {key: 'bob-malicious-update'});
    assert.equal(bobMaliciousUpdate.ok, false);

    // 3. F09 쪽지 (Conversations & Messages) 검증
    const convSharedId = randomUUID();
    const nowIso = new Date().toISOString();
    const createConvResult = await alice.bundle({
      atomic: [
        {
          create: 'Conversation',
          values: {
            user: '2',
            from_user: '2',
            to_user: '3',
            shared_id: convSharedId,
            subject: 'Confidential inquiry',
            date_created: nowIso,
            date_modified: nowIso,
            trash: false,
            draft: false,
            unread: false,
          },
        },
        {
          create: 'Conversation',
          values: {
            user: '3',
            from_user: '2',
            to_user: '3',
            shared_id: convSharedId,
            subject: 'Confidential inquiry',
            date_created: nowIso,
            date_modified: nowIso,
            trash: false,
            draft: false,
            unread: true,
          },
        },
      ],
    }, {key: `create-conv-${convSharedId}`});
    assert.equal(createConvResult.ok, true);

    // Bob(수신자) 쪽지함 확인
    const bobConvs = (await bob.read({
      read: 'Conversation',
      select: ['id', 'subject', 'unread', 'trash'],
      filter: [{field: 'trash', op: 'eq', value: false}],
    })).rows;
    assert.equal(bobConvs.length, 1);
    assert.equal(bobConvs[0].subject, 'Confidential inquiry');
    assert.equal(bobConvs[0].unread, true);

    const bobConvId = bobConvs[0].id;

    // Bob이 Message 생성 (답장)
    const msgResult = await bob.bundle({
      atomic: [
        {
          create: 'Message',
          values: {
            conversation: bobConvId,
            user: '3',
            message: 'Received your inquiry. Everything looks good.',
            date_created: new Date().toISOString(),
          },
        },
        {
          apply: 'Conversation.markRead',
          target: {ids: [bobConvId]},
        },
      ],
    }, {key: `bob-reply-${bobConvId}`});
    assert.equal(msgResult.ok, true);

    // Bob의 대화 읽음 확인
    const bobConvAfterRead = (await bob.read({
      read: 'Conversation',
      select: ['id', 'unread'],
      filter: [{field: 'id', op: 'eq', value: bobConvId}],
    })).rows;
    assert.equal(bobConvAfterRead[0].unread, false);

    // Message 확인
    const messages = (await bob.read({
      read: 'Message',
      select: ['id', 'conversation', 'message'],
      filter: [{field: 'conversation', op: 'eq', value: bobConvId}],
    })).rows;
    assert.equal(messages.length, 1);
    assert.equal(messages[0].message, 'Received your inquiry. Everything looks good.');

    // 휴지통 이동 및 복원 검증
    const trashResult = await bob.bundle({
      atomic: [
        {
          apply: 'Conversation.moveToTrash',
          target: {ids: [bobConvId]},
        },
      ],
    }, {key: `bob-trash-${bobConvId}`});
    assert.equal(trashResult.ok, true);

    const bobTrashConvs = (await bob.read({
      read: 'Conversation',
      select: ['id', 'trash'],
      filter: [{field: 'trash', op: 'eq', value: true}],
    })).rows;
    assert.equal(bobTrashConvs.length, 1);

    const restoreResult = await bob.bundle({
      atomic: [
        {
          apply: 'Conversation.restoreFromTrash',
          target: {ids: [bobConvId]},
        },
      ],
    }, {key: `bob-restore-${bobConvId}`});
    assert.equal(restoreResult.ok, true);

    // 4. F06 Tracker & Read markers 검증
    const trackResult = await alice.bundle({
      atomic: [
        {
          create: 'TopicTracker',
          values: {
            id: '1',
            user: '2',
            topic: '00000000-0000-4000-8000-000000000101',
            active: true,
          },
        },
      ],
    }, {key: 'alice-track-1'});
    assert.equal(trackResult.ok, true);

    const untrackResult = await alice.bundle({
      atomic: [
        {
          apply: 'TopicTracker.untrack',
          target: {ids: ['1']},
        },
      ],
    }, {key: 'alice-untrack-1'});
    assert.equal(untrackResult.ok, true);

    const trackerRead = (await alice.read({
      read: 'TopicTracker',
      select: ['id', 'active'],
      filter: [{field: 'id', op: 'eq', value: '1'}],
    })).rows;
    assert.equal(trackerRead[0].active, false);

    // ForumRead 생성 검증
    const forumReadResult = await alice.bundle({
      atomic: [
        {
          create: 'ForumRead',
          values: {
            user: '2',
            forum: '1',
            last_read: new Date().toISOString(),
            cleared: null,
          },
        },
      ],
    }, {key: 'alice-forumread-1'});
    assert.equal(forumReadResult.ok, true);

    // 5. F04 / F10 관리자 CRUD 검증
    // Bob(Admin)이 새 Category 생성 및 수정
    const createCatResult = await bob.bundle({
      atomic: [
        {
          create: 'Category',
          values: {
            title: 'Admin Created Category',
            description: 'Created via admin bundle',
            position: 10,
          },
        },
      ],
    }, {key: 'admin-create-cat'});
    assert.equal(createCatResult.ok, true, JSON.stringify(createCatResult));

    const newCats = (await bob.read({
      read: 'Category',
      select: ['id', 'title'],
      filter: [{field: 'title', op: 'contains', value: 'Admin Created'}],
    })).rows;
    assert.equal(newCats.length, 1);
    const newCatId = newCats[0].id;

    const editCatResult = await bob.bundle({
      atomic: [
        {
          apply: 'Category.edit',
          target: {ids: [newCatId]},
          values: {title: 'Updated Admin Category'},
        },
      ],
    }, {key: 'admin-edit-cat'});
    assert.equal(editCatResult.ok, true, JSON.stringify(editCatResult));

    const updatedCat = (await bob.read({
      read: 'Category',
      select: ['id', 'title'],
      filter: [{field: 'id', op: 'eq', value: newCatId}],
    })).rows;
    assert.equal(updatedCat[0].title, 'Updated Admin Category');

    // 일반 회원 Alice가 Category를 생성하려 하면 거부
    const aliceForbiddenCat = await alice.bundle({
      atomic: [
        {
          create: 'Category',
          values: {
            title: 'Alice Unauthorized Category',
            description: 'Should fail',
            position: 99,
          },
        },
      ],
    }, {key: 'alice-unauthorized-cat'});
    assert.equal(aliceForbiddenCat.ok, false);

    // Bob(Admin)이 Alice의 계정 상태를 업데이트 (F10)
    const adminUserUpdate = await bob.bundle({
      atomic: [
        {
          apply: 'User.adminUpdateUser',
          target: {ids: ['2']},
          values: {activated: true},
        },
      ],
    }, {key: 'admin-update-alice'});
    assert.equal(adminUserUpdate.ok, true, JSON.stringify(adminUserUpdate));

    console.log(JSON.stringify({schema, ok: true, featuresVerified: ['F07_search', 'F03_profile', 'F09_conversations', 'F06_tracker_read', 'F04_F10_admin_crud']}));
  } finally {
    if (server) {
      server.kill();
      await once(server, 'close').catch(() => {});
    }
    sql(`DROP SCHEMA IF EXISTS "${schema}" CASCADE;`);
    rmSync(temp, {recursive: true, force: true});
  }
});
