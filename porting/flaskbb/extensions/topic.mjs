// Topic.move is one official write extension. Every ctx.data call is an
// internal apply grant inside the owner's single, fenced PostgreSQL transaction.
export async function moveToForum(input, ctx) {
  const topics = await ctx.data.read({
    read: "Topic",
    select: ["id", "forum"],
    filter: [{field: "id", op: "eq", value: input.topic}],
    limit: 1,
  });
  const topic = topics.rows[0];
  if (!topic) throw new Error("topic not found");

  const source = String(topic.forum);
  const destination = String(input.destination);

  // FlaskBB's route authorizes both forums before Topic.move checks for a no-op.
  await ctx.data.apply("Forum", "authorizeTopicMove", [source]);
  if (destination !== source) {
    await ctx.data.apply("Forum", "authorizeTopicMove", [destination]);
  }
  if (destination === source) return {moved: false};

  // The internal transition changes Topic.forum, mirrors that relation to every
  // AIP Post.forum, and removes only this topic's read rows in the same txn.
  await ctx.data.apply("Topic", "relocate", [input.topic], undefined, {forum: destination});

  // Each projection selects by date_created only. Equal-date winners remain
  // intentionally unspecified, as in FlaskBB's ORM query.
  await ctx.data.apply("Forum", "refreshTopicMoveProjection", [source]);
  await ctx.data.apply("Forum", "refreshTopicMoveProjection", [destination]);
  return {moved: true};
}
