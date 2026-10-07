import { test } from "node:test";
import assert from "node:assert/strict";
import { connect } from "../../../product/sdk/index.ts";

const fingerprint = "a".repeat(64);

function clientFor(rows: unknown[]) {
  const binding = {
    fingerprint,
    idWire: "legacy",
    readDescriptors: {
      Post: {
        root: true,
        maxRows: 10,
        fields: { id: { type: "Id", nullable: false }, title: { type: "Text", nullable: false } },
        traverse: { author: { target: "Comment", select: ["body"] } },
        traverseMany: { comments: { target: "Comment", select: ["id", "body"], maxLimit: 3 } },
        maxOffset: 20,
        cursor: true,
      },
      Comment: {
        root: false,
        maxRows: 0,
        fields: { id: { type: "Id", nullable: false }, body: { type: "Text", nullable: false } },
        traverse: {},
      },
    },
  } as any;
  return connect("https://api.example.test", null, binding, async (input) => {
    const url = String(input);
    const body = url.endsWith("/session")
      ? { ok: true, principal: { actorId: null }, remainingMs: null }
      : { ok: true, contractFingerprint: fingerprint, deps: [], rows, maxAgeMs: 0 };
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  });
}

const selection = ["id", { author: { select: ["body"] } }, { comments: { select: ["id", "body"], limit: 2 } }];

test("typed read accepts 1:N arrays within selected fields and requested limit", async () => {
  const client = clientFor([
    { id: 1, author: { body: "author" }, comments: [{ id: 2, body: "visible" }] },
    { id: 2, author: null, comments: [] },
  ]);
  const result = await client.read({ read: "Post", select: selection } as any);
  assert.deepEqual(result.rows, [
    { id: 1, author: { body: "author" }, comments: [{ id: 2, body: "visible" }] },
    { id: 2, author: null, comments: [] },
  ]);
});

test("typed read rejects invalid 1:N child shapes, fields, and limits", async () => {
  const invalidRows = [
    { id: 1, author: null, comments: null },
    { id: 1, author: null, comments: [{ id: 2, body: "visible", secret: "hidden" }] },
    { id: 1, author: null, comments: [{ id: "bad", body: "visible" }] },
    { id: 1, author: null, comments: "not-an-array" },
    { id: 1, author: null, comments: [{ id: 2, body: 7 }] },
    { id: 1, author: null, comments: [{ id: 2, body: "a" }, { id: 3, body: "b" }, { id: 4, body: "c" }] },
    { id: 1, author: null, comments: [{ id: 2, body: "visible" }], unknown: true },
  ];
  for (const row of invalidRows) {
    await assert.rejects(clientFor([row]).read({ read: "Post", select: selection } as any), (error: any) => error.code === "PROTOCOL_ERROR", JSON.stringify(row));
  }
  await assert.rejects(
    clientFor([{ id: 1, comments: [] }]).read({ read: "Post", select: ["id", { comments: { select: ["id"], limit: 4 } }] } as any),
    (error: any) => error.code === "PROTOCOL_ERROR",
  );
  await assert.rejects(
    clientFor([{ id: 1, comments: [] }]).read({ read: "Post", select: ["id", { comments: { select: ["secret"] } }] } as any),
    (error: any) => error.code === "PROTOCOL_ERROR",
  );
  await assert.rejects(
    clientFor([{ id: 1, comments: [] }]).read({ read: "Post", select: ["id", { comments: { select: ["id"], unknown: true } }] } as any),
    (error: any) => error.code === "PROTOCOL_ERROR",
  );
});

test("typed read retains single traverse object and null semantics", async () => {
  const binding = {
    fingerprint,
    idWire: "legacy",
    readDescriptors: {
      Post: {
        root: true,
        maxRows: 10,
        fields: { id: { type: "Id", nullable: false } },
        traverse: { author: { target: "Comment", select: ["body"] } },
      },
      Comment: { root: false, maxRows: 0, fields: { body: { type: "Text", nullable: false } }, traverse: {} },
    },
  } as any;
  const client = connect("https://api.example.test", null, binding, async (input) => {
    const url = String(input);
    const body = url.endsWith("/session")
      ? { ok: true, principal: { actorId: null }, remainingMs: null }
      : { ok: true, contractFingerprint: fingerprint, deps: [], rows: [{ id: 1, author: null }], maxAgeMs: 0 };
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  });
  assert.deepEqual((await client.read({ read: "Post", select: ["id", { author: { select: ["body"] } }] } as any)).rows, [{ id: 1, author: null }]);
});
