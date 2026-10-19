import assert from 'node:assert/strict';
import {test} from 'node:test';
import {randomUUID} from 'node:crypto';
import {GROUPS_SQL, USERS_SQL, binary, startFixture} from './harness.mjs';

// G-11 physical delete. Upstream deletes are real row deletes:
// DeleteConversation removes the owner's mailbox copy plus its messages
// (ORM cascade), DeleteReport removes the report row (moderator+), DeleteUser
// removes the row (admin, never self) with DB cascades for trackers, reads,
// moderatorships, secondary memberships and mailbox copies, SET NULL for
// message authorship and last-post user, and RESTRICT for posts/topics/reports
// (their user FKs carry no ondelete action, so upstream refuses those too).
// The port declares `purge` transitions with the core `delete` flag: effects
// run before the target delete in one transaction and both are audited.
// Two narrow divergences stay: mailbox copies block User.purge (their Message
// children need a two-hop scope the delete effect cannot express) and the
// upstream SET NULL columns block it too (many-update rejects nullable-FK
// scope), where upstream nulls them. PRODUCT-GAPS.md G-11 records both.
const id = n => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`;

test('FlaskBB conversation purge deletes the owner mailbox and its messages only (real PostgreSQL)', {timeout: 90000}, async () => {
  const fx = await startFixture({
    prefix: 'aip-flaskbb-purge-conv-',
    actors: {'alice-sub': 2, 'bob-sub': 3, 'carol-sub': 6},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
    },
  });
  const {sql, q, run, config} = fx;
  try {
    const alice = fx.client('alice-sub');
    const bob = fx.client('bob-sub');
    const carol = fx.client('carol-sub');
    const nowIso = new Date().toISOString();
    const shared = randomUUID();
    const created = await alice.bundle({atomic: [
      {create: 'Conversation', values: {user: '2', from_user: '2', to_user: '3', shared_id: shared, subject: 'Purge me', date_created: nowIso, date_modified: nowIso, trash: false, draft: false, unread: false}},
      {create: 'Conversation', values: {user: '3', from_user: '2', to_user: '3', shared_id: shared, subject: 'Purge me', date_created: nowIso, date_modified: nowIso, trash: false, draft: false, unread: true}},
    ]}, {key: 'purge-conv-create'});
    assert.equal(created.ok, true, JSON.stringify(created));
    const [aliceConv, bobConv] = created.created.map(r => String(r.id));
    const msgs = await alice.bundle({atomic: [
      {create: 'Message', values: {conversation: aliceConv, user: '2', message: 'alice copy note', date_created: nowIso}},
      {create: 'Message', values: {conversation: bobConv, user: '2', message: 'bob copy note', date_created: nowIso}},
    ]}, {key: 'purge-conv-msgs'});
    assert.equal(msgs.ok, true, JSON.stringify(msgs));
    const [aliceMsg, bobMsg] = msgs.created.map(r => String(r.id));
    const convs = () => Number(sql(`SELECT count(*) FROM ${q}.conversation`));
    const messages = () => Number(sql(`SELECT count(*) FROM ${q}.message`));
    assert.equal(convs(), 2);
    assert.equal(messages(), 2);

    // A third party cannot purge someone else's mailbox. Upstream 404s; the
    // port refuses and changes nothing.
    const byCarol = await carol.bundle({atomic: [{apply: 'Conversation.purge', target: {ids: [aliceConv]}}]}, {key: 'purge-conv-carol'});
    assert.equal(byCarol.ok, false, JSON.stringify(byCarol));
    assert.equal(convs(), 2);
    assert.equal(messages(), 2);

    // The owner purge removes her mailbox row and its message; the other
    // party's copy and message stay, like upstream.
    const purged = await alice.bundle({atomic: [{apply: 'Conversation.purge', target: {ids: [aliceConv]}}]}, {key: 'purge-conv-alice'});
    assert.equal(purged.ok, true, JSON.stringify(purged));
    assert.equal(convs(), 1);
    assert.equal(messages(), 1);
    assert.equal(sql(`SELECT count(*) FROM ${q}.conversation WHERE id=${aliceConv}`), '0');
    assert.equal(sql(`SELECT count(*) FROM ${q}.message WHERE id=${aliceMsg}`), '0');
    assert.equal(sql(`SELECT count(*) FROM ${q}.conversation WHERE id=${bobConv}`), '1');
    assert.equal(sql(`SELECT count(*) FROM ${q}.message WHERE id=${bobMsg}`), '1');
    const aliceReads = await alice.read({read: 'Conversation', select: ['id'], filter: [{field: 'user', op: 'eq', value: '2'}], limit: 10});
    assert.equal(aliceReads.rows.length, 0);
    const bobReads = await bob.read({read: 'Conversation', select: ['id'], filter: [{field: 'user', op: 'eq', value: '3'}], limit: 10});
    assert.equal(bobReads.rows.length, 1);

    // Both the mailbox delete and the message effect are audited with before
    // snapshots and null after.
    const convAudit = JSON.parse(run(binary, ['service', 'audit', '--config', config, 'list', '--resource', 'Conversation', '--target-id', aliceConv]));
    assert.equal(convAudit.ok, true, JSON.stringify(convAudit));
    assert.equal(convAudit.audit.length, 1);
    assert.deepEqual([convAudit.audit[0].transition, convAudit.audit[0].after], ['purge', null]);
    assert.equal(convAudit.audit[0].before.subject, 'Purge me');
    const msgAudit = JSON.parse(run(binary, ['service', 'audit', '--config', config, 'list', '--resource', 'Message', '--target-id', aliceMsg]));
    assert.equal(msgAudit.ok, true, JSON.stringify(msgAudit));
    assert.equal(msgAudit.audit.length, 1);
    assert.equal(msgAudit.audit[0].transition, 'purge:delete');
  } finally {
    await fx.stop();
  }
});

test('FlaskBB report purge deletes report rows for moderators, atomically in bulk (real PostgreSQL)', {timeout: 90000}, async () => {
  const fx = await startFixture({
    prefix: 'aip-flaskbb-purge-report-',
    actors: {'alice-sub': 2, 'admin-sub': 3, 'mod-sub': 4},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','c',1);`);
      sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'Forum','f',1,false,false);`);
      sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,2),(2,1,3),(3,1,4);`);
      sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES ('${id(1)}',1,'Topic 1',2,'member-alice',now(),now(),false,false,0,true,false,false);`);
      sql(`INSERT INTO ${q}.post (id,topic_id,forum_id,user_id,username,content,date_created,date_modified,published,deleted,hidden,version) VALUES ('${id(101)}','${id(1)}',1,2,'member-alice','Post 101',now(),now(),true,false,false,1);`);
    },
  });
  const {sql, q, run, config} = fx;
  try {
    const alice = fx.client('alice-sub');
    const admin = fx.client('admin-sub');
    const mod = fx.client('mod-sub');
    const post101 = id(101);
    const made = [];
    for (let i = 1; i <= 4; i++) {
      const rep = await alice.bundle({atomic: [{create: 'Report', values: {reporter: '2', post: post101, reason: `Purge report ${i}`}}]}, {key: `purge-report-create-${i}`});
      assert.equal(rep.ok, true, JSON.stringify(rep));
      const repId = String(rep.created[0].id);
      made.push(repId);
      const sub = await alice.bundle({atomic: [{apply: 'Report.submit', target: {ids: [repId]}}]}, {key: `purge-report-submit-${i}`});
      assert.equal(sub.ok, true, JSON.stringify(sub));
    }
    const reports = () => Number(sql(`SELECT count(*) FROM ${q}.report`));
    assert.equal(reports(), 4);

    // Neither the reporter nor an ordinary member can purge. Upstream needs
    // IsAtleastModerator.
    const byReporter = await alice.bundle({atomic: [{apply: 'Report.purge', target: {ids: [made[0]]}}]}, {key: 'purge-report-alice'});
    assert.equal(byReporter.ok, false, JSON.stringify(byReporter));
    assert.equal(reports(), 4);

    // A moderator bulk-purges two rows atomically, like upstream's ids loop
    // but in one transaction; the admin purges the rest singly.
    const bulk = await mod.bundle({atomic: [{apply: 'Report.purge', target: {ids: [made[0], made[1]]}}]}, {key: 'purge-report-bulk'});
    assert.equal(bulk.ok, true, JSON.stringify(bulk));
    assert.equal(reports(), 2);
    const single = await admin.bundle({atomic: [{apply: 'Report.purge', target: {ids: [made[2]]}}]}, {key: 'purge-report-single'});
    assert.equal(single.ok, true, JSON.stringify(single));
    assert.equal(reports(), 1);

    // A mixed batch (one unknown id) is refused as a whole and deletes nothing.
    const mixed = await mod.bundle({atomic: [{apply: 'Report.purge', target: {ids: [made[3], '999999']}}]}, {key: 'purge-report-mixed'});
    assert.equal(mixed.ok, false, JSON.stringify(mixed));
    assert.equal(reports(), 1);

    const audit = JSON.parse(run(binary, ['service', 'audit', '--config', config, 'list', '--resource', 'Report', '--target-id', made[0]]));
    assert.equal(audit.ok, true, JSON.stringify(audit));
    assert.equal(audit.audit.length, 1);
    assert.deepEqual([audit.audit[0].transition, audit.audit[0].after], ['purge', null]);
    assert.equal(audit.audit[0].before.reason, 'Purge report 1');
  } finally {
    await fx.stop();
  }
});

test('FlaskBB user purge deletes an unreferenced account with its memberships, refuses the rest (real PostgreSQL)', {timeout: 120000}, async () => {
  const fx = await startFixture({
    prefix: 'aip-flaskbb-purge-user-',
    actors: {'alice-sub': 2, 'admin-sub': 3},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}."user" (id,username,email,date_joined,lastseen,primary_group_id,activated,language) VALUES
        (7,'eve-post','eve@example.test',now(),now(),2,true,'en'),
        (8,'frank-conv','frank@example.test',now(),now(),2,true,'en'),
        (9,'grace-msg','grace@example.test',now(),now(),2,true,'en');`);
      sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','c',1);`);
      sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'Forum','f',1,false,false);`);
      sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,2),(2,1,3),(3,1,4);`);
      sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES ('${id(1)}',1,'Topic 1',7,'eve-post',now(),now(),false,false,0,true,false,false);`);
      sql(`INSERT INTO ${q}.post (id,topic_id,forum_id,user_id,username,content,date_created,date_modified,published,deleted,hidden,version) VALUES ('${id(101)}','${id(1)}',1,7,'eve-post','Eve post',now(),now(),true,false,false,1);`);
      // Carol holds only cascading rows: secondary membership, tracker, reads,
      // and a forum moderatorship. Upstream cascades all of these.
      sql(`INSERT INTO ${q}.user_group (id,user_id,group_id) VALUES (1,6,4);`);
      sql(`INSERT INTO ${q}.topic_tracker (id,user_id,topic_id,active) VALUES (1,6,'${id(1)}',true);`);
      sql(`INSERT INTO ${q}.forum_read (id,user_id,forum_id,last_read,cleared) VALUES (1,6,1,now(),NULL);`);
      sql(`INSERT INTO ${q}.topic_read (id,user_id,topic_id,forum_id,last_read) VALUES (1,6,'${id(1)}',1,now());`);
      sql(`INSERT INTO ${q}.forum_moderator (id,forum_id,user_id) VALUES (1,1,6);`);
      // Frank owns a mailbox copy; grace authored a message in alice's copy.
      sql(`INSERT INTO ${q}.conversation (id,user_id,from_user_id,to_user_id,shared_id,subject,date_created,date_modified,trash,draft,unread) VALUES
        (11,8,8,2,'${randomUUID()}','Frank box',now(),now(),false,false,false),
        (12,2,2,9,'${randomUUID()}','Alice box',now(),now(),false,false,false);`);
      sql(`INSERT INTO ${q}.message (id,conversation_id,user_id,message,date_created) VALUES (21,12,9,'grace note',now());`);
    },
  });
  const {sql, q, run, config} = fx;
  try {
    const alice = fx.client('alice-sub');
    const admin = fx.client('admin-sub');
    const users = () => Number(sql(`SELECT count(*) FROM ${q}."user"`));
    const memberships = uid => Number(sql(`SELECT count(*) FROM ${q}.user_group WHERE user_id=${uid}`));
    assert.equal(users(), 9);

    // Non-admin purge is refused. Upstream needs IsAdmin.
    const byMember = await alice.bundle({atomic: [{apply: 'User.purge', target: {ids: ['6']}}]}, {key: 'purge-user-member'});
    assert.equal(byMember.ok, false, JSON.stringify(byMember));
    // Self purge is refused, like upstream skipping the current user.
    const selfPurge = await admin.bundle({atomic: [{apply: 'User.purge', target: {ids: ['3']}}]}, {key: 'purge-user-self'});
    assert.equal(selfPurge.ok, false, JSON.stringify(selfPurge));
    assert.equal(users(), 9);

    // Referenced accounts are refused and left untouched: eve owns a topic
    // and post (upstream RESTRICTs these too), frank owns a mailbox copy,
    // grace authored a message. The last two diverge from upstream, which
    // cascades the mailbox and nulls the authorship; see PRODUCT-GAPS G-11.
    for (const [uid, key] of [[7, 'eve'], [8, 'frank'], [9, 'grace']]) {
      const refused = await admin.bundle({atomic: [{apply: 'User.purge', target: {ids: [String(uid)]}}]}, {key: `purge-user-${key}`});
      assert.equal(refused.ok, false, `purge of user ${uid}: ${JSON.stringify(refused)}`);
    }
    assert.equal(users(), 9);
    assert.equal(Number(sql(`SELECT count(*) FROM ${q}.post`)), 1);
    assert.equal(Number(sql(`SELECT count(*) FROM ${q}.conversation`)), 2);
    assert.equal(Number(sql(`SELECT count(*) FROM ${q}.message`)), 1);

    // Carol is unreferenced apart from cascading rows: she goes, and her
    // memberships go with her in the same transaction.
    assert.equal(memberships(6), 1);
    const carol = await admin.bundle({atomic: [{apply: 'User.purge', target: {ids: ['6']}}]}, {key: 'purge-user-carol'});
    assert.equal(carol.ok, true, JSON.stringify(carol));
    assert.equal(users(), 8);
    assert.equal(sql(`SELECT count(*) FROM ${q}."user" WHERE id=6`), '0');
    assert.equal(memberships(6), 0);
    assert.equal(Number(sql(`SELECT count(*) FROM ${q}.topic_tracker WHERE user_id=6`)), 0);
    assert.equal(Number(sql(`SELECT count(*) FROM ${q}.forum_read WHERE user_id=6`)), 0);
    assert.equal(Number(sql(`SELECT count(*) FROM ${q}.topic_read WHERE user_id=6`)), 0);
    assert.equal(Number(sql(`SELECT count(*) FROM ${q}.forum_moderator WHERE user_id=6`)), 0);

    // Purging an unknown id is refused and deletes nothing.
    const unknown = await admin.bundle({atomic: [{apply: 'User.purge', target: {ids: ['999999']}}]}, {key: 'purge-user-unknown'});
    assert.equal(unknown.ok, false, JSON.stringify(unknown));
    assert.equal(users(), 8);

    const audit = JSON.parse(run(binary, ['service', 'audit', '--config', config, 'list', '--resource', 'User', '--target-id', '6']));
    assert.equal(audit.ok, true, JSON.stringify(audit));
    assert.equal(audit.audit.length, 1);
    assert.deepEqual([audit.audit[0].transition, audit.audit[0].after], ['purge', null]);
    assert.equal(audit.audit[0].before.username, 'member-carol');
    const cascade = JSON.parse(run(binary, ['service', 'audit', '--config', config, 'list', '--resource', 'UserGroup', '--target-id', '1']));
    assert.equal(cascade.ok, true, JSON.stringify(cascade));
    assert.equal(cascade.audit.length, 1);
    assert.equal(cascade.audit[0].transition, 'purge:delete');
  } finally {
    await fx.stop();
  }
});
