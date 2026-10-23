export type AuthState = {
  getAccessToken(): string;
  setAccessToken(token: string): void;
};

type AuthError = Error & { status?: number; code?: string; body?: unknown };

// The AIP service refuses a call with HTTP 200 and `{ ok: false, code }`; these two codes are the ones it documents as "reauthenticate".
const REAUTHENTICATE_CODES = new Set(['WIRE.TOKEN_EXPIRED', 'WIRE.UNAUTHENTICATED']);
// An error envelope is tiny; anything larger is a success body that is not worth reading twice.
const MAX_ENVELOPE_BYTES = 8192;

/** A refused access token: HTTP 401 (the original service) or a small AIP error envelope carrying a reauthenticate code. */
async function accessTokenRefused(response: Response): Promise<boolean> {
  if (response.status === 401) return true;
  if (!response.ok || !(response.headers.get('content-type') ?? '').includes('json')) return false;
  const length = Number(response.headers.get('content-length'));
  if (Number.isFinite(length) && length > MAX_ENVELOPE_BYTES) return false;
  const text = await response.clone().text();
  if (text.length > MAX_ENVELOPE_BYTES) return false;
  try {
    const body: unknown = JSON.parse(text);
    const record = body && typeof body === 'object' ? body as Record<string, unknown> : {};
    return record.ok === false && typeof record.code === 'string' && REAUTHENTICATE_CODES.has(record.code);
  } catch {
    return false;
  }
}

function responseError(response: Response): Promise<AuthError> {
  return response.clone().json().catch(() => ({})).then((body: unknown) => {
    const record = body && typeof body === 'object' ? body as Record<string, unknown> : {};
    return Object.assign(new Error(typeof record.msg === 'string' ? record.msg : `AIP request failed (${response.status})`), {
      status: response.status,
      code: typeof record.code === 'string' ? record.code : undefined,
      body,
    });
  });
}

export function createAuthTransport(options: {
  baseUrl: string;
  state: AuthState;
  fetchImpl?: typeof fetch;
  onUnauthorized?: () => void;
}) {
  const fetchImpl = options.fetchImpl ?? fetch;
  let refreshInFlight: Promise<string> | null = null;

  async function refresh(): Promise<string> {
    if (refreshInFlight) return refreshInFlight;
    refreshInFlight = (async () => {
      const base = options.baseUrl.replace(/\/$/, '');
      const response = await fetchImpl(`${base}/auth/reissue`, {
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
      });
      if (!response.ok) throw await responseError(response);
      const body: unknown = await response.json();
      const token = body && typeof body === 'object' ? (body as Record<string, unknown>).access_token : undefined;
      if (typeof token !== 'string' || token.length === 0) {
        // A refused reissue is also an HTTP 200 envelope; keep its wire code instead of calling it a protocol error.
        const record = body && typeof body === 'object' ? body as Record<string, unknown> : {};
        throw Object.assign(new Error(typeof record.msg === 'string' ? record.msg : 'AIP reissue response has no access token'), {
          code: typeof record.code === 'string' ? record.code : 'CLIENT.PROTOCOL_ERROR', status: response.status,
        });
      }
      options.state.setAccessToken(token);
      return token;
    })().finally(() => { refreshInFlight = null; });
    return refreshInFlight;
  }

  async function fetchWithRefresh(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
    const token = options.state.getAccessToken();
    const send = (accessToken: string) => {
      const headers = new Headers(init?.headers);
      if (accessToken) headers.set('Authorization', `Bearer ${accessToken}`);
      return fetchImpl(input, { ...init, headers, credentials: init?.credentials ?? 'include' });
    };
    const response = await send(token);
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    if (!token || url.includes('/auth/reissue') || !(await accessTokenRefused(response))) return response;

    try {
      const nextToken = await refresh();
      return await send(nextToken);
    } catch (error) {
      options.onUnauthorized?.();
      throw error;
    }
  }

  async function authPost<T>(path: string, payload: Record<string, unknown>): Promise<{ status: number; data: T }> {
    const base = options.baseUrl.replace(/\/$/, '');
    const response = await fetchImpl(`${base}${path}`, {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    if (!response.ok) throw await responseError(response);
    const data = await response.json() as T;
    const token = data && typeof data === 'object' ? (data as Record<string, unknown>).access_token : undefined;
    if (typeof token === 'string' && token.length > 0) options.state.setAccessToken(token);
    return { status: response.status, data };
  }

  return {
    fetch: fetchWithRefresh,
    refresh,
    authPost,
  };
}
