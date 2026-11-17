import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {test} from 'node:test';
import {GROUPS_SQL, USERS_SQL, seedNativePostOrders, startFixture} from './harness.mjs';

const fixture = JSON.parse(readFileSync(fileURLToPath(new URL('./post-order-regression.json', import.meta.url)), 'utf8'));
const unhideCases = fixture.unhide_cases;
test('Post.unhide follows the shared upstream pointer conditions and preserves stale projections', {timeout: 180000}, async () => {
  const cases = unhideCases.map((item, index) => ({...item, forumId: index + 1, index}));
  const postId = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`;
  const topicId = (n) => `00000000-0000-4000-9000-${String(n).padStart(12, '0')}`;
  const users = {2: 'member-alice', 3: 'admin-bob'};
  const fx = await startFixture({
    prefix: 'aip-flaskbb-post-unhide-', actors: {admin: 3},
    seed(sql, q, schema) {
      sql(GROUPS_SQL(schema)); sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category(id,title,description,position) VALUES(1,'General','fixture',1);`);
      for (const [index, item] of cases.entries()) {
        const forum = item.forumId;
        const topic = topicId(100 + index);
        const candidate = item.posts.find(post => post.id === item.candidate);
        const target = item.posts.find(post => post.id === item.target);
        const initial = item.forum;
        sql(`INSERT INTO ${q}.forum(id,category_id,title,description,position,locked,show_moderators) VALUES(${forum},1,'Forum ${item.name}','fixture',${forum},false,false);
          INSERT INTO ${q}.forum_group_access(id,forum_id,group_id) VALUES(${forum},${forum},3);
          INSERT INTO ${q}.topic(id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden)
            VALUES('${topic}',${forum},'${item.topic_title}',${item.topic_user},'${users[item.topic_user]}','2025-01-01T00:00:00Z','${item.topic_last_updated}',false,false,0,true,false,${item.topic_hidden});`);
        for (const post of [candidate, target]) {
          sql(`INSERT INTO ${q}.post(id,topic_id,forum_id,user_id,username,content,date_created,hidden,hidden_by_id,hidden_at,published,version,deleted)
            VALUES('${postId(post.id)}','${topic}',${forum},${post.user},'${users[post.user]}','body-${post.id}','${post.date_created}',${post.hidden},${post.hidden ? 3 : 'NULL'},${post.hidden ? "'2025-02-10T00:00:00Z'" : 'NULL'},true,0,false);`);
        }
        sql(`INSERT INTO ${q}.topic_first_post(topic_id,post_id) VALUES('${topic}','${postId(candidate.id)}');
          UPDATE ${q}.topic SET last_post_id='${postId(item.topic_last_post)}' WHERE id='${topic}';
          UPDATE ${q}.forum SET last_post_id='${postId(initial.last_post)}',last_post_user_id=${initial.last_post_user},
            last_post_title='${initial.last_post_title}',last_post_username='${initial.last_post_username}',
            last_post_created='${initial.last_post_created}' WHERE id=${forum};`);
      }
      seedNativePostOrders(sql, q, cases.flatMap(item => item.posts.map(post => postId(post.id))));
    },
  });
  const {q, sql} = fx;
  const failures = [];
  const check = (label, fn) => { try { fn(); } catch (error) { failures.push(`${label}: ${error.message.split('\n')[0]}`); } };
  const rows = item => {
    const forum = item.forumId;
    const topic = topicId(100 + item.index);
    return {
      topic: JSON.parse(sql(`SELECT jsonb_build_array(last_post_id,to_char(last_updated AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'))::text FROM ${q}.topic WHERE id='${topic}'`)),
      forum: JSON.parse(sql(`SELECT jsonb_build_array(last_post_id,last_post_user_id,last_post_title,last_post_username,to_char(last_post_created AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'))::text FROM ${q}.forum WHERE id=${forum}`)),
      hidden: sql(`SELECT hidden FROM ${q}.post WHERE id='${postId(item.target)}'`),
    };
  };
  try {
    const admin = fx.client('admin');
    for (const item of cases) {
      const target = postId(item.target);
      const result = await admin.bundle({atomic: [{apply: 'Post.unhide', target: {ids: [target]}}]}, {key: `post-unhide-${item.name}`});
      check(`${item.name} applies`, () => assert.equal(result.ok, true, JSON.stringify(result)));
      if (!result.ok) continue;
      const actual = rows(item);
      const expectedForum = item.expect.forum;
      check(`${item.name} Topic.last_post`, () => assert.equal(actual.topic[0], postId(item.expect.topic_last_post)));
      check(`${item.name} Topic.last_updated is preserved`, () => assert.equal(actual.topic[1], item.expect.topic_last_updated));
      check(`${item.name} Forum five projections`, () => assert.deepEqual(actual.forum, [
        expectedForum.last_post === null ? null : postId(expectedForum.last_post),
        expectedForum.last_post_user, expectedForum.last_post_title, expectedForum.last_post_username,
        expectedForum.last_post_created,
      ]));
      check(`${item.name} target is visible`, () => assert.equal(actual.hidden, 'f'));
    }
    assert.deepEqual(failures, [], failures.join('\n'));
  } finally { await fx.stop(); }
});
