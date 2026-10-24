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

export function createMypageApi(options: { baseUrl: string; fetchImpl: typeof fetch }) {
  const base = options.baseUrl.replace(/\/$/, '');
  // The original backend used PATCH /password; AIP serves every /auth/* route as POST, so the browser preflight only admits POST.
  const request = async <T>(path: string, payload: Record<string, unknown> = {}) => {
    const response = await options.fetchImpl(`${base}${path}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    if (!response.ok) throw await responseError(response);
    return { status: response.status, data: await response.json() as T };
  };

  return {
    sendEmailVerification: () => request('/auth/email/verification'),
    verifyEmailCode: (props: { code: string }) => request('/auth/email/verify', props),
    verifyPasswordCode: (props: { code: string }) => request<{ reset_token: string; message?: string }>('/auth/password/verification', props),
    changePassword: (props: { reset_token: string; new_password: string }) => request('/auth/password', props),
  };
}
