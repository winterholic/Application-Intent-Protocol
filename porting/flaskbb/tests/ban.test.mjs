import assert from 'node:assert/strict';
import {test} from 'node:test';
import {randomUUID} from 'node:crypto';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

// F10 ban/unban. The original swaps the user's primary group for the Banned
// group (and back to Member). The port expresses that as the administrator's
// `User.adminUpdateUser`; moderators with mod_banuser cannot ban because a
// transition cannot assign a reference to a server-chosen row (PRODUCT-GAPS G-10).
test('FlaskBB ban and unban through the group swap strip and restore posting rights (real PostgreSQL)', {timeout: 90000}, async () => {
  const fx = await startFixture({
    prefix: 'aip-flaskbb-ban-',
    actors: {'alice-sub': 2, 'bob-sub': 3, 'editor-sub': 4},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(`INSERT INTO ${q}."group" (id,name,guest,admin,super_mod,mod,banned,editpost,deletepost,deletetopic,posttopic,postreply,viewhidden,makehidden,mod_edituser,mod_banuser,description) VALUES
        (6,'Banned',false,false,false,false,true,false,false,false,false,false,false,false,false,false,NULL);`);
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','c',1);`);
      sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'Forum','a',1,false,false);`);
      sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,1,2),(3,1,3),(4,1,4),(5,1,5),(6,1,6);`);
      sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES
        ('00000000-0000-4000-8000-000000000301',1,'Open thread',3,'admin-bob',now(),now(),false,false,0,true,false,false);`);
    },
  });
  const {sql, q} = fx;
  try {
    const topic = '00000000-0000-4000-8000-000000000301';
    let key = 0;
    const reply = async text => {
      const id = randomUUID();
      return fx.client('alice-sub').bundle({atomic: [{create: 'Post', values: {id, topic, forum: '1', user: '2', username: 'member-alice', content: text, hidden: false, published: false, version: 0, deleted: false}}]}, {key: `ban-${++key}`});
    };
    const newTopic = async () => fx.client('alice-sub').bundle({atomic: [{create: 'Topic', values: {id: randomUUID(), forum: '1', title: 'banned topic', user: '2', username: 'member-alice', locked: false, important: false, views: 0, published: false, deleted: false, hidden: false}}]}, {key: `ban-${++key}`});
    const posts = () => Number(sql(`SELECT count(*) FROM ${q}.post`));
    const group = () => sql(`SELECT primary_group_id FROM ${q}."user" WHERE id=2`);
    const swap = (who, groupId) => fx.client(who).bundle({atomic: [{apply: 'User.adminUpdateUser', target: {ids: ['2']}, values: {primary_group: String(groupId), activated: true}}]}, {key: `ban-${++key}`});

    assert.equal((await reply('before the ban')).ok, true);
    assert.equal(posts(), 1);

    // A moderator without admin rights cannot swap the group, whatever mod_banuser says.
    const byModerator = await swap('editor-sub', 6);
    assert.equal(byModerator.ok, false, JSON.stringify(byModerator));
    assert.equal(group(), '2', 'a rejected ban leaves the group unchanged');
    const byMember = await fx.client('alice-sub').bundle({atomic: [{apply: 'User.adminUpdateUser', target: {ids: ['2']}, values: {primary_group: '3', activated: true}}]}, {key: `ban-${++key}`});
    assert.equal(byMember.ok, false, 'self-promotion stays refused');
    assert.equal(group(), '2');

    // Administrator bans: posting and starting topics are refused and write nothing; reading still works.
    const banned = await swap('bob-sub', 6);
    assert.equal(banned.ok, true, JSON.stringify(banned));
    assert.equal(group(), '6');
    const before = posts();
    assert.equal((await reply('while banned')).ok, false, 'a banned user cannot reply');
    assert.equal((await newTopic()).ok, false, 'a banned user cannot start a topic');
    assert.equal(posts(), before);
    assert.equal(Number(sql(`SELECT count(*) FROM ${q}.topic`)), 1);
    const visible = (await fx.client('alice-sub').read({read: 'Topic', select: ['id', 'title']})).rows;
    assert.deepEqual(visible.map(row => row.title), ['Open thread'], 'a banned user still reads public forums');

    // Unban restores the Member group and posting.
    const unbanned = await swap('bob-sub', 2);
    assert.equal(unbanned.ok, true, JSON.stringify(unbanned));
    assert.equal(group(), '2');
    assert.equal((await reply('after the unban')).ok, true);
    assert.equal(posts(), before + 1);

    console.log(JSON.stringify({schema: fx.schema, ok: true}));
  } finally {
    await fx.stop();
  }
});
