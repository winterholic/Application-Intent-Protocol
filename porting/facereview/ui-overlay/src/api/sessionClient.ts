import { subjectOf } from './adminReadAdapter.ts';

/**
 * One SDK client per signed-in user. The SDK's replaceSession moves pending writes to a refreshed token of the SAME principal and
 * refuses another one (ScopeConflict), so a different user signing in after a logout must get a new client, not the old one: nothing
 * the first user read or left pending is handed to the second. The token itself is only compared by its subject here; the server
 * verifies the signature on every call.
 */
export interface SessionSdk {
  replaceSession(token: string): Promise<unknown>;
}

export function createSessionClientProvider<C extends SessionSdk>(options: {
  getToken: () => string;
  connectWith: (token: string) => C;
}) {
  let current: C | null = null;
  let currentToken: string | null = null;
  let currentSubject: string | null = null;

  return {
    async client(): Promise<C> {
      const token = options.getToken();
      if (!token) throw Object.assign(new Error('a signed-in user is required'), { code: 'WIRE.UNAUTHENTICATED' });
      let subject: string;
      try {
        subject = subjectOf(token);
      } catch {
        throw Object.assign(new Error('access token has no usable subject'), { code: 'WIRE.UNAUTHENTICATED' });
      }
      if (current === null || currentSubject !== subject) {
        current = options.connectWith(token);
      } else if (currentToken !== token) {
        await current.replaceSession(token);
      }
      currentToken = token;
      currentSubject = subject;
      return current;
    },
  };
}
