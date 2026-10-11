import { test } from "node:test";
import assert from "node:assert/strict";
import { createInfrastructureClient, readSse } from "./infrastructure.ts";

const fingerprint = "a".repeat(64);
const id = "9d05f3a0-29ef-4efc-9000-4c5e3f3eb10d";
const token = "39b0a2ee-dadb-4a8f-8291-182994c040fc";
const ok = (fields: Record<string, unknown> = {}) => ({ ok: true, contractFingerprint: fingerprint, ...fields });
const lease = {
  id, owner: "issuer|alice", queue: "uploads", payload: { key: id }, deliveries: 1, max_deliveries: 4,
  epoch: 2, token, worker: "worker-1", lease_until: "2026-10-11T01:02:03Z",
};

test("objects sign and multipart methods use managed routes and return immutable validated values", async () => {
  const calls: unknown[][] = [];
  const client = createInfrastructureClient(async (route, body) => {
    calls.push([route, body]);
    if (route === "/objects/sign") return ok({ id, method: "PUT", url: "https://objects.example/upload", headers: { "content-type": "application/octet-stream", "content-length": "1024" }, expiresIn: 300 });
    if (route === "/objects/multipart/start") return ok({ id, uploadId: "opaque-upload" });
    if (route === "/objects/multipart/part") return ok({ id, partNumber: 1, method: "PUT", url: "https://objects.example/part", headers: { "content-length": "1024" }, expiresIn: 300 });
    if (route === "/objects/multipart/complete") return ok({ id, completed: true });
    return ok({ id });
  }, fingerprint);

  const signed = await client.objects.sign({ method: "PUT", size: 1024, contentType: "application/octet-stream" });
  assert.deepEqual(signed, { id, method: "PUT", url: "https://objects.example/upload", headers: { "content-type": "application/octet-stream", "content-length": "1024" }, expiresIn: 300 });
  assert.ok(Object.isFrozen(signed) && Object.isFrozen(signed.headers));
  const started = await client.objects.multipartStart({ contentType: "application/octet-stream" });
  assert.deepEqual(await client.objects.part({ id: started.id, uploadId: started.uploadId, partNumber: 1, size: 1024 }), {
    id, uploadId: "opaque-upload", partNumber: 1, size: 1024, method: "PUT", url: "https://objects.example/part", headers: { "content-length": "1024" }, expiresIn: 300,
  });
  assert.deepEqual(await client.objects.complete({ ...started, parts: [{ number: 1, etag: '"part-etag"' }] }), {
    id, completed: true,
  });
  assert.equal((await client.objects.abort(started)).aborted, true);
  assert.deepEqual(calls.map(([route]) => route), [
    "/objects/sign", "/objects/multipart/start", "/objects/multipart/part", "/objects/multipart/complete", "/objects/multipart/abort",
  ]);
  assert.deepEqual(calls[0]?.[1], { method: "PUT", size: 1024, contentType: "application/octet-stream" });
  assert.deepEqual(calls[2]?.[1], { id, uploadId: "opaque-upload", partNumber: 1, size: 1024 });
});

test("messages support enqueue, lease claim, ack, nack, bounded DLQ page and redrive", async () => {
  const calls: unknown[][] = [];
  const client = createInfrastructureClient(async (route, body) => {
    calls.push([route, body]);
    if (route === "/messages/enqueue") return ok({ message: {
      id, owner: "issuer|alice", queue: "uploads", payload: { key: id }, state: "READY", deliveries: 0, max_deliveries: 4, last_error: null,
    } });
    if (route === "/messages/claim") return ok({ lease });
    if (route === "/messages/ack") return ok({ acknowledged: true });
    if (route === "/messages/nack") return ok({ released: true });
    if (route === "/messages/dead") return ok({ items: [{
      id, owner: "issuer|alice", queue: "uploads", payload: { key: id }, deliveries: 4, max_deliveries: 4,
      last_error: "TEMPORARY", created_at: "2026-10-10T00:00:00Z",
    }], next_cursor: null });
    return ok({ redriven: true });
  }, fingerprint);

  const message = await client.messages.enqueue({ queue: "uploads", key: "message-1", payload: { key: id } });
  assert.equal(message.state, "READY");
  assert.ok(Object.isFrozen(message) && Object.isFrozen(message.payload));
  const claimed = await client.messages.claim({ queue: "uploads", worker: "worker-1" });
  assert.equal(claimed?.token, token);
  assert.ok(Object.isFrozen(claimed));
  assert.equal(await client.messages.ack({ queue: "uploads", lease: claimed! }), true);
  assert.equal(await client.messages.nack({ queue: "uploads", lease: claimed!, error: "TEMPORARY" }), true);
  const dead = await client.messages.dead({ queue: "uploads", limit: 10 });
  assert.equal(dead.items.length, 1);
  assert.ok(Object.isFrozen(dead.items) && Object.isFrozen(dead.items[0]));
  assert.equal(await client.messages.redrive({ queue: "uploads", id, maxDeliveries: 5 }), true);
  assert.deepEqual(calls[0]?.[1], { queue: "uploads", key: "message-1", payload: { key: id } });
  assert.deepEqual(calls[1]?.[1], { queue: "uploads", worker: "worker-1" });
  assert.deepEqual(calls[2]?.[1], { queue: "uploads", lease: { ...lease, max_deliveries: 4 } });
});

test("rejects malformed input before transport, enforces object and payload/page bounds", async () => {
  let calls = 0;
  const client = createInfrastructureClient(async () => { calls++; return ok(); }, fingerprint);
  await assert.rejects(client.objects.sign({ method: "GET" } as any), (error: any) => error.code === "BAD_VALUE");
  await assert.rejects(client.objects.part({ id, uploadId: "u", partNumber: 1, size: 0 }), (error: any) => error.code === "BAD_VALUE");
  await assert.rejects(client.messages.enqueue({ queue: "bad queue", key: "k", payload: {} }), (error: any) => error.code === "BAD_VALUE");
  await assert.rejects(client.messages.dead({ queue: "uploads", limit: 1001 }), (error: any) => error.code === "BAD_VALUE");
  await assert.rejects(client.messages.enqueue({ queue: "uploads", key: "k", payload: { data: "x".repeat(1 << 20) } }), (error: any) => error.code === "BAD_VALUE");
  assert.equal(calls, 0);
});

test("message pagination and duration limits match the HTTP contract", async () => {
  const calls: unknown[][] = [];
  const client = createInfrastructureClient(async (route, body) => {
    calls.push([route, body]);
    return ok({ items: [], next_cursor: null });
  }, fingerprint);
  await client.messages.dead({ queue: "uploads", cursor: id, limit: 10 });
  assert.deepEqual(calls, [["/messages/dead", { queue: "uploads", limit: 10, after: id }]]);
  await assert.rejects(client.messages.enqueue({ queue: "uploads", key: "k", payload: {}, ttlMs: 86_400_001 }), (error: any) => error.code === "BAD_VALUE");
  await assert.rejects(client.messages.claim({ queue: "uploads", worker: "worker", leaseMs: 300_001 }), (error: any) => error.code === "BAD_VALUE");
  await assert.rejects(client.messages.dead({ queue: "uploads", cursor: "not-a-uuid" }), (error: any) => error.code === "BAD_VALUE");
  assert.equal(calls.length, 1);
});

test("message inputs enforce UTF-8 byte and payload limits before sending", async () => {
  let calls = 0;
  const client = createInfrastructureClient(async () => { calls++; return ok(); }, fingerprint);
  for (const key of ["한".repeat(86), "bad\u0085key"]) {
    await assert.rejects(client.messages.enqueue({ queue: "uploads", key, payload: {} }), (error: any) => error.code === "BAD_VALUE");
  }
  await assert.rejects(client.messages.enqueue({ queue: "uploads", key: "k", payload: "x".repeat(512 * 1024) }), (error: any) => error.code === "BAD_VALUE");
  await assert.rejects(client.messages.claim({ queue: "uploads", worker: "한".repeat(86) }), (error: any) => error.code === "BAD_VALUE");
  const validLease = { id, owner: lease.owner, queue: lease.queue, payload: lease.payload, deliveries: 1, maxDeliveries: 4, epoch: 2, token, worker: "worker-1", leaseUntil: lease.lease_until };
  await assert.rejects(client.messages.nack({ queue: "uploads", lease: validLease, error: "한".repeat(171) }), (error: any) => error.code === "BAD_VALUE");
  assert.equal(calls, 0);
});

test("validates response envelope, fingerprints, UUIDs and lease ownership without trusting bad data", async () => {
  const mismatch = createInfrastructureClient(async () => ({ ...ok({ lease }), contractFingerprint: "b".repeat(64) }), fingerprint);
  await assert.rejects(mismatch.messages.claim({ queue: "uploads", worker: "worker-1" }), (error: any) => error.code === "CONTRACT_MISMATCH");
  const invalid = createInfrastructureClient(async () => ok({ lease: { ...lease, id: "../private" } }), fingerprint);
  await assert.rejects(invalid.messages.claim({ queue: "uploads", worker: "worker-1" }), (error: any) => error.code === "PROTOCOL_ERROR");

  const wrongOwnerCalls: unknown[][] = [];
  const wrongOwner = createInfrastructureClient(async (route, body) => { wrongOwnerCalls.push([route, body]); return ok({ acknowledged: true }); }, fingerprint);
  const forged = { id, owner: "issuer|bob", queue: "uploads", payload: { key: id }, deliveries: 1, maxDeliveries: 4, epoch: 2, token, worker: "worker-1", leaseUntil: "2026-10-11T01:02:03Z" };
  assert.equal(await wrongOwner.messages.ack({ queue: "uploads", lease: forged as any }), true);
  assert.equal((wrongOwnerCalls[0]?.[1] as any).lease.owner, "issuer|bob", "service must replace this untrusted owner using the authenticated principal");
});

test("readSse parses split UTF-8 frames, multiline data, ids, event names, and retry values", async () => {
  const encoder = new TextEncoder();
  const bytes = encoder.encode(": heartbeat\r\nid: evt-1\r\nevent: update\r\nretry: 2500\r\ndata: 첫째\r\ndata: 둘째\r\n\r\ndata: tail");
  const chunks = [bytes.slice(0, 17), bytes.slice(17, 23), bytes.slice(23)];
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      const next = chunks.shift();
      if (next) controller.enqueue(next);
      else controller.close();
    },
  });
  const response = new Response(body, { headers: { "content-type": "text/event-stream; charset=utf-8" } });
  const events = [];
  for await (const event of readSse(response)) events.push(event);
  assert.deepEqual(events, [
    { event: "update", data: "첫째\n둘째", id: "evt-1", retryMs: 2500 },
    { event: "message", data: "tail", id: "evt-1" },
  ]);
  assert.ok(Object.isFrozen(events[0]));
});

test("readSse rejects oversized frames and invalid response content types", async () => {
  const huge = new Response(new ReadableStream<Uint8Array>({ start(controller) {
    controller.enqueue(new TextEncoder().encode(`data: ${"x".repeat(64 * 1024 + 1)}\n\n`));
    controller.close();
  } }), { headers: { "content-type": "text/event-stream" } });
  await assert.rejects(async () => { for await (const _ of readSse(huge)) {} }, (error: any) => error.code === "PROTOCOL_ERROR");
  const wrongType = new Response("data: x\n\n", { headers: { "content-type": "application/json" } });
  await assert.rejects(async () => { for await (const _ of readSse(wrongType)) {} }, (error: any) => error.code === "PROTOCOL_ERROR");
});

test("readSse accepts lone CR line endings split across chunks", async () => {
  const chunks = [new TextEncoder().encode("data: one\r"), new TextEncoder().encode("\rdata: two\r\r")];
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      const next = chunks.shift();
      if (next) controller.enqueue(next);
      else controller.close();
    },
  });
  const response = new Response(body, { headers: { "content-type": "text/event-stream" } });
  const events = [];
  for await (const event of readSse(response)) events.push(event);
  assert.deepEqual(events, [{ event: "message", data: "one" }, { event: "message", data: "two" }]);
});

test("readSse cancellation stops the pending stream read", async () => {
  const response = new Response(new ReadableStream<Uint8Array>({ start() {} }), { headers: { "content-type": "text/event-stream" } });
  const controller = new AbortController();
  const iterator = readSse(response, controller.signal);
  const pending = iterator.next();
  controller.abort();
  await assert.rejects(pending, (error: any) => error.name === "AbortError");
});


test("product client shares session replacement and fingerprint with infrastructure routes", async () => {
  const { connect } = await import("../../../product/sdk/index.ts");
  let release!: (value: Response) => void;
  const requests: Array<{path:string; auth:string|null; contract:string|null}> = [];
  const fetcher: typeof fetch = async (url, init) => {
    const path = new URL(String(url)).pathname;
    const headers = new Headers(init?.headers);
    requests.push({path,auth:headers.get("authorization"),contract:headers.get("x-aip-contract")});
    if (path === "/session") return new Response(JSON.stringify({ok:true,principal:{actorId:"1"},remainingMs:60000}));
    return new Promise(resolve => { release = resolve; });
  };
  const client = connect("https://aip.invalid", "old", {fingerprint,idWire:"safe-number-v13",readDescriptors:{},extensions:{},operations:{}} as any, fetcher);
  assert.ok(client.messages, "product SDK must expose the managed infrastructure facade");
  const pending = client.messages.claim({queue:"uploads",worker:"worker"});
  while (!release) await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(requests[1], {path:"/messages/claim",auth:"Bearer old",contract:fingerprint});
  await client.replaceSession("new");
  release(new Response(JSON.stringify(ok({lease:null}))));
  await assert.rejects(pending, (error:any) => error.name === "ScopeChanged");
  assert.equal((client as any).callManaged, undefined);
});
