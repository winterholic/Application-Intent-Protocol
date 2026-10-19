import assert from 'node:assert/strict';
import {test} from 'node:test';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {join, resolve} from 'node:path';
import {spawn} from 'node:child_process';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';
import {stopOwnedProcess} from './mail-fixture.mjs';

// WBS 7.5.3 F15. Upstream whooshee_search matches every field its whoosheer
// indexes (utils/search.py): Post username/modified_by/content, Topic
// title/username/first-post content, Forum title/description, User
// username/email. The port filters one declared field per read, so the
// renderer issues one read per field and merges. Pinned deliberately:
// User.email stays out of the declared filters (an email filter lets any
// member probe registered addresses even though no email is ever returned),
// and Topic first-post content has no filter path (topic and post are
// separate resources). Both stay in PRODUCT-GAPS.md G-4.
test('FlaskBB search covers the original indexed fields and form bounds (real PostgreSQL)', {timeout: 120000}, async () => {
  const track = resolve(new URL('..', import.meta.url).pathname);
  const root = resolve(track, '../..');
  const python = process.env.AIP_FLASKBB_TEST_PYTHON;
  assert.ok(python, 'AIP_FLASKBB_TEST_PYTHON must name the run-owned UI Python environment');
  const id = (kind, n) => `00000000-0000-4000-8000-${kind}${String(n).padStart(11, '0')}`;
  const [t1, t2] = [id('3', 1), id('3', 2)];
  const [p1, p2] = [id('4', 1), id('4', 2)];
  const fx = await startFixture({
    prefix: 'aip-flaskbb-searchfields-',
    actors: {'alice-sub': 2, 'bob-sub': 3, 'carol-sub': 6},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','c',1);`);
      sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES
        (1,1,'Wildlife Forum','Quokka habitat talk',1,false,false);`);
      sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,1,2),(3,1,3),(4,1,4),(5,1,5);`);
      sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES
        ('${t1}',1,'Zebra stripes',2,'member-alice',now(),now(),false,false,0,true,false,false),
        ('${t2}',1,'Giraffe necks',6,'member-carol',now(),now(),false,false,0,true,false,false);`);
      sql(`INSERT INTO ${q}.post (id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted) VALUES
        ('${p1}','${t1}',1,2,'member-alice','savannah notes',now(),false,true,0,false),
        ('${p2}','${t2}',1,6,'member-carol','tall tales',now(),false,true,0,false);`);
      sql(`INSERT INTO ${q}.topic_first_post (topic_id,post_id) VALUES ('${t1}','${p1}'),('${t2}','${p2}');`);
    },
  });
  const uiHosts = [];
  try {
    // Admin edits Alice's post, which stamps modified_by with the editor.
    const edit = await fx.client('bob-sub').bundle({atomic: [{apply: 'Post.edit', target: {ids: [p1]}, values: {content: 'savannah notes (edited)'}, expect: {version: 0}}]}, {key: 'search-fields-edit'});
    assert.equal(edit.ok, true, JSON.stringify(edit));

    // Fresh client per read: the SDK caches identical reads for up to 1 s per client.
    const read = (who, resource, field, value, select) => fx.client(`${who}-sub`).read({read: resource, select, filter: [{field, op: 'contains', value}]});
    const ids = result => result.rows.map(r => r.id).sort();
    assert.deepEqual(ids(await read('carol', 'Topic', 'username', 'alice', ['id'])), [t1], 'topic author name');
    assert.deepEqual(ids(await read('carol', 'Post', 'username', 'carol', ['id'])), [p2], 'post author name');
    assert.deepEqual(ids(await read('carol', 'Post', 'modified_by', 'admin', ['id'])), [p1], 'post editor name');
    assert.deepEqual(ids(await read('carol', 'Forum', 'description', 'Quokka', ['id'])), ['1'], 'forum description');

    // Users are searched by username only. An email filter is not declared.
    await assert.rejects(() => read('carol', 'User', 'email', 'alice@', ['id']), /WIRE\.FILTER_NOT_ALLOWED/);

    const uiEnv = {
      PATH: process.env.PATH,
      PYTHONPATH: [join(track, 'ui'), join(root, 'product/python')].join(':'),
      FLASKBB_TEST_INTEGRATION: '1',
      FLASKBB_TEST_ACCESS_TOKEN: fx.token('carol-sub'),
      FLASKBB_TEST_SUBJECT: 'carol-sub',
      FLASKBB_TEST_USERNAME: 'member-carol',
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
    const page = async (query, status = 200) => {
      const res = await fetch(`${base}/search?${new URLSearchParams(query)}`, {headers: {Cookie: cookie}});
      assert.equal(res.status, status, `search ${JSON.stringify(query)}`);
      return res.text();
    };

    const byAuthor = await page({q: 'member-alice', type: 'all'});
    assert.ok(byAuthor.includes('Zebra stripes'), 'topic found by its author name');
    assert.ok(byAuthor.includes('savannah notes'), 'post found by its author name');
    assert.ok(!byAuthor.includes('Giraffe necks'), 'other author stays out');
    const byEditor = await page({q: 'admin-bob', type: 'post'});
    assert.ok(byEditor.includes('savannah notes (edited)'), 'post found by the name of its last editor');
    const byDescription = await page({q: 'Quokka', type: 'forum'});
    assert.ok(byDescription.includes('Wildlife Forum'), 'forum found by its description');
    const merged = await page({q: 'Zebra', type: 'topic'});
    assert.equal(merged.split('Zebra stripes').length - 1, 1, 'a topic matching one field is listed once');

    // The form bounds apply before any read; the results block never renders.
    for (const [q, text] of [['ab', 'between 3 and 50'], ['x'.repeat(51), 'between 3 and 50'], ['   ', 'required']]) {
      const html = await page({q, type: 'all'});
      assert.ok(html.includes(text), `error for ${JSON.stringify(q)}`);
      assert.ok(!html.includes('Search Results for'), 'no results block for an invalid query');
    }
    assert.ok((await page({q: 'Zebra', type: 'bogus'})).includes('not a valid'), 'unknown type is rejected');

    console.log(JSON.stringify({schema: fx.schema, ok: true}));
  } finally {
    await Promise.allSettled(uiHosts.map(child => stopOwnedProcess(child)));
    await fx.stop();
  }
});
