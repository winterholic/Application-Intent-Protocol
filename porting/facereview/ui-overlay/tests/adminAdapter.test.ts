import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createAdminWriteApi } from '../src/api/adminAdapter.ts';
import type { WriteClient } from '../src/api/writeAdapter.ts';

function fakeClient(result: { ok: boolean; code?: string } = { ok: true }) {
  const calls: Array<{ request: Record<string, unknown>; key?: string }> = [];
  const client: Pick<WriteClient, 'apply'> = {
    async apply(request, options) {
      calls.push({ request, key: options?.key });
      return result;
    },
  };
  return { client, calls };
}

const keyFactory = () => { let n = 0; return () => `admin-${++n}`; };

test('each admin action maps to the declared transition and sends only the target id and the declared patch', async () => {
  const { client, calls } = fakeClient();
  const api = createAdminWriteApi(client, { newKey: keyFactory() });

  const deactivated = await api.deactivateUser('user-1');
  await api.changeUserRole('user-2', 'ADMIN');
  await api.deleteVideo('video-1');
  await api.deleteComment('comment-1');
  await api.rejectVideoRequest('request-1', '중복 영상입니다');

  assert.deepEqual(calls, [
    { key: 'admin-1', request: { apply: 'User.deactivate', target: { ids: ['user-1'] } } },
    { key: 'admin-2', request: { apply: 'User.changeRole', target: { ids: ['user-2'] }, values: { role: 'ADMIN' } } },
    { key: 'admin-3', request: { apply: 'Video.remove', target: { ids: ['video-1'] } } },
    { key: 'admin-4', request: { apply: 'Comment.remove', target: { ids: ['comment-1'] } } },
    { key: 'admin-5', request: { apply: 'VideoRequest.reject', target: { ids: ['request-1'] }, values: { admin_comment: '중복 영상입니다' } } },
  ]);
  // The original admin API answers an axios response whose data is a MessageResponse.
  assert.equal(deactivated.status, 200);
  assert.equal(typeof deactivated.data.message, 'string');
});

test('a server refusal surfaces its code so the UI can show it, and no success message is produced', async () => {
  const { client } = fakeClient({ ok: false, code: 'WIRE.ACCESS_DENIED' });
  const api = createAdminWriteApi(client, { newKey: keyFactory() });
  await assert.rejects(api.changeUserRole('user-1', 'SUPER_ADMIN'), (error: Error & { code?: string }) => {
    assert.equal(error.code, 'WIRE.ACCESS_DENIED');
    return true;
  });
  await assert.rejects(api.deleteVideo('video-1'), (error: Error & { code?: string }) => error.code === 'WIRE.ACCESS_DENIED');
});

test('an unknown role is rejected before any request, so the browser cannot invent a role value', async () => {
  const { client, calls } = fakeClient();
  const api = createAdminWriteApi(client, { newKey: keyFactory() });
  await assert.rejects(api.changeUserRole('user-1', 'ROOT' as never), (error: Error & { code?: string }) => error.code === 'CLIENT.BAD_ARGUMENT');
  assert.equal(calls.length, 0);
});

test('a missing target id or an empty reject comment never reaches the server', async () => {
  const { client, calls } = fakeClient();
  const api = createAdminWriteApi(client, { newKey: keyFactory() });
  await assert.rejects(api.deactivateUser(''), (error: Error & { code?: string }) => error.code === 'CLIENT.BAD_ARGUMENT');
  await assert.rejects(api.rejectVideoRequest('request-1', ''), (error: Error & { code?: string }) => error.code === 'CLIENT.BAD_ARGUMENT');
  assert.equal(calls.length, 0);
});
