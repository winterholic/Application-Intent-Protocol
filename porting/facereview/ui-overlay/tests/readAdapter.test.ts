import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createReadApi, parseText, type AipClient } from '../src/api/readAdapter.ts';
import { commentReadQuery } from '../src/api/readQueries.ts';

type Call = { kind: 'extension' | 'writeExtension' | 'apply'; name?: string; input: unknown; key?: string };

function fakeClient(answers: Record<string, unknown>, applyResult: { ok: boolean; code?: string } | Error = { ok: true }) {
  const calls: Call[] = [];
  const client: AipClient = {
    async read() { return { ok: true, rows: [] }; },
    async extension(name, input) {
      calls.push({ kind: 'extension', name, input });
      return answers[name] as Record<string, unknown>;
    },
    async writeExtension(name, input, options) {
      calls.push({ kind: 'writeExtension', name, input, key: options?.key });
      return answers[name] as never;
    },
    async apply(request, options) {
      calls.push({ kind: 'apply', input: request, key: options?.key });
      if (applyResult instanceof Error) throw applyResult;
      return applyResult;
    },
  };
  return { client, calls };
}

const keys = () => {
  let n = 0;
  return () => `key-${++n}`;
};

const video = (id: string) => ({ video_id: id, youtube_url: `yt-${id}`, title: `t-${id}`, dominant_emotion: 'happy', dominant_emotion_per: 61.5 });

test('home category: the extension takes no input and its JSON text becomes the original list', async () => {
  const lists = [{ category_name: 'animal', videos: [video('a')] }];
  const { client, calls } = fakeClient({ 'Video.homeCategory': { categories: JSON.stringify(lists) } });
  const api = createReadApi(client);
  assert.deepEqual(await api.getVideoList('animal'), lists);
  assert.deepEqual(calls, [{ kind: 'extension', name: 'Video.homeCategory', input: {} }], 'category_name is not sent: the original ignores it');
});

test('home video adapter emits only allowlisted fixture-list evidence when opted in', async () => {
  const fixtureId = 'aaaaaaaa-0000-4000-8000-000000000001';
  const events: unknown[] = [];
  const globals = globalThis as typeof globalThis & {
    __AIP_SAFE_LIST_FIXTURE_ID__?: string;
    __AIP_SAFE_LIST_EVIDENCE__?: (event: unknown) => void;
  };
  globals.__AIP_SAFE_LIST_FIXTURE_ID__ = fixtureId;
  globals.__AIP_SAFE_LIST_EVIDENCE__ = (event) => events.push(event);
  try {
    const { client } = fakeClient({ 'Video.homeVideoAll': {
      videos: JSON.stringify([{ ...video(fixtureId), title: 'must-not-be-recorded' }]),
      total: 1, page: 1, size: 20, has_next: false,
    } });
    await createReadApi(client).getAllVideo({ page: 1, size: 20, emotion: 'all' });
  } finally {
    delete globals.__AIP_SAFE_LIST_FIXTURE_ID__;
    delete globals.__AIP_SAFE_LIST_EVIDENCE__;
  }
  assert.deepEqual(events, [{ extension: 'Video.homeVideoAll', adapter_list_length: 1, contains_fixture_video: true }]);
  assert.equal(JSON.stringify(events).includes('must-not-be-recorded'), false);
});

test('search: the UI sends no emotions, the page keeps total/page/size/has_next and the videos are parsed', async () => {
  const output = { videos: JSON.stringify([video('a'), video('b')]), total: 7, page: 2, size: 2, has_next: true };
  const { client, calls } = fakeClient({ 'Video.homeSearch': output });
  const result = await createReadApi(client).searchVideos({ page: 2, size: 2, keyword_type: 'title', keyword: 'cat' });
  assert.deepEqual(result, { videos: [video('a'), video('b')], total: 7, page: 2, size: 2, has_next: true });
  assert.deepEqual(calls[0].input, { emotions: '', keyword: 'cat', keyword_type: 'title', page: 2, size: 2 });
});

test('video/all and bookmark lists return the videos array of their page', async () => {
  const page = { videos: JSON.stringify([video('a')]), total: 1, page: 1, size: 20, has_next: false };
  const { client, calls } = fakeClient({ 'Video.homeVideoAll': page, 'Video.homeBookmark': page });
  const api = createReadApi(client);
  assert.deepEqual(await api.getAllVideo({ page: 1, size: 20, emotion: 'happy' }), [video('a')]);
  assert.deepEqual(await api.getBookmarkVideos({ page: 3, size: 5, emotion: 'all' }), [video('a')]);
  assert.deepEqual(calls.map((call) => [call.name, call.input]), [
    ['Video.homeVideoAll', { emotion: 'happy', page: 1, size: 20 }],
    ['Video.homeBookmark', { emotion: 'all', page: 3, size: 5 }],
  ]);
});

test('personalized list', async () => {
  const { client } = fakeClient({ 'Video.homePersonalized': { videos: JSON.stringify([video('p')]) } });
  assert.deepEqual(await createReadApi(client).getPersonalRecommendedVideo(), [video('p')]);
});

test('comment read matches the public Comment query and nested User selection contract', () => {
  const query = commentReadQuery('fixture-video-id');
  assert.deepEqual(query, {
    read: 'Comment',
    select: ['id', 'content', 'is_modified', 'created_at', { user: { select: ['id', 'name'] } }],
    filter: [
      { field: 'video', op: 'eq', value: 'fixture-video-id' },
      { field: 'is_deleted', op: 'eq', value: false },
    ],
    sort: [{ field: 'created_at', dir: 'asc' }, { field: 'id', dir: 'asc' }],
    limit: 100,
  });
  assert.equal('traverse' in query, false);
});

test('watch detail equals the original answer of the golden case, with the timeline parsed, and then counts the view once', async () => {
  const golden = JSON.parse(readFileSync(new URL('../../../../product/tests/fixtures/facereview-mysql/slice13f-golden.json', import.meta.url), 'utf8'));
  const first = golden.cases[0];
  const timeline = { happy: [{ x: 1, y: 2.5 }], neutral: [{ x: 1, y: 0 }], surprise: [], sad: [], angry: [] };
  const output = { ...first.expected, timeline_data: JSON.stringify(timeline) };
  const { client, calls } = fakeClient({ 'Video.watchDetail': output });
  const detail = await createReadApi(client, { newKey: keys() }).getVideoDetail({ video_id: first.video });
  assert.deepEqual(detail, { ...first.expected, timeline_data: timeline });
  assert.deepEqual(calls.map((call) => call.kind), ['extension', 'apply'], 'the view is counted after the read, not before');
  assert.deepEqual(calls[0].input, { video_id: first.video });
  assert.deepEqual(calls[1], { kind: 'apply', input: { apply: 'Video.view', target: { ids: [first.video] } }, key: 'key-1' });
});

test('a view that cannot be counted is reported and the video is still returned', async () => {
  for (const failure of [{ ok: false, code: 'WIRE.DB_UNAVAILABLE' }, new Error('network down')]) {
    const warnings: string[] = [];
    const { client } = fakeClient({ 'Video.watchDetail': { video_id: 'v', timeline_data: '{}' } }, failure);
    const detail = await createReadApi(client, { warn: (message) => warnings.push(message) }).getVideoDetail({ video_id: 'v' });
    assert.equal(detail.video_id, 'v');
    assert.equal(warnings.length, 1);
  }
});

test('recent videos use the original defaults and unwrap the body', async () => {
  const body = { videos: [{ ...video('r'), timeline_data: undefined }] };
  const { client, calls } = fakeClient({ 'User.myRecentVideos': { body: JSON.stringify(body) } });
  const api = createReadApi(client);
  assert.deepEqual(await api.getRecentVideo(), JSON.parse(JSON.stringify(body.videos)));
  assert.deepEqual(calls[0].input, { emotion: 'all', page: 1, size: 10 });
  await api.getRecentVideo({ page: 4, size: 3, emotion: 'sad' });
  assert.deepEqual(calls[1].input, { emotion: 'sad', page: 4, size: 3 });
});

test('a body without videos is an empty list (the original answer `data.videos ?? []`)', async () => {
  const { client } = fakeClient({ 'User.myRecentVideos': { body: '{}' } });
  assert.deepEqual(await createReadApi(client).getRecentVideo(), []);
});

test('the emotion summary is a write: each call has its own key, and a refusal is an error with the wire code', async () => {
  const summary = { emotion_percentages: { happy: 50 }, emotion_seconds: { happy: 10 } };
  const { client, calls } = fakeClient({ 'User.myEmotionSummary': { ok: true, output: { body: JSON.stringify(summary) } } });
  const api = createReadApi(client, { newKey: keys() });
  assert.deepEqual(await api.getEmotionSummary(), summary);
  await api.getEmotionSummary();
  assert.deepEqual(calls.map((call) => call.key), ['key-1', 'key-2'], 'a repeated key would replay the first answer');
  const refused = fakeClient({ 'User.myEmotionSummary': { ok: false, code: 'WIRE.CONFLICT', retry: 'later' } });
  await assert.rejects(createReadApi(refused.client).getEmotionSummary(), (error: { code?: string }) => error.code === 'WIRE.CONFLICT');
});

test('related videos send page 1 and size 10 (the original defaults) and return the videos', async () => {
  const { client, calls } = fakeClient({ 'Video.watchRecommended': { videos: JSON.stringify([video('x')]), total: 1, page: 1, size: 10, has_next: false } });
  assert.deepEqual(await createReadApi(client).getRelatedVideo({ video_id: 'v' }), [video('x')]);
  assert.deepEqual(calls[0].input, { video_id: 'v', page: 1, size: 10 });
});

test('a text field that is missing or not JSON is a protocol error, never an empty answer', async () => {
  for (const output of [{}, { videos: 'not json' }, { videos: 5 }]) {
    const { client } = fakeClient({ 'Video.homePersonalized': output });
    await assert.rejects(createReadApi(client).getPersonalRecommendedVideo(), (error: { code?: string }) => error.code === 'CLIENT.PROTOCOL_ERROR');
  }
  assert.throws(() => parseText(undefined, 'x'), (error: { code?: string }) => error.code === 'CLIENT.PROTOCOL_ERROR');
});

test('errors of the client reach the screen unchanged', async () => {
  const client: AipClient = {
    async read() { throw new Error('unused'); },
    async extension() {
      throw Object.assign(new Error('expired'), { code: 'WIRE.TOKEN_EXPIRED' });
    },
    async writeExtension() {
      throw new Error('unused');
    },
    async apply() {
      throw new Error('unused');
    },
  };
  await assert.rejects(createReadApi(client).getAllVideo({ page: 1, size: 1, emotion: 'all' }), (error: { code?: string }) => error.code === 'WIRE.TOKEN_EXPIRED');
});
