import assert from 'node:assert/strict';
import {test} from 'node:test';
import {mkdtempSync, readFileSync, rmSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {GROUPS_SQL, USERS_SQL, binary, definition, startFixture} from './harness.mjs';

test('FlaskBB PostOrder schema upgrade preserves legacy UUIDs and requires the exact migration digest', {timeout: 120000}, async () => {
  const dir = mkdtempSync(join(tmpdir(), 'aip-flaskbb-order-upgrade-'));
  let fx;
  try {
    // Derive the immediately preceding schema without a second full app copy.
    // Each removal must be unique so a changed definition cannot silently turn
    // this into a migration from the new schema to itself.
    let legacy = readFileSync(definition, 'utf8');
    // This is a preceding-schema fixture, not the old behavior implementation.
    // Remove the new Topic ordering behavior before removing its nullable fields.
    const orderPredicates=legacy.match(/^predicate topicHide(?:OrderKnown|ReplacesForum|NullReplacement).*$/gm);
    assert.equal(orderPredicates?.length,3);
    legacy=legacy.replace(/^predicate topicHide(?:OrderKnown|ReplacesForum|NullReplacement).*\n/gm,'');
    const hideStart=legacy.indexOf('  transition hide {',legacy.indexOf('resource Topic {'));
    const hideEnd=legacy.indexOf('  expose apply hide { target id; bulk maxRows 1 }',hideStart);
    assert.ok(hideStart>0 && hideEnd>hideStart);
    legacy=legacy.slice(0,hideStart)+'  transition hide { allow canHide(actor, forum); from hidden != true and deleted = false; to hidden = true, hidden_at = now, hidden_by = actor }\n'+legacy.slice(hideEnd);
    const removeOnce = text => {
      assert.equal(legacy.split(text).length, 2, `legacy schema anchor must be unique: ${text}`);
      legacy = legacy.replace(text, '');
    };
    removeOnce('    source_order: PostOrder?\n');
    // This fixture preserves the prior schema, without guessing a historical Topic pointer.
    assert.equal(legacy.split('resource Topic {\n  fields {\n    id: Uuid\n    last_post: Post?\n').length,2);
    legacy=legacy.replace('resource Topic {\n  fields {\n    id: Uuid\n    last_post: Post?\n','resource Topic {\n  fields {\n    id: Uuid\n');
    removeOnce('    update Topic where id = this.topic { last_post = this; last_updated = now }\n');
    legacy=legacy.replace('last_post = this.last_post\n','last_post = this.forum.last_post\n');
    removeOnce('  unique source_order\n  check sourceOrderBelongsToPost when source_order = null or source_order.post = this\n');
    removeOnce(' and not (exists Post where published = true and source_order = null)');
    removeOnce('    create PostOrder { post = this }\n    update Post where id = this { source_order = latest PostOrder mapped where mapped.post = this order by mapped.id select mapped.id }\n');
    removeOnce('resource PostOrder {\n  fields { id: Id; post: Post }\n  unique post\n  rows read when canReadPost(actor, post)\n}\n');
    const oldFile = join(dir, 'legacy.aip');
    writeFileSync(oldFile, legacy);
    const post = 'ffffffff-ffff-4fff-8fff-ffffffffffff';
    const topic = '00000000-0000-4000-8000-400000000001';
    fx = await startFixture({definitionFile: oldFile, prefix: 'aip-flaskbb-order-upgrade-db-',
      seed(sql, q, schema) {
        sql(GROUPS_SQL(schema)); sql(USERS_SQL(schema));
        sql(`INSERT INTO ${q}.category(id,title,description,position) VALUES (1,'Legacy','unchanged',1);
          INSERT INTO ${q}.forum(id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'Legacy','unchanged',1,false,false);
          INSERT INTO ${q}.topic(id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden)
            VALUES ('${topic}',1,'Legacy',2,'member-alice','2020-01-01T00:00:00Z','2020-01-01T00:00:00Z',false,false,0,true,false,false);
          INSERT INTO ${q}.post(id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted)
            VALUES ('${post}','${topic}',1,2,'member-alice','preserve original content','2020-01-01T00:00:00Z',false,true,0,false);`);
      }});
    const {run, sql, q, config} = fx;
    const snapshot = () => sql(`SELECT json_build_object('posts',(SELECT json_agg(row_to_json(p)) FROM
      (SELECT id,topic_id,user_id,content,date_created,published FROM ${q}.post ORDER BY id) p),
      'users',(SELECT json_agg(row_to_json(u)) FROM (SELECT * FROM ${q}."user" ORDER BY id) u))::text`);
    const before = snapshot();
    const plan = JSON.parse(run(binary, ['service', 'migrate', definition, '--config', config]));
    assert.equal(plan.ok, true, JSON.stringify(plan));
    assert.equal(plan.blocked, false, JSON.stringify(plan));
    assert.equal(plan.requiresReview, true);
    assert.equal(snapshot(), before, 'preview must not rewrite legacy data');
    assert.throws(() => run(binary, ['service', 'migrate', definition, '--config', config, '--apply', 'wrong-digest']), /PLAN_CHANGED/);
    assert.equal(snapshot(), before, 'a rejected digest must preserve the original rows');
    const applied = JSON.parse(run(binary, ['service', 'migrate', definition, '--config', config, '--apply', plan.digest]));
    assert.equal(applied.ok, true, JSON.stringify(applied));
    assert.equal(snapshot(), before, 'additive migration must preserve UUIDs, content and actors');
    assert.equal(sql(`SELECT count(*) FROM ${q}.post_order`), '0', 'migration must not invent original IDs');
    assert.equal(sql(`SELECT source_order_id IS NULL FROM ${q}.post WHERE id='${post}'`), 't');
    assert.equal(sql(`SELECT last_post_id IS NULL FROM ${q}.topic WHERE id='${topic}'`),'t','migration does not invent a source Topic pointer');
    assert.equal(JSON.parse(run(binary, ['service', 'check', definition])).ok, true);
  } finally {
    try { if (fx) await fx.stop(); }
    finally { rmSync(dir, {recursive: true, force: true}); }
  }
});
