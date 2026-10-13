import { test } from "node:test";
import assert from "node:assert/strict";
import { connect as rawConnect, WriteUnsettled } from "./transport.ts";

// 응답 본문 계약은 그대로 두고, 응답 헤더 x-request-id를 실패 객체의 requestId로 옮긴다.
const ID = "req_0123456789abcdef01234567";
const reply = (body: unknown, requestId: string | null = ID) =>
  new Response(JSON.stringify(body), { headers: requestId === null ? {} : { "x-request-id": requestId } });

function connect(impl: (path: string, body: any) => Response) {
  return rawConnect("http://spike.invalid", "session", async (url, options) => {
    const path = new URL(String(url)).pathname;
    if (path === "/session") return reply({ ok: true, principal: { actorId: "1" }, remainingMs: 60_000 });
    return impl(path, JSON.parse(String(options?.body)));
  }, { contractFingerprint: "a".repeat(64) });
}

test("거부된 읽기 오류가 요청 ID를 싣는다", async () => {
  const client = connect(() => reply({ ok: false, code: "FILTER_NOT_ALLOWED", msg: "x" }));
  await assert.rejects(client.read({ read: "A", select: ["id"] }), (e: any) => e.code === "FILTER_NOT_ALLOWED" && e.requestId === ID);
});

test("계약 위반 응답의 프로토콜 오류도 요청 ID를 싣는다", async () => {
  const client = connect(() => reply({ ok: true, contractFingerprint: "b".repeat(64), rows: [], deps: [] }, "proxy-7f3a9c.01_ab"));
  await assert.rejects(client.read({ read: "A", select: ["id"] }), (e: any) => e.code === "CONTRACT_MISMATCH" && e.requestId === "proxy-7f3a9c.01_ab");
});

test("확장·operation·관리 호출의 거부도 요청 ID를 싣는다", async () => {
  const client = connect(() => reply({ ok: false, code: "NOT_EXPOSED" }));
  await assert.rejects(client.callExtension("e", {}), (e: any) => e.code === "NOT_EXPOSED" && e.requestId === ID);
  await assert.rejects(client.callOperation("o", {}), (e: any) => e.code === "NOT_EXPOSED" && e.requestId === ID);
  await assert.rejects(client.callManaged("/jobs/status", {}), (e: any) => e.code === "NOT_EXPOSED" && e.requestId === ID);
});

test("쓰기 거부 결과와 미확정 오류가 마지막 응답의 요청 ID를 싣는다", async () => {
  const rejected = connect(() => reply({ ok: false, code: "BAD_VALUE" }));
  const outcome = await rejected.apply({ apply: "A.b" }, { key: "k1" });
  assert.deepEqual({ ok: outcome.ok, code: outcome.code, requestId: (outcome as any).requestId }, { ok: false, code: "BAD_VALUE", requestId: ID });

  const unsettled = connect(() => reply({ ok: false, code: "COMMIT_UNKNOWN" }, "req_aaaaaaaaaaaaaaaaaaaaaaaa"));
  await assert.rejects(unsettled.apply({ apply: "A.b" }, { key: "k2", retries: 0 }), (e: any) => e instanceof WriteUnsettled && e.requestId === "req_aaaaaaaaaaaaaaaaaaaaaaaa");
});

test("헤더가 없거나 형식이 틀리면 requestId를 만들지 않고 본문 계약은 그대로다", async () => {
  for (const header of [null, "short", "has space in it", "x".repeat(65), "semi;colon-1234"]) {
    const client = connect(() => reply({ ok: false, code: "NOT_FOUND" }, header));
    await assert.rejects(client.read({ read: "A", select: ["id"] }), (e: any) => e.code === "NOT_FOUND" && !Object.hasOwn(e, "requestId"), String(header));
  }
  const ok = connect(() => reply({ ok: true, tags: [], changed: [], unchanged: [] }));
  const result: any = await ok.apply({ apply: "A.b" }, { key: "k3" });
  assert.equal(result.ok, true);
  assert.equal(Object.hasOwn(result, "requestId"), false, "a success carries no requestId");
});
