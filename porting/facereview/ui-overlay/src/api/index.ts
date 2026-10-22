// Legacy Axios API calls must be explicitly overlaid before this adapter is used for a cutover build.
// This file documents the shared AIP base URL for build and dev proxy verification; API modules used by this slice call `aipClient`.
export const aipBaseUrl: string = import.meta.env.VITE_AIP_URL ?? '/aip';

const notPorted = (..._args: unknown[]): never => {
  throw Object.assign(new Error('route is not ported to AIP'), { code: 'PORT.NOT_PORTED' });
};

const api = {
  get<T = unknown>(_url: string, _config?: unknown): Promise<{ data: T; status: number }> { return notPorted(); },
  post<T = unknown>(_url: string, _body?: unknown, _config?: unknown): Promise<{ data: T; status: number }> { return notPorted(); },
  patch<T = unknown>(_url: string, _body?: unknown, _config?: unknown): Promise<{ data: T; status: number }> { return notPorted(); },
  delete<T = unknown>(_url: string, _config?: unknown): Promise<{ data: T; status: number }> { return notPorted(); },
};

export default api;
