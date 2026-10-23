/** What the admin write adapter needs of the SDK client (`apply()` of `@aip/sdk`). It stands alone: the admin UI has no member write adapter. */
export interface AdminApplyClient {
  apply(request: Record<string, unknown>, options?: { key?: string }): Promise<{ ok: boolean; code?: string }>;
}

export type AdminRole = 'GENERAL' | 'ADMIN' | 'SUPER_ADMIN';

export interface AdminWriteOptions {
  newKey?: () => string;
}

export interface MessageResult {
  data: { message: string };
  status: number;
}

const ROLES: readonly AdminRole[] = ['GENERAL', 'ADMIN', 'SUPER_ADMIN'];

const failure = (message: string, code: string): never => {
  throw Object.assign(new Error(message), { code });
};

/**
 * Admin console write actions of `new-facereview-admin-front` (pinned ab0386c) over the declared AIP transitions.
 * The server owns every authorization decision (`isAdmin(actor)`, SUPER_ADMIN-only role change), so this adapter only
 * shapes requests and surfaces the server's code. Approval is not a transition: it is the durable job in `createAdminApprovalApi`.
 */
export function createAdminWriteApi(client: AdminApplyClient, options: AdminWriteOptions = {}) {
  const newKey = options.newKey ?? (() => globalThis.crypto.randomUUID());

  const run = async (request: Record<string, unknown>, message: string): Promise<MessageResult> => {
    const result = await client.apply(request, { key: newKey() });
    if (!result.ok) return failure(`AIP admin write failed: ${result.code ?? 'WIRE.EXTENSION_ERROR'}`, result.code ?? 'WIRE.EXTENSION_ERROR');
    return { data: { message }, status: 200 };
  };
  const id = (value: string): string => {
    if (typeof value !== 'string' || value.length === 0) return failure('a target id is required', 'CLIENT.BAD_ARGUMENT');
    return value;
  };

  return {
    deactivateUser: async (userId: string) =>
      run({ apply: 'User.deactivate', target: { ids: [id(userId)] } }, '회원이 비활성화되었습니다.'),
    async changeUserRole(userId: string, role: AdminRole) {
      if (!ROLES.includes(role)) return failure('unknown role', 'CLIENT.BAD_ARGUMENT');
      return run({ apply: 'User.changeRole', target: { ids: [id(userId)] }, values: { role } }, '권한이 변경되었습니다.');
    },
    deleteVideo: async (videoId: string) =>
      run({ apply: 'Video.remove', target: { ids: [id(videoId)] } }, '영상이 삭제되었습니다.'),
    deleteComment: async (commentId: string) =>
      run({ apply: 'Comment.remove', target: { ids: [id(commentId)] } }, '댓글이 삭제되었습니다.'),
    async rejectVideoRequest(requestId: string, adminComment: string) {
      if (typeof adminComment !== 'string' || adminComment.length === 0) return failure('a reject comment is required', 'CLIENT.BAD_ARGUMENT');
      return run(
        { apply: 'VideoRequest.reject', target: { ids: [id(requestId)] }, values: { admin_comment: adminComment } },
        '요청이 거절되었습니다.',
      );
    },
  };
}

export type JobState = 'QUEUED' | 'RUNNING' | 'CANCEL_REQUESTED' | 'SUCCEEDED' | 'FAILED' | 'CANCELLED';

/** What the approval adapter needs of the SDK's `jobs` client (the typed durable-job client of `@aip/sdk`). */
export interface AdminJobClient {
  enqueue(operation: 'approveVideoRequest', input: {
    request: string; youtube_title: string; channel_name: string; duration: number; category: string;
  }, options: { key: string }): Promise<{ id: string; status: JobState; error?: string }>;
  status(id: string): Promise<{ id: string; status: JobState; error?: string }>;
  result(id: string, operation: 'approveVideoRequest'): Promise<{ video_id: string }>;
}

export interface AdminApprovalOptions {
  newKey?: () => string;
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
  /** Poll gap; the job's own deadline is 60 s, so the default wait ends a little after that. */
  intervalMs?: number;
  timeoutMs?: number;
}

/**
 * `POST /v2/admin/video-requests/{id}/approve` of the original answered once the video existed. The AIP approval is the managed
 * durable job `approveVideoRequest` (video row, the two initial MongoDB documents and the request's ACCEPTED state, converging on
 * retry), so this adapter enqueues it and waits for the run to finish before it resolves: the screen keeps its one-step flow, and a
 * failed or unfinished run is an error, never a success. No external YouTube call is part of this job.
 */
export function createAdminApprovalApi(jobs: AdminJobClient, options: AdminApprovalOptions = {}) {
  const newKey = options.newKey ?? (() => globalThis.crypto.randomUUID());
  const sleep = options.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
  const now = options.now ?? (() => Date.now());
  const intervalMs = options.intervalMs ?? 500;
  const timeoutMs = options.timeoutMs ?? 75_000;

  return {
    async approveVideoRequest(
      requestId: string,
      data: { youtube_title: string; channel_name: string; duration: number; category: string },
    ): Promise<{ data: { video_id: string; message: string }; status: number }> {
      if (typeof requestId !== 'string' || requestId.length === 0) return failure('a target id is required', 'CLIENT.BAD_ARGUMENT');
      const receipt = await jobs.enqueue('approveVideoRequest', { request: requestId, ...data }, { key: newKey() });
      const deadline = now() + timeoutMs;
      let state = receipt;
      while (state.status !== 'SUCCEEDED') {
        if (state.status === 'FAILED') return failure(`AIP approval failed: ${state.error ?? 'WIRE.EXTENSION_ERROR'}`, state.error ?? 'WIRE.EXTENSION_ERROR');
        if (state.status === 'CANCELLED') return failure('AIP approval was cancelled', 'JOB.CANCELLED');
        if (now() >= deadline) return failure('AIP approval did not finish in time; the request list shows its real state', 'CLIENT.JOB_TIMEOUT');
        await sleep(intervalMs);
        state = await jobs.status(receipt.id);
      }
      const output = await jobs.result(receipt.id, 'approveVideoRequest');
      return { data: { video_id: output.video_id, message: '영상 요청이 승인되었습니다.' }, status: 200 };
    },
  };
}
