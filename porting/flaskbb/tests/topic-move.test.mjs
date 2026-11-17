import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {test} from 'node:test';
import {GROUPS_SQL, USERS_SQL, seedNativePostOrders, startFixture} from './harness.mjs';

const fixture = JSON.parse(readFileSync(fileURLToPath(new URL('./topic-move-regression.json', import.meta.url)), 'utf8'));
const uuid = (kind, n) => `00000000-0000-4000-8000-${kind}${String(n).padStart(11, '0')}`;
const topicId = item => uuid('1', item.source_id);
const postId = item => uuid('2', item.source_id);

test('Topic.move is atomic, checks both forum moderators, refreshes date projections, and clears moved reads', {timeout: 90000}, async () => {
  const topics = Object.fromEntries(fixture.topics.map(item => [item.key, topicId(item)]));
  const posts = Object.fromEntries(fixture.posts.map(item => [item.source_id, postId(item)]));
  let fx;
  try { fx = await startFixture({
    prefix: 'aip-flaskbb-topic-move-',
    actors: {'alice-sub': 2, 'admin-sub': 3, 'carol-sub': 6},
    serviceExtra: {workers: {
      directory: fileURLToPath(new URL('../mail/', import.meta.url)),
      language: 'python',
      // Ubuntu runner system Python lives under /usr, which the sandbox already
      // ro-binds. Do not use the oracle venv or the setup-python toolcache.
      runtime_executable: '/usr/bin/python3',
      // Same value mail-jobs.test.mjs sets on Linux; keep the OS isolation on.
      isolation: process.platform === 'darwin' ? 'mac_net_deny' : 'auto',
      enable_writes: true,
      // Enabling write extensions also validates the app's existing durable
      // jobs. This local-only broker satisfies their declared http.mail grant;
      // the Topic.move fixture never invokes it.
      integrations: [{
        name: 'mail',
        url: 'http://127.0.0.1:58080/api/v1/send',
        method: 'POST',
        timeout_ms: 5000,
        max_request_bytes: 65536,
        max_response_bytes: 65536,
        allow_loopback: true,
      }],
    }},
    seed(sql, q, schema) {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category(id,title,description,position) VALUES (1,'General','move fixture',1);
        INSERT INTO ${q}.forum(id,category_id,title,description,position,locked,show_moderators) VALUES
          (1,1,'Source','fixture',1,false,false),(2,1,'Destination','fixture',2,false,false),(3,1,'Failing destination','fixture',3,false,false);
        INSERT INTO ${q}.forum_group_access(id,forum_id,group_id) VALUES
          (1,1,1),(2,1,2),(3,1,3),(4,2,1),(5,2,2),(6,2,3),(7,3,1),(8,3,2),(9,3,3);
        INSERT INTO ${q}.forum_moderator(id,forum_id,user_id) VALUES (1,1,2),(2,2,6);
        INSERT INTO ${q}.topic(id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES
          ('${topics.moving}',1,'Moving topic',2,'member-alice','2025-01-01T12:00:00Z','2025-01-05T12:00:00Z',false,false,0,true,false,false),
          ('${topics.source_remaining}',1,'Source remains',2,'member-alice','2025-01-03T12:00:00Z','2025-01-03T12:00:00Z',false,false,0,true,false,false),
          ('${topics.destination_existing}',2,'Destination existing',3,'admin-bob','2025-01-04T12:00:00Z','2025-01-04T12:00:00Z',false,false,0,true,false,false);`);
      for (const item of fixture.posts) {
        const topic = topics[item.topic];
        sql(`INSERT INTO ${q}.post(id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted)
          VALUES ('${posts[item.source_id]}','${topic}',${fixture.topics.find(t => t.key === item.topic).forum},${item.user},'${item.user === 2 ? 'member-alice' : 'admin-bob'}','post-${item.source_id}','${item.date_created}',false,true,0,false);`);
        if (item.first) sql(`INSERT INTO ${q}.topic_first_post(topic_id,post_id) VALUES ('${topic}','${posts[item.source_id]}');`);
      }
      for (const item of fixture.topics) sql(`UPDATE ${q}.topic SET last_post_id='${posts[item.last_post]}' WHERE id='${topics[item.key]}';`);
      sql(`UPDATE ${q}.forum SET last_post_id='${posts[42]}',last_post_user_id=2,last_post_title='Moving topic',last_post_username='member-alice',last_post_created='2025-01-05T12:00:00Z' WHERE id=1;
        UPDATE ${q}.forum SET last_post_id='${posts[44]}',last_post_user_id=3,last_post_title='Destination existing',last_post_username='admin-bob',last_post_created='2025-01-04T12:00:00Z' WHERE id=2;`);
      fixture.read_rows.forEach((row, index) => sql(`INSERT INTO ${q}.topic_read(id,user_id,topic_id,forum_id,last_read)
        VALUES (${index + 1},${row.user},'${topics[row.topic]}',${row.forum},'2025-01-06T12:00:00Z');`));
      // Source order disagrees with date order across forums: moved Post 42 sorts before Post 44 here,
      // but Forum.update_last_post chooses by date_created and therefore selects Post 42 after the move.
      seedNativePostOrders(sql, q, fixture.post_order_source_ids.map(sourceId => posts[sourceId]));
    },
  }); } catch (error) {
    console.error(`TOPIC_MOVE_SETUP_FAIL stage=fixture-init error=${error.stack || error.message}`);
    throw error;
  }
  const {sql, q} = fx;
  const snapshot = () => sql(`SELECT concat_ws('|', (SELECT forum_id FROM ${q}.topic WHERE id='${topics.moving}'),
    (SELECT string_agg(id::text || ':' || coalesce(last_post_id::text,'null') || ':' || coalesce(last_post_user_id::text,'null') || ':' || coalesce(last_post_title,'null') || ':' || coalesce(last_post_username,'null') || ':' || coalesce(last_post_created::text,'null'), ',' ORDER BY id) FROM ${q}.forum),
    (SELECT string_agg(f.id::text || ':' || (SELECT count(*) FROM ${q}.topic t WHERE t.forum_id=f.id AND t.published AND NOT t.deleted)::text || ':' || (SELECT count(*) FROM ${q}.post p JOIN ${q}.topic t ON t.id=p.topic_id WHERE p.forum_id=f.id AND p.published AND NOT p.deleted AND NOT t.deleted)::text, ',' ORDER BY f.id) FROM ${q}.forum f),
    (SELECT string_agg(id::text || ':' || forum_id::text || ':' || title || ':' || coalesce(last_post_id::text,'null') || ':' || published::text || ':' || deleted::text, ',' ORDER BY id) FROM ${q}.topic),
    (SELECT string_agg(id::text || ':' || user_id::text || ':' || topic_id::text || ':' || forum_id::text || ':' || last_read::text, ',' ORDER BY id) FROM ${q}.topic_read),
    (SELECT string_agg(id::text || ':' || topic_id::text || ':' || forum_id::text || ':' || content || ':' || date_created::text || ':' || hidden::text || ':' || published::text || ':' || deleted::text, ',' ORDER BY id) FROM ${q}.post))`);
  const resetBaseline = () => {
    sql(`UPDATE ${q}.topic SET forum_id=CASE id WHEN '${topics.moving}' THEN 1 WHEN '${topics.source_remaining}' THEN 1 ELSE 2 END;
      UPDATE ${q}.post p SET forum_id=t.forum_id FROM ${q}.topic t WHERE p.topic_id=t.id;
      UPDATE ${q}.forum SET last_post_id=NULL,last_post_user_id=NULL,last_post_title=NULL,last_post_username=NULL,last_post_created=NULL;
      UPDATE ${q}.forum SET last_post_id='${posts[42]}',last_post_user_id=2,last_post_title='Moving topic',last_post_username='member-alice',last_post_created='2025-01-05T12:00:00Z' WHERE id=1;
      UPDATE ${q}.forum SET last_post_id='${posts[44]}',last_post_user_id=3,last_post_title='Destination existing',last_post_username='admin-bob',last_post_created='2025-01-04T12:00:00Z' WHERE id=2;
      DELETE FROM ${q}.topic_read;
      INSERT INTO ${q}.topic_read(id,user_id,topic_id,forum_id,last_read) VALUES
        (1,2,'${topics.moving}',1,'2025-01-06T12:00:00Z'),
        (2,2,'${topics.source_remaining}',1,'2025-01-06T12:00:00Z'),
        (3,3,'${topics.destination_existing}',2,'2025-01-06T12:00:00Z');`);
  };
  try {
    let setup;
    try {
      setup = sql(`SELECT concat_ws('|', (SELECT count(*) FROM ${q}.forum), (SELECT count(*) FROM ${q}.topic),
        (SELECT count(*) FROM ${q}.post), (SELECT count(*) FROM ${q}.topic_read),
        (SELECT count(*) FROM ${q}.forum_moderator WHERE user_id=2 AND forum_id=1),
        (SELECT count(*) FROM ${q}.forum_moderator WHERE user_id=2 AND forum_id=2)`);
      assert.equal(setup, '3|3|4|3|1|0', `setup seed/role mismatch: ${setup}`);
      assert.equal(sql(`SELECT string_agg(topic_id::text, ',' ORDER BY topic_id) FROM ${q}.topic_read`), `${topics.moving},${topics.source_remaining},${topics.destination_existing}`);
      console.log(`TOPIC_MOVE_SETUP_OK schema=${fx.schema} rows=${setup} source_moderator=1 destination_moderator=0 actors=bound`);
    } catch (error) {
      console.error(`TOPIC_MOVE_SETUP_FAIL stage=seed-validation rows=${setup || 'unavailable'} snapshot=${snapshot()} error=${error.message}`);
      throw error;
    }
    const post = async (subject, key, request) => {
      const response = await fetch(`http://${fx.address}/apply`, {
        method: 'POST',
        headers: {
          authorization: `Bearer ${fx.token(subject)}`,
          'x-aip-contract': fx.contract.fingerprint,
          'content-type': 'application/json',
        },
        body: JSON.stringify({key, request}),
      });
      const bodyText = await response.text();
      let body;
      try { body = JSON.parse(bodyText); }
      catch { body = {unparsed: bodyText}; }
      return {status: response.status, body, bodyText};
    };
    const move = (subject, topic, destination, key) => post(subject, key, {extension: 'Topic.moveToForum', input: {topic, destination}});
    const failures = [];
    let lastResponse = null;
    const journey = async (name, action) => {
      let before = 'snapshot-unavailable';
      lastResponse = null;
      try {
        resetBaseline();
        before = snapshot();
        await action(before);
        console.log(`TOPIC_MOVE_JOURNEY_OK name=${name} response=${JSON.stringify(lastResponse)} snapshot=${snapshot()}`);
      } catch (error) {
        const after = snapshot();
        const record = {name, error: error.message, response: lastResponse, before, after};
        failures.push(record);
        console.error(`TOPIC_MOVE_JOURNEY_FAIL ${JSON.stringify(record)}`);
      }
    };

    await journey('same-forum-noop', async before => {
      lastResponse = await move('admin-sub', topics.moving, '1', 'topic-move-same');
      assert.equal(lastResponse.status, 200, JSON.stringify(lastResponse));
      assert.equal(lastResponse.body.ok, true, JSON.stringify(lastResponse));
      assert.deepEqual(lastResponse.body.output, {moved: false});
      assert.equal(snapshot(), before);
    });
    await journey('destination-moderator-denial', async before => {
      lastResponse = await move('alice-sub', topics.moving, '2', 'topic-move-denied');
      assert.equal(lastResponse.body.ok, false, JSON.stringify(lastResponse));
      assert.equal(snapshot(), before);
    });
    await journey('source-moderator-denial', async before => {
      lastResponse = await move('carol-sub', topics.moving, '2', 'topic-move-source-denied');
      assert.equal(lastResponse.body.ok, false, JSON.stringify(lastResponse));
      assert.equal(snapshot(), before);
    });
    await journey('internal-transition-direct-http-denial', async before => {
      lastResponse = await post('admin-sub', 'topic-move-internal-direct', {
        atomic: [{apply: 'Topic.relocate', target: {ids: [topics.moving]}, values: {forum: '2'}}],
      });
      assert.equal(lastResponse.status, 200, JSON.stringify(lastResponse));
      assert.equal(lastResponse.body.ok, false, JSON.stringify(lastResponse));
      assert.equal(lastResponse.body.code, 'WIRE.NOT_EXPOSED', JSON.stringify(lastResponse));
      assert.equal(snapshot(), before);
    });
    await journey('projection-failure-atomicity', async before => {
      sql(`CREATE FUNCTION ${q}.fail_move_projection() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW.id=3 AND NEW.last_post_id IS DISTINCT FROM OLD.last_post_id THEN RAISE EXCEPTION 'injected move projection failure'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER fail_move_projection BEFORE UPDATE ON ${q}.forum FOR EACH ROW EXECUTE FUNCTION ${q}.fail_move_projection();`);
      try {
        lastResponse = await move('admin-sub', topics.moving, '3', 'topic-move-rollback');
      } finally {
        sql(`DROP TRIGGER IF EXISTS fail_move_projection ON ${q}.forum; DROP FUNCTION IF EXISTS ${q}.fail_move_projection();`);
      }
      assert.equal(lastResponse.body.ok, false, JSON.stringify(lastResponse));
      assert.equal(snapshot(), before, 'failed projection update must preserve every seeded reference and row');
    });
    await journey('normal-two-forum-move', async () => {
      lastResponse = await move('admin-sub', topics.moving, '2', 'topic-move-success');
      assert.equal(lastResponse.body.ok, true, JSON.stringify(lastResponse));
      assert.deepEqual(lastResponse.body.output, {moved: true});
      const moved = fixture.after_move;
      assert.equal(sql(`SELECT forum_id FROM ${q}.topic WHERE id='${topics.moving}'`), String(moved.topic_forum));
      assert.equal(sql(`SELECT string_agg(forum_id::text, ',' ORDER BY id) FROM ${q}.post WHERE topic_id='${topics.moving}'`), '2,2', 'AIP Post.forum must follow Topic.forum');
      assert.equal(sql(`SELECT last_post_id FROM ${q}.forum WHERE id=1`), posts[moved.source_last_post]);
      assert.equal(sql(`SELECT last_post_id FROM ${q}.forum WHERE id=2`), posts[moved.destination_last_post]);
      assert.equal(sql(`SELECT string_agg(id::text, ',' ORDER BY id) FROM ${q}.post WHERE topic_id='${topics.moving}'`), `${posts[41]},${posts[42]}`);
      assert.equal(sql(`SELECT count(*) FROM ${q}.topic_read WHERE topic_id='${topics.moving}'`), '0');
      assert.equal(sql(`SELECT count(*) FROM ${q}.topic_read`), String(moved.remaining_read_rows));
      assert.equal(sql(`SELECT count(*) FROM ${q}.topic WHERE id IN ('${topics.source_remaining}','${topics.destination_existing}')`), '2');
      assert.equal(sql(`SELECT count(*) FROM ${q}.post WHERE id IN ('${posts[41]}','${posts[42]}','${posts[43]}','${posts[44]}')`), '4');
      assert.equal(sql(`SELECT string_agg(f.id::text || ':' || (SELECT count(*) FROM ${q}.topic t WHERE t.forum_id=f.id AND t.published AND NOT t.deleted)::text || ':' || (SELECT count(*) FROM ${q}.post p JOIN ${q}.topic t ON t.id=p.topic_id WHERE p.forum_id=f.id AND p.published AND NOT p.deleted AND NOT t.deleted)::text, ',' ORDER BY f.id) FROM ${q}.forum f`), '1:1:1,2:2:3,3:0:0', 'Forum counter semantics must follow the relocated AIP references');
      console.log(`TOPIC_MOVE_ORACLE_TARGET source_last=${moved.source_last_post} destination_last=${moved.destination_last_post} post_relation=topic_id reads_removed=1`);
    });
    await journey('empty-source-forum-projection', async () => {
      lastResponse = await move('admin-sub', topics.moving, '2', 'topic-move-empty-source-first');
      assert.equal(lastResponse.body.ok, true, JSON.stringify(lastResponse));
      assert.deepEqual(lastResponse.body.output, {moved: true});
      lastResponse = await move('admin-sub', topics.source_remaining, '2', 'topic-move-empty-source');
      assert.equal(lastResponse.body.ok, true, JSON.stringify(lastResponse));
      assert.deepEqual(lastResponse.body.output, {moved: true});
      assert.equal(sql(`SELECT concat_ws('|',last_post_id,last_post_user_id,last_post_title,last_post_username,last_post_created) FROM ${q}.forum WHERE id=1`), '||||');
    });
    assert.deepEqual(failures, [], `Topic.move HTTP journey failures: ${JSON.stringify(failures)}`);
  } finally { await fx.stop(); }
});
