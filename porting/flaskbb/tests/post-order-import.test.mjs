import assert from 'node:assert/strict';
import {test} from 'node:test';
import {join} from 'node:path';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {GROUPS_SQL, USERS_SQL, dbUrl, root, startFixture} from './harness.mjs';

// Explicit original IDs intentionally oppose UUID and date order. Hidden and
// deleted source-equivalent posts stay in the manifest; a draft stays unmapped.
test('FlaskBB historical PostOrder import binds approval, fences writers, preserves rows and resumes native allocation above source high-water', {timeout: 120000}, async () => {
  const topic = '00000000-0000-4000-8000-900000000001';
  const old = 'ffffffff-ffff-4fff-8fff-ffffffffffff';
  const later = '00000000-0000-4000-8000-900000000002';
  const draft = '00000000-0000-4000-8000-900000000003';
  const source = {namespace: "pinned fixture ' explicit mapping", revision: 'a'.repeat(40), high_water: 80};
  const fx = await startFixture({prefix: 'aip-flaskbb-order-import-', actors: {'import-alice': 2},
    seed(sql, q, schema) {
      sql(GROUPS_SQL(schema)); sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category(id,title,description,position) VALUES (1,'General','fixture',1);
        INSERT INTO ${q}.forum(id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'Forum','fixture',1,false,false);
        INSERT INTO ${q}.forum_group_access(id,forum_id,group_id) VALUES (1,1,2);
        INSERT INTO ${q}.topic(id,forum_id,title,user_id,username,locked,important,views,published,deleted,hidden)
          VALUES ('${topic}',1,'Imported',2,'member-alice',false,false,0,true,false,false);`);
      for (const [post, published, hidden, deleted, date] of [[old,true,false,false,'2025-01-01'],[later,true,true,true,'2020-01-01'],[draft,false,false,false,'2026-01-01']]) {
        sql(`INSERT INTO ${q}.post(id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted)
          VALUES ('${post}','${topic}',1,2,'member-alice','preserved content','${date}',${hidden},${published},0,${deleted});`);
      }
    }});
  const {run, sql, q, schema} = fx;
  const tool = join(root, 'porting/flaskbb/seed/post_order_import.py');
  const emit = (input, args=[]) => run('python3', [tool,...args], {input: JSON.stringify(input)});
  const execute = input => run('psql', ['-X','-v','ON_ERROR_STOP=1','-Atq','-d',dbUrl], {input});
  const state = () => sql(`SELECT jsonb_build_object('posts',(SELECT jsonb_agg(to_jsonb(p) ORDER BY id) FROM ${q}.post p),
    'orders',(SELECT COALESCE(jsonb_agg(to_jsonb(o) ORDER BY id),'[]'::jsonb) FROM ${q}.post_order o),
    'generator',(SELECT to_jsonb(s) FROM ${q}.post_order_id_seq s))::text`);
  const unchanged = () => sql(`SELECT jsonb_agg(to_jsonb(p)-'source_order_id' ORDER BY id)::text FROM ${q}.post p`);
  let holder;
  try {
    const snapshot = JSON.parse(execute(emit({schema,source}, ['--snapshot-sql'])));
    const target = JSON.parse(sql(`SELECT json_build_object('schema','${schema}','facts_digest',facts_digest,'structure_digest',structure_digest) FROM ${q}.aip_migrate_meta WHERE singleton=true`));
    const request = {manifest: {version:1,source,posts:[{source_id:41,post_uuid:old},{source_id:42,post_uuid:later}]}, snapshot,target};
    const plan = JSON.parse(emit(request));
    const before = state(), content = unchanged();
    assert.equal(plan.activation_authorized,false);
    assert.equal(state(),before,'preview is read-only');
    assert.throws(() => emit(request,['--apply','wrong-digest']), /PLAN_CHANGED/);
    const importSql = emit(request,['--apply',plan.digest]);
    // A product writer's shared deployment fence prevents import from starting.
    holder = spawn('psql',['-X','-v','ON_ERROR_STOP=1','-Atq','-d',dbUrl], {stdio:['pipe','pipe','pipe']});
    await new Promise((resolve,reject) => {
      let output='';
      holder.stdout.on('data',data => {output+=data; if(output.includes('writer-held')) resolve();});
      holder.once('error',reject);
      holder.once('exit',code => reject(new Error(`writer holder exited ${code}: ${output}`)));
      holder.stdin.write(`BEGIN; SELECT pg_advisory_xact_lock_shared(1095323725,hashtext('${schema}')); SELECT 'writer-held';\n`);
    });
    assert.throws(() => execute(importSql), /lock timeout/);
    assert.equal(state(),before,'lock refusal has no effects');
    const released = once(holder,'exit'); holder.stdin.end('ROLLBACK;\n'); await released; holder=undefined;
    const changedTarget = {...request,target:{...target,facts_digest:'0'.repeat(64)}};
    assert.throws(() => execute(emit(changedTarget,['--apply',JSON.parse(emit(changedTarget)).digest])), /POST_ORDER_TARGET_CHANGED/);
    assert.equal(state(),before);
    // Drift after review is detected under the fence, including tombstones.
    sql(`UPDATE ${q}.post SET published=false WHERE id='${later}'`);
    assert.throws(() => execute(importSql), /POST_ORDER_SNAPSHOT_CHANGED/);
    sql(`UPDATE ${q}.post SET published=true WHERE id='${later}'`);
    assert.equal(state(),before);
    // Failure after mapping/linking AND transactional sequence restart rolls all back.
    assert.throws(() => execute(importSql.replace('COMMIT;',"SELECT 1/0;\nCOMMIT;")), /division by zero/);
    assert.equal(state(),before,'failed import rolls back pairs, links and sequence');
    execute(importSql);
    assert.equal(unchanged(),content,'all original UUIDs, content, visibility, authors and dates survive');
    assert.equal(sql(`SELECT string_agg(id||':'||post_id::text,',' ORDER BY id) FROM ${q}.post_order`),`41:${old},42:${later}`);
    assert.equal(sql(`SELECT source_order_id IS NULL FROM ${q}.post WHERE id='${draft}'`),'t');
    assert.equal(sql(`SELECT last_value||'|'||is_called FROM ${q}.post_order_id_seq`),'81|false');
    const applied = state();
    assert.throws(() => execute(importSql), /POST_ORDER_SNAPSHOT_CHANGED/);
    assert.equal(state(),applied,'a stale approval cannot reset the allocator');
    const result = await fx.client('import-alice').bundle({atomic:[{apply:'Post.publish',target:{ids:[draft]}}]}, {key:'import-native-next'});
    assert.equal(result.ok,true,JSON.stringify(result));
    assert.equal(sql(`SELECT source_order_id FROM ${q}.post WHERE id='${draft}'`),'81');
    assert.equal(sql(`SELECT count(*) FROM ${q}.post_order`),'3');
  } finally {
    if (holder) {const closed=once(holder,'exit'); holder.stdin.end('ROLLBACK;\n'); await closed;}
    await fx.stop();
  }
});
