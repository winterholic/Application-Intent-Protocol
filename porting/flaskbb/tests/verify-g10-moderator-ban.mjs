import assert from 'node:assert/strict';
import {test} from 'node:test';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

test('G-10 actual upstream Moderator with mod_banuser=true still cannot ban through the port', {timeout: 90000}, async () => {
  const fx = await startFixture({actors: {'editor-sub': 4}, seed: (sql, q, schema) => {
    sql(GROUPS_SQL(schema)); sql(USERS_SQL(schema));
    sql(`INSERT INTO ${q}."group" (id,name,guest,admin,super_mod,mod,banned,editpost,deletepost,deletetopic,posttopic,postreply,viewhidden,makehidden,mod_edituser,mod_banuser,description) VALUES (6,'Banned',false,false,false,false,true,false,false,false,false,false,false,false,false,false,NULL);`);
    sql(`UPDATE ${q}."group" SET mod_banuser=true WHERE id=4;`);
  }});
  try {
    const before = fx.sql(`SELECT primary_group_id FROM ${fx.q}."user" WHERE id=2`);
    const result = await fx.client('editor-sub').bundle({atomic: [{apply: 'User.adminUpdateUser', target: {ids: ['2']}, values: {primary_group: '6', activated: true}}]}, {key: 'g10-mod-ban-probe'});
    assert.equal(result.ok, false, JSON.stringify(result));
    assert.equal(fx.sql(`SELECT primary_group_id FROM ${fx.q}."user" WHERE id=2`), before);
    console.log(JSON.stringify({ok: true, mod_banuser: true, moderatorBanAccepted: false, targetPrimaryGroup: before}));
  } finally { await fx.stop(); }
});
