import { test } from "node:test";
import assert from "node:assert/strict";
import { connect } from "./transport.ts";
// /read, /apply and /status need the contract header; the Rust harness passes the served fingerprint.
const OPTIONS = { contractFingerprint: process.env.AIP_FINGERPRINT };

test("대조: 기존 숫자 응답을 다시 쓰면 2^53+1 대신 이웃 행이 바뀜", async () => {
  const client = connect(process.env.AIP_LEGACY_URL!, process.env.AIP_LEGACY_TOKEN!, fetch, OPTIONS);
  const raw = await client.post("/read", { query: { read: "Item", select: ["id"], filter: [{ field: "id", op: "eq", value: "9007199254740993" }] } });
  assert.equal(raw.ok, true);
  assert.equal(String(raw.rows[0].id), "9007199254740992");
  const applied = await client.apply({ apply: "Item.read", target: { ids: [String(raw.rows[0].id)] } }, { key: "legacy-loss" });
  assert.equal(applied.ok, true);
  assert.deepEqual(applied.changed, [9007199254740992]);
});
