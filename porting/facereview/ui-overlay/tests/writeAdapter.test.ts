import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createWriteApi, type WriteClient } from '../src/api/writeAdapter.ts';

function fakeClient(initialRows: Record<string, unknown>[] = []) {
  let rows = initialRows;
  const calls: Array<{ kind: string; value: unknown; key?: string }> = [];
  const client: WriteClient = {
    async read(query) {
      calls.push({ kind: 'read', value: query });
      return { ok: true, rows };
    },
    async apply(request, options) {
      calls.push({ kind: 'apply', value: request, key: options?.key });
      if ('apply' in request && request.apply === 'VideoBookmark.remove') rows = [];
      return { ok: true, created: [] };
    },
    async bundle(request, options) {
      calls.push({ kind: 'bundle', value: request, key: options?.key });
      const step = request.atomic?.[0];
      if (step && 'create' in step) rows = [{ id: 'new-id', video: 'v1' }];
      return { ok: true, created: step && 'create' in step && step.create === 'Comment' ? [{ resource: 'Comment', id: 'new-id' }] : [] };
    },
    async writeExtension(name, input, options) {
      calls.push({ kind: 'writeExtension', value: { name, input }, key: options?.key });
      return { ok: true, output: { requested: 2 } };
    },
  };
  return { client, calls };
}

const keyFactory = () => { let n = 0; return () => `write-${++n}`; };

test('bookmark toggle translates current state into explicit create then remove', async () => {
  const { client, calls } = fakeClient();
  const api = createWriteApi(client, { userId: () => 'member-1', newKey: keyFactory() });
  assert.deepEqual(await api.toggleBookmark({ video_id: 'v1' }), { is_bookmarked: true, message: '북마크에 추가했어요.' });
  assert.deepEqual(calls[0], {
    kind: 'read', value: { read: 'VideoBookmark', select: ['id'], filter: [{ field: 'video', op: 'eq', value: 'v1' }], limit: 1 },
  });
  assert.deepEqual(calls[1], {
    kind: 'bundle', key: 'write-1', value: { atomic: [{ create: 'VideoBookmark', values: { video: 'v1', user: 'member-1' } }] },
  });

  calls.length = 0;
  const existing = fakeClient([{ id: 'bookmark-1' }]);
  assert.deepEqual(await createWriteApi(existing.client, { userId: () => 'member-1', newKey: keyFactory() }).toggleBookmark({ video_id: 'v1' }), {
    is_bookmarked: false, message: '북마크를 해제했어요.',
  });
  assert.deepEqual(existing.calls[1], {
    kind: 'apply', key: 'write-1', value: { apply: 'VideoBookmark.remove', target: { ids: ['bookmark-1'] } },
  });
});

test('like add/remove use separate create and remove operations', async () => {
  const add = fakeClient();
  await createWriteApi(add.client, { userId: () => 'member-1', newKey: keyFactory() }).addLike({ video_id: 'v1' });
  assert.deepEqual(add.calls[0], { kind: 'bundle', key: 'write-1', value: { atomic: [{ create: 'VideoLike', values: { video: 'v1', user: 'member-1' } }] } });

  const remove = fakeClient([{ id: 'like-1' }]);
  await createWriteApi(remove.client, { userId: () => 'member-1', newKey: keyFactory() }).cancelLike({ video_id: 'v1' });
  assert.deepEqual(remove.calls[1], { kind: 'apply', key: 'write-1', value: { apply: 'VideoLike.remove', target: { ids: ['like-1'] } } });
});

test('request form sends the existing write extension and never falls back to REST', async () => {
  const { client, calls } = fakeClient();
  const result = await createWriteApi(client, { userId: () => 'member-1', newKey: keyFactory() }).updateRequestVideoList({ youtube_url_list: ['a', 'b'] });
  assert.deepEqual(result, { result: 'requested', requested: 2, message: '영상 등록 요청을 접수했습니다.' });
  assert.deepEqual(calls, [{
    kind: 'writeExtension', key: 'write-1', value: { name: 'VideoRequest.recommend', input: { user: 'member-1', youtube_url_list: '["a","b"]' } },
  }]);
});

test('comment creation, edit and delete keep owner and payload decisions in the AIP contract', async () => {
  const { client, calls } = fakeClient();
  const api = createWriteApi(client, { userId: () => 'member-1', newKey: keyFactory() });
  assert.deepEqual(await api.sendNewComment({ content: 'hello', video_id: 'v1' }), { comment_id: 'new-id', message: '댓글이 등록되었습니다.' });
  await api.modifyComment({ comment_id: 'comment-1', content: 'edited' });
  await api.deleteComment({ comment_id: 'comment-1' });
  assert.deepEqual(calls.map((call) => call.value), [
    { atomic: [{ create: 'Comment', values: { video: 'v1', user: 'member-1', content: 'hello' } }] },
    { apply: 'Comment.edit', target: { ids: ['comment-1'] }, values: { content: 'edited' } },
    { apply: 'Comment.delete', target: { ids: ['comment-1'] } },
  ]);
});

test('profile update bundles the member row patch and favorite genre replacement atomically', async () => {
  const { client, calls } = fakeClient([
    { id: 'genre-1', genre: 'drama' },
    { id: 'genre-2', genre: 'eating' },
  ]);
  const api = createWriteApi(client, { userId: () => 'member-1', newKey: keyFactory() });

  assert.equal(typeof api.updateProfile, 'function');
  await api.updateProfile({ name: '새 이름', profile_image_id: 12, favorite_genres: ['eating', 'horror'] });

  assert.deepEqual(calls, [
    { kind: 'read', value: { read: 'UserFavoriteGenre', select: ['id', 'genre'], limit: 50 } },
    {
      kind: 'bundle', key: 'write-1', value: { atomic: [
        { apply: 'User.updateProfile', target: { ids: ['member-1'] }, values: { name: '새 이름', profile_image_id: 12 } },
        { apply: 'UserFavoriteGenre.remove', target: { ids: ['genre-1'] } },
        { create: 'UserFavoriteGenre', values: { user: 'member-1', genre: 'horror' } },
      ] },
    },
  ]);
});
