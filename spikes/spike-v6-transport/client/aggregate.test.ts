import { test } from "node:test";
import assert from "node:assert/strict";
import { connect } from "../../../product/sdk/index.ts";

const fingerprint = "b".repeat(64);

function clientFor(rows: unknown[], seen: unknown[] = []) {
  const binding = {
    fingerprint,
    idWire: "legacy",
    readDescriptors: {
      User: { root: true, maxRows: 10, fields: { id: { type: "Id", nullable: false } }, traverse: {} },
    },
  } as any;
  return connect("https://api.example.test", null, binding, async (input, init) => {
    const url = String(input);
    let body: unknown;
    if (url.endsWith("/session")) body = { ok: true, principal: { actorId: null }, remainingMs: null };
    else {
      seen.push(JSON.parse(String((init as RequestInit).body)));
      body = { ok: true, contractFingerprint: fingerprint, deps: [], rows, maxAgeMs: 0 };
    }
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  });
}

test("aggregate sends the standalone query and returns the one scalar row", async () => {
  const seen: any[] = [];
  const client = clientFor([42], seen);
  const total = await client.aggregate({ aggregate: "User.totalCount", filter: [{ field: "is_deleted", op: "eq", value: false }] });
  assert.equal(total, 42);
  assert.deepEqual(seen[0].query, { aggregate: "User.totalCount", input: {}, filter: [{ field: "is_deleted", op: "eq", value: false }] });
});

test("aggregate sends no filter key unless the caller gives one: a no-caller-filter aggregate refuses even an empty list", async () => {
  const seen: any[] = [];
  const client = clientFor([3], seen);
  await client.aggregate({ aggregate: "User.signupCount", input: { from: "2026-01-01T00:00:00Z", to: "2026-02-01T00:00:00Z" } });
  assert.deepEqual(seen[0].query, { aggregate: "User.signupCount", input: { from: "2026-01-01T00:00:00Z", to: "2026-02-01T00:00:00Z" } });
  assert.equal(Object.hasOwn(seen[0].query, "filter"), false);
});

test("aggregate keeps a null result (an empty sum) and refuses anything but one number or null", async () => {
  assert.equal(await clientFor([null]).aggregate({ aggregate: "Video.categoryViews", input: { category: "vlog" } }), null);
  for (const rows of [[], [1, 2], ["7"], [{ value: 1 }]]) {
    await assert.rejects(clientFor(rows).aggregate({ aggregate: "User.totalCount" }), { code: "CLIENT.PROTOCOL_ERROR" });
  }
});

test("aggregate names must be Resource.name before anything is sent", async () => {
  const seen: unknown[] = [];
  const client = clientFor([1], seen);
  for (const name of ["totalCount", "User.", "user.totalCount", "User.total-count", ""]) {
    await assert.rejects(client.aggregate({ aggregate: name }), { code: "WIRE.BAD_VALUE" });
  }
  assert.equal(seen.length, 0);
});
