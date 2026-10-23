import { aipClient, aipWrites, authTransport } from './aipClient';
import { useAuthStorage } from 'store/authStore';
import type { EmailCheckResponse, UserResponse } from 'types';

type TokenReply = { access_token: string; expires_at?: string; token_type?: string };

function currentUserId(): string {
  const token = useAuthStorage.getState().access_token;
  const payload = token.split('.')[1];
  if (!payload) throw Object.assign(new Error('signed-in access token is required'), { code: 'WIRE.UNAUTHENTICATED' });
  try {
    const normalized = payload.replace(/-/g, '+').replace(/_/g, '/');
    const claims = JSON.parse(atob(normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '='))) as { sub?: unknown };
    if (typeof claims.sub === 'string' && claims.sub.length > 0) return claims.sub;
  } catch {
    // The parsed subject is a query value only. Server signature verification and row policy still control every read/write.
  }
  throw Object.assign(new Error('access token has no usable subject'), { code: 'CLIENT.PROTOCOL_ERROR' });
}

export const checkEmail = (props: { email: string }) =>
  authTransport.authPost<EmailCheckResponse>('/auth/check-email', { email: props.email });

export const signIn = (props: { email: string; password: string }) => authTransport.authPost<TokenReply>('/auth/login', props);

export const signUp = (props: { email: string; password: string; name: string; favorite_genres: string[] }) =>
  authTransport.authPost<TokenReply>('/auth/signup', props);

export const refreshToken = async () => ({ status: 200, data: { access_token: await authTransport.refresh() } });

export const signOut = () => authTransport.authPost('/auth/logout', {});

export const getUserName = async (): Promise<{ status: number; data: UserResponse }> => {
  const userId = currentUserId();
  const [userResult, genreResult] = await Promise.all([
    aipClient.read({
      read: 'User',
      select: ['id', 'email', 'name', 'role', 'profile_image_id', 'is_tutorial_done', 'is_verify_email_done', 'created_at'],
      filter: [{ field: 'id', op: 'eq', value: userId }],
      limit: 1,
    }),
    aipClient.read({
      read: 'UserFavoriteGenre',
      select: ['genre'],
      // The resource's owner row policy already limits these rows to this member; user.eq is not an exposed filter.
      limit: 50,
    }),
  ]);
  const user = userResult.rows[0];
  if (!user) throw Object.assign(new Error('current user is not readable'), { code: 'WIRE.UNAUTHENTICATED' });
  const userData: UserResponse = {
    user_id: String(user.id),
    email: String(user.email ?? ''),
    name: String(user.name ?? ''),
    profile_image_id: typeof user.profile_image_id === 'number' ? user.profile_image_id : 0,
    is_tutorial_done: user.is_tutorial_done === true,
    is_verify_email_done: user.is_verify_email_done === true,
    role: user.role === 'SUPER_ADMIN' ? 'ADMIN' : user.role as 'GENERAL' | 'ADMIN',
    created_at: typeof user.created_at === 'string' ? user.created_at : '',
    favorite_genres: genreResult.rows.map((row) => row.genre).filter((genre): genre is string => typeof genre === 'string'),
  };
  return {
    status: 200,
    data: userData,
  };
};

export const completeTutorial = async () => {
  const result = await aipClient.apply({ apply: 'User.completeTutorial', target: { ids: [currentUserId()] } });
  if (!result.ok) throw Object.assign(new Error(`tutorial update failed: ${result.code}`), { code: result.code });
  return { status: 200, data: result };
};

export const updateProfile = async (props: { name?: string; profile_image_id?: number; favorite_genres?: string[] }) => {
  const result = await aipWrites.updateProfile(props);
  return { status: 200, data: result };
};

export const withdraw = async () => {
  const result = await aipWrites.withdraw();
  return { status: 200, data: result };
};
