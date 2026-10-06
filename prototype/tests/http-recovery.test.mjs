import {test} from 'node:test';
import assert from 'node:assert/strict';
import {connectTypedApply} from '../../spikes/spike-v6-transport/client/typed.ts';
import {WriteUnsettled} from '../../spikes/spike-v6-transport/client/transport.ts';

const binding = {fingerprint: 'a'.repeat(64), idWire: 'decimal-string-v13'};
const request = {apply: 'Event.check', target: {ids: ['11']}};
const response = value => new Response(JSON.stringify(value));

for (const code of ['ORIGIN_NOT_ALLOWED', 'REQUEST_TIMEOUT', 'DB_UNAVAILABLE', 'WORKER_BUSY', 'DEADLINE_EXCEEDED', 'CONFLICT']) {
  test(`${code} on retry cannot settle a previously lost write response`, async () => {
    let attempts = 0;
    const sent = [];
    const client = connectTypedApply('http://recovery.invalid', 'token', binding, async (url, init) => {
      if (url.endsWith('/session')) return response({ok: true, principal: {actorId: '1'}, remainingMs: 60_000});
      sent.push(JSON.parse(init.body));
      attempts++;
      if (attempts === 1) throw new TypeError('committed response lost');
      if (attempts === 2) return response({ok: false, code});
      return response({ok: true, changed: ['11'], unchanged: [], tags: ['Event'], replayed: true});
    });
    await assert.rejects(client.apply(request, {key: 'stable-key', retries: 0}), WriteUnsettled);
    await assert.rejects(client.apply(request, {key: 'stable-key', retries: 0}), WriteUnsettled);
    assert.deepEqual(client.pending(), ['stable-key']);
    const recovered = await client.retryPending();
    assert.equal(recovered[0].replayed, true);
    assert.deepEqual(client.pending(), []);
    assert.deepEqual(sent, Array(3).fill({request, key: 'stable-key'}));
  });
  test(`${code} in the first response settles the execution rejection`, async () => {
    const client = connectTypedApply('http://recovery.invalid', 'token', binding, async url => response(url.endsWith('/session')
      ? {ok: true, principal: {actorId: '1'}, remainingMs: 60_000}
      : {ok: false, code}));
    assert.deepEqual(await client.apply(request, {key: 'fresh-key', retries: 0}), {ok: false, code});
    assert.deepEqual(client.pending(), []);
  });
}
