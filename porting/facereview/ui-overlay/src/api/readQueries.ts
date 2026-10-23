export const commentReadQuery = (videoId: string) => ({
  read: 'Comment',
  select: ['id', 'content', 'is_modified', 'created_at', { user: { select: ['id', 'name'] } }],
  filter: [
    { field: 'video', op: 'eq', value: videoId },
    { field: 'is_deleted', op: 'eq', value: false },
  ],
  sort: [{ field: 'created_at', dir: 'asc' }, { field: 'id', dir: 'asc' }],
  limit: 100,
});
