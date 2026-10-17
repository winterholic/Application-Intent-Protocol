import assert from 'node:assert/strict';
import {test} from 'node:test';
import {randomUUID} from 'node:crypto';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

// 7.5.3 F15: the original keeps a Whoosh index that has to be rebuilt and can
// lag behind writes. The port searches with `contains` filters straight on the
// rows, so there is no index to sync: every committed write is searchable at
// once and every row policy still applies. This test pins that consistency and
// records the semantic difference (case-sensitive substring, no ranking).
test('FlaskBB search stays consistent with writes, row policy and restarts without an index (real PostgreSQL)', {timeout: 120000}, async () => {
  const fx = await startFixture({
    prefix: 'aip-flaskbb-search-',
    actors: {'alice-sub': 2, 'bob-sub': 3, 'carol-sub': 6},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','c',1);`);
      sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'Search Forum','a',1,false,false);`);
      sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,1,2),(3,1,3),(4,1,4),(5,1,5);`);
    },
  });
  try {
    const alice = fx.client('alice-sub');
    const bob = fx.client('bob-sub');
    const now = () => new Date().toISOString();
    let key = 0;
    const bundle = async (who, atomic) => {
      const result = await who.bundle({atomic}, {key: `search-${++key}`});
      assert.equal(result.ok, true, JSON.stringify(result));
      return result;
    };
    const startTopic = async (title, content) => {
      const topic = randomUUID();
      const post = randomUUID();
      await bundle(alice, [{create: 'Topic', values: {id: topic, forum: '1', title, user: '2', username: 'member-alice', locked: false, important: false, views: 0, published: false, deleted: false, hidden: false}}]);
      await bundle(alice, [{create: 'Post', values: {id: post, topic, forum: '1', user: '2', username: 'member-alice', content, hidden: false, published: false, version: 0, deleted: false}}]);
      await bundle(alice, [{apply: 'Post.publish', target: {ids: [post]}}]);
      await bundle(alice, [{create: 'TopicFirstPost', values: {topic, post}}]);
      return {topic, post};
    };
    const publish = topic => bundle(alice, [{apply: 'Topic.publish', target: {ids: [topic]}}]);
    // A fresh client per query: the SDK caches identical reads for up to 1 s per client instance (writes by
    // the same client invalidate it, writes by other users do not), which is not the search index lag under test.
    const topicsFor = async (who, term) => (await fx.client(`${who}-sub`).read({read: 'Topic', select: ['id', 'title'], filter: [{field: 'title', op: 'contains', value: term}]})).rows.map(r => r.title).sort();
    const postsFor = async (who, term) => (await fx.client(`${who}-sub`).read({read: 'Post', select: ['id', 'content'], filter: [{field: 'content', op: 'contains', value: term}]})).rows.map(r => r.content).sort();

    // Unpublished drafts are visible to their author only; publishing makes them searchable at once.
    const {topic, post} = await startTopic('Zebra migration notes', 'Herds cross the river in autumn');
    assert.deepEqual(await topicsFor('alice', 'Zebra'), ['Zebra migration notes'], 'author sees own draft');
    assert.deepEqual(await topicsFor('carol', 'Zebra'), [], 'other member does not see a draft');
    await publish(topic);
    assert.deepEqual(await topicsFor('carol', 'Zebra'), ['Zebra migration notes'], 'published topic is searchable immediately');
    assert.deepEqual(await postsFor('carol', 'autumn'), ['Herds cross the river in autumn']);

    // Editing replaces what the search finds (no stale index entry).
    await bundle(alice, [{apply: 'Post.edit', target: {ids: [post]}, values: {content: 'Herds cross the river in spring'}, expect: {version: 0}}]);
    assert.deepEqual(await postsFor('carol', 'autumn'), [], 'old text is gone at once');
    assert.deepEqual(await postsFor('carol', 'spring'), ['Herds cross the river in spring']);

    // Hiding removes the topic from members and keeps it for a viewer with viewhidden.
    await bundle(bob, [{apply: 'Topic.hide', target: {ids: [topic]}}]);
    assert.deepEqual(await topicsFor('carol', 'Zebra'), []);
    assert.deepEqual(await postsFor('carol', 'spring'), [], 'posts of a hidden topic are hidden too');
    assert.deepEqual(await topicsFor('bob', 'Zebra'), ['Zebra migration notes'], 'administrator still finds it');
    await bundle(bob, [{apply: 'Topic.unhide', target: {ids: [topic]}}]);
    assert.deepEqual(await topicsFor('carol', 'Zebra'), ['Zebra migration notes'], 'unhide restores the result at once');

    // Deleting removes it from the search for everyone who cannot see deleted rows.
    await bundle(bob, [{apply: 'Topic.delete', target: {ids: [topic]}}]);
    assert.deepEqual(await topicsFor('carol', 'Zebra'), []);
    assert.deepEqual(await topicsFor('alice', 'Zebra'), []);

    // Record the semantic difference to Whoosh: substring match, case-sensitive, wildcard characters are literal.
    const literal = await startTopic('Discount 100% off', 'a_b pattern');
    await publish(literal.topic);
    assert.deepEqual(await topicsFor('carol', 'Discount'), ['Discount 100% off']);
    assert.deepEqual(await topicsFor('carol', 'discount'), [], 'contains is case-sensitive (Whoosh is not)');
    assert.deepEqual(await topicsFor('carol', '100%'), ['Discount 100% off']);
    assert.deepEqual(await topicsFor('carol', '%'), ['Discount 100% off'], 'a percent sign is a literal, not a wildcard');
    assert.deepEqual(await postsFor('carol', 'a_b'), ['a_b pattern']);
    assert.deepEqual(await postsFor('carol', 'axb'), [], 'an underscore is a literal, not a wildcard');
    assert.deepEqual(await topicsFor('carol', 'off Discount'), [], 'no tokenization: word order matters');

    // No index to rebuild: a restart returns identical results.
    await fx.killServer();
    await fx.restart(new URL('../app.aip', import.meta.url).pathname);
    assert.deepEqual(await topicsFor('carol', 'Discount'), ['Discount 100% off']);

    console.log(JSON.stringify({schema: fx.schema, ok: true}));
  } finally {
    await fx.stop();
  }
});
