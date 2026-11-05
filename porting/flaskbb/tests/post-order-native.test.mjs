import assert from 'node:assert/strict';
import {test} from 'node:test';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

// These are new, disposable native posts. No historical UUID/date ordering is
// inferred. A deliberately unmapped legacy row must fence further allocation.
test('FlaskBB native publish owns its PostOrder, replays once and fences unmapped legacy posts', {timeout: 90000}, async () => {
  const topic = '00000000-0000-4000-8000-100000000001';
  const first = 'ffffffff-ffff-4fff-8fff-ffffffffffff';
  const next = '00000000-0000-4000-8000-200000000002';
  const denied = '00000000-0000-4000-8000-200000000003';
  const legacy = '00000000-0000-4000-8000-200000000004';
  const fx = await startFixture({
    prefix: 'aip-flaskbb-native-order-', actors: {'alice-sub': 2, 'carol-sub': 6},
    seed(sql, q, schema) {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category(id,title,description,position) VALUES (1,'General','fixture',1);
        INSERT INTO ${q}.forum(id,category_id,title,description,position,locked,show_moderators)
          VALUES (1,1,'Forum','fixture',1,false,false);
        INSERT INTO ${q}.forum_group_access(id,forum_id,group_id) VALUES (1,1,2);
        INSERT INTO ${q}.topic(id,forum_id,title,user_id,username,locked,important,views,published,deleted,hidden)
          VALUES ('${topic}',1,'Native',2,'member-alice',false,false,0,false,false,false);`);
      for (const post of [first, next, denied]) {
        sql(`INSERT INTO ${q}.post(id,topic_id,forum_id,user_id,username,content,hidden,published,version,deleted)
          VALUES ('${post}','${topic}',1,2,'member-alice','fixture',false,false,0,false);`);
      }
    },
  });
  const {sql, q} = fx;
  const publish = post => ({atomic: [{apply: 'Post.publish', target: {ids: [post]}}]});
  try {
    const alice = fx.client('alice-sub');
    const outcome = await alice.bundle(publish(first), {key: 'native-order-first'});
    assert.equal(outcome.ok, true, JSON.stringify(outcome));
    const pair = sql(`SELECT p.source_order_id || '|' || o.post_id::text
      FROM ${q}.post p JOIN ${q}.post_order o ON o.id=p.source_order_id WHERE p.id='${first}'`);
    const [order, publicId] = pair.split('|');
    assert.match(order, /^[1-9][0-9]*$/);
    assert.equal(publicId, first, 'the reverse link must keep the public UUID');
    assert.equal(sql(`SELECT count(*) FROM ${q}.post_order`), '1');
    const before = sql(`SELECT last_value FROM ${q}.post_order_id_seq`);
    assert.equal((await alice.bundle(publish(first), {key: 'native-order-first'})).ok, true);
    assert.equal(sql(`SELECT count(*) FROM ${q}.post_order`), '1', 'idempotent replay allocates once');
    assert.equal(sql(`SELECT last_value FROM ${q}.post_order_id_seq`), before);

    const refusal = await fx.client('carol-sub').bundle(publish(denied), {key: 'native-order-denied'});
    assert.equal(refusal.ok, false);
    assert.equal(sql(`SELECT published FROM ${q}.post WHERE id='${denied}'`), 'f');
    assert.equal(sql(`SELECT last_value FROM ${q}.post_order_id_seq`), before, 'denial must not allocate');

    // Simulate a pre-upgrade published row without a verified source manifest.
    sql(`INSERT INTO ${q}.post(id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted)
      VALUES ('${legacy}','${topic}',1,2,'member-alice','legacy','2020-01-01T00:00:00Z',false,true,0,false);`);
    const blocked = await alice.bundle(publish(next), {key: 'native-order-unmapped'});
    assert.equal(blocked.ok, false, 'native allocation must wait for complete historical mappings');
    assert.equal(blocked.code, 'WIRE.FORBIDDEN');
    assert.equal(sql(`SELECT published || '|' || (source_order_id IS NULL) FROM ${q}.post WHERE id='${next}'`), 'false|true');
    assert.equal(sql(`SELECT last_value FROM ${q}.post_order_id_seq`), before);
    assert.equal(sql(`SELECT count(*) FROM ${q}.post_order`), '1');
    assert.equal(sql(`SELECT content FROM ${q}.post WHERE id='${legacy}'`), 'legacy');
    assert.equal(sql(`SELECT last_post_id::text FROM ${q}.forum WHERE id=1`), first, 'refusals preserve the published projection');
  } finally { await fx.stop(); }
});
