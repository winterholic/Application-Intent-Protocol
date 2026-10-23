import type { WriteClient } from './writeAdapter.ts';

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
 * shapes requests and surfaces the server's code. Approval is not here: it is a durable job, not a transition.
 */
export function createAdminWriteApi(client: Pick<WriteClient, 'apply'>, options: AdminWriteOptions = {}) {
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
