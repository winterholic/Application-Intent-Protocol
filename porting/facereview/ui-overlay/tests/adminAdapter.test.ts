import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createAdminApprovalApi, createAdminWriteApi } from '../src/api/adminAdapter.ts';
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

type Receipt = { id: string; status: string; error?: string };

function fakeJobs(statuses: string[], options: { enqueueError?: unknown; output?: Record<string, unknown> } = {}) {
  const calls: Array<{ name: string; args: unknown[] }> = [];
  const waits: number[] = [];
  let polled = 0;
  const jobs = {
    async enqueue(operation: string, input: Record<string, unknown>, enqueueOptions: { key: string }): Promise<Receipt> {
      calls.push({ name: 'enqueue', args: [operation, input, enqueueOptions] });
      if (options.enqueueError) throw options.enqueueError;
      return { id: 'job-1', status: 'QUEUED' };
    },
    async status(id: string): Promise<Receipt> {
      calls.push({ name: 'status', args: [id] });
      const status = statuses[Math.min(polled++, statuses.length - 1)];
      return status === 'FAILED' ? { id, status, error: 'WIRE.ALREADY_EXISTS' } : { id, status };
    },
    async result(id: string, operation: string) {
      calls.push({ name: 'result', args: [id, operation] });
      return options.output ?? { video_id: 'video-77' };
    },
  };
  return { jobs, calls, waits, sleep: async (ms: number) => { waits.push(ms); } };
}

const approval = { youtube_title: '승인 영상', channel_name: '채널', duration: 321, category: 'vlog' };

test('approval enqueues the declared durable job with the administrator choices, waits for SUCCEEDED and answers with the new video id', async () => {
  const { jobs, calls, waits, sleep } = fakeJobs(['RUNNING', 'SUCCEEDED']);
  const api = createAdminApprovalApi(jobs, { newKey: keyFactory(), sleep, intervalMs: 250 });
  const reply = await api.approveVideoRequest('request-1', approval);

  assert.deepEqual(calls[0], {
    name: 'enqueue',
    args: ['approveVideoRequest', { request: 'request-1', youtube_title: '승인 영상', channel_name: '채널', duration: 321, category: 'vlog' }, { key: 'admin-1' }],
  });
  assert.deepEqual(calls.map((call) => call.name), ['enqueue', 'status', 'status', 'result']);
  assert.deepEqual(calls.at(-1)?.args, ['job-1', 'approveVideoRequest']);
  assert.deepEqual(waits, [250, 250]);
  assert.equal(reply.status, 200);
  assert.equal(reply.data.video_id, 'video-77');
  assert.equal(typeof reply.data.message, 'string');
});

test('a FAILED run is an error with the server code and never reads a result or claims approval', async () => {
  const { jobs, calls, sleep } = fakeJobs(['RUNNING', 'FAILED']);
  const api = createAdminApprovalApi(jobs, { newKey: keyFactory(), sleep });
  await assert.rejects(api.approveVideoRequest('request-1', approval), (error: Error & { code?: string }) => error.code === 'WIRE.ALREADY_EXISTS');
  assert.equal(calls.some((call) => call.name === 'result'), false);
});

test('a CANCELLED run and a refused enqueue are errors too, the refusal keeping its code', async () => {
  const cancelled = fakeJobs(['CANCELLED']);
  await assert.rejects(createAdminApprovalApi(cancelled.jobs, { sleep: cancelled.sleep }).approveVideoRequest('request-1', approval), (error: Error & { code?: string }) => error.code === 'JOB.CANCELLED');
  const refused = fakeJobs(['SUCCEEDED'], { enqueueError: Object.assign(new Error('denied'), { code: 'WIRE.ACCESS_DENIED' }) });
  await assert.rejects(createAdminApprovalApi(refused.jobs, { sleep: refused.sleep }).approveVideoRequest('request-1', approval), (error: Error & { code?: string }) => error.code === 'WIRE.ACCESS_DENIED');
  assert.equal(refused.calls.length, 1);
});

test('a job that never finishes stops waiting with a timeout code instead of reporting success', async () => {
  let clock = 0;
  const { jobs, sleep } = fakeJobs(['RUNNING']);
  const api = createAdminApprovalApi(jobs, { newKey: keyFactory(), sleep: async (ms) => { clock += ms; await sleep(ms); }, now: () => clock, intervalMs: 1000, timeoutMs: 3500 });
  await assert.rejects(api.approveVideoRequest('request-1', approval), (error: Error & { code?: string }) => error.code === 'CLIENT.JOB_TIMEOUT');
});

test('an empty request id is refused before anything is enqueued', async () => {
  const { jobs, calls, sleep } = fakeJobs(['SUCCEEDED']);
  await assert.rejects(createAdminApprovalApi(jobs, { sleep }).approveVideoRequest('', approval), (error: Error & { code?: string }) => error.code === 'CLIENT.BAD_ARGUMENT');
  assert.equal(calls.length, 0);
});
