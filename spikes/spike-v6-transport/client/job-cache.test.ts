import { test } from "node:test";
import assert from "node:assert/strict";
import { connect } from "./transport.ts";

// A read is cached for the server's maxAgeMs and the client's own writes invalidate it by tag. A durable job writes on the server
// where the client cannot know the tags, so a job that has finished must not leave the stale read behind.
const fingerprint = "c".repeat(64);
const query = { read: "VideoRequest", select: ["id"] };
const reply = (body: unknown) => new Response(JSON.stringify(body));

function clientWith(terminal: "SUCCEEDED" | "FAILED" | "RUNNING" = "SUCCEEDED") {
  const reads: number[] = [];
  const fetchImpl: typeof fetch = async (url) => {
    const path = new URL(String(url)).pathname;
    if (path === "/session") return reply({ ok: true, principal: { actorId: "1" }, remainingMs: 60_000 });
    if (path === "/read") {
      reads.push(reads.length);
      return reply({ ok: true, contractFingerprint: fingerprint, rows: [reads.length], deps: ["VideoRequest"], maxAgeMs: 60_000 });
    }
    if (path === "/jobs/status") return reply({ ok: true, contractFingerprint: fingerprint, run: { id: "9d05f3a0-29ef-4efc-9000-4c5e3f3eb10d", operation: "approve", state: terminal, attempt: 1 } });
    if (path === "/jobs/result") return reply({ ok: true, contractFingerprint: fingerprint, id: "9d05f3a0-29ef-4efc-9000-4c5e3f3eb10d", operation: "approve", output: {} });
    return reply({ ok: false, code: "WIRE.NOT_EXPOSED" });
  };
  const client = connect("http://spike.invalid", "token", fetchImpl, { contractFingerprint: fingerprint });
  return { client, reads };
}

test("a read is cached until a job reaches a terminal state, then it is read again", async () => {
  const { client, reads } = clientWith("SUCCEEDED");
  await client.read(query);
  await client.read(query);
  assert.equal(reads.length, 1, "within maxAgeMs the second read is served from the cache");
  await client.callManaged("/jobs/status", { id: "9d05f3a0-29ef-4efc-9000-4c5e3f3eb10d" });
  const fresh = await client.read(query);
  assert.equal(reads.length, 2, "the finished job made the cached read stale");
  assert.deepEqual(fresh.rows, [2]);
});

test("fetching a job result also drops cached reads", async () => {
  const { client, reads } = clientWith("SUCCEEDED");
  await client.read(query);
  await client.callManaged("/jobs/result", { id: "9d05f3a0-29ef-4efc-9000-4c5e3f3eb10d" });
  await client.read(query);
  assert.equal(reads.length, 2);
});

test("a job that is still running leaves the cache alone", async () => {
  const { client, reads } = clientWith("RUNNING");
  await client.read(query);
  await client.callManaged("/jobs/status", { id: "9d05f3a0-29ef-4efc-9000-4c5e3f3eb10d" });
  await client.read(query);
  assert.equal(reads.length, 1);
});

test("a failed job may have written before it failed, so it drops cached reads too", async () => {
  const { client, reads } = clientWith("FAILED");
  await client.read(query);
  await client.callManaged("/jobs/status", { id: "9d05f3a0-29ef-4efc-9000-4c5e3f3eb10d" });
  await client.read(query);
  assert.equal(reads.length, 2);
});
