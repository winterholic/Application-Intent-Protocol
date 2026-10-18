import assert from 'node:assert/strict';
import {test} from 'node:test';
import {randomUUID} from 'node:crypto';
import {GROUPS_SQL, USERS_SQL, startFixture} from './harness.mjs';

test('G-8 shared_id may be duplicated within one mailbox through a sender-side duplicate', {timeout: 90000}, async () => {
  const fx = await startFixture({actors: {'alice-sub': 2, 'carol-sub': 6}, seed: (sql, q, schema) => {sql(GROUPS_SQL(schema)); sql(USERS_SQL(schema));}});
  const {sql, q, client} = fx;
  try {
    const shared = randomUUID();
    const now = new Date().toISOString();
    const copy = (user, unread) => ({user: String(user), from_user: '2', to_user: '6', shared_id: shared, subject: 'G8', date_created: now, date_modified: now, trash: false, draft: false, unread});
    const sent = await client('alice-sub').bundle({atomic: [
      {create: 'Conversation', values: copy(2, false)},
      {create: 'Conversation', values: copy(6, true)},
    ]}, {key: `g8-base-${shared}`});
    assert.equal(sent.ok, true, JSON.stringify(sent));
    const duplicate = await client('alice-sub').bundle({atomic: [{create: 'Conversation', values: copy(2, false)}]}, {key: `g8-duplicate-${shared}`});
    const count = sql(`SELECT count(*) FROM ${q}.conversation WHERE user_id=2 AND shared_id='${shared}'`);
    if (process.env.G8_EXPECT_UNIQUE === '1') {
      assert.equal(duplicate.ok, false, JSON.stringify(duplicate));
      assert.equal(count, '1');
    } else {
      assert.equal(duplicate.ok, true, JSON.stringify(duplicate));
      assert.equal(count, '2');
    }
    console.log(JSON.stringify({ok: true, uniqueExpected: process.env.G8_EXPECT_UNIQUE === '1', duplicateAccepted: duplicate.ok, sameMailboxRows: Number(count)}));
  } finally { await fx.stop(); }
});
