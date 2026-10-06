// 확장 구현 예시(C B.7)와 시험용 변형. 서버 계약의 implementation 이름이 `recruitment.<함수>`를 가리킨다.
import net from "node:net";

let lastCtx = null;

export async function stats(input, ctx) {
  const r = await ctx.data.aggregate("Apply", "approvedCount", { clubId: input.clubId });
  return { approvedApplicants: r.value };
}
export async function statsOtherClub(input, ctx) {
  const r = await ctx.data.aggregate("Apply", "approvedCount", { clubId: "11" });
  return { approvedApplicants: r.value };
}
export async function statsUndeclared(input, ctx) {
  const r = await ctx.data.aggregate("Recruitment", "bookmarkCount", {});
  return { approvedApplicants: r.value };
}
export async function statsImpersonate(input, ctx) {
  const r = await ctx._raw({ op: "aggregate", name: "Apply.approvedCount", input: { clubId: input.clubId }, actor: 3 });
  return { approvedApplicants: r.value };
}
export async function statsBadOutput() {
  return { approvedApplicants: "many" };
}
export async function statsExtraOutput() {
  return { approvedApplicants: 1, internalNote: "x" };
}
export async function statsSlow(input, ctx) {
  lastCtx = ctx;
  await new Promise((r) => setTimeout(r, 3000));
  return { approvedApplicants: 0 };
}
export async function statsReuse(input, ctx) {
  // 이전 호출의 ctx를 붙잡아 다시 쓰려는 확장
  const r = await lastCtx.data.aggregate("Apply", "approvedCount", { clubId: input.clubId });
  return { approvedApplicants: r.value };
}
export async function statsCrash() {
  process.exit(1);
}
export async function probeEnv() {
  const n = Object.keys(process.env).filter((k) => k.startsWith("PG") || k.includes("DATABASE")).length;
  return { approvedApplicants: n };
}
export async function probeDb() {
  // 프로세스 분리만으로 네트워크 접근이 막히는지 확인한다.
  const ok = await new Promise((resolve) => {
    const s = net.connect(5432, "127.0.0.1", () => { s.destroy(); resolve(1); });
    s.on("error", () => resolve(0));
  });
  return { approvedApplicants: ok };
}
export async function roleOut() {
  return { approvedApplicants: "ADMIN" };
}
