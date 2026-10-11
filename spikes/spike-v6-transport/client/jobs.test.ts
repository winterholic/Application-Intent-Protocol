import { test } from "node:test";
import assert from "node:assert/strict";
import { createJobsClient } from "./jobs.ts";
import { connectTypedExtensions } from "./typed.ts";
import type { OperationDescriptors } from "../../spike-v5-sdk/sdk/generic.ts";

type Operations = {
  summarize: { input: { text: string }; output: { length: number } };
};
const fingerprint = "a".repeat(64);
const binding = {
  fingerprint,
  idWire: "safe-number-v13" as const,
  operations: {
    summarize: {
      input: { text: { type: "Text", nullable: false, range: [1, 20] as const } },
      output: { length: { type: "Int", nullable: false, range: [1, 20] as const } },
      lifetime: "durable" as const,
      effect: "none" as const,
      deadlineMs: 2000,
      dependencies: { worker: [], authorization: ["database"] },
    },
  } satisfies OperationDescriptors<Operations>,
};

const ok = (fields: Record<string, unknown> = {}) => ({ ok: true, contractFingerprint: fingerprint, ...fields });
const jobId = "9d05f3a0-29ef-4efc-9000-4c5e3f3eb10d";

test("enqueue snapshots typed input and sends the declared operation with defaults", async () => {
  const calls: unknown[][] = [];
  const client = createJobsClient(async (route, body) => {
    calls.push([route, body]);
    return ok({ run: { id: jobId, operation: "summarize", state: "QUEUED", attempt: 0 } });
  }, binding);
  const input = { text: "hello" };
  const job = await client.enqueue("summarize", input, { key: "request-1" });
  input.text = "changed";
  assert.deepEqual(calls[0], ["/jobs/enqueue", { operation: "summarize", input: { text: "hello" }, key: "request-1", delayMs: 0 }]);
  assert.deepEqual(job, { id: jobId, status: "QUEUED" });
  assert.ok(Object.isFrozen(job));
});

test("enqueue rejects invalid input before transport and validates the returned job", async () => {
  let calls = 0;
  const client = createJobsClient(async () => { calls++; return ok({ run: { id: "bad", operation: "summarize", state: "QUEUED" } }); }, binding);
  await assert.rejects(client.enqueue("summarize", { text: "" }, { key: "request-1" }), (error: any) => error.code === "BAD_VALUE");
  await assert.rejects(client.enqueue("summarize", { text: "valid" }, { key: "request-1" }), (error: any) => error.code === "PROTOCOL_ERROR");
  assert.equal(calls, 1);
});

test("enqueue rejects operations declared with sync lifetime", async () => {
  let calls = 0;
  const syncBinding = { ...binding, operations: { summarize: { ...binding.operations.summarize, lifetime: "sync" as const } } };
  const client = createJobsClient(async () => { calls++; return ok(); }, syncBinding);
  await assert.rejects(client.enqueue("summarize", { text: "hello" }, { key: "request-1" }), (error: any) => error.code === "NOT_DURABLE");
  assert.equal(calls, 0);
});

test("status, cancel, and result use job routes and validate operation output", async () => {
  const calls: unknown[][] = [];
  const client = createJobsClient(async (route, body) => {
    calls.push([route, body]);
    if (route === "/jobs/status") return ok({ run: { id: jobId, operation: "summarize", state: "RUNNING", attempt: 1 } });
    if (route === "/jobs/cancel") return ok({ run: { id: jobId, operation: "summarize", state: "CANCELLED", attempt: 1 } });
    return ok({ id: jobId, operation: "summarize", output: { length: 5 } });
  }, binding);
  assert.equal((await client.status(jobId)).status, "RUNNING");
  assert.equal((await client.cancel(jobId)).status, "CANCELLED");
  const result = await client.result(jobId, "summarize");
  assert.deepEqual(result, { length: 5 });
  assert.ok(Object.isFrozen(result));
  assert.deepEqual(calls.map(([route]) => route), ["/jobs/status", "/jobs/cancel", "/jobs/result"]);
});

test("result rejects an output outside the declared descriptor", async () => {
  const client = createJobsClient(async () => ok({ id: jobId, operation: "summarize", output: { length: 21 } }), binding);
  await assert.rejects(client.result(jobId, "summarize"), (error: any) => error.code === "PROTOCOL_ERROR");
});

test("events validates cursor and page size and preserves opaque cursors", async () => {
  const calls: unknown[][] = [];
  const client = createJobsClient(async (route, body) => {
    calls.push([route, body]);
    return ok({ events: [{ cursor: "opaque:cursor", payload: { id: jobId } }], cursor: "opaque:next" });
  }, binding);
  const page = await client.events.read("notifications", { after: "cursor/from-server", limit: 10 });
  assert.equal(page.cursor, "opaque:next");
  assert.deepEqual(calls, [["/events/read", { stream: "notifications", after: "cursor/from-server", limit: 10 }]]);
  await client.events.read("notifications");
  assert.deepEqual(calls[1], ["/events/read", { stream: "notifications", after: "0", limit: 100 }]);
  await assert.rejects(client.events.read("notifications", { limit: 101 }), (error: any) => error.code === "BAD_VALUE");
});

test("file operations encode and decode base64 JSON within the request size limit", async () => {
  const calls: unknown[][] = [];
  const client = createJobsClient(async (route, body) => {
    calls.push([route, body]);
    if (route === "/files/put") return ok({ file: { id: "file-1", bucket: "reports", name: "a.txt", mediaType: "text/plain", size: 5 } });
    if (route === "/files/get") return ok({ file: { id: "file-1", bucket: "reports", name: "a.txt", mediaType: "text/plain", size: 5 }, base64: "aGVsbG8=" });
    return ok({ deleted: true });
  }, binding);
  assert.deepEqual(await client.files.put("reports", "a.txt", new Uint8Array([104, 101, 108, 108, 111]), "text/plain"), { id: "file-1", bucket: "reports", name: "a.txt", mediaType: "text/plain", size: 5 });
  const file = await client.files.get("file-1");
  assert.deepEqual([...file.bytes], [104, 101, 108, 108, 111]);
  assert.equal(await client.files.delete("file-1"), true);
  assert.deepEqual(calls[0], ["/files/put", { bucket: "reports", name: "a.txt", mediaType: "text/plain", base64: "aGVsbG8=" }]);
  await assert.rejects(client.files.put("reports", "large.bin", new Uint8Array(768 * 1024 - 1024 + 1), "application/octet-stream"), (error: any) => error.code === "BAD_VALUE");
});

test("server errors and contract mismatches are surfaced with stable codes", async () => {
  const denied = createJobsClient(async () => ({ ok: false, code: "FORBIDDEN" }), binding);
  await assert.rejects(denied.status(jobId), (error: any) => error.code === "FORBIDDEN");
  const mismatch = createJobsClient(async () => ({ ...ok({ run: { id: jobId, state: "QUEUED" } }), contractFingerprint: "b".repeat(64) }), binding);
  await assert.rejects(mismatch.status(jobId), (error: any) => error.code === "CONTRACT_MISMATCH");
});

test("typed jobs share the authenticated contract transport and reject responses after session replacement", async () => {
  let release!: (response: Response) => void;
  const requests: { path: string; authorization: string | null; fingerprint: string | null; body: unknown }[] = [];
  const response = (value: unknown) => new Response(JSON.stringify(value));
  const typedBinding = {
    ...binding,
    operations: { summarize: { ...binding.operations.summarize, lifetime: "durable" } },
    readDescriptors: {},
    extensions: {},
  } as any;
  const fetcher: typeof fetch = async (url, init) => {
    const path = new URL(String(url)).pathname;
    const headers = new Headers(init?.headers);
    requests.push({ path, authorization: headers.get("authorization"), fingerprint: headers.get("x-aip-contract"), body: init?.body ? JSON.parse(String(init.body)) : {} });
    if (path === "/session") return response({ ok: true, principal: { actorId: "1" }, remainingMs: 60000 });
    return new Promise(resolve => { release = resolve; });
  };
  const client = connectTypedExtensions("http://jobs.invalid", "old-token", typedBinding, fetcher);
  assert.equal((client as any).callManaged, undefined);
  const pending = client.jobs.enqueue("summarize", { text: "hello" }, { key: "job-key" });
  while (!release) await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(requests[0], { path: "/session", authorization: "Bearer old-token", fingerprint: null, body: {} });
  assert.deepEqual(requests[1], {
    path: "/jobs/enqueue", authorization: "Bearer old-token", fingerprint,
    body: { operation: "summarize", input: { text: "hello" }, key: "job-key", delayMs: 0 },
  });
  await client.replaceSession("new-token");
  release(response({ ok: true, contractFingerprint: fingerprint, run: { id: jobId, operation: "summarize", state: "QUEUED", attempt: 0 } }));
  await assert.rejects(pending, (error: any) => error.name === "ScopeChanged");
});

test("typed client leaves jobs unavailable when operation descriptors have no lifetime metadata", () => {
  const typedBinding = {
    ...binding,
    operations: { summarize: { input: binding.operations.summarize.input, output: binding.operations.summarize.output } },
    readDescriptors: {},
    extensions: {},
  } as any;
  const client = connectTypedExtensions("http://jobs.invalid", "token", typedBinding, async () => new Response("{}"));
  assert.equal(client.jobs, undefined);
  assert.equal(client.files, undefined);
  assert.equal(client.events, undefined);
});

test("typed client exposes jobs only for durable operation descriptors", () => {
  const typedBinding = { ...binding, readDescriptors: {}, extensions: {}, operations: {
    summarize: { ...binding.operations.summarize, lifetime: "sync" },
  } } as any;
  const client = connectTypedExtensions("http://jobs.invalid", "token", typedBinding, async () => new Response("{}"));
  assert.equal(client.jobs, undefined);
});

test("events.watch resumes after the last yielded cursor and reconnects after an empty page", async () => {
  const calls: unknown[][] = [];
  let page = 0;
  const client = createJobsClient(async (route, body) => {
    calls.push([route, body]);
    page++;
    if (page === 1) return ok({ events: [], cursor: "cursor:advanced" });
    if (page === 2) return ok({ events: [{ cursor: "cursor:one", payload: { n: 1 } }, { cursor: "cursor:two", payload: { n: 2 } }], cursor: "cursor:two" });
    return ok({ events: [], cursor: "cursor:two" });
  }, binding);
  const controller = new AbortController();
  const watch = client.events.watch("notifications", { after: "cursor:start", intervalMs: 50, signal: controller.signal });
  const first = await watch.next();
  assert.deepEqual(first.value, { cursor: "cursor:one", payload: { n: 1 } });
  const second = await watch.next();
  assert.deepEqual(second.value, { cursor: "cursor:two", payload: { n: 2 } });
  const waiting = watch.next();
  while (calls.length < 3) await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(calls.slice(0, 3).map(([, body]) => (body as any).after), ["cursor:start", "cursor:advanced", "cursor:two"]);
  controller.abort();
  assert.equal((await waiting).done, true);
});

test("events.watch aborts during polling delay without sending another request", async () => {
  let notify!: () => void;
  const called = new Promise<void>(resolve => { notify = resolve; });
  let calls = 0;
  const client = createJobsClient(async () => { calls++; notify(); return ok({ events: [], cursor: "cursor:0" }); }, binding);
  const controller = new AbortController();
  const watch = client.events.watch("notifications", { intervalMs: 50, signal: controller.signal });
  const pending = watch.next();
  await called;
  controller.abort();
  assert.equal((await pending).done, true);
  assert.equal(calls, 1);
});

test("events.watch retries temporary errors but immediately surfaces expired cursors", async () => {
  let calls = 0;
  const client = createJobsClient(async () => {
    calls++;
    if (calls === 1) throw Object.assign(new Error("offline"), { code: "DB_UNAVAILABLE" });
    return ok({ events: [{ cursor: "cursor:ok", payload: { n: 1 } }], cursor: "cursor:ok" });
  }, binding);
  const watch = client.events.watch("notifications", { intervalMs: 50 });
  assert.deepEqual((await watch.next()).value, { cursor: "cursor:ok", payload: { n: 1 } });
  assert.equal(calls, 2);
  await watch.return();

  calls = 0;
  const expired = createJobsClient(async () => { calls++; return { ok: false, code: "CURSOR_EXPIRED" }; }, binding);
  const expiredWatch = expired.events.watch("notifications", { intervalMs: 50 });
  await assert.rejects(expiredWatch.next(), (error: any) => error.code === "CURSOR_EXPIRED");
  assert.equal(calls, 1);
});

test("events.watch validates interval bounds before reading", async () => {
  let calls = 0;
  const client = createJobsClient(async () => { calls++; return ok({ events: [], cursor: "0" }); }, binding);
  await assert.rejects(client.events.watch("notifications", { intervalMs: 49 }).next(), (error: any) => error.code === "BAD_VALUE");
  await assert.rejects(client.events.watch("notifications", { intervalMs: 30001 }).next(), (error: any) => error.code === "BAD_VALUE");
  assert.equal(calls, 0);
});

test("failed job receipts preserve uncertain effect diagnostics", async () => {
  const client = createJobsClient(async () => ok({ run: { id: jobId, operation: "summarize", state: "FAILED", attempt: 1, error: "CANCELLATION_UNCONFIRMED" } }), binding);
  assert.deepEqual(await client.status(jobId), { id: jobId, status: "FAILED", error: "CANCELLATION_UNCONFIRMED" });
});
