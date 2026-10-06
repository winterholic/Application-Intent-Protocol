// 쓰기 확장 예시: 관리자 위임. ctx 쓰기는 서버 트랜잭션 안에서 실행되고 커밋은 서버가 한다.
let held = null;
export async function delegate(input, ctx) {
  await ctx.data.apply("ClubMember", "makeAdmin", [input.target]);
  await ctx.data.apply("ClubMember", "resignAdmin", [input.self]);
  return { done: true };
}
export async function delegateThrow(input, ctx) {
  await ctx.data.apply("ClubMember", "makeAdmin", [input.target]);
  throw new Error("중간 실패");
}
export async function delegateSwallow(input, ctx) {
  await ctx.data.apply("ClubMember", "makeAdmin", [input.target]);
  try {
    await ctx.data.apply("ClubMember", "resignAdmin", [input.target]);
  } catch {}
  return { done: true };
}
export async function delegateSlow(input, ctx) {
  held = ctx;
  await ctx.data.apply("ClubMember", "makeAdmin", [input.target]);
  await new Promise((r) => setTimeout(r, 1500));
  return { done: true };
}
export async function lateWrite(input) {
  await held.data.apply("ClubMember", "resignAdmin", [input.self]);
  return { done: true };
}
export async function onlyPromote(input, ctx) {
  await ctx.data.apply("ClubMember", "makeAdmin", [input.target]);
  return { done: true };
}
export async function undeclared(input, ctx) {
  await ctx.data.apply("Recruitment", "close", ["100"]);
  return { done: true };
}
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
export async function delegateAfter500(input, ctx) {
  await wait(500);
  return delegate(input, ctx);
}
export async function staleWriter(input, ctx) {
  await wait(200);
  await ctx.data.apply("ClubMember", "resignAdmin", [input.self]);
  return { done: true };
}
export async function delegateAfter350(input, ctx) {
  await wait(350);
  return delegate(input, ctx);
}
