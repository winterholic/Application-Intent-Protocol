import assert from 'node:assert/strict';
import {test} from 'node:test';
import {randomUUID} from 'node:crypto';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

// F-4: moderation rights must not depend on whether any `user_group` row
// exists. `exists UserGroup where A or B` scopes the whole `A or B` to the
// set, so a disjunct that never mentions the row (primary-group flags, the
// forum-moderator lookup) only held when some unrelated row existed.
test('FlaskBB moderation does not depend on unrelated user_group rows (real PostgreSQL)', {timeout: 90000}, async () => {
  const fx = await startFixture({
    prefix: 'aip-flaskbb-moderation-',
    actors: {'alice-sub': 2, 'bob-sub': 3, 'editor-sub': 4, 'supermod-sub': 5, 'carol-sub': 6, 'fmod-sub': 7},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
      sql(`INSERT INTO ${q}.user (id,username,email,date_joined,lastseen,primary_group_id,activated,language) VALUES (7,'forum-mod','fmod@example.test',now(),now(),2,true,'en');`);
      sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','c',1);`);
      sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'Forum One','a',1,false,false),(2,1,'Forum Two','b',2,false,false);`);
      sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,1,2),(3,1,3),(4,1,4),(5,1,5),(6,2,1),(7,2,2),(8,2,3),(9,2,4),(10,2,5);`);
      sql(`INSERT INTO ${q}.forum_moderator (id,forum_id,user_id) VALUES (1,1,7);`);
    },
  });
  const {sql, q, client} = fx;
  try {
    const alice = client('alice-sub');
    const actors = {
      moderator: client('editor-sub'),
      superMod: client('supermod-sub'),
      admin: client('bob-sub'),
      otherMember: client('carol-sub'),
      forumModerator: client('fmod-sub'),
      owner: alice,
    };
    const newTopic = forum => {
      const id = randomUUID();
      sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES ('${id}',${forum},'probe ${id.slice(0, 8)}',2,'member-alice',now(),now(),false,false,0,true,false,false);`);
      return id;
    };
    const deleted = id => sql(`SELECT deleted FROM ${q}.topic WHERE id='${id}'`) === 't';
    const attempt = async (who, forum) => {
      const id = newTopic(forum);
      const result = await actors[who].bundle({atomic: [{apply: 'Topic.delete', target: {ids: [id]}}]}, {key: `del-${id}`});
      assert.equal(deleted(id), result.ok === true, `${who}: response and DB agree (${JSON.stringify(result)})`);
      return result.ok === true;
    };
    const matrix = async () => ({
      moderator: await attempt('moderator', 1),
      superMod: await attempt('superMod', 1),
      admin: await attempt('admin', 1),
      owner: await attempt('owner', 1),
      otherMember: await attempt('otherMember', 1),
      forumModeratorOwnForum: await attempt('forumModerator', 1),
      forumModeratorOtherForum: await attempt('forumModerator', 2),
    });
    const expected = {
      moderator: true,
      superMod: true,
      admin: true,
      owner: true,
      otherMember: false,
      forumModeratorOwnForum: true,
      forumModeratorOtherForum: false,
    };

    assert.equal(sql(`SELECT count(*) FROM ${q}.user_group`), '0', 'fixture starts with an empty user_group table');
    const withoutRows = await matrix();
    assert.deepEqual(withoutRows, expected, 'empty user_group');

    // An unrelated membership row (carol in Member, nothing to do with any actor above) must change nothing.
    sql(`INSERT INTO ${q}.user_group (id,user_id,group_id) VALUES (1,6,2);`);
    const withUnrelatedRow = await matrix();
    assert.deepEqual(withUnrelatedRow, expected, 'one unrelated user_group row');
    assert.deepEqual(withUnrelatedRow, withoutRows, 'outcome is independent of user_group content');

    // Secondary Moderator membership still grants moderation (the rows that do matter).
    sql(`DELETE FROM ${q}.user_group;`);
    sql(`INSERT INTO ${q}.user_group (id,user_id,group_id) VALUES (2,6,4);`);
    assert.equal(await attempt('otherMember', 1), true, 'secondary Moderator group moderates');

    console.log(JSON.stringify({schema: fx.schema, ok: true, withoutRows, withUnrelatedRow}));
  } finally {
    await fx.stop();
  }
});
