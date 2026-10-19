import assert from 'node:assert/strict';
import {test} from 'node:test';
import {randomUUID} from 'node:crypto';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

// Original EditConversation: only the owner of a draft edits it. "Save" changes the
// recipient and the first message and keeps the draft (the subject is not touched, and the
// recipient is not validated); "send" turns the draft into a sent conversation with a fresh
// date_created while the caller writes the recipient copy. A sent conversation is never
// editable again. The port expresses save as Conversation.editDraft + Message.editDraftMessage
// and send as Conversation.sendDraft plus the usual recipient-copy create.
test('FlaskBB draft conversations can be edited and sent only by their owner (real PostgreSQL)', {timeout: 120000}, async () => {
  const fx = await startFixture({
    prefix: 'aip-flaskbb-conv-draft-',
    actors: {'alice-sub': 2, 'bob-sub': 3, 'carol-sub': 6},
    seed: (sql, q, schema) => {
      sql(GROUPS_SQL(schema));
      sql(USERS_SQL(schema));
    },
  });
  const {sql, q, client} = fx;
  try {
    const alice = client('alice-sub');
    const bob = client('bob-sub');
    const carol = client('carol-sub');
    const past = new Date(Date.now() - 3600_000).toISOString();
    let key = 0;
    const run = (who, atomic) => who.bundle({atomic}, {key: `draft-${++key}`});
    const mustReject = async (label, who, atomic) => {
      const before = snapshot();
      const result = await run(who, atomic);
      assert.equal(result.ok, false, `${label} must be rejected: ${JSON.stringify(result)}`);
      assert.equal(snapshot(), before, `${label}: nothing may change`);
    };
    const snapshot = () => sql(`SELECT coalesce((SELECT string_agg(id || ':' || user_id || ':' || coalesce(to_user_id, 0) || ':' || draft || ':' || coalesce(subject, '') || ':' || date_created || ':' || date_modified, ',' ORDER BY id) FROM ${q}.conversation), '') || '|' || coalesce((SELECT string_agg(id || ':' || conversation_id || ':' || message, ',' ORDER BY id) FROM ${q}.message), '')`);
    const conversationRow = id => sql(`SELECT user_id || '/' || coalesce(to_user_id, 0) || '/' || draft || '/' || subject FROM ${q}.conversation WHERE id=${id}`);
    const messageText = id => sql(`SELECT message FROM ${q}.message WHERE id=${id}`);
    const newDraft = async (toUser, text) => {
      const shared = randomUUID();
      const created = await run(alice, [{create: 'Conversation', values: {
        user: '2', from_user: '2', to_user: String(toUser), shared_id: shared, subject: 'draft subject',
        date_created: past, date_modified: past, trash: false, draft: true, unread: false,
      }}]);
      assert.equal(created.ok, true, JSON.stringify(created));
      const id = String(created.created[0].id);
      const message = await run(alice, [{create: 'Message', values: {conversation: id, user: '2', message: text, date_created: past}}]);
      assert.equal(message.ok, true, JSON.stringify(message));
      return {id, messageId: String(message.created[0].id), shared};
    };
    const save = (who, draft, values = {}, text) => run(who, [
      {apply: 'Conversation.editDraft', target: {ids: [draft.id]}, values: {to_user: '6', ...values}},
      ...(text === undefined ? [] : [{apply: 'Message.editDraftMessage', target: {ids: [draft.messageId]}, values: {message: text}}]),
    ]);

    // --- save: recipient and first message change, still a draft, subject untouched, date_modified moves
    const draft = await newDraft(3, 'first text');
    const saved = await save(alice, draft, {to_user: '6'}, 'edited text');
    assert.equal(saved.ok, true, JSON.stringify(saved));
    assert.equal(conversationRow(draft.id), '2/6/true/draft subject', 'recipient changed, subject kept, still a draft');
    assert.equal(messageText(draft.messageId), 'edited text');
    assert.equal(sql(`SELECT date_modified > now() - interval '5 minutes' AND date_created = '${past}'::timestamptz FROM ${q}.conversation WHERE id=${draft.id}`), 't', 'save refreshes date_modified only');

    // --- only the owner edits: the recipient and a stranger cannot, and nothing is written
    await mustReject('recipient editing the draft', bob, [{apply: 'Conversation.editDraft', target: {ids: [draft.id]}, values: {to_user: '3'}}]);
    await mustReject('stranger editing the draft message', carol, [{apply: 'Message.editDraftMessage', target: {ids: [draft.messageId]}, values: {message: 'hijack'}}]);
    await mustReject('stranger sending the draft', carol, [{apply: 'Conversation.sendDraft', target: {ids: [draft.id]}}]);

    // --- only the first message of a draft is rewritten
    const second = await run(alice, [{create: 'Message', values: {conversation: draft.id, user: '2', message: 'second text', date_created: new Date().toISOString()}}]);
    assert.equal(second.ok, true, JSON.stringify(second));
    const secondId = String(second.created[0].id);
    await mustReject('rewriting a later message of the draft', alice, [{apply: 'Message.editDraftMessage', target: {ids: [secondId]}, values: {message: 'rewrite'}}]);
    assert.equal((await save(alice, draft, {to_user: '6'}, 'edited again')).ok, true);
    assert.equal(messageText(draft.messageId), 'edited again');

    // --- the original save path does not validate the recipient: a draft addressed to oneself saves,
    //     but send refuses and rolls the whole bundle back
    assert.equal((await save(alice, draft, {to_user: '2'})).ok, true, 'saving a draft to oneself stays allowed like the original');
    assert.equal(conversationRow(draft.id), '2/2/true/draft subject');
    await mustReject('sending a draft addressed to oneself', alice, [{apply: 'Conversation.sendDraft', target: {ids: [draft.id]}}]);
    await mustReject('retargeting then sending a draft to oneself in one bundle', alice, [
      {apply: 'Conversation.editDraft', target: {ids: [draft.id]}, values: {to_user: '2'}},
      {apply: 'Conversation.sendDraft', target: {ids: [draft.id]}},
    ]);

    // --- send: retarget, mark sent with a fresh date_created, then the caller writes the recipient copy
    const sent = await run(alice, [
      {apply: 'Conversation.editDraft', target: {ids: [draft.id]}, values: {to_user: '3'}},
      {apply: 'Conversation.sendDraft', target: {ids: [draft.id]}},
      {create: 'Conversation', values: {
        user: '3', from_user: '2', to_user: '3', shared_id: draft.shared, subject: 'draft subject',
        date_created: new Date().toISOString(), date_modified: new Date().toISOString(), trash: false, draft: false, unread: true,
      }},
    ]);
    assert.equal(sent.ok, true, JSON.stringify(sent));
    assert.equal(conversationRow(draft.id), '2/3/false/draft subject', 'the draft is now a sent conversation to bob');
    assert.equal(sql(`SELECT date_created > now() - interval '5 minutes' FROM ${q}.conversation WHERE id=${draft.id}`), 't', 'send refreshes date_created');
    const recipientId = String(sent.created[0].id);
    const delivered = await run(alice, [{create: 'Message', values: {conversation: recipientId, user: '2', message: 'edited text for bob', date_created: new Date().toISOString()}}]);
    assert.equal(delivered.ok, true, JSON.stringify(delivered));
    assert.equal(sql(`SELECT count(*) FROM ${q}.conversation WHERE shared_id='${draft.shared}' AND draft=false`), '2', 'sender and recipient copies exist');
    const inbox = await bob.read({read: 'Conversation', select: ['id', 'unread', 'draft'], filter: [{field: 'user', op: 'eq', value: '3'}]});
    assert.deepEqual(inbox.rows.map(row => [String(row.id), row.unread, row.draft]), [[recipientId, true, false]], 'bob receives one unread, non-draft copy');

    // --- a sent conversation is never editable again, in either copy
    await mustReject('editing the recipient of a sent conversation', alice, [{apply: 'Conversation.editDraft', target: {ids: [draft.id]}, values: {to_user: '6'}}]);
    await mustReject('editing the recipient of the recipient copy', bob, [{apply: 'Conversation.editDraft', target: {ids: [recipientId]}, values: {to_user: '6'}}]);
    await mustReject('sending a sent conversation twice', alice, [{apply: 'Conversation.sendDraft', target: {ids: [draft.id]}}]);
    await mustReject('rewriting the message of a sent conversation', alice, [{apply: 'Message.editDraftMessage', target: {ids: [draft.messageId]}, values: {message: 'late edit'}}]);

    // --- a draft sent and edited while trashed behaves like the original (no trash check)
    const trashed = await newDraft(3, 'trashed text');
    assert.equal((await run(alice, [{apply: 'Conversation.moveToTrash', target: {ids: [trashed.id]}}])).ok, true);
    assert.equal((await save(alice, trashed, {to_user: '6'}, 'edited in trash')).ok, true, 'original EditConversation does not look at trash');
    assert.equal(messageText(trashed.messageId), 'edited in trash');

    console.log(JSON.stringify({schema: fx.schema, ok: true}));
  } finally {
    await fx.stop();
  }
});
