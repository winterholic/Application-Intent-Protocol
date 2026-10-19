import assert from 'node:assert/strict';
import {test} from 'node:test';
import {readFileSync, writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {spawnSync} from 'node:child_process';
import {binary, definition, dbUrl, root, startFixture} from './harness.mjs';

// 7.5.3 F13: upstream `flaskbb upgrade [--all] [--fixture NAME] [--force]`
// (cli/main.py:232-263) does two things. `--all` runs `alembic.upgrade()`
// (244). `--fixture NAME` or `--all` then imports `flaskbb.fixtures.NAME`
// (248-254) and calls `update_settings_from_fixture` (257-259), which inserts
// the settings and groups the database lacks and overwrites existing ones only
// with `--force` (utils/populate.py:87-145). The migration half maps to
// `service migrate` (planned, digest-acknowledged; pinned in cli.test.mjs). The
// fixture half has no AIP counterpart: Setting and SettingsGroup have no
// write transition and `seed/install.py` is install-only (PRODUCT-GAPS.md
// G-15). This test pins both facts on a real schema so a future fixture-update
// path has to change them deliberately.
const installer = join(root, 'porting/flaskbb/seed/install.py');

test('FlaskBB upgrade: migrate keeps settings, fixture update has no service path (real PostgreSQL)', {timeout: 120000}, async () => {
  const fx = await startFixture({prefix: 'aip-flaskbb-upgrade-', seed: undefined});
  const {sql, q, schema, run, client, config, temp} = fx;
  try {
    const seedSql = run('python3', [installer, '--schema', schema, '--admin-username', 'admin', '--admin-email', 'admin@example.test']);
    const psql = input => spawnSync('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-q', '-At', '-d', dbUrl], {input, encoding: 'utf8'});
    const installed = psql(seedSql);
    assert.equal(installed.status, 0, installed.stderr);
    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'admin-sub', '--actor', '1']);
    const setting = key => sql(`SELECT value_json FROM ${q}.setting WHERE key = '${key}'`);
    const settingCount = () => Number(sql(`SELECT count(*) FROM ${q}.setting`));
    const groupCount = () => Number(sql(`SELECT count(*) FROM ${q}.settings_group`));

    // The generated caller binding exposes no write on either settings resource.
    const binding = readFileSync(join(temp, 'binding.ts'), 'utf8');
    const applyBlock = binding.slice(binding.indexOf('export interface ApplyContract {'), binding.indexOf('export const applyInputDescriptors'));
    const applies = [...applyBlock.matchAll(/^ {2}"([A-Za-z]+\.[A-Za-z]+)": \{/gm)].map(match => match[1]);
    assert.ok(applies.includes('User.purge') && applies.includes('Category.edit'), 'the parse found the declared applies');
    assert.deepEqual(applies.filter(name => /^Setting(sGroup)?\./.test(name)), []);
    const settingsRead = await client('admin-sub').read({read: 'Setting', select: ['key']});
    assert.equal(settingsRead.rows.length, settingCount());

    // State an upgrade would repair upstream: a setting an operator changed
    // and a setting a newer fixture added that this database does not have yet.
    sql(`UPDATE ${q}.setting SET value_json = '55' WHERE key = 'posts_per_page'`);
    sql(`DELETE FROM ${q}.setting WHERE key = 'topics_per_page'`);
    const settings = settingCount();
    const groups = groupCount();
    assert.deepEqual([setting('posts_per_page'), setting('topics_per_page')], ['55', '']);

    // Re-running the fixture seed is the only fixture path. It refuses as a
    // whole, so the missing setting is not added and the changed one is kept.
    const again = psql(seedSql);
    assert.notEqual(again.status, 0, 'the install seed must not run twice');
    assert.deepEqual([settingCount(), groupCount(), setting('posts_per_page'), setting('topics_per_page')], [settings, groups, '55', '']);

    // `--all`'s migration half: a Safe field addition applied with its digest
    // leaves every settings row untouched (nothing is overwritten, nothing is
    // added), where upstream would also run the fixture update afterwards.
    const v2 = join(temp, 'app-v2.aip');
    const source = readFileSync(definition, 'utf8');
    const marker = 'description: Text?; position: Int }';
    assert.equal(source.split(marker).length, 2, 'Category fields line is unique');
    writeFileSync(v2, source.replace(marker, 'description: Text?; position: Int; tagline: Text(1..100)? }'));
    const plan = JSON.parse(run(binary, ['service', 'migrate', v2, '--config', config]));
    assert.equal(plan.ok, true, JSON.stringify(plan));
    assert.deepEqual(plan.changes.map(change => change.class), ['Safe']);
    const applied = JSON.parse(run(binary, ['service', 'migrate', v2, '--config', config, '--apply', plan.digest]));
    assert.equal(applied.ok, true, JSON.stringify(applied));
    assert.equal(sql(`SELECT count(*) FROM information_schema.columns WHERE table_schema='${schema}' AND table_name='category' AND column_name='tagline'`), '1');
    assert.deepEqual([settingCount(), groupCount(), setting('posts_per_page'), setting('topics_per_page')], [settings, groups, '55', '']);
  } finally {
    await fx.stop();
  }
});
