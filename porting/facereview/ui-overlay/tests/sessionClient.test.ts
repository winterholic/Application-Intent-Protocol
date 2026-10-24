import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createSessionClientProvider } from '../src/api/sessionClient.ts';

const tokenFor = (subject: string, nonce = '1') => `h.${Buffer.from(JSON.stringify({ sub: subject, n: nonce })).toString('base64url')}.s`;

function harness(initial: string) {
  let current = initial;
  const created: Array<{ token: string; replaced: string[] }> = [];
  const provider = createSessionClientProvider({
    getToken: () => current,
    connectWith: (token: string) => {
      const client = { token, replaced: [] as string[], async replaceSession(next: string) { client.replaced.push(next); } };
      created.push(client);
      return client;
    },
  });
  return { provider, created, signIn: (token: string) => { current = token; } };
}

test('the first call connects with the current token and the same token reuses the client', async () => {
  const { provider, created } = harness(tokenFor('admin-1'));
  const first = await provider.client();
  assert.equal(await provider.client(), first);
  assert.equal(created.length, 1);
});

test('a refreshed token of the same user is carried over with replaceSession on the same client', async () => {
  const { provider, created, signIn } = harness(tokenFor('admin-1', '1'));
  const first = await provider.client();
  const refreshed = tokenFor('admin-1', '2');
  signIn(refreshed);
  assert.equal(await provider.client(), first);
  assert.deepEqual(created[0].replaced, [refreshed]);
  assert.equal(created.length, 1);
});

test('parallel reads after the same user signs in again replace the session once', async () => {
  // Each extra replaceSession bumps the SDK generation and discards reads already in flight (ScopeChanged).
  const replaced: string[] = [];
  let current = tokenFor('member-1', '1');
  const provider = createSessionClientProvider({
    getToken: () => current,
    connectWith: () => ({
      async replaceSession(next: string) {
        await new Promise((resolve) => setImmediate(resolve));
        replaced.push(next);
      },
    }),
  });
  const first = await provider.client();
  current = tokenFor('member-1', '2');
  const [a, b] = await Promise.all([provider.client(), provider.client()]);
  assert.equal(a, first);
  assert.equal(b, first);
  assert.deepEqual(replaced, [current]);
});

test('a failed replacement is attempted again by the next call', async () => {
  let attempts = 0;
  let current = tokenFor('member-1', '1');
  const provider = createSessionClientProvider({
    getToken: () => current,
    connectWith: () => ({
      async replaceSession() {
        attempts++;
        if (attempts === 1) throw Object.assign(new Error('offline'), { code: 'CLIENT.NETWORK' });
      },
    }),
  });
  const first = await provider.client();
  current = tokenFor('member-1', '2');
  await assert.rejects(provider.client(), { code: 'CLIENT.NETWORK' });
  assert.equal(await provider.client(), first);
  assert.equal(attempts, 2);
});

test('another user signing in after a logout gets a new client; the old session is never handed to them', async () => {
  const { provider, created, signIn } = harness(tokenFor('admin-1'));
  const first = await provider.client();
  signIn(tokenFor('admin-2'));
  const second = await provider.client();
  assert.notEqual(second, first);
  assert.equal(created.length, 2);
  assert.deepEqual(created[0].replaced, [], 'replaceSession (which would refuse a different principal) is not called');
  assert.equal(created[1].token, tokenFor('admin-2'));
});

test('without a token no client is built', async () => {
  const { provider, created } = harness('');
  await assert.rejects(provider.client(), { code: 'WIRE.UNAUTHENTICATED' });
  assert.equal(created.length, 0);
});

test('a token without a usable subject is refused rather than guessed', async () => {
  const { provider } = harness('not-a-jwt');
  await assert.rejects(provider.client(), { code: 'WIRE.UNAUTHENTICATED' });
});
