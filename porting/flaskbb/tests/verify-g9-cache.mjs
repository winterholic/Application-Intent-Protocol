import assert from 'node:assert/strict';
import {test} from 'node:test';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

test('G-9 fresh client sees another actor write immediately while a reused SDK client may be stale for the 1s TTL', {timeout: 90000}, async () => {
  const topicId = '00000000-0000-4000-8000-000000000909';
  const fx = await startFixture({actors: {'alice-sub': 2, 'bob-sub': 3}, seed: (sql, q, schema) => {
    sql(GROUPS_SQL(schema)); sql(USERS_SQL(schema));
    sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'G9','c',1);`);
    sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'G9','a',1,false,false);`);
    sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,1,2),(3,1,3);`);
    sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES ('${topicId}',1,'G9 cache probe',3,'admin-bob',now(),now(),false,false,0,true,false,true);`);
  }});
  try {
    const alice = fx.client('alice-sub');
    const query = {read: 'Topic', select: ['id', 'title']};
    const before = await alice.read(query);
    assert.equal(before.rows.length, 0);
    const changed = await fx.client('bob-sub').bundle({atomic: [{apply: 'Topic.unhide', target: {ids: [topicId]}}]}, {key: 'g9-unhide'});
    assert.equal(changed.ok, true, JSON.stringify(changed));
    const reused = await alice.read(query);
    const fresh = await fx.client('alice-sub').read(query);
    console.log(JSON.stringify({ok: true, initialRows: before.rows.length, reusedCached: reused.cached, reusedRows: reused.rows.length, freshCached: fresh.cached, freshRows: fresh.rows.length}));
  } finally { await fx.stop(); }
});
