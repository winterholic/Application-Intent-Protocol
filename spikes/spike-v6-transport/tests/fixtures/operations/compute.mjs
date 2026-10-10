export function length(input) { return {length: [...input.text].length}; }
export function invalid() { return {length: "invalid"}; }
export async function ctxProbe(_input, ctx) {
  try { await ctx.data.aggregate("Hidden", "total", {}); } catch {}
  return {length: 1};
}
export async function slow() { await new Promise(resolve => setTimeout(resolve, 500)); return {length: 1}; }

export function ranged(input) { return {length: input.count + 1}; }

export function unsafeInteger() { return {length: 9007199254740992}; }
