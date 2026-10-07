import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTyped, connectTypedExtensions } from "./typed.ts";

const fingerprint = "a".repeat(64);
const binding = {
  fingerprint,
  readDescriptors: {
    Person: {
      root: true,
      maxRows: 10,
      fields: {
        email: { type: "Email" },
        date: { type: "Date" },
      },
      traverse: {},
    },
  },
} as any;

function clientFor(row: unknown) {
  return connectTyped("https://api.example.test", null, binding, async (input) => {
    const url = String(input);
    const body = url.endsWith("/session")
      ? { ok: true, principal: { actorId: null }, remainingMs: null }
      : { ok: true, contractFingerprint: fingerprint, deps: [], rows: [row], maxAgeMs: 0 };
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  });
}

const decimalBinding = {
  fingerprint,
  readDescriptors: {
    Invoice: {
      root: true,
      maxRows: 10,
      fields: { amount: { type: "Decimal<5,2>" } },
      traverse: {},
    },
  },
} as any;

function decimalClientFor(amount: unknown) {
  return connectTyped("https://api.example.test", null, decimalBinding, async (input) => {
    const url = String(input);
    const body = url.endsWith("/session")
      ? { ok: true, principal: { actorId: null }, remainingMs: null }
      : { ok: true, contractFingerprint: fingerprint, deps: [], rows: [{ amount }], maxAgeMs: 0 };
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  });
}

test("typed read accepts canonical Email and Date scalar output", async () => {
  const client = clientFor({ email: "person@example.test", date: "2024-02-29" });
  const result = await client.read({ read: "Person", select: ["email", "date"] } as any);
  assert.deepEqual(result.rows, [{ email: "person@example.test", date: "2024-02-29" }]);
});

test("typed read rejects invalid Email and Date scalar output", async () => {
  for (const row of [
    { email: "not-an-email", date: "2024-02-29" },
    { email: "person@sub@@example.test", date: "2024-02-29" },
    { email: "person@example.test ", date: "2024-02-29" },
    { email: "person\u0085@example.test", date: "2024-02-29" },
    { email: "person@example.test", date: "0000-01-01" },
    { email: "person@example.test", date: "2026-02-29" },
    { email: "person@example.test", date: "2026-2-1" },
    { email: "person@example.test", date: "2024-02-29\n" },
    { email: "person@example.test", date: "2024-02-29\r\n" },
  ]) {
    const client = clientFor(row);
    await assert.rejects(client.read({ read: "Person", select: ["email", "date"] } as any), (error: any) => error.code === "PROTOCOL_ERROR", JSON.stringify(row));
  }
});

test("typed read accepts exact decimal strings and rejects values outside Decimal(p,s)", async () => {
  const client = decimalClientFor("999.99");
  assert.deepEqual((await client.read({ read: "Invoice", select: ["amount"] } as any)).rows, [{ amount: "999.99" }]);

  for (const invalid of ["1000.00", "1.001", "NaN", "Infinity", "1e3", "+1", "01.00", "1.", ".5", "12.30\n", "12.30\r\n", 12.5]) {
    const client = decimalClientFor(invalid);
    await assert.rejects(client.read({ read: "Invoice", select: ["amount"] } as any), (error: any) => error.code === "PROTOCOL_ERROR");
  }
});

test("typed extension output uses the same exact Decimal(p,s) decoder as read rows", async () => {
  const extensions = {
    "Billing.quote": {
      input: {},
      output: { amount: { type: "Decimal<5,2>", nullable: false } },
    },
  };
  const clientForAmount = (amount: unknown) => connectTypedExtensions("https://api.example.test", null, {
    fingerprint,
    idWire: "safe-number-v13",
    readDescriptors: binding.readDescriptors,
    extensions,
  } as any, async (input) => {
    const url = String(input);
    const body = url.endsWith("/session")
      ? { ok: true, principal: { actorId: null }, remainingMs: null }
      : { ok: true, contractFingerprint: fingerprint, output: { amount } };
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  });

  assert.deepEqual(await (clientForAmount("12.30") as any).extension("Billing.quote", {}), { amount: "12.30" });
  await assert.rejects((clientForAmount("1.234") as any).extension("Billing.quote", {}), (error: any) => error.code === "PROTOCOL_ERROR");
});
