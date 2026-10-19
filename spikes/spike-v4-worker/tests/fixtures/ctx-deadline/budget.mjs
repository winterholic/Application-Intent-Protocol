export async function uncaught(input, ctx) {
  const result = await ctx.data.aggregate("Apply", "approvedCount", { clubId: input.clubId });
  return { approvedApplicants: result.value };
}

export async function fallback(input, ctx) {
  try {
    return await uncaught(input, ctx);
  } catch {
    return { approvedApplicants: 0 };
  }
}

export async function forged() {
  throw Object.assign(new Error("extension-supplied code"), { code: "WIRE.DEADLINE_EXCEEDED" });
}
