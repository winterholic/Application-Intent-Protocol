export type AuthState = {
  getAccessToken(): string;
  setAccessToken(token: string): void;
};

type AuthError = Error & { status?: number; code?: string; body?: unknown };

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
        throw Object.assign(new Error('AIP reissue response has no access token'), { code: 'CLIENT.PROTOCOL_ERROR', status: response.status });
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
    if (response.status !== 401 || !token || url.includes('/auth/reissue')) return response;

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
