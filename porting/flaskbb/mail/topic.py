"""Atomic Topic.move write extension for the FlaskBB port."""


async def moveToForum(input, ctx):
    topics = await ctx.data.read({
        "read": "Topic",
        "select": ["id", "forum"],
        "filter": [{"field": "id", "op": "eq", "value": input["topic"]}],
        "limit": 1,
    })
    rows = topics.get("rows", []) if isinstance(topics, dict) else topics["rows"]
    topic = rows[0] if rows else None
    if topic is None:
        raise RuntimeError("topic not found")

    source = str(topic["forum"])
    destination = str(input["destination"])

    # FlaskBB's route authorizes both forums before Topic.move checks for a no-op.
    await ctx.data.apply("Forum", "authorizeTopicMove", [source])
    if destination != source:
        await ctx.data.apply("Forum", "authorizeTopicMove", [destination])
    if destination == source:
        return {"moved": False}

    # The internal transition moves Topic/Post.forum and clears only this
    # topic's read rows, all inside the owner's one PostgreSQL transaction.
    await ctx.data.apply(
        "Topic", "relocate", [input["topic"]], values={"forum": destination}
    )
    await ctx.data.apply("Forum", "refreshTopicMoveProjection", [source])
    await ctx.data.apply("Forum", "refreshTopicMoveProjection", [destination])
    return {"moved": True}
