import assert from 'node:assert/strict';
import {test} from 'node:test';
import {readFileSync, writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {spawnSync} from 'node:child_process';
import {binary, definition, dbUrl, root, startFixture} from './harness.mjs';

// 7.5.3 F13: the original CLI lifecycle (install, populate, db upgrade, users,
// shell/inspection) on a clean AIP-owned schema in real PostgreSQL:
//   service init -> seed (fixtures) -> principal bind -> serve -> migrate -> audit/jobs.
const fixtures = join(root, 'porting/flaskbb/upstream/flaskbb/fixtures');
const installer = join(root, 'porting/flaskbb/seed/install.py');

test('FlaskBB CLI lifecycle: init, install seed, bind, serve, migrate, audit, revoke (real PostgreSQL)', {timeout: 120000}, async () => {
  const fx = await startFixture({prefix: 'aip-flaskbb-cli-', seed: undefined});
  const {sql, q, schema, run, client, config, temp} = fx;
  try {
    // --- `flaskbb install`: groups, settings, admin user, welcome forum from the pinned upstream fixtures
    const expectedGroups = (readFileSync(join(fixtures, 'groups.py'), 'utf8').match(/^    \('[A-Za-z ]+', \{/gm) || []).length;
    const expectedSettings = (readFileSync(join(fixtures, 'settings.py'), 'utf8').match(/"value_type":/g) || []).length;
    assert.equal(expectedGroups, 6);
    assert.equal(expectedSettings, 29);
    const seedSql = run('python3', [installer, '--schema', schema, '--admin-username', 'admin', '--admin-email', 'admin@example.test', '--welcome']);
    const psql = input => spawnSync('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-q', '-At', '-d', dbUrl], {input, encoding: 'utf8'});
    const applied = psql(seedSql);
    assert.equal(applied.status, 0, applied.stderr);
    const count = table => Number(sql(`SELECT count(*) FROM ${q}.${table}`));
    assert.deepEqual(
      ['"group"', 'settings_group', 'setting', '"user"', 'category', 'forum', 'forum_group_access', 'topic', 'post'].map(count),
      [expectedGroups, 4, expectedSettings, 1, 1, 1, expectedGroups, 1, 1],
    );
    assert.equal(sql(`SELECT name FROM ${q}."group" WHERE id = 1`), 'Administrator');
    assert.equal(sql(`SELECT value_json FROM ${q}.setting WHERE key = 'posts_per_page'`), '10');
    // A second install must fail as a whole and change nothing (the original asks before dropping).
    const again = psql(seedSql);
    assert.notEqual(again.status, 0, 'a repeated install must not silently succeed');
    assert.equal(count('setting'), expectedSettings);
    assert.equal(count('"group"'), expectedGroups);

    // --- `flaskbb users new`: bind the identity provider subject to the seeded admin actor
    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'admin-sub', '--actor', '1']);
    const admin = client('admin-sub');
    const groups = (await admin.read({read: 'Group', select: ['id', 'name'], sort: [{field: 'id'}]})).rows;
    assert.equal(groups.length, expectedGroups);
    assert.equal(groups[0].name, 'Administrator');
    const settings = (await admin.read({read: 'Setting', select: ['key']})).rows;
    assert.equal(settings.length, expectedSettings);
    const topics = (await admin.read({read: 'Topic', select: ['id', 'title', 'post_count']})).rows;
    assert.deepEqual(topics.map(t => [t.title, String(t.post_count)]), [['Welcome!', '1']]);

    // --- business audit records committed transitions (not creates): who changed which row, before and after
    const created = await admin.bundle({atomic: [{create: 'Category', values: {title: 'CLI Category', description: 'audit probe', position: 9}}]}, {key: 'cli-category'});
    assert.equal(created.ok, true, JSON.stringify(created));
    const categoryId = String(created.created[0].id);
    const edited = await admin.bundle({atomic: [{apply: 'Category.edit', target: {ids: [categoryId]}, values: {title: 'CLI Category Renamed', position: 9}}]}, {key: 'cli-category-edit'});
    assert.equal(edited.ok, true, JSON.stringify(edited));
    const audit = JSON.parse(run(binary, ['service', 'audit', '--config', config, 'list', '--resource', 'Category', '--target-id', categoryId]));
    assert.equal(audit.ok, true, JSON.stringify(audit));
    assert.equal(audit.audit.length, 1);
    const [entry] = audit.audit;
    assert.deepEqual([entry.via, entry.transition, String(entry.targetId)], ['apply', 'edit', categoryId]);
    assert.equal(String(entry.actorId), '1');
    assert.equal(entry.before.title, 'CLI Category');
    assert.equal(entry.after.title, 'CLI Category Renamed');

    // --- `flaskbb db upgrade`: planned, acknowledged migration that keeps the data
    const unchanged = JSON.parse(run(binary, ['service', 'migrate', definition, '--config', config]));
    assert.deepEqual([unchanged.ok, unchanged.changes.length, unchanged.steps.length, unchanged.blocked], [true, 0, 0, false]);
    const v2 = join(temp, 'app-v2.aip');
    const source = readFileSync(definition, 'utf8');
    const marker = 'description: Text?; position: Int }';
    assert.equal(source.split(marker).length, 2, 'Category fields line is unique');
    writeFileSync(v2, source.replace(marker, 'description: Text?; position: Int; tagline: Text(1..100)? }'));
    const plan = JSON.parse(run(binary, ['service', 'migrate', v2, '--config', config]));
    assert.equal(plan.ok, true, JSON.stringify(plan));
    assert.deepEqual(plan.changes.map(c => c.class), ['Safe']);
    assert.equal(plan.blocked, false);
    const wrong = spawnSync(binary, ['service', 'migrate', v2, '--config', config, '--apply', '0'.repeat(64)], {cwd: root, env: {...process.env, AIP_FLASKBB_DATABASE: dbUrl}, encoding: 'utf8'});
    assert.notEqual(wrong.status, 0, 'a migration must refuse a digest that was not acknowledged');
    assert.equal(sql(`SELECT count(*) FROM information_schema.columns WHERE table_schema='${schema}' AND table_name='category' AND column_name='tagline'`), '0');
    const categoriesBefore = count('category');
    const applyResult = JSON.parse(run(binary, ['service', 'migrate', v2, '--config', config, '--apply', plan.digest]));
    assert.equal(applyResult.ok, true, JSON.stringify(applyResult));
    assert.equal(sql(`SELECT count(*) FROM information_schema.columns WHERE table_schema='${schema}' AND table_name='category' AND column_name='tagline'`), '1');
    assert.equal(count('category'), categoriesBefore, 'rows survive the migration');

    // --- the migrated deployment serves the new definition and keeps the principal and data
    const address = await fx.restart(v2);
    assert.ok(address);
    const afterMigrate = (await fx.client('admin-sub').read({read: 'Category', select: ['id', 'title'], sort: [{field: 'id'}]})).rows.map(c => c.title);
    assert.deepEqual(afterMigrate, ['My Category', 'CLI Category Renamed']);

    // --- `service jobs list` works on a deployment without workers (no runs yet)
    assert.deepEqual(JSON.parse(run(binary, ['service', 'jobs', '--config', config, 'list'])), {jobs: [], ok: true});

    // --- revoke ends the identity's access
    const beforeRevoke = await fx.client('admin-sub').read({read: 'Category', select: ['id']});
    assert.ok(beforeRevoke.rows.length > 0);
    run(binary, ['service', 'principal', '--config', config, 'revoke', '--subject', 'admin-sub']);
    await assert.rejects(() => fx.client('admin-sub').read({read: 'Category', select: ['id']}), 'a revoked subject must lose access on its next call');
    console.log(JSON.stringify({schema, ok: true}));
  } finally {
    await fx.stop();
  }
});
