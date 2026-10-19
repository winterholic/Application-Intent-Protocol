import assert from 'node:assert/strict';
import {test} from 'node:test';
import {randomUUID} from 'node:crypto';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

// F-5: a private message's sender must be the caller. Original FlaskBB always
// writes the sender copy (user = from = actor) and the recipient copy
// (user = to_user, from = actor, unread) and stores each Message with the
// author's own id. The port must reject every other shape and leave no rows.
test('FlaskBB conversations cannot forge the sender or mislabel a third party mailbox (real PostgreSQL)', {timeout: 90000}, async () => {
  const fx = await startFixture({
    prefix: 'aip-flaskbb-conv-security-',
    actors: {'alice-sub': 2, 'bob-sub': 3, 'carol-sub': 6},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
    },
  });
  const {sql, q, client} = fx;
  try {
    const alice = client('alice-sub');
    const carol = client('carol-sub');
    const now = new Date().toISOString();
    const row = (user, from, to, extra = {}) => ({
      user: String(user), from_user: String(from), to_user: String(to), shared_id: randomUUID(), subject: 'probe',
      date_created: now, date_modified: now, trash: false, draft: false, unread: false, ...extra,
    });
    const counts = () => sql(`SELECT (SELECT count(*) FROM ${q}.conversation) || ',' || (SELECT count(*) FROM ${q}.message)`);
    let key = 0;
    const create = (who, values, resource = 'Conversation') => who.bundle({atomic: [{create: resource, values}]}, {key: `f5-${++key}`});
    const mustReject = async (label, who, values, resource) => {
      const before = counts();
      const result = await create(who, values, resource);
      assert.equal(result.ok, false, `${label} must be rejected: ${JSON.stringify(result)}`);
      assert.equal(counts(), before, `${label}: no row may be written`);
    };

    // --- forged / mislabeled conversation rows (ids: alice 2, bob 3, carol 6)
    await mustReject('own mailbox row claiming from bob', alice, row(2, 3, 2));
    await mustReject('own mailbox row claiming from bob to carol', alice, row(2, 3, 6));
    await mustReject('message to self', alice, row(2, 2, 2));
    await mustReject('carol mailbox row mislabeled alice->bob', alice, row(6, 2, 3));
    await mustReject('carol mailbox row from bob', alice, row(6, 3, 6));
    await mustReject('recipient copy planted already trashed', alice, row(6, 2, 6, {trash: true}));
    await mustReject('recipient copy planted as already read', alice, row(6, 2, 6, {unread: false}));
    await mustReject('recipient copy planted as draft', alice, row(6, 2, 6, {draft: true, unread: true}));

    // --- legitimate send: sender copy + recipient copy, authored messages
    const shared = randomUUID();
    const sent = await alice.bundle({atomic: [
      {create: 'Conversation', values: row(2, 2, 6, {shared_id: shared})},
      {create: 'Conversation', values: row(6, 2, 6, {shared_id: shared, unread: true})},
    ]}, {key: 'f5-send'});
    assert.equal(sent.ok, true, JSON.stringify(sent));
    const [senderCopy, recipientCopy] = sent.created.map(item => String(item.id));
    const draft = await create(alice, row(2, 2, 6, {draft: true}));
    assert.equal(draft.ok, true, `own draft to a recipient stays allowed: ${JSON.stringify(draft)}`);

    // --- forged message authors
    const messageValues = (conversation, user, text = 'hello') => ({conversation, user: user === null ? null : String(user), message: text, date_created: now});
    await mustReject('message in own mailbox authored by bob', alice, messageValues(senderCopy, 3), 'Message');
    await mustReject('message in recipient mailbox authored by bob', alice, messageValues(recipientCopy, 3), 'Message');
    await mustReject('message without an author', alice, messageValues(senderCopy, null), 'Message');
    await mustReject('message authored by the recipient in their own copy', alice, messageValues(recipientCopy, 6), 'Message');

    // --- a stranger cannot write into someone else's conversation at all
    const bob = client('bob-sub');
    await mustReject('stranger message into a conversation they are not part of', bob, messageValues(senderCopy, 3), 'Message');

    // --- legitimate messages: author is the caller, in both copies; reply from the recipient
    const own = await alice.bundle({atomic: [
      {create: 'Message', values: messageValues(senderCopy, 2, 'hi carol')},
      {create: 'Message', values: messageValues(recipientCopy, 2, 'hi carol')},
    ]}, {key: 'f5-msg-a'});
    assert.equal(own.ok, true, JSON.stringify(own));
    const reply = await carol.bundle({atomic: [
      {create: 'Message', values: messageValues(recipientCopy, 6, 'hi alice')},
      {create: 'Message', values: messageValues(senderCopy, 6, 'hi alice')},
    ]}, {key: 'f5-msg-c'});
    assert.equal(reply.ok, true, JSON.stringify(reply));

    const authors = sql(`SELECT string_agg(conversation_id || ':' || user_id, ',' ORDER BY id) FROM ${q}.message`);
    assert.equal(authors, `${senderCopy}:2,${recipientCopy}:2,${recipientCopy}:6,${senderCopy}:6`);
    const labels = sql(`SELECT string_agg(user_id || '/' || from_user_id || '/' || to_user_id, ',' ORDER BY id) FROM ${q}.conversation`);
    assert.equal(labels, '2/2/6,6/2/6,2/2/6', 'only the legitimate sender copy, recipient copy and own draft exist');

    // Two independent clients race to create the same sender/recipient mailbox pair. The
    // losing transaction must roll back both copies and leave one row per mailbox.
    const racedShared = randomUUID();
    const attempts = await Promise.all([0, 1].map(attempt => client('alice-sub').bundle({atomic: [
      {create: 'Conversation', values: row(2, 2, 6, {shared_id: racedShared})},
      {create: 'Conversation', values: row(6, 2, 6, {shared_id: racedShared, unread: true})},
    ]}, {key: `f5-shared-race-${attempt}`})));
    assert.equal(attempts.filter(result => result.ok).length, 1, JSON.stringify(attempts));
    assert.equal(attempts.find(result => !result.ok)?.code, 'WIRE.ALREADY_EXISTS', JSON.stringify(attempts));
    assert.equal(sql(`SELECT string_agg(user_id::text, ',' ORDER BY user_id) FROM ${q}.conversation WHERE shared_id='${racedShared}'`), '2,6');
    const beforeDuplicate = counts();
    const duplicate = await create(alice, row(6, 2, 6, {shared_id: shared, unread: true}));
    assert.equal(duplicate.ok, false);
    assert.equal(duplicate.code, 'WIRE.ALREADY_EXISTS');
    assert.equal(counts(), beforeDuplicate, 'a duplicate mailbox copy must not change messages or conversations');

    console.log(JSON.stringify({schema: fx.schema, ok: true}));
  } finally {
    await fx.stop();
  }
});
