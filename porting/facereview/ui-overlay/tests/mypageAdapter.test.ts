import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createMypageApi } from '../src/api/mypageAdapter.ts';
import { createAuthTransport } from '../src/api/authAdapter.ts';

const reply = (status: number, body: unknown) => new Response(JSON.stringify(body), {
  status,
  headers: { 'Content-Type': 'application/json' },
});

test('password change page uses authenticated AIP mail challenge, code exchange and reset routes', async () => {
  const requests: Array<{ url: string; method: string; authorization: string | null; body: string | null }> = [];
  const transport = createAuthTransport({
    baseUrl: 'https://aip.test',
    state: { getAccessToken: () => 'test-member-token', setAccessToken: () => {} },
    fetchImpl: async (input, init) => {
      requests.push({
        url: String(input),
        method: init?.method ?? 'GET',
        authorization: new Headers(init?.headers).get('authorization'),
        body: typeof init?.body === 'string' ? init.body : null,
      });
      return reply(200, requests.length === 2 ? { reset_token: 'safe-test-token' } : { ok: true });
    },
  });
  const api = createMypageApi({
    baseUrl: 'https://aip.test/',
    fetchImpl: transport.fetch,
  });

  await api.sendEmailVerification();
  const verified = await api.verifyPasswordCode({ code: '123456' });
  await api.changePassword({ reset_token: verified.data.reset_token, new_password: 'example-only-password' });

  assert.equal(verified.data.reset_token, 'safe-test-token');
  assert.deepEqual(requests, [
    { url: 'https://aip.test/auth/email/verification', method: 'POST', authorization: 'Bearer test-member-token', body: '{}' },
    { url: 'https://aip.test/auth/password/verification', method: 'POST', authorization: 'Bearer test-member-token', body: '{"code":"123456"}' },
    { url: 'https://aip.test/auth/password', method: 'PATCH', authorization: 'Bearer test-member-token', body: '{"reset_token":"safe-test-token","new_password":"example-only-password"}' },
  ]);
});

test('failed AIP challenge response is surfaced without returning a success shape', async () => {
  const transport = createAuthTransport({
    baseUrl: 'https://aip.test',
    state: { getAccessToken: () => 'test-member-token', setAccessToken: () => {} },
    fetchImpl: async () => reply(429, { code: 'WIRE.RATE_LIMITED', msg: 'try later' }),
  });
  const api = createMypageApi({ baseUrl: 'https://aip.test', fetchImpl: transport.fetch });
  await assert.rejects(api.sendEmailVerification(), (error: { status?: number; code?: string }) => error.status === 429 && error.code === 'WIRE.RATE_LIMITED');
});
