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
const binary = process.env.AIP_BINARY || join(root, 'target/debug/aip');
const file = join(track, 'app.aip');
const b64 = value => Buffer.from(JSON.stringify(value)).toString('base64url');

test('FlaskBB ported features: search, profile, conversations, tracker, admin CRUD in real PostgreSQL', {timeout: 60000}, async () => {
  const temp = mkdtempSync(join(tmpdir(), 'aip-flaskbb-feat-'));
  const suffix = randomUUID().replaceAll('-', '').slice(0, 10);
  const schema = `aip_flaskbb_${suffix}`;
  const database = 'aip_flaskbb';
  const dbUrl = process.env.AIP_FLASKBB_TEST_DATABASE_URL || `postgresql://aip_flaskbb:aip_flaskbb_fixture_password@127.0.0.1:55439/${database}`;
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
      (3,'Administrator',false,true,false,false,false,true,true,true,true,true,true,true,true,true,NULL),
      (4,'Moderator',false,false,false,true,false,true,true,true,true,true,false,false,true,false,NULL),
      (5,'Super Moderator',false,false,true,false,false,true,true,true,true,true,true,true,true,false,NULL);`);

    sql(`INSERT INTO ${q}.user (id,username,email,date_joined,lastseen,birthday,gender,website,location,signature,avatar,notes,primary_group_id,activated,language,theme) VALUES
      (1,'guest-fixture','guest@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,1,true,'en',NULL),
      (2,'member-alice','alice@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,2,true,'en',NULL),
      (3,'admin-bob','bob@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,3,true,'en',NULL),
      (4,'mod-editor','editor@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,4,true,'en',NULL),
      (5,'super-mod','supermod@example.test',now(),now(),NULL,NULL,NULL,NULL,NULL,NULL,NULL,5,true,'en',NULL);`);

    sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General Category','Test category',1);`);
    sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,external,show_moderators,last_post_id,last_post_user_id,last_post_title,last_post_username,last_post_created) VALUES
      (1,1,'Public Forum','Visible to all',1,false,NULL,false,NULL,NULL,NULL,NULL,NULL);`);

    sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,1,2),(3,1,3),(4,1,4),(5,1,5);`);

    sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES
      ('00000000-0000-4000-8000-000000000101',1,'First Discussion Topic',2,'member-alice',now(),now(),false,false,5,true,false,false),
      ('00000000-0000-4000-8000-000000000102',1,'Secret Hidden Topic',3,'admin-bob',now(),now(),false,false,0,true,false,true);`);

    sql(`INSERT INTO ${q}.post (id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted) VALUES
      ('00000000-0000-4000-8000-000000000201','00000000-0000-4000-8000-000000000101',1,2,'member-alice','Hello world! First post content',now(),false,true,0,false),
      ('00000000-0000-4000-8000-000000000202','00000000-0000-4000-8000-000000000102',1,3,'admin-bob','Top secret hidden post content',now(),true,true,0,false);`);

    sql(`SELECT setval(pg_get_serial_sequence('${q}.category', 'id'), 100);`);
    sql(`SELECT setval(pg_get_serial_sequence('${q}.forum', 'id'), 100);`);
    sql(`SELECT setval(pg_get_serial_sequence('${q}.user', 'id'), 100);`);

    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'alice-sub', '--actor', '2']);
    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'bob-sub', '--actor', '3']);
    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'editor-sub', '--actor', '4']);
    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'supermod-sub', '--actor', '5']);

    const generated = JSON.parse(run(binary, ['service', 'gen', file, '--out', bindingPath]));
    const bindingSource = readFileSync(bindingPath, 'utf8');
    const descriptors = JSON.parse(bindingSource.match(/export const readDescriptors = (.*) as const;/)[1]);
    const identityKinds = JSON.parse(bindingSource.match(/identityKinds: (.*), fingerprint:/)[1]);
    const contract = {fingerprint: generated.fingerprint, idWire: 'decimal-string-v13', identityKinds, extensions: {}, readDescriptors: descriptors};

    server = spawn(binary, ['service', 'serve', file, '--config', config], {cwd: temp, env, stdio: ['ignore', 'pipe', 'pipe']});
    server.unref();
    let stderr = '';
    server.stderr.on('data', data => {stderr += data;});
    const lines = createInterface({input: server.stdout});
    const started = await Promise.race([
      once(lines, 'line').then(([line]) => JSON.parse(line)),
      once(server, 'exit').then(([code]) => {throw new Error(`serve exit ${code}: ${stderr}`);}),
      new Promise((_, reject) => setTimeout(() => reject(new Error(`serve timeout: ${stderr}`)), 15000).unref())
    ]);
    assert.equal(started.ok, true, JSON.stringify(started));
    lines.close();
    server.stdout.destroy();
    server.stderr.destroy();

    const {connect} = await import(pathToFileURL(join(sdkRoot, 'product/sdk/index.js')).href);
    const token = subject => {
      const now = Math.floor(Date.now() / 1000);
      const unsigned = `${b64({alg: 'RS256', typ: 'at+jwt', kid: jwk.kid})}.${b64({iss: 'https://flaskbb.test', aud: 'aip-flaskbb', sub: subject, iat: now, nbf: now, exp: now + 600})}`;
      return `${unsigned}.${sign('RSA-SHA256', Buffer.from(unsigned), privateKey).toString('base64url')}`;
    };

    const alice = connect(`http://${started.address}`, token('alice-sub'), contract);
    const bob = connect(`http://${started.address}`, token('bob-sub'), contract);
    const editor = connect(`http://${started.address}`, token('editor-sub'), contract);
    const supermod = connect(`http://${started.address}`, token('supermod-sub'), contract);

    // 1. F07 / F15 검색 (Search) 검증
    const topicSearch = (await alice.read({
      read: 'Topic',
      select: ['id', 'title'],
      filter: [{field: 'title', op: 'contains', value: 'Discussion'}],
    })).rows;
    assert.equal(topicSearch.length, 1);
    assert.equal(topicSearch[0].title, 'First Discussion Topic');

    // 숨김(hidden) 토픽이 일반 회원(alice)의 검색 결과에 노출되지 않음 검증 (음수 반례)
    const hiddenTopicSearchAlice = (await alice.read({
      read: 'Topic',
      select: ['id', 'title'],
      filter: [{field: 'title', op: 'contains', value: 'Secret Hidden'}],
    })).rows;
    assert.equal(hiddenTopicSearchAlice.length, 0);

    // 권한이 있는 관리자(bob)는 숨김 토픽 검색 가능
    const hiddenTopicSearchBob = (await bob.read({
      read: 'Topic',
      select: ['id', 'title'],
      filter: [{field: 'title', op: 'contains', value: 'Secret Hidden'}],
    })).rows;
    assert.equal(hiddenTopicSearchBob.length, 1);

    const postSearch = (await alice.read({
      read: 'Post',
      select: ['id', 'content'],
      filter: [{field: 'content', op: 'contains', value: 'First post'}],
    })).rows;
    assert.equal(postSearch.length, 1);
    assert.equal(postSearch[0].content, 'Hello world! First post content');

    // 숨김(hidden) 게시물이 일반 회원(alice)의 검색 결과에 노출되지 않음 검증 (음수 반례)
    const hiddenPostSearchAlice = (await alice.read({
      read: 'Post',
      select: ['id', 'content'],
      filter: [{field: 'content', op: 'contains', value: 'secret hidden'}],
    })).rows;
    assert.equal(hiddenPostSearchAlice.length, 0);

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

    const aliceConvId = createConvResult.created[0].id;
    const bobConvId = createConvResult.created[1].id;

    // 대화 생성 시 발신자와 수신자 양쪽 대화함에 첫 Message 생성
    const firstMsgResult = await alice.bundle({
      atomic: [
        {
          create: 'Message',
          values: {
            conversation: aliceConvId,
            user: '2',
            message: 'First confidential message from Alice',
            date_created: nowIso,
          },
        },
        {
          create: 'Message',
          values: {
            conversation: bobConvId,
            user: '2',
            message: 'First confidential message from Alice',
            date_created: nowIso,
          },
        },
      ],
    }, {key: `alice-first-msg-${convSharedId}`});
    assert.equal(firstMsgResult.ok, true);

    // Bob(수신자) 쪽지함 확인: unread: true 및 첫 메시지 조회 확인
    const bobConvs = (await bob.read({
      read: 'Conversation',
      select: ['id', 'subject', 'unread', 'trash'],
      filter: [
        {field: 'user', op: 'eq', value: '3'},
        {field: 'trash', op: 'eq', value: false},
      ],
    })).rows;
    assert.equal(bobConvs.length, 1);
    assert.equal(bobConvs[0].subject, 'Confidential inquiry');
    assert.equal(bobConvs[0].unread, true);

    const bobFirstMsgs = (await bob.read({
      read: 'Message',
      select: ['id', 'conversation', 'message'],
      filter: [{field: 'conversation', op: 'eq', value: bobConvId}],
    })).rows;
    assert.equal(bobFirstMsgs.length, 1);
    assert.equal(bobFirstMsgs[0].message, 'First confidential message from Alice');

    // Alice(발신자) 쪽지함 확인: unread: false 및 첫 메시지 조회 확인
    const aliceConvs = (await alice.read({
      read: 'Conversation',
      select: ['id', 'subject', 'unread', 'trash'],
      filter: [
        {field: 'user', op: 'eq', value: '2'},
        {field: 'trash', op: 'eq', value: false},
      ],
    })).rows;
    assert.equal(aliceConvs.length, 1);
    assert.equal(aliceConvs[0].unread, false);

    const aliceFirstMsgs = (await alice.read({
      read: 'Message',
      select: ['id', 'conversation', 'message'],
      filter: [{field: 'conversation', op: 'eq', value: aliceConvId}],
    })).rows;
    assert.equal(aliceFirstMsgs.length, 1);
    assert.equal(aliceFirstMsgs[0].message, 'First confidential message from Alice');

    // Bob이 답장 작성: 양쪽 대화에 Message 생성 + Alice 대화 bump + Bob 대화 markRead
    const replyResult = await bob.bundle({
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
          create: 'Message',
          values: {
            conversation: aliceConvId,
            user: '3',
            message: 'Received your inquiry. Everything looks good.',
            date_created: new Date().toISOString(),
          },
        },
        {
          apply: 'Conversation.markRead',
          target: {ids: [bobConvId]},
        },
        {
          apply: 'Conversation.bump',
          target: {ids: [aliceConvId]},
        },
      ],
    }, {key: `bob-reply-${bobConvId}`});
    assert.equal(replyResult.ok, true);

    // Bob의 대화 읽음 확인 (unread: false, 메시지 2건)
    const bobConvAfterRead = (await bob.read({
      read: 'Conversation',
      select: ['id', 'unread'],
      filter: [{field: 'id', op: 'eq', value: bobConvId}],
    })).rows;
    assert.equal(bobConvAfterRead[0].unread, false);

    const bobAllMessages = (await bob.read({
      read: 'Message',
      select: ['id', 'conversation', 'message'],
      filter: [{field: 'conversation', op: 'eq', value: bobConvId}],
    })).rows;
    assert.equal(bobAllMessages.length, 2);
    assert.equal(bobAllMessages[1].message, 'Received your inquiry. Everything looks good.');

    // Alice의 대화함 확인: bump되어 unread: true 로 전환, 메시지 2건 확인
    await alice.replaceSession(token('alice-sub'));
    const aliceConvAfterReply = (await alice.read({
      read: 'Conversation',
      select: ['id', 'unread'],
      filter: [{field: 'id', op: 'eq', value: aliceConvId}],
    })).rows;
    assert.equal(aliceConvAfterReply[0].unread, true);

    const aliceAllMessages = (await alice.read({
      read: 'Message',
      select: ['id', 'conversation', 'message'],
      filter: [{field: 'conversation', op: 'eq', value: aliceConvId}],
    })).rows;
    assert.equal(aliceAllMessages.length, 2);
    assert.equal(aliceAllMessages[1].message, 'Received your inquiry. Everything looks good.');

    // 비참여자(editor)가 Alice의 대화함에 메시지 주입 시도 차단 (음수 반례)
    const hackerInjectedMessage = await editor.bundle({
      atomic: [
        {
          create: 'Message',
          values: {
            conversation: aliceConvId,
            user: '4',
            message: 'Malicious injected message by editor',
            date_created: new Date().toISOString(),
          },
        },
      ],
    }, {key: 'editor-inject-msg'});
    assert.equal(hackerInjectedMessage.ok, false);

    // Alice 대화함 메시지는 여전히 2건 유지 (불변)
    const aliceMsgCountAfterAttack = (await alice.read({
      read: 'Message',
      select: ['id'],
      filter: [{field: 'conversation', op: 'eq', value: aliceConvId}],
    })).rows;
    assert.equal(aliceMsgCountAfterAttack.length, 2);

    // 개인별 휴지통 이동 및 격리 검증 (Bob이 휴지통에 넣어도 Alice는 영향 없음)
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
      filter: [
        {field: 'user', op: 'eq', value: '3'},
        {field: 'trash', op: 'eq', value: true},
      ],
    })).rows;
    assert.equal(bobTrashConvs.length, 1);

    const aliceConvsWhileBobTrash = (await alice.read({
      read: 'Conversation',
      select: ['id', 'trash'],
      filter: [
        {field: 'user', op: 'eq', value: '2'},
        {field: 'trash', op: 'eq', value: false},
      ],
    })).rows;
    assert.equal(aliceConvsWhileBobTrash.length, 1); // Alice의 대화함은 trash 되지 않음 (격리!)

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

    // =========================================================================
    // 6. 권한 상승(Privilege Escalation) 방지 검증:
    //    mod_edituser 권한을 가진 editor가 일반 사용자(alice)를 Administrator(group 3)로 승격 시도 시 거부
    // =========================================================================
    assert.equal(sql(`SELECT mod_edituser FROM ${q}."group" WHERE id=4`), 't', 'editor fixture must actually have mod_edituser');
    const editorAllowedUserEdit = await editor.bundle({
      atomic: [{apply: 'User.moderatorEditUser', target: {ids: ['2']}, values: {notes: 'Edited by moderator'}}],
    }, {key: 'editor-ordinary-user-edit'});
    assert.equal(editorAllowedUserEdit.ok, true, 'mod_edituser must retain its ordinary-user edit permission');
    assert.equal(sql(`SELECT notes FROM ${q}."user" WHERE id=2`), 'Edited by moderator');

    const editorPromoAttempt = await editor.bundle({
      atomic: [
        {
          apply: 'User.adminUpdateUser',
          target: {ids: ['2']},
          values: {primary_group: '3'},
        },
      ],
    }, {key: 'editor-promo-attempt'});
    assert.equal(editorPromoAttempt.ok, false, 'mod_edituser must NOT be allowed to promote user to Administrator');

    // DB 확인: Alice의 그룹은 여전히 '2' (Member) 불변
    const aliceGroupAfterEditorAttempt = sql(`SELECT primary_group_id FROM ${q}."user" WHERE id=2`);
    assert.equal(aliceGroupAfterEditorAttempt, '2', 'Alice primary_group must remain 2 after editor promotion attempt');

    // The same editor must not grant super_mod or any other elevated group.
    const editorSupermodPromoAttempt = await editor.bundle({
      atomic: [{apply: 'User.adminUpdateUser', target: {ids: ['2']}, values: {primary_group: '5'}}],
    }, {key: 'editor-supermod-promo-attempt'});
    assert.equal(editorSupermodPromoAttempt.ok, false, 'mod_edituser must NOT grant a super_mod group');
    assert.equal(sql(`SELECT primary_group_id FROM ${q}."user" WHERE id=2`), '2');

    // An editor also cannot edit targets who already hold elevated permissions.
    for (const [label, targetId, expectedGroup] of [
      ['administrator', '3', '3'],
      ['super moderator', '5', '5'],
    ]) {
      const profileEdit = await editor.bundle({
        atomic: [{apply: 'User.moderatorEditUser', target: {ids: [targetId]}, values: {notes: `forbidden edit of ${label}`}}],
      }, {key: `editor-profile-${targetId}`});
      assert.equal(profileEdit.ok, false, `mod_edituser must not edit an existing ${label}`);
      const denied = await editor.bundle({
        atomic: [{apply: 'User.adminUpdateUser', target: {ids: [targetId]}, values: {primary_group: '2'}}],
      }, {key: `editor-demote-${targetId}`});
      assert.equal(denied.ok, false, `mod_edituser must not change an existing ${label}`);
      assert.equal(sql(`SELECT primary_group_id FROM ${q}."user" WHERE id=${targetId}`), expectedGroup);
    }

    // Super moderators cannot grant admin or super_mod membership either.
    const supermodPromotion = await supermod.bundle({
      atomic: [{apply: 'User.adminUpdateUser', target: {ids: ['2']}, values: {primary_group: '3'}}],
    }, {key: 'supermod-promo-attempt'});
    assert.equal(supermodPromotion.ok, false, 'super_mod cannot grant an admin group');
    assert.equal(sql(`SELECT primary_group_id FROM ${q}."user" WHERE id=2`), '2');

    // 관리자(Bob)는 정상적으로 그룹 변경 가능
    const adminPromoSuccess = await bob.bundle({
      atomic: [
        {
          apply: 'User.adminUpdateUser',
          target: {ids: ['2']},
          values: {primary_group: '3'},
        },
      ],
    }, {key: 'admin-promo-success'});
    assert.equal(adminPromoSuccess.ok, true, 'Admin must be allowed to update primary_group');

    // 다시 Member(2)로 원상복구
    await bob.bundle({
      atomic: [
        {
          apply: 'User.adminUpdateUser',
          target: {ids: ['2']},
          values: {primary_group: '2'},
        },
      ],
    }, {key: 'admin-restore-group'});

    // =========================================================================
    // 7. 계정 정지/비활성화 시 기존 토큰 쓰기 거부 및 재활성화 검증:
    //    Bob이 Alice를 activated=false 로 정지하면, Alice의 기존 토큰으로 updateProfile 거부
    // =========================================================================
    const deactivateAlice = await bob.bundle({
      atomic: [
        {
          apply: 'User.adminUpdateUser',
          target: {ids: ['2']},
          values: {activated: false},
        },
      ],
    }, {key: 'deactivate-alice'});
    assert.equal(deactivateAlice.ok, true);

    const aliceActivatedStatus = sql(`SELECT activated FROM ${q}."user" WHERE id=2`);
    assert.equal(aliceActivatedStatus, 'f');

    // 비활성화된 Alice가 기존 토큰으로 프로필 수정 시도 -> 거부되어야 함
    const aliceWriteWhileDeactivated = await alice.bundle({
      atomic: [
        {
          apply: 'User.updateProfile',
          target: {ids: ['2']},
          values: {signature: 'Malicious signature while deactivated'},
        },
      ],
    }, {key: 'alice-write-deactivated'});
    assert.equal(aliceWriteWhileDeactivated.ok, false, 'Deactivated user token write must be rejected');

    // DB 확인: Alice의 signature는 바뀌지 않고 기존 값 유지
    const aliceSigAfterDeactivatedWrite = sql(`SELECT signature FROM ${q}."user" WHERE id=2`);
    assert.equal(aliceSigAfterDeactivatedWrite, 'Alice In Wonderland', 'Signature must not change while deactivated');

    // 관리자가 Alice를 다시 activated=true 로 재활성화
    const reactivateAlice = await bob.bundle({
      atomic: [
        {
          apply: 'User.adminUpdateUser',
          target: {ids: ['2']},
          values: {activated: true},
        },
      ],
    }, {key: 'reactivate-alice'});
    assert.equal(reactivateAlice.ok, true);

    // 재활성화된 후 Alice가 다시 프로필 수정 시도 -> 성공해야 함
    const aliceWriteAfterReactivation = await alice.bundle({
      atomic: [
        {
          apply: 'User.updateProfile',
          target: {ids: ['2']},
          values: {signature: 'Alice Restored Signature'},
        },
      ],
    }, {key: 'alice-write-restored'});
    assert.equal(aliceWriteAfterReactivation.ok, true, 'Reactivated user token write must succeed');

    const aliceSigAfterReactivation = sql(`SELECT signature FROM ${q}."user" WHERE id=2`);
    assert.equal(aliceSigAfterReactivation, 'Alice Restored Signature');

    // =========================================================================
    // 8. Live UI Renderer Integration (UI -> SDK -> AIP -> PG):
    //    Flask UI 렌더러를 직접 기동하여 실제 HTTP 요청으로 Search(검색 비유출), Profile(수정 및 DB반영), Conversations(조회) 검증
    // =========================================================================
    const python = '/private/tmp/flaskbb-aip-host-venv/bin/python';
    const launchUi = async (subject, username) => {
      const uiEnv = {
        PATH: process.env.PATH,
        PYTHONPATH: [join(track, 'ui'), join(root, 'product/python')].join(':'),
        FLASKBB_TEST_INTEGRATION: '1',
        FLASKBB_TEST_ACCESS_TOKEN: token(subject),
        FLASKBB_TEST_SUBJECT: subject,
        FLASKBB_TEST_USERNAME: username || subject,
        FLASKBB_TEST_AIP_URL: `http://${started.address}`,
      };
      const host = spawn(python, ['tests/run_ui_host.py'], {cwd: track, env: uiEnv, stdio: ['ignore', 'pipe', 'pipe']});
      host.unref();
      uiHosts.push(host);
      host.stderrText = '';
      host.stderr.on('data', data => {host.stderrText += data;});
      const lines = createInterface({input: host.stdout});
      const listening = await Promise.race([
        once(lines, 'line').then(([line]) => JSON.parse(line)),
        once(host, 'exit').then(([code]) => {throw new Error(`UI host exit ${code}: ${host.stderrText}`);}),
        new Promise((_, reject) => setTimeout(() => reject(new Error(`UI host timeout: ${host.stderrText}`)), 15000).unref())
      ]);
      lines.close();
      host.stdout.destroy();
      host.stderr.destroy();
      return {host, base: `http://127.0.0.1:${listening.port}`};
    };

    const cookie = 'aip_session=aip-integration-session';
    const csrf = 'aip-integration-csrf-token';

    // 8a. Alice UI 인스턴스 기동 및 Live Search 검증 (공개 검색어 통과 & 숨김 토픽/게시물 비유출 검증)
    const aliceUi = await launchUi('alice-sub', 'member-alice');
    const publicSearchRes = await fetch(`${aliceUi.base}/search?q=Discussion`, {headers: {Cookie: cookie}});
    assert.equal(publicSearchRes.status, 200);
    const publicSearchHtml = await publicSearchRes.text();
    assert.match(publicSearchHtml, /First Discussion Topic/);

    // 일반 회원 Alice는 'Secret' 검색 시 숨김 토픽/게시물이 절대 노출되지 않음 (음수 반례)
    const hiddenSearchRes = await fetch(`${aliceUi.base}/search?q=Secret`, {headers: {Cookie: cookie}});
    assert.equal(hiddenSearchRes.status, 200);
    const hiddenSearchHtml = await hiddenSearchRes.text();
    assert.doesNotMatch(hiddenSearchHtml, /Secret Hidden Topic/);
    assert.doesNotMatch(hiddenSearchHtml, /Top secret hidden post content/);

    // 8b. Bob(관리자) UI 인스턴스 기동 및 Live Search 검증 (관리자는 숨김 토픽 조회 가능)
    const bobUi = await launchUi('bob-sub', 'admin-bob');
    const adminSearchRes = await fetch(`${bobUi.base}/search?q=Secret`, {headers: {Cookie: cookie}});
    assert.equal(adminSearchRes.status, 200);
    const adminSearchHtml = await adminSearchRes.text();
    assert.match(adminSearchHtml, /Secret Hidden Topic/);

    // 8c. Live User Profile 설정 수정 (UI POST -> SDK -> AIP -> PG 실반영)
    const updateSettingsRes = await fetch(`${aliceUi.base}/user/settings`, {
      method: 'POST',
      redirect: 'manual',
      headers: {Cookie: cookie, 'Content-Type': 'application/x-www-form-urlencoded'},
      body: new URLSearchParams({
        csrf_token: csrf,
        signature: 'Signature Updated via Live UI Integration',
        website: 'https://alice-live-ui.example.com',
        location: 'Wonderland Live',
      }),
    });
    assert.equal(updateSettingsRes.status, 303, 'Successful settings POST redirects with 303');
    await updateSettingsRes.arrayBuffer();

    // DB 확인: 실제 PostgreSQL에 서명과 웹사이트가 반영되었는지 검증
    const aliceSigFromUi = sql(`SELECT signature FROM ${q}."user" WHERE id=2`);
    assert.equal(aliceSigFromUi, 'Signature Updated via Live UI Integration');
    const aliceWebFromUi = sql(`SELECT website FROM ${q}."user" WHERE id=2`);
    assert.equal(aliceWebFromUi, 'https://alice-live-ui.example.com');

    // 8d. Live Conversations Inbox 조회
    const convInboxRes = await fetch(`${aliceUi.base}/conversations/inbox`, {headers: {Cookie: cookie}});
    assert.equal(convInboxRes.status, 200);
    const convInboxHtml = await convInboxRes.text();
    assert.match(convInboxHtml, /Confidential inquiry/);

    // 8e. New message and reply round trip through the real renderer, official SDK,
    // AIP listener and PostgreSQL. Both users have independent mailbox rows.
    const createMessageRes = await fetch(`${aliceUi.base}/conversations/new`, {
      method: 'POST',
      redirect: 'manual',
      headers: {Cookie: cookie, 'Content-Type': 'application/x-www-form-urlencoded'},
      body: new URLSearchParams({
        csrf_token: csrf,
        to_username: 'admin-bob',
        subject: 'Live UI conversation round trip',
        message: 'First message from Alice through the UI',
      }),
    });
    assert.equal(createMessageRes.status, 303);
    await createMessageRes.arrayBuffer();
    const conversationRows = sql(`SELECT id || '|' || user_id || '|' || unread FROM ${q}.conversation WHERE subject='Live UI conversation round trip' ORDER BY user_id`)
      .split('\n').map(line => line.split('|'));
    assert.equal(conversationRows.length, 2, 'sender and recipient each need a mailbox row');
    const aliceConversationId = conversationRows.find(row => row[1] === '2')?.[0];
    const bobConversationId = conversationRows.find(row => row[1] === '3')?.[0];
    assert.ok(aliceConversationId && bobConversationId);
    assert.deepEqual(conversationRows.map(row => row[2]), ['false', 'true'], 'recipient starts unread; sender does not');
    assert.equal(sql(`SELECT count(*) FROM ${q}.message m JOIN ${q}.conversation c ON c.id=m.conversation_id WHERE c.subject='Live UI conversation round trip' AND m.message='First message from Alice through the UI'`), '2', 'the first Message must be visible in both mailboxes');

    const bobInbox = await fetch(`${bobUi.base}/conversations/inbox`, {headers: {Cookie: cookie}});
    assert.equal(bobInbox.status, 200);
    assert.match(await bobInbox.text(), /Live UI conversation round trip/);
    const bobView = await fetch(`${bobUi.base}/conversations/${bobConversationId}`, {headers: {Cookie: cookie}});
    const bobViewHtml = await bobView.text();
    assert.equal(bobView.status, 200, `${bobViewHtml}\nUI stderr: ${bobUi.host.stderrText}\nAIP stderr: ${stderr}`);
    assert.match(bobViewHtml, /First message from Alice through the UI/);
    assert.equal(sql(`SELECT unread FROM ${q}.conversation WHERE id=${bobConversationId}`), 'f', 'reading Bob mailbox clears only Bob unread state');
    assert.equal(sql(`SELECT unread FROM ${q}.conversation WHERE id=${aliceConversationId}`), 'f', 'Bob reading must not change Alice mailbox state');

    const replyRes = await fetch(`${bobUi.base}/conversations/${bobConversationId}/reply`, {
      method: 'POST',
      redirect: 'manual',
      headers: {Cookie: cookie, 'Content-Type': 'application/x-www-form-urlencoded'},
      body: new URLSearchParams({csrf_token: csrf, message: 'Reply from Bob through the UI'}),
    });
    assert.equal(replyRes.status, 303);
    await replyRes.arrayBuffer();
    assert.equal(sql(`SELECT count(*) FROM ${q}.message m JOIN ${q}.conversation c ON c.id=m.conversation_id WHERE c.subject='Live UI conversation round trip' AND m.message='Reply from Bob through the UI'`), '2', 'reply must be visible from each mailbox');
    assert.equal(sql(`SELECT unread FROM ${q}.conversation WHERE id=${aliceConversationId}`), 't', 'reply marks only Alice mailbox unread');

    const aliceConversationView = await fetch(`${aliceUi.base}/conversations/${aliceConversationId}`, {headers: {Cookie: cookie}});
    assert.equal(aliceConversationView.status, 200);
    assert.match(await aliceConversationView.text(), /Reply from Bob through the UI/);
    assert.equal(sql(`SELECT unread FROM ${q}.conversation WHERE id=${aliceConversationId}`), 'f');
    assert.equal(sql(`SELECT unread FROM ${q}.conversation WHERE id=${bobConversationId}`), 'f');

    const trashAlice = await fetch(`${aliceUi.base}/conversations/${aliceConversationId}/trash`, {
      method: 'POST', redirect: 'manual',
      headers: {Cookie: cookie, 'Content-Type': 'application/x-www-form-urlencoded'},
      body: new URLSearchParams({csrf_token: csrf}),
    });
    assert.equal(trashAlice.status, 303);
    await trashAlice.arrayBuffer();
    assert.equal(sql(`SELECT trash FROM ${q}.conversation WHERE id=${aliceConversationId}`), 't');
    assert.equal(sql(`SELECT trash FROM ${q}.conversation WHERE id=${bobConversationId}`), 'f', 'trashing is per-user');

    // Forged mailbox direct call rejection: Alice cannot mutate Bob's mailbox
    const forgedTrash = await alice.bundle({
      atomic: [{apply: 'Conversation.moveToTrash', target: {ids: [String(bobConversationId)]}}]
    }, {key: 'forged-mailbox-trash'});
    assert.equal(forgedTrash.ok, false, 'forged mailbox moveToTrash must be rejected');
    assert.equal(sql(`SELECT trash FROM ${q}.conversation WHERE id=${bobConversationId}`), 'f', 'bob mailbox trash unchanged in DB');

    const forgedMarkRead = await alice.bundle({
      atomic: [{apply: 'Conversation.markRead', target: {ids: [String(bobConversationId)]}}]
    }, {key: 'forged-mailbox-markread'});
    assert.equal(forgedMarkRead.ok, false, 'forged mailbox markRead must be rejected');

    // Deactivated actor writes rejection & DB invariance
    // 1. Suspend Alice
    const suspendAlice = await bob.bundle({
      atomic: [{apply: 'User.adminUpdateUser', target: {ids: ['2']}, values: {activated: false}}]
    }, {key: 'test-suspend-alice'});
    assert.equal(suspendAlice.ok, true);
    assert.equal(sql(`SELECT activated FROM ${q}.user WHERE id=2`), 'f', 'alice deactivated in DB');

    // 2. Deactivated Alice mailbox mutation attempt (moveToTrash)
    // Note: un-trash Alice mailbox first so we test trashing while deactivated
    sql(`UPDATE ${q}.conversation SET trash=false WHERE id=${aliceConversationId};`);
    assert.equal(sql(`SELECT trash FROM ${q}.conversation WHERE id=${aliceConversationId}`), 'f');
    const deactivatedMailboxWrite = await alice.bundle({
      atomic: [{apply: 'Conversation.moveToTrash', target: {ids: [String(aliceConversationId)]}}]
    }, {key: 'deactivated-mailbox-trash'});
    assert.equal(deactivatedMailboxWrite.ok, false, 'deactivated actor moveToTrash rejected');
    assert.equal(sql(`SELECT trash FROM ${q}.conversation WHERE id=${aliceConversationId}`), 'f', 'mailbox trash unchanged in DB');

    // 3. Deactivated Alice message creation attempt
    const beforeDeactivatedMsgCount = sql(`SELECT count(*) FROM ${q}.message WHERE conversation_id=${aliceConversationId}`);
    const deactivatedMsgWrite = await alice.bundle({
      atomic: [{create: 'Message', values: {conversation: String(aliceConversationId), user: '2', message: 'forbidden deactivated message', date_created: new Date().toISOString()}}]
    }, {key: 'deactivated-msg-write'});
    assert.equal(deactivatedMsgWrite.ok, false, 'deactivated actor message create rejected');
    assert.equal(sql(`SELECT count(*) FROM ${q}.message WHERE conversation_id=${aliceConversationId}`), beforeDeactivatedMsgCount, 'message count unchanged in DB');

    // 4. Suspend Bob (Admin) and test admin write rejection
    const suspendBob = await bob.bundle({
      atomic: [{apply: 'User.adminUpdateUser', target: {ids: ['3']}, values: {activated: false}}]
    }, {key: 'test-suspend-bob'});
    assert.equal(suspendBob.ok, true);
    assert.equal(sql(`SELECT activated FROM ${q}.user WHERE id=3`), 'f', 'bob deactivated in DB');

    const deactivatedAdminCategory = await bob.bundle({
      atomic: [{create: 'Category', values: {title: 'Suspended Admin Category', description: 'probe', position: 101}}]
    }, {key: 'deactivated-admin-category'});
    assert.equal(deactivatedAdminCategory.ok, false, 'deactivated admin category creation rejected');
    assert.equal(sql(`SELECT count(*) FROM ${q}.category WHERE title='Suspended Admin Category'`), '0', 'category table unchanged in DB');

    // 5. Reactivation & recovery to normal state
    // Restore Bob in DB directly since actor is deactivated
    sql(`UPDATE ${q}.user SET activated=true WHERE id=3;`);
    assert.equal(sql(`SELECT activated FROM ${q}.user WHERE id=3`), 't', 'bob reactivated in DB');

    // Bob can now create Category normally
    const reactivatedAdminCategory = await bob.bundle({
      atomic: [{create: 'Category', values: {title: 'Reactivated Admin Category', description: 'recovered', position: 102}}]
    }, {key: 'reactivated-admin-category'});
    assert.equal(reactivatedAdminCategory.ok, true, 'reactivated admin can create category');
    assert.equal(sql(`SELECT count(*) FROM ${q}.category WHERE title='Reactivated Admin Category'`), '1', 'category created in DB');

    // Bob reactivates Alice via AIP API
    const reactivateAlicePostRecovery = await bob.bundle({
      atomic: [{apply: 'User.adminUpdateUser', target: {ids: ['2']}, values: {activated: true}}]
    }, {key: 'test-reactivate-alice'});
    assert.equal(reactivateAlicePostRecovery.ok, true);
    assert.equal(sql(`SELECT activated FROM ${q}.user WHERE id=2`), 't', 'alice reactivated in DB');

    // Alice can now create message and trash conversation normally
    const reactivatedAliceMsg = await alice.bundle({
      atomic: [{create: 'Message', values: {conversation: String(aliceConversationId), user: '2', message: 'reactivated alice message', date_created: new Date().toISOString()}}]
    }, {key: 'reactivated-alice-msg'});
    assert.equal(reactivatedAliceMsg.ok, true, 'reactivated alice can send message');
    assert.equal(sql(`SELECT count(*) FROM ${q}.message WHERE conversation_id=${aliceConversationId} AND message='reactivated alice message'`), '1');

    const reactivatedTrash = await alice.bundle({
      atomic: [{apply: 'Conversation.moveToTrash', target: {ids: [String(aliceConversationId)]}}]
    }, {key: 'reactivated-alice-trash'});
    assert.equal(reactivatedTrash.ok, true, 'reactivated alice can trash conversation');
    assert.equal(sql(`SELECT trash FROM ${q}.conversation WHERE id=${aliceConversationId}`), 't', 'mailbox trashed after reactivation');

    // =========================================================================
    // 9. F01/F05 offset pagination + Forum last-post projection, F06 read
    //    markers, F08 report queue paging (7.5.2: declared offset/cursor budgets)
    // =========================================================================
    const seedTopic101 = '00000000-0000-4000-8000-000000000101';
    const seedPost201 = '00000000-0000-4000-8000-000000000201';
    for (let i = 1; i <= 6; i++) {
      const topicId = randomUUID();
      const postId = randomUUID();
      const probeIso = new Date().toISOString();
      const mkTopic = await alice.bundle({atomic: [
        {create: 'Topic', values: {id: topicId, forum: '1', title: `Pagination probe topic ${i}`, user: '2', username: 'member-alice', locked: false, important: false, views: 0, published: false, deleted: false, hidden: false}},
      ]}, {key: `page-topic-${i}`});
      assert.equal(mkTopic.ok, true, JSON.stringify(mkTopic));
      const mkPost = await alice.bundle({atomic: [
        {create: 'Post', values: {id: postId, topic: topicId, forum: '1', user: '2', username: 'member-alice', content: `Pagination probe post ${i}`, hidden: false, published: false, version: 0, deleted: false}},
      ]}, {key: `page-post-${i}`});
      assert.equal(mkPost.ok, true, JSON.stringify(mkPost));
      const pubPost = await alice.bundle({atomic: [{apply: 'Post.publish', target: {ids: [postId]}}]}, {key: `page-post-pub-${i}`});
      assert.equal(pubPost.ok, true, JSON.stringify(pubPost));
      const link = await alice.bundle({atomic: [{create: 'TopicFirstPost', values: {topic: topicId, post: postId}}]}, {key: `page-link-${i}`});
      assert.equal(link.ok, true, JSON.stringify(link));
      const pubTopic = await alice.bundle({atomic: [{apply: 'Topic.publish', target: {ids: [topicId]}}]}, {key: `page-topic-pub-${i}`});
      assert.equal(pubTopic.ok, true, JSON.stringify(pubTopic));
    }

    // 9a. Topic offset pages: 1 seed + 6 probes visible to alice (hidden seed excluded by row policy)
    const topicPage = async offset => (await alice.read({
      read: 'Topic',
      select: ['id', 'title'],
      filter: [{field: 'forum', op: 'eq', value: '1'}],
      sort: [{field: 'id', dir: 'asc'}],
      limit: 3,
      offset,
    })).rows;
    const pg0 = await topicPage(0);
    const pg1 = await topicPage(3);
    const pg2 = await topicPage(6);
    assert.deepEqual([pg0.length, pg1.length, pg2.length], [3, 3, 1]);
    const allPageIds = [...pg0, ...pg1, ...pg2].map(r => r.id);
    assert.equal(new Set(allPageIds).size, 7, 'pages must be disjoint and cover all visible topics');
    assert.ok(allPageIds.includes(seedTopic101), 'seed topic must appear in exactly one page');

    // offset beyond the declared budget cap must be rejected, rows unchanged
    let offsetRejected = false;
    try {
      const over = await alice.read({read: 'Topic', select: ['id'], sort: [{field: 'id', dir: 'asc'}], limit: 3, offset: 2001});
      offsetRejected = over.ok === false;
    } catch {
      offsetRejected = true;
    }
    assert.equal(offsetRejected, true, 'offset above budget cap must be rejected');

    // 9a2. User keyset cursor: ids 1..5 chain without gaps
    const userPage = async after => (await alice.read({
      read: 'User',
      select: ['id', 'username'],
      sort: [{field: 'id', dir: 'asc'}],
      limit: 2,
      ...(after ? {after} : {}),
    })).rows;
    const u0 = await userPage(undefined);
    assert.deepEqual(u0.map(r => r.id), ['1', '2']);
    const u1 = await userPage({id: u0[1].id});
    assert.deepEqual(u1.map(r => r.id), ['3', '4']);
    const u2 = await userPage({id: u1[1].id});
    assert.deepEqual(u2.map(r => r.id), ['5']);

    // 9b. Last-post projection: final reply drives Forum.last_post_* in the same transaction
    const probeReplyId = randomUUID();
    const replyIso = new Date().toISOString();
    const mkReply = await alice.bundle({atomic: [
      {create: 'Post', values: {id: probeReplyId, topic: seedTopic101, forum: '1', user: '2', username: 'member-alice', content: 'Final last-post probe reply', hidden: false, published: false, version: 0, deleted: false}},
    ]}, {key: 'lastpost-reply-create'});
    assert.equal(mkReply.ok, true, JSON.stringify(mkReply));
    const pubReply = await alice.bundle({atomic: [{apply: 'Post.publish', target: {ids: [probeReplyId]}}]}, {key: 'lastpost-reply-publish'});
    assert.equal(pubReply.ok, true, JSON.stringify(pubReply));
    const forum1 = (await alice.read({
      read: 'Forum',
      select: ['id', 'last_post', 'last_post_user', 'last_post_title', 'last_post_username', 'last_post_created'],
      filter: [{field: 'id', op: 'eq', value: '1'}],
    })).rows[0];
    assert.equal(forum1.last_post, probeReplyId);
    assert.equal(forum1.last_post_user, '2');
    assert.equal(forum1.last_post_title, 'First Discussion Topic');
    assert.equal(forum1.last_post_username, 'member-alice');
    assert.ok(forum1.last_post_created, 'last_post_created must be set');
    assert.ok(Math.abs(Date.parse(forum1.last_post_created) - Date.parse(replyIso)) <= 5000, 'last_post_created must track publish time');
    assert.equal(sql(`SELECT last_post_id FROM ${q}.forum WHERE id=1`), probeReplyId, 'DB last_post_id must match the published reply');
    assert.equal(sql(`SELECT last_post_username FROM ${q}.forum WHERE id=1`), 'member-alice');

    // 9c. Read markers: stranger cannot plant a marker for alice; own marker advances
    const forgedRead = await editor.bundle({atomic: [
      {create: 'TopicRead', values: {user: '2', topic: seedTopic101, forum: '1', last_read: new Date().toISOString()}},
    ]}, {key: 'forged-topicread'});
    assert.equal(forgedRead.ok, false, 'stranger TopicRead create must be rejected');
    const markerCountBefore = sql(`SELECT count(*) FROM ${q}.topic_read WHERE user_id=2`);
    assert.equal(markerCountBefore, '0', 'no marker row may appear from the forged call');
    const markerIso = new Date().toISOString();
    const mkMarker = await alice.bundle({atomic: [
      {create: 'TopicRead', values: {user: '2', topic: seedTopic101, forum: '1', last_read: markerIso}},
    ]}, {key: 'alice-topicread'});
    assert.equal(mkMarker.ok, true, JSON.stringify(mkMarker));
    const markerId = mkMarker.created[0].id;
    const advMarker = await alice.bundle({atomic: [{apply: 'TopicRead.updateRead', target: {ids: [String(markerId)]}}]}, {key: 'alice-topicread-adv'});
    assert.equal(advMarker.ok, true, JSON.stringify(advMarker));
    const markerRow = (await alice.read({read: 'TopicRead', select: ['id', 'last_read'], filter: [{field: 'id', op: 'eq', value: String(markerId)}]})).rows[0];
    assert.ok(Date.parse(markerRow.last_read) >= Date.parse(markerIso), 'updateRead must advance last_read');

    // 9d. Report queue: 3 reports page as 2+1; reporter sees none after submit; moderator sees all
    const reportIds = [];
    for (let k = 1; k <= 3; k++) {
      const rep = await alice.bundle({atomic: [
        {create: 'Report', values: {reporter: '2', post: seedPost201, reason: `probe report ${k}`}},
      ]}, {key: `probe-report-${k}`});
      assert.equal(rep.ok, true, JSON.stringify(rep));
      const sub = await alice.bundle({atomic: [{apply: 'Report.submit', target: {ids: [String(rep.created[0].id)]}}]}, {key: `probe-report-submit-${k}`});
      assert.equal(sub.ok, true, JSON.stringify(sub));
      reportIds.push(String(rep.created[0].id));
    }
    const repPage = async offset => (await bob.read({read: 'Report', select: ['id'], limit: 2, offset})).rows;
    const r0 = await repPage(0);
    const r1 = await repPage(2);
    assert.deepEqual([r0.length, r1.length], [2, 1]);
    assert.deepEqual([...r0, ...r1].map(r => r.id).sort(), [...reportIds].sort(), 'report pages must cover the queue without overlap');
    const aliceQueue = (await alice.read({read: 'Report', select: ['id']})).rows;
    assert.equal(aliceQueue.length, 0, 'reporter sees no submitted reports in the mod queue');
    const editorQueue = (await editor.read({read: 'Report', select: ['id']})).rows;
    assert.equal(editorQueue.length, 3, 'moderator group sees the full queue');

    // moderator (non-admin) management boundary: editor cannot create categories
    const editorCat = await editor.bundle({atomic: [
      {create: 'Category', values: {title: 'Editor Unauthorized Category', description: 'probe', position: 103}},
    ]}, {key: 'editor-unauthorized-cat'});
    assert.equal(editorCat.ok, false, 'non-admin moderator Category.create must be rejected');
    assert.equal(sql(`SELECT count(*) FROM ${q}.category WHERE title='Editor Unauthorized Category'`), '0');

    console.log(JSON.stringify({schema, ok: true, featuresVerified: ['F07_search', 'F03_profile', 'F09_conversations', 'F06_tracker_read', 'F04_F10_admin_crud', 'UI_live_integration', 'deactivated_writes_rejected', 'reactivation_recovered', 'forged_mailbox_rejected', 'F01_pagination', 'F05_last_post', 'F06_read_markers', 'F08_report_queue']}));
  } catch (error) {
    console.error(error);
    throw error;
  } finally {
    for (const child of [...uiHosts, server].filter(Boolean)) {
      if (child.closed) continue;
      const closed = once(child, 'close');
      if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
      await closed;
    }
    sql(`DROP SCHEMA IF EXISTS "${schema}" CASCADE;`);
    rmSync(temp, {recursive: true, force: true});
  }
});
