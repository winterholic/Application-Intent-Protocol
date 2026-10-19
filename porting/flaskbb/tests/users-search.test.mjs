import assert from 'node:assert/strict';
import {test} from 'node:test';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {join, resolve} from 'node:path';
import {spawn} from 'node:child_process';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';
import {stopOwnedProcess} from './mail-fixture.mjs';

// WBS 7.5.3 F15 member-list search. Upstream MemberList.post runs
// UserSearchForm (forum/forms.py: DataRequired + Length(min=3, max=50)) and an
// invalid form re-renders the full member list with errors. The port takes
// the query as GET /users?q= like /search and reads the declared User
// username.contains filter. Email is not searched (an email filter would let
// any member probe registered addresses; PRODUCT-GAPS.md G-4), and matching
// stays case-sensitive (strpos) until the Whoosh parity item lands — the
// case pin below must flip with it.
test('FlaskBB member list searches usernames within the original 3-50 bounds (real PostgreSQL)', {timeout: 120000}, async () => {
  const track = resolve(new URL('..', import.meta.url).pathname);
  const root = resolve(track, '../..');
  const python = process.env.AIP_FLASKBB_TEST_PYTHON;
  assert.ok(python, 'AIP_FLASKBB_TEST_PYTHON must name the run-owned UI Python environment');
  const fx = await startFixture({
    prefix: 'aip-flaskbb-usersearch-',
    actors: {'alice-sub': 2, 'carol-sub': 6},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
    },
  });
  const uiHosts = [];
  try {
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
      const res = await fetch(`${base}/users?${new URLSearchParams(query)}`, {headers: {Cookie: cookie}});
      assert.equal(res.status, status, `users ${JSON.stringify(query)}`);
      return res.text();
    };

    // The layout navbar prints the signed-in username (member-carol here), so
    // membership is judged on the member table rows only, never the full page.
    const listed = html => {
      const body = /<tbody>([\s\S]*?)<\/tbody>/.exec(html);
      assert.ok(body, 'member table rendered');
      return [...body[1].matchAll(/<strong>([^<]*)<\/strong>/g)].map(match => match[1]).sort();
    };

    const all = await page({});
    assert.ok(all.includes('href="/user/member-carol"'), 'precondition: the navbar names the signed-in user outside the table');
    assert.deepEqual(listed(all), ['admin-bob', 'guest-fixture', 'member-alice', 'member-carol', 'mod-editor', 'super-mod'], 'full list without a query');
    assert.ok(!all.includes('between 3 and 50'), 'no bounds error without a query');

    const byName = await page({q: 'alice'});
    assert.deepEqual(listed(byName), ['member-alice'], 'only the username match is listed; non-matching members stay out');
    assert.ok(byName.includes('Members matching "alice"'), 'results heading names the query');

    const shared = await page({q: 'member-'});
    assert.deepEqual(listed(shared), ['member-alice', 'member-carol'], 'shared substring matches both');

    const upper = await page({q: 'ALICE'});
    assert.deepEqual(listed(upper), [], 'matching is case-sensitive until Whoosh parity (flip with G-4)');

    const byEmail = await page({q: 'alice@example.test'});
    assert.deepEqual(listed(byEmail), [], 'registered emails are not searchable');

    // Invalid queries re-render the full list with errors, like the upstream form.
    for (const [q, text] of [['ab', 'between 3 and 50'], ['x'.repeat(51), 'between 3 and 50'], ['   ', 'required']]) {
      const html = await page({q});
      assert.ok(html.includes(text), `error for ${JSON.stringify(q)}`);
      assert.deepEqual(listed(html), listed(all), `full list fallback for ${JSON.stringify(q)}`);
      assert.ok(!html.includes('Members matching'), 'no results heading for an invalid query');
    }

    console.log(JSON.stringify({schema: fx.schema, ok: true}));
  } finally {
    await Promise.allSettled(uiHosts.map(child => stopOwnedProcess(child)));
    await fx.stop();
  }
});
