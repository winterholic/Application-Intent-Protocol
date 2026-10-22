// The one SDK client of the player. It is NOT executed by any test of this repository: it needs the UI tree (`store/authStore`), the
// generated binding of `facereview.aip` and the `@aip/sdk` tarball. The pure adapter (`readAdapter.ts`) is what the tests cover.
//
// The generated binding is produced from the definition, never written by hand:
//   aip service gen --lang ts --out src/aip/facereview.ts product/tests/fixtures/facereview-mysql/facereview.aip
import { connect } from '@aip/sdk';
import { contract } from '../aip/facereview';
import { useAuthStorage } from 'store/authStore';
import type { AipClient } from './readAdapter';
import { createAuthTransport } from './authAdapter';
import { createWriteApi, type WriteClient } from './writeAdapter';

// The AIP service's address. It is the AIP server, not a path of the original REST API.
const BASE_URL: string = import.meta.env.VITE_AIP_URL ?? '/aip';

type SdkClient = AipClient & WriteClient & { replaceSession(token: string): Promise<unknown> };
let sdk: SdkClient | null = null;
let sdkToken: string | null = null;
export const authTransport = createAuthTransport({
  baseUrl: BASE_URL,
  state: {
    getAccessToken: () => useAuthStorage.getState().access_token,
    setAccessToken: (access_token) => useAuthStorage.getState().setToken({ access_token }),
  },
  onUnauthorized: () => window.dispatchEvent(new Event('auth:unauthorized')),
});

/** The client for the member's current access token. A new token replaces the SDK session before another operation is sent. */
async function client(): Promise<SdkClient> {
  const token = useAuthStorage.getState().access_token;
  if (!token) {
    // An extension call needs a signed session: a guest's home screen is not served (decision D-P-UI-ADAPTER).
    throw Object.assign(new Error('a signed-in member is required'), { code: 'WIRE.UNAUTHENTICATED' });
  }
  if (sdk === null) {
    sdk = connect(BASE_URL, token, contract, authTransport.fetch) as unknown as SdkClient;
  } else if (sdkToken !== token) {
    await sdk.replaceSession(token);
  }
  sdkToken = token;
  return sdk;
}

export const aipClient: AipClient & WriteClient = {
  read: async (query) => (await client()).read(query) as Promise<{ ok: true; rows: Record<string, unknown>[] }>,
  extension: async (name, input) => (await client()).extension(name, input),
  writeExtension: async (name, input, options) => (await client()).writeExtension(name, input, options),
  apply: async (request, options) => (await client()).apply(request, options),
  bundle: async (request, options) => (await client()).bundle(request, options),
};

export const aipWrites = createWriteApi(aipClient, {
  userId: () => useAuthStorage.getState().user_id,
});
