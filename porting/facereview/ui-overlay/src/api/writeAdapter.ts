export interface WriteClient {
  read(query: Record<string, unknown>): Promise<{ ok: true; rows: Record<string, unknown>[] }>;
  apply(request: Record<string, unknown>, options?: { key?: string }): Promise<{
    ok: boolean; code?: string; created?: Array<{ resource: string; id: string | number | null }>;
  }>;
  bundle(request: Record<string, unknown>, options?: { key?: string }): Promise<{
    ok: boolean; code?: string; created?: Array<{ resource: string; id: string | number | null }>;
  }>;
  writeExtension(name: string, input: Record<string, unknown>, options?: { key?: string }): Promise<{
    ok: boolean; code?: string; output?: Record<string, unknown>;
  }>;
}

export interface WriteOptions {
  userId: () => string;
  newKey?: () => string;
}

const failure = (message: string, code: string): never => {
  throw Object.assign(new Error(message), { code });
};

export function createWriteApi(client: WriteClient, options: WriteOptions) {
  const newKey = options.newKey ?? (() => globalThis.crypto.randomUUID());
  const member = () => {
    const id = options.userId();
    if (!id) return failure('a signed-in member is required', 'WIRE.UNAUTHENTICATED');
    return id;
  };
  const apply = async (request: Record<string, unknown>) => {
    const result = await client.apply(request, { key: newKey() });
    if (!result.ok) return failure(`AIP write failed: ${result.code ?? 'WIRE.EXTENSION_ERROR'}`, result.code ?? 'WIRE.EXTENSION_ERROR');
    return result;
  };
  const create = async (resource: string, values: Record<string, unknown>) => {
    // The public SDK exposes creates through bundle()'s atomic envelope; apply() accepts transitions only.
    const result = await client.bundle({ atomic: [{ create: resource, values }] }, { key: newKey() });
    if (!result.ok) return failure(`AIP write failed: ${result.code ?? 'WIRE.EXTENSION_ERROR'}`, result.code ?? 'WIRE.EXTENSION_ERROR');
    return result;
  };
  const rows = async (resource: 'VideoLike' | 'VideoBookmark', videoId: string) => {
    const result = await client.read({
      read: resource,
      select: ['id'],
      filter: [{ field: 'video', op: 'eq', value: videoId }],
      limit: 1,
    });
    return result.rows;
  };

  return {
    /** Keep the profile row patch and favorite-genre replacement in one atomic SDK bundle. */
    async updateProfile(props: { name?: string; profile_image_id?: number; favorite_genres?: string[] }) {
      const userId = member();
      const changes: Record<string, unknown> = {};
      if (props.name !== undefined) changes.name = props.name;
      if (props.profile_image_id !== undefined) changes.profile_image_id = props.profile_image_id;
      const operations: Record<string, unknown>[] = [];
      if (Object.keys(changes).length > 0) {
        operations.push({ apply: 'User.updateProfile', target: { ids: [userId] }, values: changes });
      }
      if (props.favorite_genres !== undefined) {
        const current = await client.read({ read: 'UserFavoriteGenre', select: ['id', 'genre'], limit: 50 });
        const desired = new Set(props.favorite_genres);
        for (const row of current.rows) {
          if (typeof row.genre === 'string' && !desired.has(row.genre)) {
            operations.push({ apply: 'UserFavoriteGenre.remove', target: { ids: [row.id] } });
          }
        }
        const existing = new Set(current.rows.map((row) => row.genre));
        for (const genre of desired) {
          if (!existing.has(genre)) operations.push({ create: 'UserFavoriteGenre', values: { user: userId, genre } });
        }
      }
      if (operations.length === 0) return { ok: true };
      const result = await client.bundle({ atomic: operations }, { key: newKey() });
      if (!result.ok) return failure(`profile update failed: ${result.code ?? 'WIRE.EXTENSION_ERROR'}`, result.code ?? 'WIRE.EXTENSION_ERROR');
      return result;
    },
    /** Translate the original toggle into the AIP decision already declared by the current row state. */
    async toggleBookmark(props: { video_id: string }) {
      const current = await rows('VideoBookmark', props.video_id);
      if (current.length > 0) {
        const id = current[0]?.id;
        if (typeof id !== 'string' || !id) return failure('bookmark row has no id', 'CLIENT.PROTOCOL_ERROR');
        await apply({ apply: 'VideoBookmark.remove', target: { ids: [id] } });
        return { is_bookmarked: false, message: '북마크를 해제했어요.' };
      }
      await create('VideoBookmark', { video: props.video_id, user: member() });
      return { is_bookmarked: true, message: '북마크에 추가했어요.' };
    },
    async addLike(props: { video_id: string }) {
      await create('VideoLike', { video: props.video_id, user: member() });
      return { ok: true };
    },
    async cancelLike(props: { video_id: string }) {
      const current = await rows('VideoLike', props.video_id);
      if (current.length === 0) return { ok: true };
      const id = current[0]?.id;
      if (typeof id !== 'string' || !id) return failure('like row has no id', 'CLIENT.PROTOCOL_ERROR');
      await apply({ apply: 'VideoLike.remove', target: { ids: [id] } });
      return { ok: true };
    },
    async updateRequestVideoList(props: { youtube_url_list: string[] }) {
      const result = await client.writeExtension('VideoRequest.recommend', {
        user: member(),
        youtube_url_list: JSON.stringify(props.youtube_url_list),
      }, { key: newKey() });
      if (!result.ok) return failure(`video request failed: ${result.code ?? 'WIRE.EXTENSION_ERROR'}`, result.code ?? 'WIRE.EXTENSION_ERROR');
      return { result: 'requested', requested: result.output?.requested, message: '영상 등록 요청을 접수했습니다.' };
    },
    async sendNewComment(props: { content: string; video_id: string }) {
      const result = await create('Comment', { video: props.video_id, user: member(), content: props.content });
      const created = result.created?.find((row) => row.resource === 'Comment');
      if (!created || created.id === null) return failure('comment create response has no id', 'CLIENT.PROTOCOL_ERROR');
      return { comment_id: String(created.id), message: '댓글이 등록되었습니다.' };
    },
    async modifyComment(props: { comment_id: string; content: string }) {
      await apply({ apply: 'Comment.edit', target: { ids: [props.comment_id] }, values: { content: props.content } });
      return { ok: true };
    },
    async deleteComment(props: { comment_id: string }) {
      await apply({ apply: 'Comment.delete', target: { ids: [props.comment_id] } });
      return { ok: true };
    },
  };
}
