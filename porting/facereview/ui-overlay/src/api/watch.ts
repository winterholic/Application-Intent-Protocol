import type { CommentType } from 'types';
import { aipClient, aipWrites } from './aipClient';

export const getVideoComments = async (props: { video_id: string }): Promise<CommentType[]> => {
  const userId = useCurrentUserId();
  const result = await aipClient.read({
    read: 'Comment',
    select: ['id', 'content', 'is_modified', 'created_at', 'user'],
    filter: [
      { field: 'video', op: 'eq', value: props.video_id },
      { field: 'is_deleted', op: 'eq', value: false },
    ],
    sort: [{ field: 'created_at', direction: 'asc' }, { field: 'id', direction: 'asc' }],
    limit: 100,
    traverse: { user: { select: ['id', 'name', 'profile_image_id'] } },
  });
  return result.rows.map((row) => {
    const user = row.user && typeof row.user === 'object' ? row.user as Record<string, unknown> : {};
    return {
      comment_id: String(row.id),
      user_id: String(user.id ?? ''),
      user_name: String(user.name ?? ''),
      user_profile_image_id: typeof user.profile_image_id === 'number' ? user.profile_image_id : 0,
      content: String(row.content ?? ''),
      is_modified: row.is_modified === true,
      created_at: String(row.created_at ?? ''),
      is_mine: userId !== '' && user.id === userId,
    };
  });
};

function useCurrentUserId(): string {
  const token = useAuthStorage.getState().access_token;
  const encoded = token.split('.')[1];
  if (!encoded) return '';
  try {
    const payload = JSON.parse(atob(encoded.replace(/-/g, '+').replace(/_/g, '/'))) as { sub?: unknown };
    return typeof payload.sub === 'string' ? payload.sub : '';
  } catch {
    return '';
  }
}

import { useAuthStorage } from 'store/authStore';

export const sendNewComment = aipWrites.sendNewComment;
export const addLike = aipWrites.addLike;
export const cancelLike = aipWrites.cancelLike;
export const modifyComment = aipWrites.modifyComment;
export const deleteComment = aipWrites.deleteComment;
