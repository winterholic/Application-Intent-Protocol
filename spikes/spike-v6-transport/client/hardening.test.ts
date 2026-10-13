import { test } from "node:test";
import assert from "node:assert/strict";
import { connect as rawConnect, WriteUnsettled } from "./transport.ts";

const request = { apply: "MemberAlarm.read", target: { ids: ["1"] } };
const query = { read: "MemberAlarm", select: ["id"] };
const reply = (body: unknown) => new Response(JSON.stringify(body));
const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } });

type Options = Parameters<typeof rawConnect>[3];
function connect(impl: typeof fetch, options: Options = {}) {
  return rawConnect("http://spike.invalid", "session", async (url, init) => {
    if (String(url).endsWith("/session")) return reply({ ok: true, principal: { actorId: "1" }, remainingMs: 60_000 });
    return impl(url, init);
  }, options);
}

test("HTML error page from a proxy is a protocol error, not a JSON syntax error", async () => {
  const client = connect(async () => new Response("<html><body>502 Bad Gateway</body></html>", { status: 502, headers: { "content-type": "text/html" } }));
  await assert.rejects(client.read(query), (error: any) => error.code === "CLIENT.PROTOCOL_ERROR");
});

test("JSON with an explicit json media type and parameters is accepted", async () => {
  const client = connect(async () => new Response(JSON.stringify({ ok: true, rows: [], deps: [] }), { headers: { "content-type": "application/json; charset=utf-8" } }));
  assert.deepEqual((await client.read(query)).rows, []);
});

test("a response over the size limit is refused before it is parsed", async () => {
  const huge = { ok: true, rows: ["x".repeat(10_000)], deps: [] };
  const declared = connect(async () => new Response(JSON.stringify(huge), { headers: { "content-type": "application/json", "content-length": "10040" } }), { maxResponseBytes: 2000 });
  await assert.rejects(declared.read(query), (error: any) => error.code === "CLIENT.PROTOCOL_ERROR");
  // No content-length (chunked): the bytes are counted while they stream in.
  const stream = new ReadableStream({
    start(controller) {
      for (let i = 0; i < 10; i++) controller.enqueue(new TextEncoder().encode("x".repeat(1000)));
      controller.close();
    },
  });
  const chunked = connect(async () => new Response(stream, { headers: { "content-type": "application/json" } }), { maxResponseBytes: 2000 });
  await assert.rejects(chunked.read(query), (error: any) => error.code === "CLIENT.PROTOCOL_ERROR");
});

test("a pending write that the server rejected as malformed can be discarded", async () => {
  let attempts = 0;
  const client = connect(async () => {
    attempts++;
    if (attempts === 1) throw new TypeError("response lost after the server stored nothing");
    return json({ ok: false, code: "WIRE.BAD_REQUEST" });
  });
  await assert.rejects(client.apply(request, { key: "one", retries: 0 }), WriteUnsettled);
  assert.deepEqual(client.pending(), ["one"]);
  assert.equal(client.discardPending("two"), false, "an unknown key is not a pending write");
  assert.equal(client.discardPending("one"), true);
  assert.deepEqual(client.pending(), []);
  // The cache is no longer held back by an unknown write.
  assert.equal((await client.read(query).catch(() => ({ stored: false }))).stored !== undefined, true);
});

test("applying the same request again after WriteUnsettled reuses the unsettled key instead of writing twice", async () => {
  const keys: string[] = [];
  let offline = true;
  const client = connect(async (_url, init) => {
    keys.push(JSON.parse(String(init?.body)).key);
    if (offline) throw new TypeError("offline");
    return json({ ok: true, tags: ["MemberAlarm"], replayed: keys.length > 1 });
  });
  await assert.rejects(client.apply(request, { retries: 0 }), WriteUnsettled);
  offline = false;
  // The caller retries without remembering the key; a fresh random key would run a committed write a second time.
  const outcome = await client.apply(request, { retries: 0 });
  assert.equal(outcome.ok, true);
  assert.equal(keys.length, 2);
  assert.equal(keys[1], keys[0], "the retry carries the key of the unsettled attempt");
  assert.deepEqual(client.pending(), []);
});

test("a different request after WriteUnsettled still gets its own key", async () => {
  const keys: string[] = [];
  let offline = true;
  const client = connect(async (_url, init) => {
    keys.push(JSON.parse(String(init?.body)).key);
    if (offline) throw new TypeError("offline");
    return json({ ok: true, tags: ["MemberAlarm"] });
  });
  await assert.rejects(client.apply(request, { retries: 0 }), WriteUnsettled);
  offline = false;
  await client.apply({ apply: "MemberAlarm.read", target: { ids: ["2"] } }, { retries: 0 });
  assert.notEqual(keys[1], keys[0]);
});
