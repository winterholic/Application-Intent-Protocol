// Reads the resource it is told to through ctx.data.read and reports what the server answered.
export async function run(input, ctx) {
  try {
    const result = await ctx.data.read({ read: input.name, select: ["id", "title"] });
    return { status: "ok", rows: result.rows.length };
  } catch (error) {
    return { status: String(error.code ?? error.message), rows: 0 };
  }
}
