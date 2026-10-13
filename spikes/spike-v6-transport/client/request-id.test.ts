import { test } from "node:test";
import assert from "node:assert/strict";
import { connect as rawConnect, WriteUnsettled } from "./transport.ts";

// 실패 본문의 requestId·retry를 보존하고, ID가 없으면 x-request-id 헤더를 사용한다.
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
  const client = connect(() => reply({ ok: false, code: "WIRE.FILTER_NOT_ALLOWED", msg: "x" }));
  await assert.rejects(client.read({ read: "A", select: ["id"] }), (e: any) => e.code === "WIRE.FILTER_NOT_ALLOWED" && e.requestId === ID);
});

test("계약 위반 응답의 프로토콜 오류도 요청 ID를 싣는다", async () => {
  const client = connect(() => reply({ ok: true, contractFingerprint: "b".repeat(64), rows: [], deps: [] }, "proxy-7f3a9c.01_ab"));
  await assert.rejects(client.read({ read: "A", select: ["id"] }), (e: any) => e.code === "WIRE.CONTRACT_MISMATCH" && e.requestId === "proxy-7f3a9c.01_ab");
});

test("확장·operation·관리 호출의 거부도 요청 ID를 싣는다", async () => {
  const client = connect(() => reply({ ok: false, code: "WIRE.NOT_EXPOSED" }));
  await assert.rejects(client.callExtension("e", {}), (e: any) => e.code === "WIRE.NOT_EXPOSED" && e.requestId === ID);
  await assert.rejects(client.callOperation("o", {}), (e: any) => e.code === "WIRE.NOT_EXPOSED" && e.requestId === ID);
  await assert.rejects(client.callManaged("/jobs/status", {}), (e: any) => e.code === "WIRE.NOT_EXPOSED" && e.requestId === ID);
});

test("쓰기 거부 결과와 미확정 오류가 마지막 응답의 요청 ID를 싣는다", async () => {
  const rejected = connect(() => reply({ ok: false, code: "WIRE.BAD_VALUE" }));
  const outcome = await rejected.apply({ apply: "A.b" }, { key: "k1" });
  assert.deepEqual({ ok: outcome.ok, code: outcome.code, requestId: (outcome as any).requestId }, { ok: false, code: "WIRE.BAD_VALUE", requestId: ID });

  const unsettled = connect(() => reply({ ok: false, code: "WIRE.COMMIT_UNKNOWN" }, "req_aaaaaaaaaaaaaaaaaaaaaaaa"));
  await assert.rejects(unsettled.apply({ apply: "A.b" }, { key: "k2", retries: 0 }), (e: any) => e instanceof WriteUnsettled && e.requestId === "req_aaaaaaaaaaaaaaaaaaaaaaaa");
});

test("헤더가 없거나 형식이 틀리면 requestId를 만들지 않고 본문 계약은 그대로다", async () => {
  for (const header of [null, "short", "has space in it", "x".repeat(65), "semi;colon-1234"]) {
    const client = connect(() => reply({ ok: false, code: "WIRE.NOT_FOUND" }, header));
    await assert.rejects(client.read({ read: "A", select: ["id"] }), (e: any) => e.code === "WIRE.NOT_FOUND" && !Object.hasOwn(e, "requestId"), String(header));
  }
  const ok = connect(() => reply({ ok: true, tags: [], changed: [], unchanged: [] }));
  const result: any = await ok.apply({ apply: "A.b" }, { key: "k3" });
  assert.equal(result.ok, true);
  assert.equal(Object.hasOwn(result, "requestId"), false, "a success carries no requestId");
});

// CD-2: 실패 본문이 requestId(헤더와 같은 값)와 retry(레지스트리 값)를 직접 싣는다. 본문 값이 헤더보다 우선한다.
test("실패 본문의 requestId와 retry가 던져지는 오류와 쓰기 결과에 그대로 실린다", async () => {
  const bodyId = "req_bbbbbbbbbbbbbbbbbbbbbbbb";
  const client = connect(() => reply({ ok: false, code: "WIRE.DB_UNAVAILABLE", requestId: bodyId, retry: "later" }, null));
  await assert.rejects(client.read({ read: "A", select: ["id"] }), (e: any) => e.code === "WIRE.DB_UNAVAILABLE" && e.requestId === bodyId && e.retry === "later");
  const preferred = connect(() => reply({ ok: false, code: "WIRE.NOT_FOUND", requestId: bodyId, retry: "never" }));
  await assert.rejects(preferred.read({ read: "A", select: ["id"] }), (e: any) => e.requestId === bodyId && e.retry === "never");
  const outcome: any = await connect(() => reply({ ok: false, code: "WIRE.BAD_VALUE", requestId: bodyId, retry: "never" })).apply({ apply: "A.b" }, { key: "k9" });
  assert.deepEqual({ requestId: outcome.requestId, retry: outcome.retry }, { requestId: bodyId, retry: "never" });
});

test("알 수 없거나 형식이 틀린 retry는 오류에 싣지 않는다", async () => {
  for (const bad of ["sometimes", ["later"], null, 3]) {
    const client = connect(() => reply({ ok: false, code: "WIRE.NOT_FOUND", retry: bad }));
    await assert.rejects(client.read({ read: "A", select: ["id"] }), (e: any) => e.code === "WIRE.NOT_FOUND" && !Object.hasOwn(e, "retry"), String(bad));
  }
});

test("미확정 쓰기 오류는 마지막 서버 응답의 재시도 지침을 보존한다", async () => {
  const client = connect(() => reply({ ok: false, code: "WIRE.COMMIT_UNKNOWN", retry: "check-first" }));
  await assert.rejects(client.apply({ apply: "A.b" }, { key: "k-retry", retries: 0 }),
    (e: any) => e instanceof WriteUnsettled && e.requestId === ID && e.retry === "check-first");
});

test("쓰기 실패 결과도 알 수 없는 retry를 노출하지 않는다", async () => {
  const client = connect(() => reply({ ok: false, code: "WIRE.BAD_VALUE", retry: "sometimes" }));
  const outcome = await client.apply({ apply: "A.b" }, { key: "k-invalid-retry" });
  assert.equal(Object.hasOwn(outcome, "retry"), false);
});
