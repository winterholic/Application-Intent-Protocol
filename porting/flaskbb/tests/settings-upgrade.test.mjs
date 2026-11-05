import assert from 'node:assert/strict';
import {test} from 'node:test';
import {join} from 'node:path';
import {binary, root, startFixture} from './harness.mjs';

const installer = join(root, 'porting/flaskbb/seed/install.py');
const upgrader = join(root, 'porting/flaskbb/seed/upgrade_settings.py');
const seed = (sql, q, schema, run) => {
  const input = run('python3', [installer, '--schema', schema, '--admin-email', 'admin@example.test']);
  sql(input);
  // Installer group order follows pinned upstream: admin=1, super_mod=2, member=4.
  sql(`INSERT INTO ${q}."user" (id,username,email,date_joined,lastseen,primary_group_id,activated,language) VALUES
    (2,'upgrade-member','member@example.test',now(),now(),4,true,'en'),
    (3,'upgrade-supermod','supermod@example.test',now(),now(),2,true,'en'),
    (4,'upgrade-inactive','inactive@example.test',now(),now(),1,false,'en')`);
};
const actors = {'admin-sub': 1, 'member-sub': 2, 'supermod-sub': 3, 'inactive-sub': 4};

test('settings fixture upgrade adds missing rows, preserves edits, force overwrites and audits updates (real HTTP/PG)', {timeout: 120000}, async () => {
  const fx = await startFixture({prefix: 'aip-flaskbb-settings-', seed, actors});
  const {sql, q, run, config} = fx;
  try {
    const setting = key => sql(`SELECT value_json FROM ${q}.setting WHERE key='${key}'`);
    const originalCount = sql(`SELECT count(*) FROM ${q}.setting`);
    sql(`UPDATE ${q}.setting SET value_json='55' WHERE key='posts_per_page';
         DELETE FROM ${q}.setting WHERE key='topics_per_page';
         UPDATE ${q}.settings_group SET name='Operator title' WHERE key='general'`);
    const removedGroup = sql(`SELECT id FROM ${q}.settings_group WHERE key <> 'general' ORDER BY id DESC LIMIT 1`);
    sql(`DELETE FROM ${q}.setting WHERE group_id=${removedGroup}; DELETE FROM ${q}.settings_group WHERE id=${removedGroup}`);
    const invoke = force => JSON.parse(run('python3', [upgrader, '--url', `http://${fx.address}`, ...(force ? ['--force'] : [])], {
      env: {...process.env, PYTHONPATH: join(root, 'product/python'), AIP_FLASKBB_ADMIN_TOKEN: fx.token('admin-sub')},
    }));
    const added = invoke(false);
    assert.equal(added.ok, true);
    assert.equal(added.groups_written, 1);
    assert.ok(added.settings_written > 1);
    assert.equal(sql(`SELECT count(*) FROM ${q}.setting`), originalCount);
    assert.equal(setting('posts_per_page'), '55');
    assert.equal(setting('topics_per_page'), '10');
    assert.equal(sql(`SELECT name FROM ${q}.settings_group WHERE key='general'`), 'Operator title');
    assert.deepEqual(invoke(false), {ok: true, groups_written: 0, settings_written: 0});
    assert.deepEqual(invoke(true), {ok: true, groups_written: 1, settings_written: 1});
    assert.equal(setting('posts_per_page'), '10');
    assert.equal(sql(`SELECT name FROM ${q}.settings_group WHERE key='general'`), 'General Settings');
    const id = sql(`SELECT id FROM ${q}.setting WHERE key='posts_per_page'`);
    const audit = JSON.parse(run(binary, ['service', 'audit', '--config', config, 'list', '--resource', 'Setting', '--target-id', id]));
    assert.equal(audit.audit.length, 1, JSON.stringify(audit));
    assert.equal(String(audit.audit[0].actorId), '1');
    assert.equal(audit.audit[0].via, 'apply');
    assert.equal(audit.audit[0].transition, 'edit');
    assert.equal(audit.audit[0].before.value_json, '55');
    assert.equal(audit.audit[0].after.value_json, '10');
    assert.deepEqual(invoke(true), {ok: true, groups_written: 0, settings_written: 0});
  } finally {
    await fx.stop();
  }
});

test('settings writes enforce activated management authority and preserve rows on denial (real HTTP/PG)', {timeout: 120000}, async () => {
  const fx = await startFixture({prefix: 'aip-flaskbb-settings-auth-', seed, actors});
  const {sql, q, client} = fx;
  try {
    const id = sql(`SELECT id FROM ${q}.setting WHERE key='posts_per_page'`);
    const groupId = sql(`SELECT id FROM ${q}.settings_group WHERE key='general'`);
    for (const subject of ['member-sub', 'inactive-sub']) {
      const denied = await client(subject).bundle({atomic: [{apply: 'Setting.edit', target: {ids: [id]}, values: {value_json: '55'}}]});
      assert.equal(denied.ok, false, JSON.stringify(denied));
      const create = await client(subject).bundle({atomic: [{create: 'SettingsGroup', values: {key: 'unauthorized', name: 'No', description: 'No'}}]});
      assert.equal(create.ok, false, JSON.stringify(create));
      const groupEdit = await client(subject).bundle({atomic: [{apply: 'SettingsGroup.edit', target: {ids: [groupId]}, values: {name: 'No'}}]});
      assert.equal(groupEdit.ok, false, JSON.stringify(groupEdit));
      const settingCreate = await client(subject).bundle({atomic: [{create: 'Setting', values: {key: 'unauthorized', value_json: '1', group: groupId, name: 'No', description: 'No', value_type: 'integer', extra_json: null}}]});
      assert.equal(settingCreate.ok, false, JSON.stringify(settingCreate));
      assert.equal(sql(`SELECT value_json FROM ${q}.setting WHERE id=${id}`), '10');
      assert.equal(sql(`SELECT count(*) FROM ${q}.settings_group WHERE key='unauthorized'`), '0');
      assert.equal(sql(`SELECT count(*) FROM ${q}.setting WHERE key='unauthorized'`), '0');
      assert.equal(sql(`SELECT name FROM ${q}.settings_group WHERE id=${groupId}`), 'General Settings');
    }
    const accepted = await client('supermod-sub').bundle({atomic: [{apply: 'Setting.edit', target: {ids: [id]}, values: {value_json: '20'}}]});
    assert.equal(accepted.ok, true, JSON.stringify(accepted));
    assert.equal(sql(`SELECT value_json FROM ${q}.setting WHERE id=${id}`), '20');
  } finally {
    await fx.stop();
  }
});
