import assert from 'node:assert/strict';
import {test} from 'node:test';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

test('FlaskBB F08 report bulk resolve allows moderators to zap multiple reports atomically', {timeout: 90000}, async () => {
  const id = n => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`;
  const fx = await startFixture({
    prefix: 'aip-flaskbb-report-bulk-',
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
  try {
    const alice = fx.client('alice-sub');
    const mod = fx.client('mod-sub');
    const post101 = id(101);

    // 1. Reporter (Alice) creates 3 reports and submits them.
    const createdReportIds = [];
    for (let i = 1; i <= 3; i++) {
      const rep = await alice.bundle({
        atomic: [
          {create: 'Report', values: {reporter: '2', post: post101, reason: `Spam report ${i}`}},
        ],
      }, {key: `alice-report-create-${i}`});
      assert.equal(rep.ok, true, `Report ${i} creation should succeed`);
      const repId = String(rep.created[0].id);
      createdReportIds.push(repId);

      const sub = await alice.bundle({
        atomic: [
          {apply: 'Report.submit', target: {ids: [repId]}},
        ],
      }, {key: `alice-report-submit-${i}`});
      assert.equal(sub.ok, true, `Report ${i} submit should succeed`);
    }

    // 2. Moderator views reports in mod queue.
    const allBefore = await mod.read({
      read: 'Report',
      select: ['id', 'zapped', 'zapped_by', 'reason'],
      limit: 10,
    });
    const unreadBefore = allBefore.rows.filter(r => r.zapped === null);
    assert.equal(unreadBefore.length, 3, 'Moderator sees all 3 unread reports');

    // 3. Regular member (Alice) cannot resolve reports.
    const memberAttempt = await alice.bundle({
      atomic: [
        {apply: 'Report.resolve', target: {ids: [createdReportIds[0]]}},
      ],
    }, {key: 'alice-resolve-denied'});
    assert.equal(memberAttempt.ok, false, 'Non-moderator resolve must be rejected');

    // 4. Moderator bulk resolves all 3 reports in a single atomic bundle.
    const bulkResolve = await mod.bundle({
      atomic: [
        {apply: 'Report.resolve', target: {ids: createdReportIds}},
      ],
    }, {key: 'mod-bulk-resolve-all'});
    assert.equal(bulkResolve.ok, true, 'Bulk resolve by moderator must succeed');

    // 5. Verify in DB that all 3 reports are zapped with zapped_by = 4 (moderator).
    const allAfter = await mod.read({
      read: 'Report',
      select: ['id', 'zapped', 'zapped_by'],
      limit: 10,
    });
    const unreadAfter = allAfter.rows.filter(r => r.zapped === null);
    assert.equal(unreadAfter.length, 0, 'No unread reports remain in queue');
    assert.equal(allAfter.rows.length, 3);
    for (const row of allAfter.rows) {
      assert.notEqual(row.zapped, null, 'zapped timestamp must be set');
      const zappedById = typeof row.zapped_by === 'object' && row.zapped_by !== null ? row.zapped_by.id : row.zapped_by;
      assert.equal(String(zappedById), '4', 'zapped_by must be moderator (actor 4)');
    }

    // 6. Already zapped reports cannot be resolved again.
    const reResolve = await mod.bundle({
      atomic: [
        {apply: 'Report.resolve', target: {ids: [createdReportIds[0]]}},
      ],
    }, {key: 'mod-re-resolve-denied'});
    assert.equal(reResolve.ok, false, 'Resolving already zapped report must be rejected');
  } finally {
    await fx.stop();
  }
});

test('FlaskBB F08 report bulk resolve enforces the 50-row cap and rolls back a mixed batch', {timeout: 90000}, async () => {
  const id = n => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`;
  const fx = await startFixture({
    prefix: 'aip-flaskbb-report-bulk-cap-',
    actors: {'mod-sub': 4},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','c',1);`);
      sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'Forum','f',1,false,false);`);
      sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,2),(2,1,3),(3,1,4);`);
      sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES ('${id(1)}',1,'Topic 1',2,'member-alice',now(),now(),false,false,0,true,false,false);`);
      sql(`INSERT INTO ${q}.post (id,topic_id,forum_id,user_id,username,content,date_created,date_modified,published,deleted,hidden,version) VALUES ('${id(101)}','${id(1)}',1,2,'member-alice','Post 101',now(),now(),true,false,false,1);`);
      // 52 submitted reports; the last one is already zapped.
      sql(`INSERT INTO ${q}.report (id,reporter_id,reported,post_id,reason) SELECT g,2,now(),'${id(101)}','seed '||g FROM generate_series(1,52) g;`);
      sql(`UPDATE ${q}.report SET zapped=now(), zapped_by_id=3 WHERE id=52;`);
    },
  });
  try {
    const mod = fx.client('mod-sub');
    const range = (from, count) => Array.from({length: count}, (_, i) => String(from + i));
    const unzapped = async () => (await mod.read({read: 'Report', select: ['id', 'zapped'], limit: 100})).rows.filter(r => r.zapped === null).length;
    assert.equal(await unzapped(), 51, 'seed leaves 51 unzapped reports');

    // 1. 51 targets exceed maxRows 50: rejected as a whole, nothing zapped.
    const over = await mod.bundle({atomic: [{apply: 'Report.resolve', target: {ids: range(1, 51)}}]}, {key: 'mod-resolve-over-cap'});
    assert.equal(over.ok, false, '51 targets must exceed the declared bulk cap');
    assert.equal(await unzapped(), 51, 'a rejected over-cap bundle must not zap any row');

    // 2. 50 targets including the already zapped id 52: the whole bundle rolls back.
    const mixed = await mod.bundle({atomic: [{apply: 'Report.resolve', target: {ids: [...range(2, 49), '52']}}]}, {key: 'mod-resolve-mixed'});
    assert.equal(mixed.ok, false, 'a batch containing an already zapped report must be rejected');
    assert.equal(await unzapped(), 51, 'a rejected mixed batch must leave the other rows untouched');

    // 3. Exactly 50 unzapped targets at the cap succeed; row 51 stays open.
    const atCap = await mod.bundle({atomic: [{apply: 'Report.resolve', target: {ids: range(1, 50)}}]}, {key: 'mod-resolve-at-cap'});
    assert.equal(atCap.ok, true, 'exactly 50 rows must be accepted');
    assert.equal(await unzapped(), 1, 'only row 51 remains unzapped after the cap-sized bundle');
  } finally {
    await fx.stop();
  }
});
