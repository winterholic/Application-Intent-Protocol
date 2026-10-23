import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createAdminReadApi, naiveIso, subjectOf } from '../src/api/adminReadAdapter.ts';

type Query = Record<string, unknown>;

function fakeClient(handlers: {
  read?: (query: Query) => Record<string, unknown>[];
  aggregate?: (request: { aggregate: string; input?: Query; filter?: unknown[] }) => number | null;
  extension?: (name: string) => Record<string, unknown>;
} = {}) {
  const reads: Query[] = [];
  const aggregates: Array<{ aggregate: string; input?: Query; filter?: unknown[] }> = [];
  const extensions: string[] = [];
  return {
    reads, aggregates, extensions,
    client: {
      async read(query: Query) { reads.push(query); return { ok: true as const, rows: handlers.read?.(query) ?? [] }; },
      async aggregate(request: { aggregate: string; input?: Query; filter?: unknown[] }) { aggregates.push(request); return handlers.aggregate?.(request) ?? 0; },
      async extension(name: string) { extensions.push(name); return handlers.extension?.(name) ?? {}; },
    },
  };
}

const now = () => new Date('2026-10-23T09:30:00Z');

test('naiveIso turns the server RFC 3339 time into the original naive isoformat the screens parse', () => {
  assert.equal(naiveIso('2026-10-02T03:00:00Z'), '2026-10-02T03:00:00');
  assert.equal(naiveIso('2026-10-02T03:00:00.250Z'), '2026-10-02T03:00:00.250000');
  assert.equal(naiveIso('2026-10-02T12:00:00+09:00'), '2026-10-02T03:00:00');
  assert.throws(() => naiveIso('yesterday'), { code: 'CLIENT.PROTOCOL_ERROR' });
});

test('subjectOf reads the signed-in id from the access token and refuses a token without one', () => {
  const token = `h.${Buffer.from(JSON.stringify({ sub: 'user-9' })).toString('base64url')}.s`;
  assert.equal(subjectOf(token), 'user-9');
  assert.throws(() => subjectOf(''), { code: 'WIRE.UNAUTHENTICATED' });
  assert.throws(() => subjectOf(`h.${Buffer.from('{}').toString('base64url')}.s`), { code: 'CLIENT.PROTOCOL_ERROR' });
});

test('users composes the original list envelope: total of the same filter, newest first, offset by page', async () => {
  const { client, reads, aggregates } = fakeClient({
    aggregate: () => 25,
    read: () => [{
      id: 'u1', email: 'a@example.com', name: '앨리스', role: 'SUPER_ADMIN', profile_image_id: null, is_tutorial_done: null,
      is_verify_email_done: true, is_deleted: false, created_at: '2026-10-02T03:00:00Z', watchCount: 24, commentCount: null,
    }],
  });
  const api = createAdminReadApi(client);
  const reply = await api.getUsers({ keyword: '앨', is_deleted: false, page: 2, size: 10 });

  const filter = [{ or: [{ field: 'name', op: 'like', value: '앨' }, { field: 'email', op: 'like', value: '앨' }] }, { field: 'is_deleted', op: 'eq', value: false }];
  assert.deepEqual(aggregates, [{ aggregate: 'User.totalCount', filter }]);
  assert.deepEqual(reads[0], {
    read: 'User',
    select: ['id', 'email', 'name', 'role', 'profile_image_id', 'is_tutorial_done', 'is_verify_email_done', 'is_deleted', 'created_at', 'watchCount', 'commentCount'],
    filter, sort: [{ field: 'created_at', dir: 'desc' }], limit: 10, offset: 10,
  });
  assert.equal(reply.status, 200);
  assert.deepEqual(reply.data, {
    users: [{
      user_id: 'u1', email: 'a@example.com', name: '앨리스', role: 'SUPER_ADMIN', profile_image_id: null, is_tutorial_done: false,
      is_verify_email_done: true, is_deleted: false, created_at: '2026-10-02T03:00:00', total_watch_count: 24, total_comment_count: 0,
    }],
    total: 25, page: 2, size: 10, has_next: true,
  });
});

test('list paging fields: has_next is false on the last page and bad paging is refused before any request', async () => {
  const { client, reads } = fakeClient({ aggregate: () => 20 });
  const api = createAdminReadApi(client);
  assert.equal((await api.getUsers({ page: 2, size: 10 })).data.has_next, false);
  for (const bad of [{ page: 0 }, { page: 1.5 }, { size: 0 }, { size: 101 }]) {
    await assert.rejects(api.getUsers(bad), { code: 'CLIENT.BAD_ARGUMENT' });
  }
  assert.equal(reads.length, 1);
});

test('video requests traverse the requester and keep the status filter in both the total and the page', async () => {
  const { client, reads, aggregates } = fakeClient({
    aggregate: () => 1,
    read: () => [{
      id: 'r1', user: { id: 'u2', name: '밥' }, youtube_url: 'abc', youtube_full_url: 'https://youtu.be/abc', status: 'PENDING',
      admin_comment: null, created_at: '2026-10-03T00:00:00Z', updated_at: '2026-10-03T00:00:10Z',
    }],
  });
  const reply = await createAdminReadApi(client).getVideoRequests({ status: 'PENDING' });
  assert.deepEqual(aggregates[0], { aggregate: 'VideoRequest.totalCount', filter: [{ field: 'status', op: 'eq', value: 'PENDING' }] });
  assert.deepEqual(reads[0].select, ['id', { user: { select: ['id', 'name'] } }, 'youtube_url', 'youtube_full_url', 'status', 'admin_comment', 'created_at', 'updated_at']);
  assert.deepEqual(reply.data, {
    requests: [{
      video_request_id: 'r1', user_id: 'u2', user_name: '밥', youtube_url: 'abc', youtube_full_url: 'https://youtu.be/abc', status: 'PENDING',
      admin_comment: '', created_at: '2026-10-03T00:00:00', updated_at: '2026-10-03T00:00:10',
    }],
    total: 1, page: 1, size: 20, has_next: false,
  });
});

test('videos pass the keyword to title and channel and a category filter, with null channel shown as empty text', async () => {
  const { client, reads } = fakeClient({
    aggregate: () => 1,
    read: () => [{
      id: 'v1', youtube_url: 'y', title: '제목', channel_name: null, category: 'vlog', duration: 12, view_count: 3, likeCount: 2, commentCount: 1,
      created_at: '2026-10-01T00:00:00Z',
    }],
  });
  const reply = await createAdminReadApi(client).getVideos({ keyword: '제', category: 'vlog' });
  assert.deepEqual(reads[0].filter, [
    { or: [{ field: 'title', op: 'like', value: '제' }, { field: 'channel_name', op: 'like', value: '제' }] },
    { field: 'category', op: 'eq', value: 'vlog' },
  ]);
  assert.deepEqual(reply.data.videos[0], {
    video_id: 'v1', youtube_url: 'y', title: '제목', channel_name: '', category: 'vlog', duration: 12, view_count: 3, like_count: 2, comment_count: 1,
    created_at: '2026-10-01T00:00:00', is_deleted: false,
  });
});

test('comments read the video reference separately so a hidden video keeps its id and shows no title', async () => {
  const { client, reads } = fakeClient({
    aggregate: () => 2,
    read: (query) => {
      const select = query.select as unknown[];
      if (select.includes('video')) return [{ id: 'c1', video: 'v1' }, { id: 'c2', video: 'v-hidden' }];
      return [
        { id: 'c1', video: { id: 'v1', title: '보임' }, user: { id: 'u1', name: '앨리스' }, content: '안녕', is_modified: null, is_deleted: false, created_at: '2026-10-04T00:00:00Z' },
        { id: 'c2', video: null, user: { id: 'u1', name: '앨리스' }, content: '삭제됨', is_modified: true, is_deleted: true, created_at: '2026-10-03T00:00:00Z' },
      ];
    },
  });
  const reply = await createAdminReadApi(client).getComments({ video_id: 'v1', keyword: '안', is_deleted: false });
  assert.deepEqual(reads[0].filter, [
    { field: 'video', op: 'eq', value: 'v1' }, { field: 'content', op: 'like', value: '안' }, { field: 'is_deleted', op: 'eq', value: false },
  ]);
  assert.deepEqual(reply.data.comments.map((row) => [row.comment_id, row.video_id, row.video_title, row.is_modified, row.is_deleted]), [
    ['c1', 'v1', '보임', false, false], ['c2', 'v-hidden', null, true, true],
  ]);
});

test('signup trend buckets by day, by Monday week and by month with the original start and end clipping', async () => {
  const seen: Array<{ from: string; to: string }> = [];
  const { client } = fakeClient({ aggregate: (request) => { seen.push(request.input as { from: string; to: string }); return seen.length; } });
  const api = createAdminReadApi(client, { now });

  const week = await api.getSignupTrend('7d');
  assert.equal(week.data.granularity, 'day');
  assert.deepEqual(week.data.points.map((point) => point.date), ['2026-10-17', '2026-10-18', '2026-10-19', '2026-10-20', '2026-10-21', '2026-10-22', '2026-10-23']);
  assert.deepEqual(seen[0], { from: '2026-10-17T00:00:00Z', to: '2026-10-18T00:00:00Z' });
  assert.deepEqual(seen[6], { from: '2026-10-23T00:00:00Z', to: '2026-10-24T00:00:00Z' });

  seen.length = 0;
  const months = await api.getSignupTrend('1y');
  assert.equal(months.data.granularity, 'month');
  assert.equal(months.data.points[0].date, '2025-10-01');
  assert.equal(months.data.points.at(-1)?.date, '2026-10-01');
  assert.deepEqual(seen[0], { from: '2025-10-24T00:00:00Z', to: '2025-11-01T00:00:00Z' });
  assert.deepEqual(seen.at(-1), { from: '2026-10-01T00:00:00Z', to: '2026-10-24T00:00:00Z' });

  seen.length = 0;
  const weeks = await api.getSignupTrend('3m');
  assert.equal(weeks.data.granularity, 'week');
  // 2026-10-23 minus 89 days is 2026-07-26 (Sunday); its Monday is 2026-07-20, and the first bucket counts from the 26th.
  assert.equal(weeks.data.points[0].date, '2026-07-20');
  assert.deepEqual(seen[0], { from: '2026-07-26T00:00:00Z', to: '2026-07-27T00:00:00Z' });
  assert.equal(weeks.data.points.at(-1)?.date, '2026-10-19');
  assert.deepEqual(seen.at(-1), { from: '2026-10-19T00:00:00Z', to: '2026-10-24T00:00:00Z' });

  const unknown = await api.getSignupTrend('bogus' as never);
  assert.equal(unknown.data.granularity, 'day');
  assert.equal(unknown.data.points.length, 7);
});

test('business stats join the aggregates and the two read extensions; only genres with a visible video get views, most viewed first', async () => {
  const { client, aggregates, extensions } = fakeClient({
    aggregate: (request) => {
      if (request.aggregate === 'User.weeklyActive') return 5;
      if (request.aggregate === 'Video.totalCount') return (request.filter as Array<{ value: string }>)[0].value === 'etc' ? 0 : 1;
      if (request.aggregate === 'Video.categoryViews') return ({ vlog: 90, music: 400 } as Record<string, number>)[(request.input as { category: string }).category] ?? 7;
      return 1;
    },
    extension: (name) => name === 'VideoRequest.pipeline'
      ? { body: JSON.stringify({ pending_count: 3, avg_processing_minutes: null }) }
      : { body: JSON.stringify({ avg_completion_rate: 0.5, emotion_distribution: { neutral: 0.1, happy: 0.2, surprise: 0.3, sad: 0.2, angry: 0.2 }, dominant_emotion_video_counts: [{ emotion: 'happy', video_count: 2 }] }) },
  });
  const reply = await createAdminReadApi(client, { now }).getBusinessStats();

  assert.deepEqual(extensions.sort(), ['Video.contentHealth', 'VideoRequest.pipeline']);
  assert.deepEqual(aggregates.find((request) => request.aggregate === 'User.weeklyActive')?.input, { since: '2026-10-16T09:30:00Z' });
  const data = reply.data;
  assert.equal(data.signup_trend.length, 7);
  assert.equal(data.weekly_active_users, 5);
  assert.deepEqual(data.video_request_pipeline, { pending_count: 3, avg_processing_minutes: null });
  assert.equal(data.content_health.avg_completion_rate, 0.5);
  assert.deepEqual(data.content_health.dominant_emotion_video_counts, [{ emotion: 'happy', video_count: 2 }]);
  const categories = data.content_health.category_top5;
  assert.equal(categories.length, 15);
  assert.equal(categories.some((row) => row.category === 'etc'), false);
  assert.deepEqual(categories.slice(0, 2), [{ category: 'music', view_count: 400 }, { category: 'vlog', view_count: 90 }]);
  assert.equal(data.computed_at, '2026-10-23T09:30:00');
});

test('system status is the extension document and a malformed document is a protocol error', async () => {
  const status = { server: { cpu_usage: 1, memory_usage: 2, memory_total_mb: 3, disk_usage: 4 }, api: { total_requests_1h: 0, avg_response_time_ms: 0, error_rate_1h: 0 }, connections: { mysql: 'ok', redis: 'ok', mongodb: 'ok' }, checked_at: '2026-10-23T00:00:00' };
  const ok = fakeClient({ extension: () => ({ body: JSON.stringify(status) }) });
  assert.deepEqual((await createAdminReadApi(ok.client).getSystemStatus()).data, status);
  assert.deepEqual(ok.extensions, ['User.systemStatus']);
  const broken = fakeClient({ extension: () => ({ body: 'not json' }) });
  await assert.rejects(createAdminReadApi(broken.client).getSystemStatus(), { code: 'CLIENT.PROTOCOL_ERROR' });
});

test('system status keeps null host figures as null: a host without Linux counters is not a measured zero', async () => {
  const status = { server: { cpu_usage: null, memory_usage: null, memory_total_mb: null, disk_usage: null }, api: { total_requests_1h: 0, avg_response_time_ms: 0, error_rate_1h: 0 }, connections: { mysql: 'ok', redis: 'error', mongodb: 'ok' }, checked_at: '2026-10-23T00:00:00' };
  const { client } = fakeClient({ extension: () => ({ body: JSON.stringify(status) }) });
  const reply = await createAdminReadApi(client).getSystemStatus();
  assert.deepEqual(reply.data.server, { cpu_usage: null, memory_usage: null, memory_total_mb: null, disk_usage: null });
  assert.equal(reply.data.connections.redis, 'error');
});

test('me keeps SUPER_ADMIN and the signed-in member own favorites, and refuses an unreadable account', async () => {
  const { client, reads } = fakeClient({
    read: (query) => query.read === 'User'
      ? [{ id: 'user-9', email: 'c@example.com', name: '캐롤', role: 'SUPER_ADMIN', profile_image_id: 2, is_tutorial_done: true, is_verify_email_done: null, created_at: '2026-10-02T01:00:00Z' }]
      : [{ genre: 'vlog' }],
  });
  const reply = await createAdminReadApi(client).getMe('user-9');
  assert.deepEqual(reads[0].filter, [{ field: 'id', op: 'eq', value: 'user-9' }]);
  assert.deepEqual(reply.data, {
    user_id: 'user-9', email: 'c@example.com', name: '캐롤', profile_image_id: 2, is_tutorial_done: true, is_verify_email_done: false,
    role: 'SUPER_ADMIN', created_at: '2026-10-02T01:00:00', favorite_genres: ['vlog'],
  });
  await assert.rejects(createAdminReadApi(fakeClient().client).getMe('missing'), { code: 'WIRE.UNAUTHENTICATED' });
});

test('a server refusal reaches the caller with its code and nothing is swallowed into an empty list', async () => {
  const client = {
    async read() { return { ok: true as const, rows: [] }; },
    async aggregate(): Promise<number | null> { throw Object.assign(new Error('denied'), { code: 'WIRE.ACCESS_DENIED' }); },
    async extension() { return {}; },
  };
  await assert.rejects(createAdminReadApi(client).getUsers({}), { code: 'WIRE.ACCESS_DENIED' });
});
