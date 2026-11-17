import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {test} from 'node:test';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

// Upstream Post._deal_with_last_post (forum/models.py 291-331) updates the Topic only
// when the pre-state Topic.last_post is this Post, and the Forum only when that same
// pointer is also Forum.last_post. The ORM test reads the same JSON cases.
const fixture = JSON.parse(readFileSync(fileURLToPath(new URL('./post-order-regression.json', import.meta.url)), 'utf8'));
const usernames = {2: 'member-alice', 3: 'admin-bob'};
const postId = n => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`;
const topicId = (c, key) => `00000000-0000-4000-9000-${String(c * 10 + key.charCodeAt(0) - 64).padStart(12, '0')}`;
const iso = value => new Date(value).toISOString().replace('.000Z', 'Z');

for (const action of ['hide', 'delete']) test(`Post.${action} updates Topic and Forum pointers only from the pre-state pointer conditions`, {timeout: 180000}, async () => {
  const cases = fixture.pointer_cases.map((item, index) => ({...item, forum: index + 1, index: index + 1}));
  const postsById = new Map(cases.flatMap(item => item.posts.map(post => [post.id, {...post, case: item}])));
  const forumProjection = (item, post) => {
    const found = postsById.get(post);
    return [postId(post), found.user, item.topics.find(topic => topic.key === found.topic).title, usernames[found.user], iso(found.date_created)];
  };
  const fx = await startFixture({
    prefix: 'aip-flaskbb-post-pointer-',
    actors: {admin: 3, carol: 6},
    seed(sql, q, schema) {
      sql(GROUPS_SQL(schema)); sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category(id,title,description,position) VALUES(1,'General','fixture',1);`);
      for (const item of cases) {
        sql(`INSERT INTO ${q}.forum(id,category_id,title,description,position,locked,show_moderators) VALUES(${item.forum},1,'${item.name}','fixture',${item.forum},false,false);
          INSERT INTO ${q}.forum_group_access(id,forum_id,group_id) VALUES(${item.forum * 2 - 1},${item.forum},3),(${item.forum * 2},${item.forum},2);`);
        for (const topic of item.topics) sql(`INSERT INTO ${q}.topic(id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden)
          VALUES('${topicId(item.index, topic.key)}',${item.forum},'${topic.title}',${topic.user},'${usernames[topic.user]}','2025-01-01T00:00:00Z','${topic.last_updated}',false,false,0,true,false,false);`);
        for (const post of item.posts) sql(`INSERT INTO ${q}.post(id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted)
          VALUES('${postId(post.id)}','${topicId(item.index, post.topic)}',${item.forum},${post.user},'${usernames[post.user]}','post-${post.id}','${post.date_created}',false,true,0,false);
          ${post.first ? `INSERT INTO ${q}.topic_first_post(topic_id,post_id) VALUES('${topicId(item.index, post.topic)}','${postId(post.id)}');` : ''}`);
      }
      // The original Post.id is the order key; UUID and date order are deliberately not used.
      sql(`INSERT INTO ${q}.post_order(id,post_id) OVERRIDING SYSTEM VALUE VALUES
        ${[...postsById.keys()].map(id => `(${id},'${postId(id)}')`).join(',')};
        UPDATE ${q}.post p SET source_order_id=o.id FROM ${q}.post_order o WHERE o.post_id=p.id;
        SELECT setval('${schema}.post_order_id_seq',1000,true);`);
      for (const item of cases) {
        for (const topic of item.topics) sql(`UPDATE ${q}.topic SET last_post_id='${postId(topic.last_post)}' WHERE id='${topicId(item.index, topic.key)}';`);
        const [id, user, title, username, created] = forumProjection(item, item.forum_last_post);
        sql(`UPDATE ${q}.forum SET last_post_id='${id}',last_post_user_id=${user},last_post_title='${title}',last_post_username='${username}',last_post_created='${created}' WHERE id=${item.forum};`);
      }
    },
  });
  const {q, sql} = fx;
  const forumRow = item => sql(`SELECT jsonb_build_array(last_post_id,last_post_user_id,last_post_title,last_post_username,to_char(last_post_created AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'))::text FROM ${q}.forum WHERE id=${item.forum}`);
  const topicRow = item => sql(`SELECT jsonb_agg(jsonb_build_array(id,last_post_id,to_char(last_updated AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"')) ORDER BY id)::text FROM ${q}.topic WHERE forum_id=${item.forum}`);
  const postRows = (item, except) => sql(`SELECT coalesce(jsonb_agg(to_jsonb(p) ORDER BY id),'[]')::text FROM ${q}.post p WHERE forum_id=${item.forum} AND id!='${except}'`);
  const everything = () => sql(`SELECT jsonb_build_array((SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM ${q}.forum f),(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM ${q}.topic t),(SELECT jsonb_agg(to_jsonb(p) ORDER BY id) FROM ${q}.post p))::text`);
  const failures = [];
  const check = (label, fn) => { try { fn(); } catch (error) { failures.push(`${label}: ${error.message.split('\n')[0]}`); } };
  try {
    const first = cases[0];
    const initial = everything();
    // A member without hide/delete rights and an absent target are rejected without any pointer or row change.
    const denied = await fx.client('carol').bundle({atomic: [{apply: `Post.${action}`, target: {ids: [postId(first.target)]}}]}, {key: `pointer-denied-${action}`});
    assert.equal(denied.ok, false, `non-moderator ${action} must be rejected`);
    assert.equal(everything(), initial, 'denied request changes nothing');
    const absent = await fx.client('admin').bundle({atomic: [{apply: `Post.${action}`, target: {ids: [postId(999)]}}]}, {key: `pointer-absent-${action}`});
    assert.equal(absent.ok, false, `absent ${action} target must be rejected`);
    assert.equal(everything(), initial, 'absent target changes nothing');

    for (const item of cases) {
      const target = postsById.get(item.target);
      const targetTopic = item.topics.find(topic => topic.key === target.topic);
      const before = {forums: cases.filter(other => other !== item).map(forumRow), topics: cases.filter(other => other !== item).map(topicRow)};
      const siblings = postRows(item, postId(item.target));
      const siblingTopics = JSON.parse(topicRow(item)).filter(row => row[0] !== topicId(item.index, targetTopic.key));
      const result = await fx.client('admin').bundle({atomic: [{apply: `Post.${action}`, target: {ids: [postId(item.target)]}}]}, {key: `pointer-${action}-${item.index}`});
      check(`${item.name} apply`, () => assert.equal(result.ok, true, JSON.stringify(result)));
      if (!result.ok) continue;

      check(`${item.name} Forum projection`, () => assert.deepEqual(JSON.parse(forumRow(item)), forumProjection(item, item.expect.forum_last_post)));
      const topic = JSON.parse(sql(`SELECT jsonb_build_array(last_post_id,to_char(last_updated AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'))::text FROM ${q}.topic WHERE id='${topicId(item.index, targetTopic.key)}'`));
      const spec = item.expect.target_topic;
      if (spec === 'unchanged') check(`${item.name} Topic pointer`, () => assert.deepEqual(topic, [postId(targetTopic.last_post), iso(targetTopic.last_updated)]));
      else {
        const allowed = (spec.last_post_one_of ?? [spec.last_post]).map(postId);
        check(`${item.name} Topic pointer set`, () => assert.ok(allowed.includes(topic[0]), `Topic.last_post ${topic[0]} not in ${allowed}`));
        // Upstream assigns last_post_id from posts[-2] but last_updated from the stale loaded relationship.
        check(`${item.name} Topic last_updated follows the upstream independent date rule`, () => assert.equal(topic[1], iso(postsById.get(spec.last_updated_post[action]).date_created)));
      }
      check(`${item.name} target flag`, () => assert.equal(sql(`SELECT ${action === 'hide' ? 'hidden AND hidden_by_id=3 AND hidden_at IS NOT NULL' : 'deleted'} FROM ${q}.post WHERE id='${postId(item.target)}'`), 't'));
      check(`${item.name} sibling Posts`, () => assert.equal(postRows(item, postId(item.target)), siblings));
      check(`${item.name} sibling Topics`, () => assert.deepEqual(JSON.parse(topicRow(item)).filter(row => row[0] !== topicId(item.index, targetTopic.key)), siblingTopics));
      check(`${item.name} unrelated forums`, () => assert.deepEqual({forums: cases.filter(other => other !== item).map(forumRow), topics: cases.filter(other => other !== item).map(topicRow)}, before));

      const applied = everything();
      const replay = await fx.client('admin').bundle({atomic: [{apply: `Post.${action}`, target: {ids: [postId(item.target)]}}]}, {key: `pointer-${action}-${item.index}`});
      check(`${item.name} same-key replay`, () => { assert.equal(replay.ok, true); assert.equal(everything(), applied); });
    }
    assert.deepEqual(failures, [], failures.join('\n'));
  } finally { await fx.stop(); }
});
