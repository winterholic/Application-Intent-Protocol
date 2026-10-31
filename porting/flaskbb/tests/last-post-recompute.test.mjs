import assert from 'node:assert/strict';
import {test} from 'node:test';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

// WBS 7.5.2 G-6. Upstream Post.hide/Post.delete call `_deal_with_last_post`: when the
// post is both its topic's and its forum's last post, the forum falls back to the newest
// remaining visible post (or NULL when none is left) and the topic's last_updated moves
// back to the new last post. Posts that are not the forum's last post leave the forum
// untouched. Pin both the restored projection and the absence of a half-null visible row.
test('FlaskBB forum and topic last-post projections recover after post hide/delete', {timeout: 90000}, async () => {
  const id = (kind, n) => `00000000-0000-4000-8000-${kind}${String(n).padStart(11, '0')}`;
  const topicA = id('1', 1);
  const topicB = id('1', 2);
  const [a1, a2, b1, b2] = [id('2', 1), id('2', 2), id('2', 3), id('2', 4)];
  const fx = await startFixture({
    prefix: 'aip-flaskbb-lastpost-',
    actors: {'alice-sub': 2, 'admin-sub': 3},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','c',1);`);
      sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'Forum','f',1,false,false);`);
      sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,1,2),(3,1,3);`);
      // Topic A: a1 (2025-01-01), a2 (2025-01-03). Topic B: b1 (2025-01-02), b2 (2025-01-04) is the forum's newest.
      sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES
        ('${topicA}',1,'Topic A',2,'member-alice','2025-01-01T12:00:00Z','2025-01-03T12:00:00Z',false,false,0,true,false,false),
        ('${topicB}',1,'Topic B',3,'admin-bob','2025-01-02T12:00:00Z','2025-01-04T12:00:00Z',false,false,0,true,false,false);`);
      for (const [post, topic, user, name, date] of [
        [a1, topicA, 2, 'member-alice', '2025-01-01T12:00:00Z'], [a2, topicA, 2, 'member-alice', '2025-01-03T12:00:00Z'],
        [b1, topicB, 3, 'admin-bob', '2025-01-02T12:00:00Z'], [b2, topicB, 3, 'admin-bob', '2025-01-04T12:00:00Z'],
      ]) sql(`INSERT INTO ${q}.post (id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted) VALUES ('${post}','${topic}',1,${user},'${name}','body','${date}',false,true,0,false);`);
      sql(`INSERT INTO ${q}.topic_first_post (topic_id,post_id) VALUES ('${topicA}','${a1}'),('${topicB}','${b1}');`);
      sql(`UPDATE ${q}.forum SET last_post_id='${b2}', last_post_user_id=3, last_post_title='Topic B', last_post_username='admin-bob', last_post_created='2025-01-04T12:00:00Z' WHERE id=1;`);
    },
  });
  const {sql, q} = fx;
  try {
    const admin = fx.client('admin-sub');
    const forumRow = () => sql(`SELECT concat_ws('|', last_post_id, last_post_username, last_post_title, to_char(last_post_created AT TIME ZONE 'UTC', 'YYYY-MM-DD')) FROM ${q}.forum WHERE id=1`);
    const topicUpdated = topic => sql(`SELECT to_char(last_updated AT TIME ZONE 'UTC', 'YYYY-MM-DD') FROM ${q}.topic WHERE id='${topic}'`);
    const run = async (apply, post, key) => {
      const result = await admin.bundle({atomic: [{apply, target: {ids: [post]}}]}, {key});
      assert.equal(result.ok, true, `${apply} ${post}: ${JSON.stringify(result)}`);
    };
    const before = forumRow();
    assert.equal(before, `${b2}|admin-bob|Topic B|2025-01-04`);

    // The separate upstream Topic.hide/delete path excludes the hidden/deleted topic before
    // selecting its replacement; it is not the Post._deal_with_last_post query.
    await run('Topic.hide', topicB, 'lp-hide-topic-b');
    const upstreamAfterTopicHide = `${a2}|member-alice|Topic A|2025-01-03`;
    assert.equal(forumRow(), upstreamAfterTopicHide);
    await run('Topic.unhide', topicB, 'lp-unhide-topic-b');
    assert.equal(forumRow(), before);

    // Upstream rule that already holds: a post that is not the forum's last post leaves the forum alone.
    await run('Post.hide', a2, 'lp-hide-a2');
    assert.equal(forumRow(), before, 'hiding a non-last forum post leaves Forum.last_post_*');
    await run('Post.unhide', a2, 'lp-unhide-a2');

    // Hiding the forum's last post falls back to the newest remaining visible post (a2).
    await run('Post.hide', b2, 'lp-hide-b2');
    const upstreamAfterHide = `${a2}|member-alice|Topic A|2025-01-03`;
    assert.equal(forumRow(), upstreamAfterHide);
    assert.equal(topicUpdated(topicB), '2025-01-02');

    // Ordinary readers must see the replacement post and its matching denormalized author.
    const alice = fx.client('alice-sub');
    const [forum] = (await alice.read({
      read: 'Forum', select: ['id', 'last_post', 'last_post_username', 'last_post_created'],
      filter: [{field: 'id', op: 'eq', value: '1'}], limit: 1,
    })).rows;
    assert.equal(forum.last_post, a2);
    assert.equal(forum.last_post_username, 'member-alice');

    // Delete (tombstone) of the forum's last post follows the same fallback.
    await run('Post.unhide', b2, 'lp-unhide-b2');
    assert.equal(forumRow(), before, 'unhide restores the pointer and projection');
    await run('Post.delete', b2, 'lp-delete-b2');
    assert.equal(forumRow(), upstreamAfterHide);
    assert.equal(topicUpdated(topicB), '2025-01-02');
    assert.equal(sql(`SELECT deleted FROM ${q}.post WHERE id='${b2}'`), 't');

    await run('Topic.delete', topicB, 'lp-delete-topic-b');
    assert.equal(forumRow(), upstreamAfterTopicHide);
  } finally { await fx.stop(); }
});
