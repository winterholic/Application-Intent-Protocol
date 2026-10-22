// The one SDK client of the player. It is NOT executed by any test of this repository: it needs the UI tree (`store/authStore`), the
// generated binding of `facereview.aip` and the `@aip/sdk` tarball. The pure adapter (`readAdapter.ts`) is what the tests cover.
//
// The generated binding is produced from the definition, never written by hand:
//   aip service gen --lang ts --out src/aip/facereview.ts product/tests/fixtures/facereview-mysql/facereview.aip
import { connect } from '@aip/sdk';
import { contract } from '../aip/facereview';
import { useAuthStorage } from 'store/authStore';
import type { AipClient } from './readAdapter';

// The AIP service's address. It is the AIP server, not a path of the original REST API.
const BASE_URL: string = import.meta.env.VITE_AIP_URL ?? '/aip';

type SdkClient = AipClient & { replaceSession(token: string): Promise<unknown> };
let sdk: SdkClient | null = null;
let sdkToken: string | null = null;

/** The client for the member's current access token. A new token (after a reissue) replaces the session of the same client. */
function client(): AipClient {
  const token = useAuthStorage.getState().access_token;
  if (!token) {
    // An extension call needs a signed session: a guest's home screen is not served (decision D-P-UI-ADAPTER).
    throw Object.assign(new Error('a signed-in member is required'), { code: 'WIRE.UNAUTHENTICATED' });
  }
  if (sdk === null) {
    sdk = connect(BASE_URL, token, contract) as unknown as SdkClient;
  } else if (sdkToken !== token) {
    void sdk.replaceSession(token);
  }
  sdkToken = token;
  return sdk;
}

export const aipClient: AipClient = {
  extension: (name, input) => client().extension(name, input),
  writeExtension: (name, input, options) => client().writeExtension(name, input, options),
  apply: (request, options) => client().apply(request, options),
};
