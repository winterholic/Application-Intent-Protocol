import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTypedExtensions } from "../../spikes/spike-v6-transport/client/typed.ts";

const fingerprint = "a".repeat(64);
const session = { ok: true, principal: { actorId: "1" }, remainingMs: 60_000 };
const resourceContract = {
  Echo: { root: true, fields: { id: "number" }, traverse: {}, filterFields: {}, filter: "never", sort: "never", maxRows: 20 },
};
const idDescriptor = { type: "Id", nullable: false };
const descriptor = (type, nullable = false, values) => ({ type, nullable, ...(values ? { values } : {}) });
const extensions = (input, output) => ({ "Echo.echo": { input, output } });

function binding(idWire, input, output) {
  return { fingerprint, idWire, __contract: resourceContract, __apply: {}, readDescriptors: {Echo: {root:true,maxRows:20,fields:{id:idDescriptor},traverse:{}}}, extensions: extensions(input, output) };
}

function jsonResponse(body) {
  return new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } });
}

function makeFetch(extensionHandler = async () => jsonResponse({
  ok: true,
  output: { valueId: 42 },
  contractFingerprint: fingerprint,
})) {
  const requests = [];
  const fetcher = async (url, init = {}) => {
    const path = new URL(String(url)).pathname;
    const headers = new Headers(init.headers);
    const body = init.body === undefined ? undefined : JSON.parse(init.body);
    requests.push({ path, headers, body });
    if (path === "/session") return jsonResponse(session);
    if (path === "/extension") return extensionHandler({ path, headers, body, requests });
    if (path === "/read") {
      return jsonResponse({ ok: true, rows: [{ id: 1 }], deps: ["Echo"], maxAgeMs: 60_000, contractFingerprint: fingerprint });
    }
    return jsonResponse({ ok: false, code: "NOT_FOUND" });
  };
  return { fetcher, requests };
}

function safeClient(handler, outputs = { valueId: idDescriptor }) {
  const mock = makeFetch(handler);
  return { ...mock, client: connectTypedExtensions(
    "http://extension.invalid",
    "old-token",
    binding("safe-number-v13", { valueId: idDescriptor }, outputs),
    mock.fetcher,
  ) };
}

function decimalClient(handler, outputs = { valueId: idDescriptor }) {
  const mock = makeFetch(handler);
  return { ...mock, client: connectTypedExtensions(
    "http://extension.invalid",
    "old-token",
    binding("decimal-string-v13", { valueId: idDescriptor }, outputs),
    mock.fetcher,
  ) };
}

const codeIs = (code) => (error) => error?.code === code;

test("extension makes repeated authenticated calls, returns a frozen output, and leaves cache/pending alone", async () => {
  let calls = 0;
  const { client, requests } = safeClient(async ({ headers, body }) => {
    calls++;
    assert.equal(headers.get("x-aip-contract"), fingerprint);
    assert.equal(headers.get("authorization"), "Bearer old-token");
    assert.deepEqual(body, { extension: "Echo.echo", input: { valueId: 42 } });
    return jsonResponse({ ok: true, output: { valueId: 42 }, contractFingerprint: fingerprint });
  });

  assert.equal(client.cache.size(), 0);
  assert.deepEqual(client.pending(), []);
  const first = await client.extension("Echo.echo", { valueId: 42 });
  const second = await client.extension("Echo.echo", { valueId: 42 });

  assert.deepEqual(first, { valueId: 42 });
  assert.ok(Object.isFrozen(first));
  assert.deepEqual(second, { valueId: 42 });
  assert.equal(calls, 2, "extensions execute on each call rather than using the read cache");
  assert.equal(requests.filter((request) => request.path === "/extension").length, 2);
  assert.equal(client.cache.size(), 0);
  assert.deepEqual(client.pending(), []);
});

test("invalid input is rejected before any network request", async () => {
  const { client, requests } = safeClient();
  await assert.rejects(client.extension("Echo.echo", { valueId: -1 }), codeIs("BAD_VALUE"));
  assert.equal(requests.length, 0);
});

test("safe-number and decimal-string Id outputs enforce their wire bounds", async () => {
  for (const valueId of [-1, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
    const { client } = safeClient(async () => jsonResponse({ ok: true, output: { valueId }, contractFingerprint: fingerprint }));
    await assert.rejects(client.extension("Echo.echo", { valueId: 42 }), codeIs("PROTOCOL_ERROR"));
  }
  for (const valueId of [42, "042", "9223372036854775808"]) {
    const { client } = decimalClient(async () => jsonResponse({ ok: true, output: { valueId }, contractFingerprint: fingerprint }));
    await assert.rejects(client.extension("Echo.echo", { valueId: "42" }), codeIs("PROTOCOL_ERROR"));
  }
  const { client } = decimalClient(async () => jsonResponse({ ok: true, output: { valueId: "9223372036854775807" }, contractFingerprint: fingerprint }));
  assert.deepEqual(await client.extension("Echo.echo", { valueId: "42" }), { valueId: "9223372036854775807" });
});

test("output requires exact keys and honors nullable declarations", async () => {
  const outputs = { valueId: idDescriptor, note: descriptor("Text", true) };
  for (const output of [
    { valueId: 42, extra: "unknown" },
    { valueId: 42 },
    { valueId: null, note: null },
  ]) {
    const { client } = safeClient(async () => jsonResponse({ ok: true, output, contractFingerprint: fingerprint }), outputs);
    await assert.rejects(client.extension("Echo.echo", { valueId: 42 }), codeIs("PROTOCOL_ERROR"));
  }
  const { client } = safeClient(async () => jsonResponse({ ok: true, output: { valueId: 42, note: null }, contractFingerprint: fingerprint }), outputs);
  assert.deepEqual(await client.extension("Echo.echo", { valueId: 42 }), { valueId: 42, note: null });
});

test("runtime output validation rejects unknown enum values, NUL strings, and invalid Time", async () => {
  const outputs = {
    role: descriptor("Enum", false, ["ADMIN", "MEMBER"]),
    text: descriptor("Text"),
    url: descriptor("Url"),
    at: descriptor("Time"),
  };
  const validInput = { valueId: 42 };
  for (const output of [
    { role: "UNKNOWN", text: "ok", url: "https://example.test", at: "2024-02-29T12:34:56Z" },
    { role: "ADMIN", text: "left\u0000right", url: "https://example.test", at: "2024-02-29T12:34:56Z" },
    { role: "ADMIN", text: "ok", url: "https://example.test\u0000path", at: "2024-02-29T12:34:56Z" },
    { role: "ADMIN", text: "ok", url: "https://example.test", at: "2026-02-29T12:34:56Z" },
  ]) {
    const { client } = safeClient(async () => jsonResponse({ ok: true, output, contractFingerprint: fingerprint }), outputs);
    await assert.rejects(client.extension("Echo.echo", validInput), codeIs("PROTOCOL_ERROR"));
  }
  const validOutput = { role: "ADMIN", text: "ok", url: "https://example.test", at: "2024-02-29t12:34:56.123456+05:30" };
  const { client } = safeClient(async () => jsonResponse({ ok: true, output: validOutput, contractFingerprint: fingerprint }), outputs);
  assert.deepEqual(await client.extension("Echo.echo", validInput), validOutput);
});

test("server failures keep their code and a successful response with a different fingerprint is rejected", async () => {
  const denied = safeClient(async () => jsonResponse({ ok: false, code: "ACCESS_DENIED", msg: "denied" }));
  await assert.rejects(denied.client.extension("Echo.echo", { valueId: 42 }), codeIs("ACCESS_DENIED"));

  const stale = safeClient(async () => jsonResponse({ ok: true, output: { valueId: 42 }, contractFingerprint: "b".repeat(64) }));
  await assert.rejects(stale.client.extension("Echo.echo", { valueId: 42 }), codeIs("CONTRACT_MISMATCH"));
  assert.equal(stale.client.cache.size(), 0);
  assert.deepEqual(stale.client.pending(), []);
});

test("a late extension response after same-principal session replacement is discarded without clearing the new cache", async () => {
  let releaseExtension;
  let extensionStarted;
  const started = new Promise((resolve) => { extensionStarted = resolve; });
  const extensionReply = new Promise((resolve) => { releaseExtension = resolve; });
  const requests = [];
  const fetcher = async (url, init = {}) => {
    const path = new URL(String(url)).pathname;
    const headers = new Headers(init.headers);
    const body = init.body === undefined ? undefined : JSON.parse(init.body);
    requests.push({ path, headers, body });
    if (path === "/session") return jsonResponse(session);
    if (path === "/read") return jsonResponse({ ok: true, rows: [{ id: 1 }], deps: ["Echo"], maxAgeMs: 60_000, contractFingerprint: fingerprint });
    if (path === "/extension") {
      assert.equal(headers.get("authorization"), "Bearer old-token");
      extensionStarted();
      return extensionReply;
    }
    return jsonResponse({ ok: false, code: "NOT_FOUND" });
  };
  const client = connectTypedExtensions(
    "http://extension.invalid",
    "old-token",
    binding("safe-number-v13", { valueId: idDescriptor }, { valueId: idDescriptor }),
    fetcher,
  );

  await client.read({ read: "Echo", select: ["id"] });
  assert.equal((await client.read({ read: "Echo", select: ["id"] })).cached, true);
  assert.equal(client.cache.size(), 1);
  const pendingExtension = client.extension("Echo.echo", { valueId: 42 });
  await started;
  await client.replaceSession("new-token");
  await client.read({ read: "Echo", select: ["id"] });
  assert.equal((await client.read({ read: "Echo", select: ["id"] })).cached, true);
  assert.equal(client.cache.size(), 1);

  releaseExtension(jsonResponse({ ok: true, output: { valueId: 42 }, contractFingerprint: fingerprint }));
  await assert.rejects(pendingExtension, (error) => error?.name === "ScopeChanged");
  assert.equal((await client.read({ read: "Echo", select: ["id"] })).cached, true, "late extension response must not invalidate the refreshed session cache");
  assert.deepEqual(client.pending(), []);
  assert.equal(requests.find((request) => request.path === "/extension").headers.get("authorization"), "Bearer old-token");
});


test("a connected decimal binding cannot change its wire by mutating the caller's object", async () => {
  const b = binding("decimal-string-v13", { valueId: idDescriptor }, { valueId: idDescriptor });
  const mock = makeFetch(async () => jsonResponse({ ok:true, output:{valueId:42}, contractFingerprint:fingerprint }));
  const client = connectTypedExtensions("http://extension.invalid", "old-token", b, mock.fetcher);
  b.idWire = "safe-number-v13";
  await assert.rejects(client.extension("Echo.echo", { valueId:42 }), codeIs("BAD_VALUE"));
  assert.equal(mock.requests.length, 0);
});

test("the validated input snapshot is the same value sent to the worker", async () => {
  let reads = 0;
  const input = { get valueId() { return ++reads === 1 ? 42 : "changed"; } };
  const { client } = safeClient(async ({body}) => {
    assert.deepEqual(body.input,{valueId:42});
    return jsonResponse({ok:true,output:{valueId:42},contractFingerprint:fingerprint});
  });
  assert.deepEqual(await client.extension("Echo.echo", input),{valueId:42});
  assert.equal(reads,1);
});
