import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createAuthTransport, type AuthState } from '../src/api/authAdapter.ts';

function state(initial = 'old-token'): AuthState & { token: string; writes: string[] } {
  const target = {
    token: initial,
    writes: [] as string[],
    getAccessToken() { return this.token; },
    setAccessToken(token: string) { this.writes.push(token); this.token = token; },
  };
  return target;
}

const response = (status: number, body: unknown) => new Response(JSON.stringify(body), {
  status,
  headers: { 'Content-Type': 'application/json' },
});

test('a 401 reissues once with the HttpOnly cookie and retries using the new access token', async () => {
  const session = state();
  const requests: Array<{ url: string; authorization: string | null; credentials: RequestCredentials | undefined }> = [];
  const fetcher: typeof fetch = async (input, init) => {
    const url = String(input);
    const headers = new Headers(init?.headers);
    requests.push({ url, authorization: headers.get('authorization'), credentials: init?.credentials });
    if (url.endsWith('/auth/reissue')) return response(200, { access_token: 'new-token' });
    return headers.get('authorization') === 'Bearer old-token' ? response(401, { code: 'WIRE.TOKEN_EXPIRED' }) : response(200, { ok: true });
  };
  const transport = createAuthTransport({ baseUrl: 'https://aip.test', state: session, fetchImpl: fetcher });

  const result = await transport.fetch('https://aip.test/read', { method: 'POST', body: '{}' });

  assert.equal(result.status, 200);
  assert.equal(session.token, 'new-token');
  assert.deepEqual(requests.map(({ url, authorization }) => [url, authorization]), [
    ['https://aip.test/read', 'Bearer old-token'],
    ['https://aip.test/auth/reissue', null],
    ['https://aip.test/read', 'Bearer new-token'],
  ]);
  assert.equal(requests[1]?.credentials, 'include');
});

test('concurrent 401 responses share one reissue and all retry once', async () => {
  const session = state();
  let reissues = 0;
  let releaseReissue!: () => void;
  const gate = new Promise<void>((resolve) => { releaseReissue = resolve; });
  const fetcher: typeof fetch = async (input, init) => {
    const url = String(input);
    if (url.endsWith('/auth/reissue')) {
      reissues += 1;
      await gate;
      return response(200, { access_token: 'new-token' });
    }
    return new Headers(init?.headers).get('authorization') === 'Bearer old-token' ? response(401, {}) : response(200, {});
  };
  const transport = createAuthTransport({ baseUrl: 'https://aip.test', state: session, fetchImpl: fetcher });

  const first = transport.fetch('https://aip.test/read/1');
  const second = transport.fetch('https://aip.test/read/2');
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(reissues, 1);
  releaseReissue();
  const replies = await Promise.all([first, second]);

  assert.deepEqual(replies.map((reply) => reply.status), [200, 200]);
  assert.equal(reissues, 1);
  assert.deepEqual(session.writes, ['new-token']);
});

test('a failed reissue dispatches unauthorized and does not loop or replace the stored token', async () => {
  const session = state();
  const calls: string[] = [];
  let unauthorized = 0;
  const fetcher: typeof fetch = async (input) => {
    calls.push(String(input));
    return response(401, { code: 'WIRE.UNAUTHENTICATED' });
  };
  const transport = createAuthTransport({
    baseUrl: 'https://aip.test', state: session, fetchImpl: fetcher, onUnauthorized: () => { unauthorized += 1; },
  });

  await assert.rejects(transport.fetch('https://aip.test/read'), (error: { status?: number }) => error.status === 401);

  assert.deepEqual(calls, ['https://aip.test/read', 'https://aip.test/auth/reissue']);
  assert.equal(unauthorized, 1);
  assert.equal(session.token, 'old-token');
  assert.deepEqual(session.writes, []);
});

test('a request without an access token never attempts cookie reissue', async () => {
  const session = state('');
  let calls = 0;
  const transport = createAuthTransport({
    baseUrl: 'https://aip.test', state: session,
    fetchImpl: async () => { calls += 1; return response(401, {}); },
  });
  assert.equal((await transport.fetch('https://aip.test/read')).status, 401);
  assert.equal(calls, 1);
});

// The AIP service answers a refused call with HTTP 200 and an `{ ok: false, code }` envelope, never HTTP 401, so the reissue has to
// follow the wire codes the service documents as "reauthenticate" (found by a real browser run whose expired token was never reissued).
const wireRefusal = (code: string) => response(200, { ok: false, code, msg: 'refused' });

for (const code of ['WIRE.TOKEN_EXPIRED', 'WIRE.UNAUTHENTICATED']) {
  test(code + ' answered with HTTP 200 reissues once and retries with the new access token', async () => {
    const session = state();
    const seen: Array<[string, string | null]> = [];
    const fetcher: typeof fetch = async (input, init) => {
      const url = String(input);
      const authorization = new Headers(init?.headers).get('authorization');
      seen.push([url, authorization]);
      if (url.endsWith('/auth/reissue')) return response(200, { access_token: 'new-token' });
      return authorization === 'Bearer old-token' ? wireRefusal(code) : response(200, { ok: true, rows: [1] });
    };
    const transport = createAuthTransport({ baseUrl: 'https://aip.test', state: session, fetchImpl: fetcher });

    const reply = await transport.fetch('https://aip.test/read', { method: 'POST', body: '{}' });

    assert.deepEqual(await reply.json(), { ok: true, rows: [1] });
    assert.equal(session.token, 'new-token');
    assert.deepEqual(seen, [
      ['https://aip.test/read', 'Bearer old-token'],
      ['https://aip.test/auth/reissue', null],
      ['https://aip.test/read', 'Bearer new-token'],
    ]);
  });
}

test('concurrent wire-coded expiries share one reissue', async () => {
  const session = state();
  let reissues = 0;
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const fetcher: typeof fetch = async (input, init) => {
    if (String(input).endsWith('/auth/reissue')) { reissues += 1; await gate; return response(200, { access_token: 'new-token' }); }
    return new Headers(init?.headers).get('authorization') === 'Bearer old-token' ? wireRefusal('WIRE.TOKEN_EXPIRED') : response(200, { ok: true });
  };
  const transport = createAuthTransport({ baseUrl: 'https://aip.test', state: session, fetchImpl: fetcher });
  const first = transport.fetch('https://aip.test/read/1');
  const second = transport.fetch('https://aip.test/read/2');
  await new Promise((resolve) => setTimeout(resolve, 20));
  release();
  const replies = await Promise.all([first, second]);
  assert.equal(reissues, 1);
  assert.deepEqual(await Promise.all(replies.map((reply) => reply.json())), [{ ok: true }, { ok: true }]);
});

test('other wire refusals, non-JSON bodies and large successes are returned as they are, without a reissue', async () => {
  const large = response(200, { ok: true, rows: Array.from({ length: 4000 }, (_, id) => ({ id })) });
  const html = new Response('<html>nope</html>', { status: 200, headers: { 'Content-Type': 'text/html' } });
  for (const reply of [wireRefusal('WIRE.FORBIDDEN'), wireRefusal('WIRE.INVALID_STATE'), html, large]) {
    const session = state();
    const calls: string[] = [];
    const transport = createAuthTransport({ baseUrl: 'https://aip.test', state: session, fetchImpl: async (input) => { calls.push(String(input)); return reply.clone(); } });
    const result = await transport.fetch('https://aip.test/read');
    assert.deepEqual(calls, ['https://aip.test/read']);
    assert.equal(await result.text(), await reply.text());
  }
});

test('a refused reissue answered with HTTP 200 and a wire code dispatches unauthorized and surfaces that code', async () => {
  const session = state();
  let unauthorized = 0;
  const transport = createAuthTransport({
    baseUrl: 'https://aip.test', state: session, fetchImpl: async () => wireRefusal('WIRE.TOKEN_EXPIRED'), onUnauthorized: () => { unauthorized += 1; },
  });
  await assert.rejects(transport.fetch('https://aip.test/read'), (error: { code?: string }) => error.code === 'WIRE.TOKEN_EXPIRED');
  assert.equal(unauthorized, 1);
  assert.equal(session.token, 'old-token');
});

test('a retry that is refused again is returned and does not reissue a second time', async () => {
  const session = state();
  let reissues = 0;
  const fetcher: typeof fetch = async (input) => {
    if (String(input).endsWith('/auth/reissue')) { reissues += 1; return response(200, { access_token: 'new-token' }); }
    return wireRefusal('WIRE.TOKEN_EXPIRED');
  };
  const transport = createAuthTransport({ baseUrl: 'https://aip.test', state: session, fetchImpl: fetcher });
  const reply = await transport.fetch('https://aip.test/read');
  assert.equal(reissues, 1);
  assert.equal((await reply.json()).code, 'WIRE.TOKEN_EXPIRED');
});
