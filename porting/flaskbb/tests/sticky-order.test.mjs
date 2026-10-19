import assert from 'node:assert/strict';
import {test} from 'node:test';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

// Upstream Forum.get_topics orders important DESC, last_updated DESC. UUID is an
// explicit tie breaker so offset pages are stable even when timestamps match.
test('FlaskBB sticky ordering spans page boundaries and follows moderation changes', {timeout: 90000}, async () => {
  const id = n => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`;
  const fx = await startFixture({
    prefix: 'aip-flaskbb-sticky-',
    actors: {'alice-sub': 2, 'admin-sub': 3},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','c',1);`);
      sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'Forum','f',1,false,false);`);
      sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,2),(2,1,3);`);
      for (const [n, sticky, date, hidden, deleted] of [
        [1, true, '2020-01-01', false, false],
        [2, true, '2021-01-01', false, false],
        [3, false, '2025-01-01', false, false],
        [4, false, '2025-01-01', false, false],
        [5, false, '2024-01-01', false, false],
        [6, true, '2026-01-01', true, false],
        [7, true, '2026-01-01', false, true],
      ]) sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES ('${id(n)}',1,'Topic ${n}',2,'member-alice','${date}','${date}',false,${sticky},0,true,${deleted},${hidden});`);
    },
  });
  try {
    const sort = [{field: 'important', dir: 'desc'}, {field: 'last_updated', dir: 'desc'}, {field: 'id', dir: 'asc'}];
    const page = async offset => (await fx.client('alice-sub').read({
      read: 'Topic', select: ['id', 'title', 'important', 'last_updated'],
      filter: [{field: 'forum', op: 'eq', value: '1'}], sort, limit: 2, offset,
    })).rows.map(row => row.id);
    const pages = [await page(0), await page(2), await page(4)];
    assert.deepEqual(pages.map(p => p.length), [2, 2, 1]);
    assert.deepEqual(pages.flat(), [2, 1, 3, 4, 5].map(id), 'sticky beats recency; equal times use id; hidden and deleted are excluded');
    assert.equal(new Set(pages.flat()).size, 5, 'pages do not repeat rows');
    const admin = fx.client('admin-sub');
    assert.equal((await admin.bundle({atomic: [{apply: 'Topic.highlight', target: {ids: [id(3)]}}]}, {key: 'sticky-highlight'})).ok, true);
    assert.deepEqual(await page(0), [3, 2].map(id));
    assert.equal((await admin.bundle({atomic: [{apply: 'Topic.trivialize', target: {ids: [id(2)]}}]}, {key: 'sticky-trivialize'})).ok, true);
    assert.deepEqual([...(await page(0)), ...(await page(2)), ...(await page(4))], [3, 1, 4, 5, 2].map(id));
  } finally { await fx.stop(); }
});
