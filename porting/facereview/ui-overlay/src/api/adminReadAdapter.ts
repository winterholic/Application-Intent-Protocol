// Administrator console reads of `new-facereview-admin-front` (pinned ab0386c) over the AIP SDK instead of `/v2/admin/*` and `/v2/auth/me`.
//
// The original answers one JSON per route. AIP answers `/read` rows, standalone aggregates and read extensions, so the envelope the screens
// parse is composed here the way `product/tests/fixtures/facereview-mysql/reference/slice13e_oracle.py` and `slice14h_oracle.py` compose it
// when they compare AIP with the original `AdminService` (62 list cases and 25 dashboard cases matched). Authorization is the server's:
// every total, count and read extension below is administrator-only there, so a member's call ends in `WIRE.ACCESS_DENIED` and this adapter
// never decides who may ask.

export type AdminRole = 'GENERAL' | 'ADMIN' | 'SUPER_ADMIN';
export type VideoRequestStatus = 'PENDING' | 'ACCEPTED' | 'REJECTED';
export type SignupTrendPeriod = '7d' | '30d' | '3m' | '1y' | '3y';

// These mirror `src/types/index.ts` of the pinned admin UI so that its build checks the composed shapes against what the screens read.
export interface AdminUser {
  user_id: string; email: string; name: string; role: AdminRole; profile_image_id: number; is_tutorial_done: boolean;
  is_verify_email_done: boolean; is_deleted: boolean; created_at: string; total_watch_count: number; total_comment_count: number;
}
export interface AdminVideoRequest {
  video_request_id: string; user_id: string; user_name: string; youtube_url: string; youtube_full_url: string; status: VideoRequestStatus;
  admin_comment: string; created_at: string; updated_at: string;
}
export interface AdminVideo {
  video_id: string; youtube_url: string; title: string; channel_name: string; category: string; duration: number; view_count: number;
  like_count: number; comment_count: number; created_at: string; is_deleted: boolean;
}
export interface AdminComment {
  comment_id: string; video_id: string; video_title: string; user_id: string; user_name: string; content: string; is_modified: boolean;
  is_deleted: boolean; created_at: string;
}
export type Page<K extends string, T> = { [P in K]: T[] } & { total: number; page: number; size: number; has_next: boolean };
export interface SystemStatus {
  server: { cpu_usage: number; memory_usage: number; memory_total_mb: number; disk_usage: number };
  api: { total_requests_1h: number; avg_response_time_ms: number; error_rate_1h: number };
  connections: { mysql: 'ok' | 'error'; redis: 'ok' | 'error'; mongodb: 'ok' | 'error' };
  checked_at: string;
}
export interface SignupTrendPoint { date: string; count: number }
export interface SignupTrend { points: SignupTrendPoint[]; granularity: 'day' | 'week' | 'month' }
export interface BusinessStats {
  signup_trend: SignupTrendPoint[];
  weekly_active_users: number;
  video_request_pipeline: { pending_count: number; avg_processing_minutes: number | null };
  content_health: {
    avg_completion_rate: number;
    emotion_distribution: { neutral: number; happy: number; surprise: number; sad: number; angry: number };
    category_top5: Array<{ category: string; view_count: number }>;
    dominant_emotion_video_counts: Array<{ emotion: string; video_count: number }>;
  };
  computed_at: string;
}
export interface Me {
  user_id: string; email: string; name: string; profile_image_id: number; is_tutorial_done: boolean; is_verify_email_done: boolean;
  role: AdminRole; created_at: string; favorite_genres: string[];
}

/** What the adapter needs of the SDK client (`connect()` of `@aip/sdk`; `aggregate` is the standalone-aggregate call of its typed client). */
export interface AdminReadClient {
  read(query: Record<string, unknown>): Promise<{ rows: Record<string, unknown>[] }>;
  aggregate(request: { aggregate: string; input?: Record<string, unknown>; filter?: unknown[] }): Promise<number | null>;
  extension(name: string, input: Record<string, unknown>): Promise<Record<string, unknown>>;
}

export interface AdminReadOptions {
  now?: () => Date;
}

const DAY = 86_400_000;
const GENRES = ['drama', 'eating', 'travel', 'cook', 'show', 'information', 'game', 'sports', 'music', 'animal', 'beauty', 'comedy', 'horror',
  'exercise', 'vlog', 'etc'];
const PERIODS: Record<SignupTrendPeriod, readonly [number, 'day' | 'week' | 'month']> = {
  '7d': [7, 'day'], '30d': [30, 'day'], '3m': [90, 'week'], '1y': [365, 'month'], '3y': [1095, 'month'],
};
const CALL_BATCH = 6;

const failure = (message: string, code: string): never => {
  throw Object.assign(new Error(message), { code });
};

/** The original returns a naive `datetime.isoformat()`; AIP returns RFC 3339. Both name the same UTC instant. */
export function naiveIso(value: string): string {
  const match = /^(\d{4}-\d{2}-\d{2})[Tt](\d{2}:\d{2}:\d{2})(\.\d+)?([Zz]|[+-]\d{2}:\d{2})$/.exec(value);
  if (!match) return failure('unexpected time value from the server', 'CLIENT.PROTOCOL_ERROR');
  const fraction = (match[3] ?? '').slice(1).padEnd(6, '0').slice(0, 6);
  const suffix = /^0*$/.test(fraction) ? '' : `.${fraction}`;
  if (/^[Zz]$/.test(match[4])) return `${match[1]}T${match[2]}${suffix}`;
  const instant = new Date(value);
  if (Number.isNaN(instant.getTime())) return failure('unexpected time value from the server', 'CLIENT.PROTOCOL_ERROR');
  return `${instant.toISOString().slice(0, 19)}${suffix}`;
}

/** The signed-in id is the `sub` of the access token. It is a query value only; the server verifies the token and applies its row policy. */
export function subjectOf(token: string): string {
  const payload = token.split('.')[1];
  if (!payload) return failure('signed-in access token is required', 'WIRE.UNAUTHENTICATED');
  try {
    const normalized = payload.replace(/-/g, '+').replace(/_/g, '/');
    const claims = JSON.parse(atob(normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '='))) as { sub?: unknown };
    if (typeof claims.sub === 'string' && claims.sub.length > 0) return claims.sub;
  } catch {
    // Fall through to the protocol error below.
  }
  return failure('access token has no usable subject', 'CLIENT.PROTOCOL_ERROR');
}

const rfc = (ms: number): string => new Date(ms).toISOString().replace(/\.\d{3}Z$/, 'Z');
const dayText = (ms: number): string => new Date(ms).toISOString().slice(0, 10);
const midnight = (ms: number): number => Math.floor(ms / DAY) * DAY;
const mondayOffset = (ms: number): number => (new Date(ms).getUTCDay() + 6) % 7;
const nextMonth = (ms: number): number => { const at = new Date(ms); return Date.UTC(at.getUTCFullYear(), at.getUTCMonth() + 1, 1); };
const monthStart = (ms: number): number => { const at = new Date(ms); return Date.UTC(at.getUTCFullYear(), at.getUTCMonth(), 1); };

async function inBatches<T, R>(items: readonly T[], work: (item: T) => Promise<R>): Promise<R[]> {
  const out: R[] = [];
  for (let at = 0; at < items.length; at += CALL_BATCH) out.push(...await Promise.all(items.slice(at, at + CALL_BATCH).map(work)));
  return out;
}

function documentOf<T>(output: Record<string, unknown>): T {
  if (typeof output.body !== 'string') return failure('extension answered without a document', 'CLIENT.PROTOCOL_ERROR');
  try {
    return JSON.parse(output.body) as T;
  } catch {
    return failure('extension document is not JSON', 'CLIENT.PROTOCOL_ERROR');
  }
}

const reply = <T>(data: T): { data: T; status: number } => ({ data, status: 200 });
const newest = [{ field: 'created_at', dir: 'desc' }];

export function createAdminReadApi(client: AdminReadClient, options: AdminReadOptions = {}) {
  const clock = options.now ?? (() => new Date());

  const paging = (page?: number, size?: number): { page: number; size: number } => {
    const wantedPage = page ?? 1;
    const wantedSize = size ?? 20;
    if (!Number.isInteger(wantedPage) || wantedPage < 1) return failure('page must be a positive integer', 'CLIENT.BAD_ARGUMENT');
    if (!Number.isInteger(wantedSize) || wantedSize < 1 || wantedSize > 100) return failure('size must be 1 to 100', 'CLIENT.BAD_ARGUMENT');
    return { page: wantedPage, size: wantedSize };
  };
  const envelope = <K extends string, T>(key: K, rows: T[], total: number | null, page: number, size: number): Page<K, T> => {
    const count = total ?? 0;
    return { [key]: rows, total: count, page, size, has_next: (page - 1) * size + size < count } as Page<K, T>;
  };
  const either = (left: string, right: string, keyword: string) => ({ or: [{ field: left, op: 'like', value: keyword }, { field: right, op: 'like', value: keyword }] });
  const equals = (field: string, value: unknown) => ({ field, op: 'eq', value });

  const trend = async (period: string): Promise<SignupTrend> => {
    const [days, granularity] = Object.hasOwn(PERIODS, period) ? PERIODS[period as SignupTrendPeriod] : PERIODS['7d'];
    const today = midnight(clock().getTime());
    const start = today - (days - 1) * DAY;
    const end = today + DAY;
    const buckets: Array<{ date: string; low: number; high: number }> = [];
    if (granularity === 'day') {
      for (let at = 0; at < days; at++) buckets.push({ date: dayText(start + at * DAY), low: start + at * DAY, high: start + (at + 1) * DAY });
    } else if (granularity === 'week') {
      for (let cursor = start - mondayOffset(start) * DAY; cursor <= today; cursor += 7 * DAY) {
        buckets.push({ date: dayText(cursor), low: Math.max(cursor, start), high: Math.min(cursor + 7 * DAY, end) });
      }
    } else {
      for (let cursor = monthStart(start); cursor <= today; cursor = nextMonth(cursor)) {
        buckets.push({ date: dayText(cursor), low: Math.max(cursor, start), high: Math.min(nextMonth(cursor), end) });
      }
    }
    const counts = await inBatches(buckets, (bucket) => client.aggregate({ aggregate: 'User.signupCount', input: { from: rfc(bucket.low), to: rfc(bucket.high) } }));
    return { points: buckets.map((bucket, at) => ({ date: bucket.date, count: counts[at] ?? 0 })), granularity };
  };

  return {
    async getUsers(params: { keyword?: string; is_deleted?: boolean; page?: number; size?: number }) {
      const { page, size } = paging(params.page, params.size);
      const filter: unknown[] = [];
      if (params.keyword) filter.push(either('name', 'email', params.keyword));
      if (params.is_deleted !== undefined) filter.push(equals('is_deleted', params.is_deleted));
      const total = await client.aggregate({ aggregate: 'User.totalCount', filter });
      const { rows } = await client.read({
        read: 'User',
        select: ['id', 'email', 'name', 'role', 'profile_image_id', 'is_tutorial_done', 'is_verify_email_done', 'is_deleted', 'created_at', 'watchCount', 'commentCount'],
        filter, sort: newest, limit: size, offset: (page - 1) * size,
      });
      const users = rows.map((row): AdminUser => ({
        user_id: String(row.id), email: String(row.email), name: String(row.name), role: row.role as AdminRole,
        profile_image_id: row.profile_image_id as number, is_tutorial_done: row.is_tutorial_done === true,
        is_verify_email_done: row.is_verify_email_done === true, is_deleted: row.is_deleted === true, created_at: naiveIso(String(row.created_at)),
        total_watch_count: Number(row.watchCount ?? 0), total_comment_count: Number(row.commentCount ?? 0),
      }));
      return reply(envelope('users', users, total, page, size));
    },

    async getVideoRequests(params: { status?: VideoRequestStatus; page?: number; size?: number }) {
      const { page, size } = paging(params.page, params.size);
      const filter: unknown[] = params.status ? [equals('status', params.status)] : [];
      const total = await client.aggregate({ aggregate: 'VideoRequest.totalCount', filter });
      const { rows } = await client.read({
        read: 'VideoRequest',
        select: ['id', { user: { select: ['id', 'name'] } }, 'youtube_url', 'youtube_full_url', 'status', 'admin_comment', 'created_at', 'updated_at'],
        filter, sort: newest, limit: size, offset: (page - 1) * size,
      });
      const requests = rows.map((row): AdminVideoRequest => {
        const user = (row.user ?? {}) as { id?: unknown; name?: unknown };
        return {
          video_request_id: String(row.id), user_id: String(user.id ?? ''), user_name: String(user.name ?? ''), youtube_url: String(row.youtube_url),
          youtube_full_url: String(row.youtube_full_url), status: row.status as VideoRequestStatus, admin_comment: String(row.admin_comment ?? ''),
          created_at: naiveIso(String(row.created_at)), updated_at: naiveIso(String(row.updated_at)),
        };
      });
      return reply(envelope('requests', requests, total, page, size));
    },

    async getVideos(params: { keyword?: string; category?: string; page?: number; size?: number }) {
      const { page, size } = paging(params.page, params.size);
      const filter: unknown[] = [];
      if (params.keyword) filter.push(either('title', 'channel_name', params.keyword));
      if (params.category) filter.push(equals('category', params.category));
      const total = await client.aggregate({ aggregate: 'Video.totalCount', filter });
      const { rows } = await client.read({
        read: 'Video',
        select: ['id', 'youtube_url', 'title', 'channel_name', 'category', 'duration', 'view_count', 'likeCount', 'commentCount', 'created_at'],
        filter, sort: newest, limit: size, offset: (page - 1) * size,
      });
      const videos = rows.map((row): AdminVideo => ({
        video_id: String(row.id), youtube_url: String(row.youtube_url), title: String(row.title), channel_name: String(row.channel_name ?? ''),
        category: String(row.category), duration: Number(row.duration), view_count: Number(row.view_count), like_count: Number(row.likeCount ?? 0),
        comment_count: Number(row.commentCount ?? 0), created_at: naiveIso(String(row.created_at)),
        // The list's row policy only returns videos that are not deleted.
        is_deleted: false,
      }));
      return reply(envelope('videos', videos, total, page, size));
    },

    async getComments(params: { video_id?: string; keyword?: string; is_deleted?: boolean; page?: number; size?: number }) {
      const { page, size } = paging(params.page, params.size);
      const filter: unknown[] = [];
      if (params.video_id) filter.push(equals('video', params.video_id));
      if (params.keyword) filter.push({ field: 'content', op: 'like', value: params.keyword });
      if (params.is_deleted !== undefined) filter.push(equals('is_deleted', params.is_deleted));
      const window = { filter, sort: newest, limit: size, offset: (page - 1) * size };
      const total = await client.aggregate({ aggregate: 'Comment.totalCount', filter });
      const detail = await client.read({
        read: 'Comment',
        select: ['id', { video: { select: ['id', 'title'] } }, { user: { select: ['id', 'name'] } }, 'content', 'is_modified', 'is_deleted', 'created_at'],
        ...window,
      });
      // A video the row policy hides (soft-deleted) has a null traversal for everyone; its id is still a readable reference.
      const references = await client.read({ read: 'Comment', select: ['id', 'video'], ...window });
      const videoOf = new Map(references.rows.map((row) => [String(row.id), String(row.video)]));
      const comments = detail.rows.map((row): AdminComment => {
        const video = row.video as { title?: unknown } | null;
        const user = (row.user ?? {}) as { id?: unknown; name?: unknown };
        return {
          comment_id: String(row.id), video_id: videoOf.get(String(row.id)) ?? '', video_title: (video?.title ?? null) as string,
          user_id: String(user.id ?? ''), user_name: String(user.name ?? ''), content: String(row.content), is_modified: row.is_modified === true,
          is_deleted: row.is_deleted === true, created_at: naiveIso(String(row.created_at)),
        };
      });
      return reply(envelope('comments', comments, total, page, size));
    },

    async getSystemStatus() {
      return reply(documentOf<SystemStatus>(await client.extension('User.systemStatus', {})));
    },

    async getSignupTrend(period: SignupTrendPeriod) {
      return reply(await trend(period));
    },

    async getBusinessStats() {
      const now = clock();
      const [signup, weekly, pipeline, health, categories] = await Promise.all([
        trend('7d'),
        client.aggregate({ aggregate: 'User.weeklyActive', input: { since: rfc(now.getTime() - 7 * DAY) } }),
        client.extension('VideoRequest.pipeline', {}),
        client.extension('Video.contentHealth', {}),
        inBatches(GENRES, async (category) => {
          // Only a genre with a visible video is a group of the original's query; the sum alone cannot tell none from zero views.
          const visible = await client.aggregate({ aggregate: 'Video.totalCount', filter: [equals('category', category)] });
          if (!visible) return null;
          const views = await client.aggregate({ aggregate: 'Video.categoryViews', input: { category } });
          return { category, view_count: Math.trunc(views ?? 0) };
        }),
      ]);
      const content = documentOf<Omit<BusinessStats['content_health'], 'category_top5'>>(health);
      const stats: BusinessStats = {
        signup_trend: signup.points,
        weekly_active_users: weekly ?? 0,
        video_request_pipeline: documentOf<BusinessStats['video_request_pipeline']>(pipeline),
        content_health: {
          avg_completion_rate: content.avg_completion_rate,
          emotion_distribution: content.emotion_distribution,
          category_top5: categories.filter((row): row is { category: string; view_count: number } => row !== null).sort((a, b) => b.view_count - a.view_count),
          dominant_emotion_video_counts: content.dominant_emotion_video_counts,
        },
        computed_at: naiveIso(`${now.toISOString().slice(0, 19)}Z`),
      };
      return reply(stats);
    },

    /** `/v2/auth/me`: the administrator's own row (SUPER_ADMIN stays SUPER_ADMIN) and their own favorite genres. */
    async getMe(userId: string) {
      const [user, genres] = await Promise.all([
        client.read({
          read: 'User',
          select: ['id', 'email', 'name', 'role', 'profile_image_id', 'is_tutorial_done', 'is_verify_email_done', 'created_at'],
          filter: [equals('id', userId)], limit: 1,
        }),
        // The row policy of UserFavoriteGenre already limits these rows to the caller.
        client.read({ read: 'UserFavoriteGenre', select: ['genre'], limit: 50 }),
      ]);
      const row = user.rows[0];
      if (!row) return failure('current user is not readable', 'WIRE.UNAUTHENTICATED');
      const me: Me = {
        user_id: String(row.id), email: String(row.email ?? ''), name: String(row.name ?? ''), profile_image_id: typeof row.profile_image_id === 'number' ? row.profile_image_id : 0,
        is_tutorial_done: row.is_tutorial_done === true, is_verify_email_done: row.is_verify_email_done === true, role: row.role as AdminRole,
        created_at: naiveIso(String(row.created_at)), favorite_genres: genres.rows.map((item) => item.genre).filter((genre): genre is string => typeof genre === 'string'),
      };
      return reply(me);
    },
  };
}
