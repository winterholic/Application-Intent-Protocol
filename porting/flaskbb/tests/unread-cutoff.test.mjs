import assert from 'node:assert/strict';
import {test} from 'node:test';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {join, resolve} from 'node:path';
import {spawn} from 'node:child_process';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';
import {stopOwnedProcess} from './mail-fixture.mjs';

// WBS 7.5.2 unread cutoff. Upstream computes forum/topic unread at render time
// (helpers.py topic_is_unread/forum_is_unread): guests read everything, a zero
// TRACKER_LENGTH disables the tracker, and anything older than
// `now - TRACKER_LENGTH days` (fixture default 7) reads as read. Otherwise a
// topic is unread when its last update beats the viewer's topic marker (or the
// forum cleared stamp when no topic marker exists), and a forum is unread when
// its last post beats the cutoff and the viewer's forum marker. The port keeps
// that computation in the renderer (ui/read_tracking.py): AIP reads expose
// select/filter/sort only, so the cutoff cannot be declared, and the viewer
// session cannot read the Setting resource (admin/super_mod only), so the
// renderer uses the fixture default of 7 days (see PRODUCT-GAPS.md).
test('FlaskBB unread cutoff renders from read markers and post age (real PostgreSQL)', {timeout: 120000}, async () => {
  const track = resolve(new URL('..', import.meta.url).pathname);
  const root = resolve(track, '../..');
  const python = process.env.AIP_FLASKBB_TEST_PYTHON;
  assert.ok(python, 'AIP_FLASKBB_TEST_PYTHON must name the run-owned UI Python environment');
  const id = (kind, n) => `00000000-0000-4000-8000-${kind}${String(n).padStart(11, '0')}`;
  const [t1, t2, t3, t4, t5, t6] = [id('3', 1), id('3', 2), id('3', 3), id('3', 4), id('3', 5), id('3', 6)];
  const [p1, p5, p6] = [id('4', 1), id('4', 5), id('4', 6)];
  const fx = await startFixture({
    prefix: 'aip-flaskbb-unread-',
    actors: {'alice-sub': 2},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','c',1);`);
      sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES
        (1,1,'Fresh Forum','f',1,false,false),(2,1,'Cleared Forum','f',2,false,false),(3,1,'Quiet Forum','f',3,false,false);`);
      sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES
        (1,1,1),(2,1,2),(3,1,3),(4,2,1),(5,2,2),(6,2,3),(7,3,1),(8,3,2),(9,3,3);`);
      sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES
        ('${t1}',1,'Never opened',2,'member-alice',now() - interval '1 hour',now() - interval '1 hour',false,false,0,true,false,false),
        ('${t2}',1,'Already read',2,'member-alice',now() - interval '1 hour',now() - interval '1 hour',false,false,0,true,false,false),
        ('${t3}',1,'Stale marker',2,'member-alice',now() - interval '1 hour',now() - interval '1 hour',false,false,0,true,false,false),
        ('${t4}',1,'Below cutoff',2,'member-alice',now() - interval '30 days',now() - interval '30 days',false,false,0,true,false,false),
        ('${t5}',2,'Cleared forum topic',2,'member-alice',now() - interval '1 hour',now() - interval '1 hour',false,false,0,true,false,false),
        ('${t6}',3,'Quiet old topic',2,'member-alice',now() - interval '30 days',now() - interval '30 days',false,false,0,true,false,false);`);
      for (const [post, topic, forum, age] of [
        [p1, t1, 1, '1 hour'], [p5, t5, 2, '1 hour'], [p6, t6, 3, '30 days'],
      ]) sql(`INSERT INTO ${q}.post (id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted) VALUES ('${post}','${topic}',${forum},2,'member-alice','body',now() - interval '${age}',false,true,0,false);`);
      sql(`INSERT INTO ${q}.topic_first_post (topic_id,post_id) VALUES ('${t1}','${p1}'),('${t5}','${p5}'),('${t6}','${p6}');`);
      sql(`UPDATE ${q}.forum SET last_post_id='${p1}', last_post_user_id=2, last_post_title='Never opened', last_post_username='member-alice', last_post_created=now() - interval '1 hour' WHERE id=1;`);
      sql(`UPDATE ${q}.forum SET last_post_id='${p5}', last_post_user_id=2, last_post_title='Cleared forum topic', last_post_username='member-alice', last_post_created=now() - interval '1 hour' WHERE id=2;`);
      sql(`UPDATE ${q}.forum SET last_post_id='${p6}', last_post_user_id=2, last_post_title='Quiet old topic', last_post_username='member-alice', last_post_created=now() - interval '30 days' WHERE id=3;`);
    },
  });
  const uiHosts = [];
  try {
    const alice = fx.client('alice-sub');
    const fresh = new Date().toISOString();
    const stale = new Date(Date.now() - 2 * 3600_000).toISOString();
    const mark = async (create, values, key) => {
      const result = await alice.bundle({atomic: [{create, values}]}, {key});
      assert.equal(result.ok, true, `${key}: ${JSON.stringify(result)}`);
    };
    await mark('TopicRead', {user: '2', topic: t2, forum: '1', last_read: fresh}, 'unread-mark-t2');
    await mark('TopicRead', {user: '2', topic: t3, forum: '1', last_read: stale}, 'unread-mark-t3');
    await mark('ForumRead', {user: '2', forum: '2', last_read: fresh, cleared: fresh}, 'unread-mark-f2');

    const uiEnv = {
      PATH: process.env.PATH,
      PYTHONPATH: [join(track, 'ui'), join(root, 'product/python')].join(':'),
      FLASKBB_TEST_INTEGRATION: '1',
      FLASKBB_TEST_ACCESS_TOKEN: fx.token('alice-sub'),
      FLASKBB_TEST_SUBJECT: 'alice-sub',
      FLASKBB_TEST_USERNAME: 'member-alice',
      FLASKBB_TEST_AIP_URL: `http://${fx.address}`,
    };
    const host = spawn(python, ['tests/run_ui_host.py'], {cwd: track, env: uiEnv, stdio: ['ignore', 'pipe', 'pipe']});
    host.unref();
    uiHosts.push(host);
    host.stderrText = '';
    host.stderr.on('data', data => {host.stderrText += data;});
    const lines = createInterface({input: host.stdout});
    const listening = await Promise.race([
      once(lines, 'line').then(([line]) => JSON.parse(line)),
      once(host, 'error').then(([error]) => {throw error;}),
      once(host, 'exit').then(([code]) => {throw new Error(`UI host exit ${code}: ${host.stderrText}`);}),
      new Promise((_, reject) => setTimeout(() => reject(new Error(`UI host timeout: ${host.stderrText}`)), 15000).unref()),
    ]);
    lines.close();
    host.stdout.destroy();
    host.stderr.destroy();
    const base = `http://127.0.0.1:${listening.port}`;
    const cookie = 'aip_session=aip-integration-session';

    const get = async path => {
      const res = await fetch(`${base}${path}`, {headers: {Cookie: cookie}});
      assert.equal(res.status, 200, `${path} renders`);
      return res.text();
    };
    // Forum 1 mixes all four topic branches: no marker, fresh marker, stale
    // marker, and below the 7-day cutoff.
    const forum1 = await get('/forum/1');
    const topicRow = title => forum1.split('forum-row').find(chunk => chunk.includes(title));
    assert.ok(topicRow('Never opened').includes('topic-unread'), 'fresh topic without a marker is unread');
    assert.ok(topicRow('Already read').includes('topic-read'), 'fresh topic with a fresh marker is read');
    assert.ok(!topicRow('Already read').includes('topic-unread'), 'read topic carries no unread icon');
    assert.ok(topicRow('Stale marker').includes('topic-unread'), 'fresh topic with a stale marker is unread');
    assert.ok(topicRow('Below cutoff').includes('topic-read'), 'topic older than the cutoff is read');
    // Forum 2 was cleared after its last post, so its topic reads as read
    // without any topic marker.
    const forum2 = await get('/forum/2');
    const clearedRow = forum2.split('forum-row').find(chunk => chunk.includes('Cleared forum topic'));
    assert.ok(clearedRow.includes('topic-read'), 'topic covered by a forum cleared stamp is read');
    // The index shows one unread forum (fresh last post, no marker) and two
    // read forums (cleared; last post below the cutoff).
    const index = await get('/');
    const forumRow = title => index.split('category-row').find(chunk => chunk.includes(title));
    assert.ok(forumRow('Fresh Forum').includes('forum-unread'), 'forum with a fresh last post and no marker is unread');
    assert.ok(forumRow('Cleared Forum').includes('forum-read'), 'cleared forum is read');
    assert.ok(forumRow('Quiet Forum').includes('forum-read'), 'forum whose last post is below the cutoff is read');
    assert.ok(!forumRow('Quiet Forum').includes('forum-unread'), 'old forum carries no unread icon');

    console.log(JSON.stringify({schema: fx.schema, ok: true}));
  } finally {
    await Promise.allSettled(uiHosts.map(child => stopOwnedProcess(child)));
    await fx.stop();
  }
});
