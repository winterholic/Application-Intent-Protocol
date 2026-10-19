import assert from 'node:assert/strict';
import {test} from 'node:test';
import {randomUUID} from 'node:crypto';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

// G-10 moderator ban. The original lets any holder of the mod_banuser
// permission (Moderator and Super Moderator groups carry it in
// upstream/flaskbb/fixtures/groups.py, secondary groups count via Has())
// swap a member's primary group to Banned and back to Member, except
// self-bans and moderator-vs-admin bans. The port cannot express that:
// `to` only takes path values, so no transition can assign the server-chosen
// Banned/Member row, and `patch` values cannot be constrained to banned-only
// (PRODUCT-GAPS.md G-10). Only the administrator's `User.adminUpdateUser`
// swaps groups. This test pins that gap: moderators WITH mod_banuser stay
// refused while the admin path works as the positive control.
test('FlaskBB moderators with mod_banuser cannot ban (real PostgreSQL)', {timeout: 90000}, async () => {
  const fx = await startFixture({
    prefix: 'aip-flaskbb-modban-',
    actors: {'alice-sub': 2, 'bob-sub': 3, 'editor-sub': 4, 'supermod-sub': 5, 'carol-sub': 6},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      // Upstream-faithful permissions: Moderator(4) and Super Moderator(5)
      // both carry mod_banuser upstream; the shared fixture leaves them false.
      sql(`UPDATE ${q}."group" SET mod_banuser=true WHERE id IN (4,5);`);
      sql(`INSERT INTO ${q}."group" (id,name,guest,admin,super_mod,mod,banned,editpost,deletepost,deletetopic,posttopic,postreply,viewhidden,makehidden,mod_edituser,mod_banuser,description) VALUES
        (6,'Banned',false,false,false,false,true,false,false,false,false,false,false,false,false,false,NULL);`);
      sql(USERS_SQL(schema));
      // Carol (Member) holds a secondary Moderator membership: the original
      // counts it for mod_banuser via Has().
      sql(`INSERT INTO ${q}.user_group (id,user_id,group_id) VALUES (1,6,4);`);
      sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','c',1);`);
      sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,show_moderators) VALUES (1,1,'Forum','a',1,false,false);`);
      sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,1,2),(3,1,3),(4,1,4),(5,1,5),(6,1,6);`);
      sql(`INSERT INTO ${q}.topic (id,forum_id,title,user_id,username,date_created,last_updated,locked,important,views,published,deleted,hidden) VALUES
        ('00000000-0000-4000-8000-000000000301',1,'Open thread',3,'admin-bob',now(),now(),false,false,0,true,false,false);`);
    },
  });
  const {sql, q} = fx;
  try {
    // The pin is vacuous unless the permission seed really applied.
    assert.equal(sql(`SELECT count(*) FROM ${q}."group" WHERE id IN (4,5) AND mod_banuser=true`), '2');
    assert.equal(sql(`SELECT count(*) FROM ${q}.user_group WHERE user_id=6 AND group_id=4`), '1');

    const topic = '00000000-0000-4000-8000-000000000301';
    let key = 0;
    const reply = async text => {
      const id = randomUUID();
      return fx.client('alice-sub').bundle({atomic: [{create: 'Post', values: {id, topic, forum: '1', user: '2', username: 'member-alice', content: text, hidden: false, published: false, version: 0, deleted: false}}]}, {key: `modban-${++key}`});
    };
    const posts = () => Number(sql(`SELECT count(*) FROM ${q}.post`));
    const groupOf = uid => sql(`SELECT primary_group_id FROM ${q}."user" WHERE id=${uid}`);
    const swap = (who, target, groupId) => fx.client(who).bundle({atomic: [{apply: 'User.adminUpdateUser', target: {ids: [String(target)]}, values: {primary_group: String(groupId), activated: true}}]}, {key: `modban-${++key}`});

    assert.equal((await reply('before any ban attempt')).ok, true);
    assert.equal(posts(), 1);

    // A Moderator holding mod_banuser cannot ban a member. The original bans.
    const byModerator = await swap('editor-sub', 2, 6);
    assert.equal(byModerator.ok, false, JSON.stringify(byModerator));
    assert.equal(groupOf(2), '2', 'a refused ban leaves the group unchanged');

    // A Super Moderator holding mod_banuser cannot ban either. The original
    // bans (IsAtleastSuperModerator needs no extra permission).
    const bySuperMod = await swap('supermod-sub', 2, 6);
    assert.equal(bySuperMod.ok, false, JSON.stringify(bySuperMod));
    assert.equal(groupOf(2), '2');

    // A secondary Moderator membership does not grant banning. The original
    // counts secondary groups for Has("mod_banuser").
    const bySecondary = await swap('carol-sub', 2, 6);
    assert.equal(bySecondary.ok, false, JSON.stringify(bySecondary));
    assert.equal(groupOf(2), '2');

    // The refused bans are no-ops: the member still posts.
    assert.equal((await reply('after refused bans')).ok, true);
    assert.equal(posts(), 2);

    // Moderators cannot touch other group swaps either: no self-promotion to
    // Administrator (opening `patch primary_group` to moderators would allow
    // exactly this, which is why the port keeps the swap admin-only).
    const selfPromotion = await swap('editor-sub', 4, 3);
    assert.equal(selfPromotion.ok, false, JSON.stringify(selfPromotion));
    assert.equal(groupOf(4), '4');

    // A moderator cannot ban an administrator. This matches the original's
    // outcome (moderator-vs-admin is refused there too), trivially, because
    // no moderator swap is allowed at all.
    const vsAdmin = await swap('editor-sub', 3, 6);
    assert.equal(vsAdmin.ok, false, JSON.stringify(vsAdmin));
    assert.equal(groupOf(3), '3');

    // Positive control: the administrator swap path works, so the refusals
    // above are permission denials, not a broken fixture.
    const banned = await swap('bob-sub', 2, 6);
    assert.equal(banned.ok, true, JSON.stringify(banned));
    assert.equal(groupOf(2), '6');
    assert.equal((await reply('while banned')).ok, false, 'a banned member cannot reply');
    assert.equal(posts(), 2);
    const unbanned = await swap('bob-sub', 2, 2);
    assert.equal(unbanned.ok, true, JSON.stringify(unbanned));
    assert.equal(groupOf(2), '2');
    assert.equal((await reply('after the unban')).ok, true);
    assert.equal(posts(), 3);

    console.log(JSON.stringify({schema: fx.schema, ok: true}));
  } finally {
    await fx.stop();
  }
});
