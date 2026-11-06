import assert from 'node:assert/strict';
import {test} from 'node:test';
import {GROUPS_SQL, USERS_SQL, seedNativePostOrders, startFixture} from './harness.mjs';

const id = n => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`;
const base = (sql, q, schema) => {
  sql(GROUPS_SQL(schema)); sql(USERS_SQL(schema));
  sql(`INSERT INTO ${q}.category(id,title,description,position) VALUES(1,'General','fixture',1);
    INSERT INTO ${q}.forum(id,category_id,title,description,position,locked,show_moderators) VALUES(1,1,'Forum','fixture',1,false,false);
    INSERT INTO ${q}.forum_group_access(id,forum_id,group_id) VALUES(1,1,2),(2,1,3);`);
};

test('native publish advances the Topic pointer and date together with Forum and PostOrder', {timeout:90000}, async () => {
  const topic=id(1), post=id(2);
  const fx=await startFixture({actors:{alice:2}, seed(sql,q,schema) {
    base(sql,q,schema);
    sql(`INSERT INTO ${q}.topic(id,forum_id,title,user_id,username,locked,important,views,published,deleted,hidden)
      VALUES('${topic}',1,'Topic',2,'member-alice',false,false,0,false,false,false);
      INSERT INTO ${q}.post(id,topic_id,forum_id,user_id,username,content,hidden,published,version,deleted)
      VALUES('${post}','${topic}',1,2,'member-alice','draft',false,false,0,false);`);
  }});
  try {
    const result=await fx.client('alice').bundle({atomic:[{apply:'Post.publish',target:{ids:[post]}}]}, {key:'topic-pointer-publish'});
    assert.equal(result.ok,true,JSON.stringify(result));
    // JSON field lookup makes a missing Topic pointer a semantic RED, not a SQL error.
    assert.equal(fx.sql(`SELECT to_jsonb(t)->>'last_post_id' FROM ${fx.q}.topic t WHERE id='${topic}'`),post);
    assert.equal(fx.sql(`SELECT t.last_updated=p.date_created AND f.last_post_created=p.date_created
      FROM ${fx.q}.topic t JOIN ${fx.q}.post p ON p.id='${post}' JOIN ${fx.q}.forum f ON f.id=1 WHERE t.id='${topic}'`),'t');
    const before=fx.sql(`SELECT to_jsonb(t)::text FROM ${fx.q}.topic t WHERE id='${topic}'`);
    assert.equal((await fx.client('alice').bundle({atomic:[{apply:'Post.publish',target:{ids:[post]}}]}, {key:'topic-pointer-publish'})).ok,true);
    assert.equal(fx.sql(`SELECT to_jsonb(t)::text FROM ${fx.q}.topic t WHERE id='${topic}'`),before);
  } finally { await fx.stop(); }
});

for (const mode of ['older','tie','newer','empty']) {
  test(`Topic unhide ${mode}: strict source comparison, Topic projection and first-post metadata`, {timeout:90000}, async () => {
    const topic=id(10), first=id(11), last=id(12), otherTopic=id(20), other=id(21);
    const fx=await startFixture({actors:{admin:3,alice:2}, seed(sql,q,schema) {
      base(sql,q,schema);
      sql(`INSERT INTO ${q}.topic(id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden,hidden_by_id,hidden_at)
        VALUES('${topic}',1,'Restored Topic',NULL,'topic-author','2020-01-01','2025-01-02',false,false,0,true,false,true,3,'2025-01-03'),
        ('${otherTopic}',1,'Other Topic',3,'admin-bob','2020-01-01','2030-01-01',false,false,0,true,false,false,NULL,NULL);
        INSERT INTO ${q}.post(id,topic_id,forum_id,user_id,username,content,date_created,hidden,hidden_by_id,hidden_at,published,version,deleted)
        VALUES('${first}','${topic}',1,2,'post-author','first','2020-01-01',true,3,'2025-01-03',true,0,false),
        ('${last}','${topic}',1,2,'reply-author','last','2025-01-01',false,NULL,NULL,true,0,false),
        ('${other}','${otherTopic}',1,3,'other-author','other','2030-01-01',false,NULL,NULL,true,0,false);
        INSERT INTO ${q}.topic_first_post(topic_id,post_id) VALUES('${topic}','${first}');
        UPDATE ${q}.topic SET last_post_id='${last}' WHERE id='${topic}';
        UPDATE ${q}.topic SET last_post_id='${other}' WHERE id='${otherTopic}';`);
      seedNativePostOrders(sql,q,[first,last,other]);
      if(mode!=='empty') {
        const date={older:'2025-01-03',tie:'2025-01-02',newer:'2025-01-01'}[mode];
        sql(`UPDATE ${q}.forum SET last_post_id='${other}',last_post_user_id=3,last_post_title='sentinel-title',
          last_post_username='sentinel-name',last_post_created='${date}' WHERE id=1`);
      }
    }});
    const {q,sql}=fx;
    const forum=()=>sql(`SELECT jsonb_build_array(last_post_id,last_post_user_id,last_post_title,last_post_username,last_post_created)::text FROM ${q}.forum WHERE id=1`);
    try {
      const before=forum();
      const preserved=sql(`SELECT to_jsonb(t)-'hidden'-'hidden_by_id'-'hidden_at' FROM ${q}.topic t WHERE id='${topic}'`);
      const denied=await fx.client('alice').bundle({atomic:[{apply:'Topic.unhide',target:{ids:[topic]}}]}, {key:`topic-unhide-denied-${mode}`});
      assert.equal(denied.ok,false);
      assert.equal(forum(),before);
      assert.equal(sql(`SELECT hidden FROM ${q}.topic WHERE id='${topic}'`),'t');
      const result=await fx.client('admin').bundle({atomic:[{apply:'Topic.unhide',target:{ids:[topic]}}]}, {key:`topic-unhide-${mode}`});
      assert.equal(result.ok,true,JSON.stringify(result));
      assert.equal(sql(`SELECT to_jsonb(t)-'hidden'-'hidden_by_id'-'hidden_at' FROM ${q}.topic t WHERE id='${topic}'`),preserved,'unhide preserves Topic pointer, dates and unrelated columns');
      assert.equal(sql(`SELECT NOT hidden AND hidden_by_id IS NULL AND hidden_at IS NULL FROM ${q}.post WHERE id='${first}'`),'t');
      if(mode==='older'||mode==='tie') assert.equal(forum(),before,'older/equal restored Topic cannot refresh any Forum field');
      else {
        assert.deepEqual(JSON.parse(forum()).slice(0,4),[last,null,'Restored Topic','topic-author']);
        assert.equal(sql(`SELECT last_post_created='2025-01-02'::timestamptz FROM ${q}.forum WHERE id=1`),'t');
      }
      assert.equal(sql(`SELECT content||'|'||hidden FROM ${q}.post WHERE id='${other}'`),'other|false');
      const applied=forum();
      assert.equal((await fx.client('admin').bundle({atomic:[{apply:'Topic.unhide',target:{ids:[topic]}}]}, {key:`topic-unhide-${mode}`})).ok,true);
      assert.equal(forum(),applied);
    } finally { await fx.stop(); }
  });
}

// Pinned upstream Topic._remove_topic_from_forum observes the two leading
// visible Topics BEFORE hiding this Topic. The key is original Post.id,
// not a UUID or a date. A NULL last_post sorts first on PostgreSQL DESC.
for (const mode of ['top','non-top','only','null-self','null-other']) {
  test(`Topic hide ${mode}: pre-state Topic order, projection and first-post metadata`, {timeout:90000}, async () => {
    const topic=id(30), first=id(31), last=id(32), otherTopic=id(40), other=id(41), excludedTopic=id(50), excluded=id(51);
    const fx=await startFixture({actors:{admin:3,alice:2}, seed(sql,q,schema) {
      base(sql,q,schema);
      sql(`INSERT INTO ${q}.topic(id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden)
        VALUES('${topic}',1,'Hidden Topic',2,'topic-owner','2000-01-01','2025-01-01',false,false,0,true,false,false),
        ('${otherTopic}',1,'Replacement Topic',NULL,'replacement-topic-owner','2001-01-01','2035-01-01',false,false,0,true,false,${mode==='only'}),
        ('${excludedTopic}',1,'Already hidden',3,'excluded-topic-owner','2002-01-01','2040-01-01',false,false,0,true,false,true);
        INSERT INTO ${q}.post(id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted)
        VALUES('${first}','${topic}',1,2,'first-post-owner','first','2000-01-01',false,true,0,false),
        ('${last}','${topic}',1,2,'reply-owner','reply','2025-01-01',false,true,0,false),
        ('${other}','${otherTopic}',1,3,'replacement-post-owner','other','1990-01-01',false,true,0,false),
        ('${excluded}','${excludedTopic}',1,3,'excluded-post-owner','excluded','2050-01-01',false,true,0,false);
        INSERT INTO ${q}.topic_first_post(topic_id,post_id) VALUES('${topic}','${first}');
        UPDATE ${q}.topic SET last_post_id='${last}' WHERE id='${topic}';
        UPDATE ${q}.topic SET last_post_id='${other}' WHERE id='${otherTopic}';
        UPDATE ${q}.topic SET last_post_id='${excluded}' WHERE id='${excludedTopic}';
        UPDATE ${q}.forum SET last_post_id='${last}',last_post_user_id=2,last_post_title='sentinel-title',
          last_post_username='sentinel-owner',last_post_created='2025-01-01' WHERE id=1;`);
      // UUID/date ordering disagree with the source key, and an excluded hidden
      // Topic has the greatest mapped key. Neither may decide the replacement.
      seedNativePostOrders(sql,q,mode==='non-top' ? [first,last,other,excluded] : [first,other,last,excluded]);
      if(mode==='null-self') sql(`UPDATE ${q}.topic SET last_post_id=NULL WHERE id='${topic}'`);
      if(mode==='null-other') sql(`UPDATE ${q}.topic SET last_post_id=NULL WHERE id='${otherTopic}'`);
    }});
    const {q,sql}=fx;
    const forum=()=>sql(`SELECT jsonb_build_array(last_post_id,last_post_user_id,last_post_title,last_post_username,last_post_created)::text FROM ${q}.forum WHERE id=1`);
    try {
      const before=forum();
      const topicBefore=sql(`SELECT to_jsonb(t)-'hidden'-'hidden_by_id'-'hidden_at' FROM ${q}.topic t WHERE id='${topic}'`);
      const unrelated=sql(`SELECT jsonb_agg(to_jsonb(p) ORDER BY id)::text FROM ${q}.post p WHERE id!='${first}'`);
      const denied=await fx.client('alice').bundle({atomic:[{apply:'Topic.hide',target:{ids:[topic]}}]}, {key:`topic-hide-denied-${mode}`});
      assert.equal(denied.ok,false);
      assert.equal(forum(),before);
      assert.equal(sql(`SELECT hidden FROM ${q}.topic WHERE id='${topic}'`),'f');
      const result=await fx.client('admin').bundle({atomic:[{apply:'Topic.hide',target:{ids:[topic]}}]}, {key:`topic-hide-${mode}`});
      assert.equal(result.ok,true,JSON.stringify(result));
      if(mode==='non-top'||mode==='null-other') assert.equal(forum(),before,'hiding a non-leading Topic preserves every Forum field');
      else if(mode==='only') assert.deepEqual(JSON.parse(forum()),[null,null,null,null,null]);
      else {
        assert.deepEqual(JSON.parse(forum()).slice(0,4),[other,null,'Replacement Topic','replacement-topic-owner']);
        assert.equal(sql(`SELECT last_post_created='2035-01-01'::timestamptz FROM ${q}.forum WHERE id=1`),'t');
      }
      assert.equal(sql(`SELECT t.hidden AND p.hidden AND p.hidden_by_id=t.hidden_by_id AND p.hidden_by_id=3
        AND p.hidden_at=t.hidden_at AND p.hidden_at IS NOT NULL FROM ${q}.topic t JOIN ${q}.post p ON p.id='${first}' WHERE t.id='${topic}'`),'t');
      assert.equal(sql(`SELECT to_jsonb(t)-'hidden'-'hidden_by_id'-'hidden_at' FROM ${q}.topic t WHERE id='${topic}'`),topicBefore);
      assert.equal(sql(`SELECT jsonb_agg(to_jsonb(p) ORDER BY id)::text FROM ${q}.post p WHERE id!='${first}'`),unrelated);
      const applied=forum();
      assert.equal((await fx.client('admin').bundle({atomic:[{apply:'Topic.hide',target:{ids:[topic]}}]}, {key:`topic-hide-${mode}`})).ok,true);
      assert.equal(forum(),applied);
    } finally { await fx.stop(); }
  });
}

// Topic.delete (upstream forum/models.py 699-715) calls Forum.update_last_post
// only for a missing pointer or the deleted Topic's last Post. The latter query
// has a viewer-dependent Post filter (HideableQuery); an admin sees hidden
// Posts and a hidden Topic does not remove its Posts from the join. Its date
// order has no tie-break, so the tie case accepts either source-valid winner.
for (const mode of ['non-current','hidden-topic','hidden-post','empty-pointer','same-pointer','only','tie']) {
  test(`Topic delete ${mode}: conditional Forum projection and source candidate set`, {timeout:90000}, async () => {
    const topic=id(60), first=id(61), last=id(62), otherTopic=id(70), other=id(71), fallbackTopic=id(80), fallback=id(81);
    const fx=await startFixture({actors:{admin:3,carol:6}, seed(sql,q,schema) {
      base(sql,q,schema);
      sql(`INSERT INTO ${q}.topic(id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden)
        VALUES('${topic}',1,'Deleted Topic',2,'topic-owner','2000-01-01','2025-01-01',false,false,0,true,false,false),
        ('${otherTopic}',1,'Replacement Topic',NULL,'topic-author','2000-01-01','2040-01-01',false,false,0,true,${mode==='only'},${mode==='hidden-topic'}),
        ('${fallbackTopic}',1,'Fallback Topic',3,'fallback-topic-author','2000-01-01','2001-01-01',false,false,0,true,${mode==='only'},false);
        INSERT INTO ${q}.post(id,topic_id,forum_id,user_id,username,content,date_created,hidden,published,version,deleted)
        VALUES('${first}','${topic}',1,2,'first-author','first','2000-01-01',false,true,0,false),
        ('${last}','${topic}',1,2,'reply-author','last','2025-01-01',false,true,0,false),
        ('${other}','${otherTopic}',1,NULL,'replacement-post-author','other','2030-01-01',${mode==='hidden-post'},true,0,${mode==='only'}),
        ('${fallback}','${fallbackTopic}',1,3,'fallback-post-author','fallback','${mode==='tie'?'2030-01-01':'2020-01-01'}',false,true,0,${mode==='only'});
        INSERT INTO ${q}.topic_first_post(topic_id,post_id) VALUES('${topic}','${first}');
        UPDATE ${q}.topic SET last_post_id='${mode==='same-pointer'?other:last}' WHERE id='${topic}';
        UPDATE ${q}.topic SET last_post_id='${other}' WHERE id='${otherTopic}';
        UPDATE ${q}.topic SET last_post_id='${fallback}' WHERE id='${fallbackTopic}';
        UPDATE ${q}.forum SET last_post_id=${mode==='empty-pointer'?'NULL':`'${mode==='non-current'||mode==='same-pointer'?other:last}'`},last_post_user_id=2,
          last_post_title='sentinel-title',last_post_username='sentinel-name',last_post_created='1990-01-01' WHERE id=1;`);
      seedNativePostOrders(sql,q,[first,other,last,fallback]);
    }});
    const {q,sql}=fx;
    const forum=()=>sql(`SELECT jsonb_build_array(last_post_id,last_post_user_id,last_post_title,last_post_username,last_post_created)::text FROM ${q}.forum WHERE id=1`);
    try {
      const before=forum();
      const allPosts=sql(`SELECT jsonb_agg(to_jsonb(p) ORDER BY id)::text FROM ${q}.post p`);
      const otherTopics=sql(`SELECT jsonb_agg(to_jsonb(t) ORDER BY id)::text FROM ${q}.topic t WHERE id!='${topic}'`);
      const denied=await fx.client('carol').bundle({atomic:[{apply:'Topic.delete',target:{ids:[topic]}}]}, {key:`topic-delete-denied-${mode}`});
      assert.equal(denied.ok,false);
      assert.equal(forum(),before);
      assert.equal(sql(`SELECT deleted FROM ${q}.topic WHERE id='${topic}'`),'f');
      const result=await fx.client('admin').bundle({atomic:[{apply:'Topic.delete',target:{ids:[topic]}}]}, {key:`topic-delete-${mode}`});
      assert.equal(result.ok,true,JSON.stringify(result));
      if(mode==='non-current'||mode==='same-pointer') assert.equal(forum(),before,'source preserves all projection fields when no new last Post is selected');
      else if(mode==='only') assert.deepEqual(JSON.parse(forum()),[null,null,null,null,null]);
      else {
        const selected=JSON.parse(forum());
        assert.ok((mode==='tie'?[other,fallback]:[other]).includes(selected[0]),'winner belongs to the upstream maximal-date candidate set');
        assert.equal(sql(`SELECT f.last_post_user_id IS NOT DISTINCT FROM p.user_id
          AND f.last_post_title IS NOT DISTINCT FROM t.title
          AND f.last_post_username IS NOT DISTINCT FROM p.username
          AND f.last_post_created IS NOT DISTINCT FROM p.date_created
          FROM ${q}.forum f JOIN ${q}.post p ON p.id=f.last_post_id JOIN ${q}.topic t ON t.id=p.topic_id WHERE f.id=1`),'t');
      }
      assert.equal(sql(`SELECT deleted FROM ${q}.topic WHERE id='${topic}'`),'t');
      // This suite verifies projection parity under the existing tombstone
      // policy. It deliberately does not claim upstream physical retention.
      assert.equal(sql(`SELECT jsonb_agg(to_jsonb(p) ORDER BY id)::text FROM ${q}.post p`),allPosts);
      assert.equal(sql(`SELECT jsonb_agg(to_jsonb(t) ORDER BY id)::text FROM ${q}.topic t WHERE id!='${topic}'`),otherTopics);
      const applied=forum();
      assert.equal((await fx.client('admin').bundle({atomic:[{apply:'Topic.delete',target:{ids:[topic]}}]}, {key:`topic-delete-${mode}`})).ok,true);
      assert.equal(forum(),applied);
    } finally { await fx.stop(); }
  });
}
