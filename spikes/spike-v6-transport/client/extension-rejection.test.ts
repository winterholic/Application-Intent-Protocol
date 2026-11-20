import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTypedExtensions } from "./typed.ts";
import { connect } from "./transport.ts";

const fingerprint = "a".repeat(64);
const placeInput = { type: "Text", nullable: false };
const binding = {
  fingerprint,
  idWire: "decimal-string-v13",
  readDescriptors: {},
  extensions: {},
  operations: {},
  writeExtensions: { "Order.place": { input: { id: placeInput }, output: { total: { type: "Text", nullable: false } }, access: ["Order.pay"] } },
} as any;
const reply = (body: unknown) => new Response(JSON.stringify(body), { headers: { "x-request-id": "req_0123456789abcdef" } });

function server(apply: (body: any) => Response): typeof fetch {
  return async (url, init) => {
    const path = new URL(String(url)).pathname;
    if (path === "/session") return reply({ ok: true, principal: { actorId: "1" }, remainingMs: 60_000 });
    assert.equal(path, "/apply");
    return apply(JSON.parse(String(init?.body)));
  };
}

test("X8: 빈 문자열 토큰은 원인을 말하는 CLIENT.INVALID_TOKEN으로 거절하고 null은 익명으로 둔다", async () => {
  for (const blank of ["", "   ", undefined as unknown as string]) {
    assert.throws(() => connectTypedExtensions("http://aip.invalid", blank, binding, server(() => reply({}))), (error: any) => {
      assert.equal(error.code, "CLIENT.INVALID_TOKEN");
      assert.match(error.message, /null/);
      return true;
    });
    assert.throws(() => connect("http://aip.invalid", blank, server(() => reply({}))), { code: "CLIENT.INVALID_TOKEN" });
  }
  const anonymous = async (url: any) => reply(new URL(String(url)).pathname === "/session" ? { ok: true, principal: { actorId: null }, remainingMs: null } : {});
  const client = connectTypedExtensions("http://aip.invalid", null, binding, anonymous as typeof fetch);
  assert.deepEqual(await client.session(), { actorId: null });
  const signedIn = connectTypedExtensions("http://aip.invalid", "t1", binding, server(() => reply({})));
  for (const blank of ["", " "]) await assert.rejects(signedIn.replaceSession(blank), { code: "CLIENT.INVALID_TOKEN" });
});

test("X2: 확장의 업무 거절은 appCode와 msg를 가진 실패 결과이고 미확정 쓰기가 아니다", async () => {
  const sent: any[] = [];
  const rejected = { ok: false, code: "WIRE.EXTENSION_REJECTED", appCode: "INSUFFICIENT_STOCK", msg: "재고가 부족합니다", retry: "never" };
  const client = connectTypedExtensions("http://aip.invalid", "t1", binding, server((body) => {
    sent.push(body);
    return reply(body.key === "bug" ? { ok: false, code: "WIRE.EXTENSION_ERROR", msg: "공식 WRITE 확장 호출 거부", retry: "never" } : rejected);
  }));
  const result = await client.writeExtension("Order.place", { id: "7" }, { key: "place-7" });
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.deepEqual([result.code, result.appCode, result.msg, result.retry], ["WIRE.EXTENSION_REJECTED", "INSUFFICIENT_STOCK", "재고가 부족합니다", "never"]);
  assert.equal(result.requestId, "req_0123456789abcdef");
  assert.deepEqual(sent[0], { request: { extension: "Order.place", input: { id: "7" } }, key: "place-7" });
  assert.deepEqual(client.pending(), [], "a business refusal is final: nothing waits for a retry");
  const bug = await client.writeExtension("Order.place", { id: "8" }, { key: "bug" });
  assert.equal(bug.ok === false && bug.code, "WIRE.EXTENSION_ERROR");
  assert.equal(bug.ok === false && bug.appCode, undefined, "a code bug carries no business code");
});
