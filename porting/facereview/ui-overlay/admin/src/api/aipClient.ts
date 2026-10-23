// The one SDK client of the admin console. Like the member player's `aipClient.ts` it is NOT executed by the unit tests: it needs the admin UI
// tree (`store/authStore`), the generated binding of `facereview.aip` and the `@aip/sdk` tarball. The pure adapters (`adminReadAdapter.ts`,
// `adminAdapter.ts`) are what the tests cover; the admin build and the browser journey cover this file.
//
// The generated binding is produced from the definition, never written by hand:
//   aip service gen --lang ts --out src/aip/facereview.ts --sdk-import @aip/sdk <the exact definition used by the AIP service>
import { connect } from '@aip/sdk';
import { contract } from '../aip/facereview';
import { useAuthStore } from 'store/authStore';
import { createAuthTransport } from './authAdapter';
import { createAdminApprovalApi, createAdminWriteApi, type AdminApplyClient, type AdminJobClient } from './adminAdapter';
import { createAdminReadApi, type AdminReadClient } from './adminReadAdapter';

// The AIP service's address. It is the AIP server, not a path of the original REST API.
const BASE_URL: string = import.meta.env.VITE_AIP_URL ?? '/aip';

type SdkClient = AdminReadClient & AdminApplyClient & { jobs: AdminJobClient; replaceSession(token: string): Promise<unknown> };
let sdk: SdkClient | null = null;
let sdkToken: string | null = null;

export const authTransport = createAuthTransport({
  baseUrl: BASE_URL,
  state: {
    getAccessToken: () => useAuthStore.getState().access_token,
    setAccessToken: (token) => useAuthStore.getState().setToken(token),
  },
  onUnauthorized: () => window.dispatchEvent(new Event('auth:unauthorized')),
});

/** The client for the administrator's current access token. A new token replaces the SDK session before another operation is sent. */
async function client(): Promise<SdkClient> {
  const token = useAuthStore.getState().access_token;
  if (!token) throw Object.assign(new Error('a signed-in administrator is required'), { code: 'WIRE.UNAUTHENTICATED' });
  if (sdk === null) {
    sdk = connect(BASE_URL, token, contract, authTransport.fetch) as unknown as SdkClient;
  } else if (sdkToken !== token) {
    await sdk.replaceSession(token);
  }
  sdkToken = token;
  return sdk;
}

const aipClient: AdminReadClient & AdminApplyClient = {
  read: async (query) => (await client()).read(query),
  aggregate: async (request) => (await client()).aggregate(request),
  extension: async (name, input) => (await client()).extension(name, input),
  apply: async (request, options) => (await client()).apply(request, options),
};

const aipJobs: AdminJobClient = {
  enqueue: async (operation, input, options) => (await client()).jobs.enqueue(operation, input, options),
  status: async (id) => (await client()).jobs.status(id),
  result: async (id, operation) => (await client()).jobs.result(id, operation),
};

export const adminReads = createAdminReadApi(aipClient);
export const adminWrites = createAdminWriteApi(aipClient);
export const adminApproval = createAdminApprovalApi(aipJobs);
