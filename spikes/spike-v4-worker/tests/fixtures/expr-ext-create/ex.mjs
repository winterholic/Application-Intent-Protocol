// Extensions of tests/expr_ext_create.rs: each one calls `ctx.data.create` the way a PostgreSQL write extension author would.
const project = (input, name = input.name) => ({ owner: input.owner, name });

export async function createProject(input, ctx) {
  return { id: await ctx.data.create('Project', project(input), 'project-1') };
}

// A parent and then a child that points at the id the first create returned.
export async function family(input, ctx) {
  const parent = await ctx.data.create('Project', project(input));
  const child = await ctx.data.create('Item', { project: parent, owner: input.owner, label: input.name });
  return { project: parent, item: child };
}

// Both rows exist inside the transaction when the extension fails afterwards.
export async function familyThenThrow(input, ctx) {
  await family(input, ctx);
  throw new Error('boom');
}

// A refused create the extension catches: it still must not commit, and the rows created before it go too.
export async function swallow(input, ctx) {
  const { project: parent } = await family(input, ctx);
  try {
    await ctx.data.create('Item', { project: parent, owner: input.owner, label: 'x', secret: 'not in the create contract' });
  } catch (error) {}
  return { project: parent, item: parent };
}

export async function noGrant(input, ctx) {
  await ctx.data.create('Project', project(input));
  await ctx.data.create('Item', { project: '1', owner: input.owner, label: 'no create.Item in access' });
  return { ok: true };
}

export async function badField(input, ctx) {
  const parent = await ctx.data.create('Project', project(input));
  await ctx.data.create('Item', { project: parent, owner: input.owner, label: 'x', secret: 'not in the create contract' });
  return { ok: true };
}

export async function hiddenRef(input, ctx) {
  await ctx.data.create('Note', { task: '20', owner: input.owner, body: 'org 2 task' });
  return { ok: true };
}

export async function badKey(input, ctx) {
  await ctx.data.create('Project', project(input), 'k'.repeat(101));
  return { ok: true };
}

export async function slot(input, ctx) {
  return { id: await ctx.data.create('Slot', { room: '1', owner: input.owner, taken: false }) };
}

export async function tag(input, ctx) {
  return { id: await ctx.data.create('Tag', { owner: input.owner, name: input.name }) };
}
