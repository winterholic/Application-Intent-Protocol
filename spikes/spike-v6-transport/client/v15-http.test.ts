import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTypedApply } from "./typed.ts";
import { contract as decimal } from "./generated-v15-string.ts";
import { contract as numeric } from "./generated-v15-safe.ts";
import { contract as oldDecimal } from "./generated-v15-string-stale.ts";
import { contract as oldNumeric } from "./generated-v15-safe-stale.ts";

const url = process.env.AIP_V15_URL!;
const token = process.env.AIP_V15_TOKEN!;
const stringWire = process.env.AIP_V15_WIRE === "string";
const binding = stringWire ? decimal : numeric;
const aip = connectTypedApply(url, token, binding);
const id = (n: number) => stringWire ? String(n) : n;

test("V15: declared Url/Enum/Time read and where share parameter meaning and policy", async () => {
  const cases = [
    ["link", "https://example.test/a?x=';--", 11],
    ["phase", "DONE", 12],
    ["date", new Date("2026-10-04T00:00:00.123Z").toISOString(), 13],
    ["date", process.env.AIP_V15_PYTHON_TIME!, 14],
    ["date", "2026-10-04T00:00:00.123456789Z", 15],
  ] as const;
  for (const [field, value, rowId] of cases) {
    const query = { read: "Event", select: ["id", "checked"], filter: [{ field, op: "eq", value }] };
    const before = await aip.read(query);
    assert.deepEqual(before.rows.map(row => row.id), [id(rowId)]);
    assert.equal(before.rows[0].checked, false);
    const result = await aip.apply({ apply: "Event.mark", target: { where: [{ field, op: "eq", value }] } });
    assert.deepEqual(result.changed, [id(rowId)]);
    const after = await aip.read(query);
    assert.equal(after.cached, false);
    assert.equal(after.rows[0].checked, true);
  }
  const other = connectTypedApply(url, process.env.AIP_V15_OTHER!, binding);
  const hidden = await other.read({ read: "Event", select: ["id"], filter: [{ field: "link", op: "eq", value: "https://example.test/a?x=';--" }] });
  assert.deepEqual(hidden.rows.map(row => row.id), [id(99)]);
  for (const [field, value, rowId] of cases) {
    const result = await other.apply({ apply: "Event.mark", target: { where: [{ field, op: "eq", value }, { field: "id", op: "eq", value: id(rowId) }] } });
    assert.deepEqual(result.changed, []);
    assert.deepEqual(result.unchanged, []);
  }
});

test("V15: invalid values are BAD_VALUE, never success or unsettled, and leave rows unchanged", async () => {
  const snapshot = async () => (await aip.post("/read", { query: { read: "Event", select: ["id", "checked"], sort: [{ field: "id", dir: "asc" }] } })).rows;
  const before = await snapshot();
  for (const value of ["2026-02-30T00:00:00Z", "2026-10-04T00:00:00+24:00", "2026-10-04T00:00:00+00:60", "0000-01-01T00:00:00Z"]) {
    await assert.rejects(aip.read({ read: "Event", select: ["id"], filter: [{ field: "date", op: "eq", value }] }), (e: any) => e.code === "BAD_VALUE");
    const result = await aip.apply({ apply: "Event.mark", target: { where: [{ field: "date", op: "eq", value }] } });
    assert.equal(result.ok, false);
    if (!result.ok) assert.equal(result.code, "BAD_VALUE");
    assert.deepEqual(aip.pending(), []);
    assert.deepEqual(await snapshot(), before);
  }
  for (const [field, value] of [["phase", "UNKNOWN"], ["phase", 1], ["link", true], ["date", null]] as const) {
    const read = await aip.post("/read", { query: { read: "Event", select: ["id"], filter: [{ field, op: "eq", value }] } });
    assert.equal(read.code, "BAD_VALUE");
    const write = await aip.post("/apply", { key: `bad-${field}-${String(value)}`, request: { apply: "Event.mark", target: { where: [{ field, op: "eq", value }] } } });
    assert.equal(write.code, "BAD_VALUE");
    assert.deepEqual(await snapshot(), before);
  }
  const notAllowed = await aip.post("/apply", { key: "not-eq", request: { apply: "Event.mark", target: { where: [{ field: "date", op: "gte", value: "2026-10-04T00:00:00Z" }] } } });
  assert.equal(notAllowed.code, "FILTER_NOT_ALLOWED");
  const unchanged = await aip.read({ read: "Event", select: ["id", "checked"], filter: [{ field: "id", op: "eq", value: id(16) }] });
  assert.equal(unchanged.rows[0].checked, false);
});

test("V15: NUL cannot reach PostgreSQL text comparison or leave a pending write", async () => {
  for (const field of ["title", "link"] as const) {
    const value = "bad\u0000text";
    await assert.rejects(aip.read({ read: "Event", select: ["id"], filter: [{ field, op: "eq", value }] }), (e: any) => e.code === "BAD_VALUE");
    const result = await aip.apply({ apply: "Event.mark", target: { where: [{ field, op: "eq", value }] } }, { retries: 0 });
    assert.equal(result.ok, false);
    if (!result.ok) assert.equal(result.code, "BAD_VALUE");
    assert.deepEqual(aip.pending(), []);
  }
});

test("V15: previous unsupported where projection is rejected before first write", async () => {
  const old = connectTypedApply(url, token, stringWire ? oldDecimal : oldNumeric);
  const rejected = await old.apply({ apply: "Event.mark", target: { ids: [id(16)] } });
  assert.equal(rejected.ok, false);
  if (!rejected.ok) assert.equal(rejected.code, "CONTRACT_MISMATCH");
  assert.deepEqual(old.pending(), []);
  assert.equal((await aip.read({ read: "Event", select: ["checked"], filter: [{ field: "id", op: "eq", value: id(16) }] })).rows[0].checked, false);
});

test("V15: prefix matches literal Unicode and wildcard characters within row policy", async () => {
  for (const [value, expected] of [["서울", [14, 15]], ["100%", [13, 16]], ["100%_", [13, 16]], ["100%_\\'", [13, 16]], ["100", [11, 13, 16]], ["", [11, 12, 13, 14, 15, 16]], ["missing", []]] as const) {
    const result = await aip.read({ read: "Event", select: ["id"], filter: [{ field: "title", op: "prefix", value }], sort: [{ field: "id", dir: "asc" }] });
    assert.deepEqual(result.rows.map(row => row.id), expected.map(id));
  }
  const forbidden = await aip.post("/apply", { key: "prefix-write", request: { apply: "Event.mark", target: { where: [{ field: "title", op: "prefix", value: "서울" }] } } });
  assert.equal(forbidden.code, "FILTER_NOT_ALLOWED");
});
