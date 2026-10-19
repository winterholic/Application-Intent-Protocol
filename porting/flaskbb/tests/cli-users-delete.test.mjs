import assert from 'node:assert/strict';
import {test} from 'node:test';
import {GROUPS_SQL, USERS_SQL, binary, startFixture} from './harness.mjs';

// 7.5.3 F13: upstream `flaskbb users delete --username U [--force]`
// (cli/users.py:67-89) looks the user up by exact username, fails with
// "The user with username {} does not exist." when there is none (79-82),
// asks for confirmation unless --force (84-86), then calls `User.delete()`
// (88), which is `db.session.delete(self)` plus commit (user/models.py:465-468).
// The AIP service CLI has no command that runs a business transition, so the
// port's equivalent is the operator path this test drives: an admin identity
// bound with `service principal bind`, a `User` read for the lookup, and the
// declared `User.purge` transition for the delete. The confirmation prompt is a
// caller concern with no server counterpart. Row-level purge behavior
// (references, cascades) is pinned by purge.test.mjs; this test pins the
// username-addressed flow and the identity-binding interaction that only the
// service CLI path has (PRODUCT-GAPS.md G-14).
test('FlaskBB users delete maps to username lookup plus User.purge; a bound principal blocks the delete (real PostgreSQL)', {timeout: 120000}, async () => {
  const fx = await startFixture({
    prefix: 'aip-flaskbb-users-delete-',
    actors: {'admin-sub': 3, 'carol-sub': 6},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
    },
  });
  const {sql, q, run, config} = fx;
  try {
    const admin = fx.client('admin-sub');
    const users = () => Number(sql(`SELECT count(*) FROM ${q}."user"`));
    const principals = actor => sql(`SELECT enabled FROM ${q}.aip_principals WHERE actor_id = ${actor}`);
    const audit = id => JSON.parse(run(binary, ['service', 'audit', '--config', config, 'list', '--resource', 'User', '--target-id', id]));
    // Upstream `filter_by(username=...)` is an exact match; the declared read
    // offers `username.prefix`, so the exact name is picked from its rows.
    const lookup = async username => (await admin.read({read: 'User', select: ['id', 'username'], filter: [{field: 'username', op: 'prefix', value: username}]}))
      .rows.filter(row => row.username === username).map(row => [String(row.id), row.username]);

    assert.equal(users(), 6);

    // Unknown username: the lookup is empty (upstream's `user is None` branch),
    // and a purge aimed at a made-up id is refused without deleting anything.
    assert.deepEqual(await lookup('nobody'), []);
    const missing = await admin.bundle({atomic: [{apply: 'User.purge', target: {ids: ['999999']}}]}, {key: 'users-delete-missing'});
    assert.equal(missing.ok, false, JSON.stringify(missing));
    assert.equal(users(), 6);

    // A prefix is not a username: `member-al` must not resolve to alice.
    assert.deepEqual(await lookup('member-al'), []);
    assert.deepEqual(await lookup('member-alice'), [['2', 'member-alice']]);

    // The delete itself, for an account with no bound identity, addressed
    // through the looked-up id.
    assert.equal(principals(2), '');
    const [[aliceId]] = await lookup('member-alice');
    const deleted = await admin.bundle({atomic: [{apply: 'User.purge', target: {ids: [aliceId]}}]}, {key: 'users-delete-alice'});
    assert.equal(deleted.ok, true, JSON.stringify(deleted));
    assert.equal(users(), 5);
    assert.equal(sql(`SELECT count(*) FROM ${q}."user" WHERE username = 'member-alice'`), '0');
    // A second run for the same name finds nobody, like upstream's second run.
    assert.deepEqual(await lookup('member-alice'), []);
    const trail = audit('2');
    assert.equal(trail.ok, true, JSON.stringify(trail));
    assert.equal(trail.audit.length, 1);
    const [entry] = trail.audit;
    assert.deepEqual([entry.via, entry.transition, String(entry.actorId), entry.after], ['apply', 'purge', '3', null]);
    assert.equal(entry.before.username, 'member-alice');

    // Carol has an identity bound with `service principal bind`. The
    // principal row references the actor (aip_principals.actor_id FK without
    // an ON DELETE action), so the delete cannot commit, the user and the
    // binding stay, and nothing is audited. Upstream has no such coupling.
    const [[carolId]] = await lookup('member-carol');
    assert.equal(principals(6), 't');
    const blocked = await admin.bundle({atomic: [{apply: 'User.purge', target: {ids: [carolId]}}]}, {key: 'users-delete-carol'}).then(
      () => assert.fail('purge of an account with a bound principal must not commit'),
      error => error,
    );
    // Gap pin (G-14): a permanent FK violation surfaces as an unconfirmed,
    // retryable write instead of a refusal. Flip this once it is classified.
    assert.equal(blocked.retry, 'later');
    assert.equal(users(), 5);
    assert.deepEqual(await lookup('member-carol'), [['6', 'member-carol']]);
    assert.equal(principals(6), 't');
    assert.deepEqual(audit('6').audit, []);

    // `principal revoke` only disables the binding (enabled=false, row kept),
    // so it does not unblock the delete and no other operator command removes it.
    run(binary, ['service', 'principal', '--config', config, 'revoke', '--subject', 'carol-sub']);
    assert.equal(principals(6), 'f');
    await assert.rejects(() => admin.bundle({atomic: [{apply: 'User.purge', target: {ids: [carolId]}}]}, {key: 'users-delete-carol-revoked'}));
    assert.equal(users(), 5);
    assert.deepEqual(await lookup('member-carol'), [['6', 'member-carol']]);
    assert.equal(principals(6), 'f');
  } finally {
    await fx.stop();
  }
});
