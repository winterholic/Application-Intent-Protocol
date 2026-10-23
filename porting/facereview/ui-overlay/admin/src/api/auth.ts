import { useAuthStore } from 'store/authStore';
import { adminReads, authTransport } from './aipClient';
import { subjectOf } from './adminReadAdapter';

// The names and results of the pinned admin UI's `src/api/auth.ts`, over the AIP `/auth/*` routes instead of `/v2/auth/*`.
// The role check stays where it was: LoginPage reads `me.role`, and the server's own `isAdmin` policy decides every admin call regardless.

export const signIn = (props: { email: string; password: string }) =>
  authTransport.authPost<{ access_token: string }>('/auth/login', props);

export const getMe = () => adminReads.getMe(subjectOf(useAuthStore.getState().access_token));

export const refreshToken = async () => ({ status: 200, data: { access_token: await authTransport.refresh() } });

export const signOut = () => authTransport.authPost('/auth/logout', {});
