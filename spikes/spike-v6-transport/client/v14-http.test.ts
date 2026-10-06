import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTypedApply } from "./typed.ts";
import { WriteUnsettled } from "./transport.ts";
import { contract as decimal } from "./generated-v14-string.ts";
import { contract as numeric } from "./generated-v14-safe.ts";
import { contract as oldDecimal } from "./generated-v14-string-stale.ts";
import { contract as oldNumeric } from "./generated-v14-safe-stale.ts";

const url = process.env.AIP_V14_URL!;
const token = process.env.AIP_V14_TOKEN!;
const stringWire = process.env.AIP_V14_WIRE === "string";
const binding = stringWire ? decimal : numeric;
const id = (n: number) => stringWire ? String(n) : n;
const aip = connectTypedApply(url, token, binding);

test("V14: 공개 쓰기 상한만 달라진 이전 binding도 첫 쓰기를 DB 전에 거부", async () => {
  const old = connectTypedApply(url, token, stringWire ? oldDecimal : oldNumeric);
  const rejected = await old.apply({ apply: "WriteOnly.mark", target: { ids: [id(7)] } }, { key: "stale" });
  assert.equal(rejected.code, "CONTRACT_MISMATCH");
  assert.deepEqual(old.pending(), []);
  const marked = await aip.apply({ apply: "WriteOnly.mark", target: { ids: [id(7)] } }, { key: "write-only" });
  assert.deepEqual(marked.changed, [id(7)]);
});

test("V14: 읽은 Id로 apply·반복 unchanged·멱등 replay와 캐시 무효화", async () => {
  const query = { read: "Inbox", select: ["id", "checked"], filter: [{ field: "id", op: "eq", value: id(42) }] };
  const before = await aip.read(query);
  assert.equal(before.rows[0].checked, false);
  assert.equal((await aip.read(query)).cached, true);
  const request = { apply: "Inbox.mark", target: { ids: [before.rows[0].id] } };
  const marked = await aip.apply(request, { key: "ids" });
  assert.deepEqual(marked.changed, [before.rows[0].id]);
  const after = await aip.read(query);
  assert.equal(after.cached, false);
  assert.equal(after.rows[0].checked, true);
  assert.deepEqual((await aip.apply(request, { key: "repeat" })).unchanged, [id(42)]);
  const replay = await aip.apply(request, { key: "ids" });
  assert.equal(replay.replayed, true);
  assert.deepEqual(replay.changed, [id(42)]);
});

test("V14: 허용된 where와 where 전용 action, raw 계약·권한 거부", async () => {
  const marked = await aip.apply({ apply: "Inbox.mark", target: { where: [{ field: "title", op: "eq", value: "where" }] } });
  assert.deepEqual(marked.changed, [id(43)]);
  assert.deepEqual((await aip.apply({ apply: "WhereOnly.mark", target: { where: [{ field: "title", op: "eq", value: "one" }] } })).changed, [id(9)]);
  const forbidden = await aip.post("/apply", { key: "bad-filter", request: { apply: "Inbox.mark", target: { where: [{ field: "count", op: "gte", value: 17 }] } } });
  assert.equal(forbidden.code, "FILTER_NOT_ALLOWED");
  const other = connectTypedApply(url, process.env.AIP_V14_OTHER!, binding);
  assert.equal((await other.apply({ apply: "Inbox.mark", target: { ids: [id(44)] } })).ok, false);
});

test("V14: 실제 응답 유실은 동일 키 재생으로 복구하고 Id wire 유지", async () => {
  const outcome = await aip.apply({ apply: "Inbox.mark", target: { ids: [id(44)] } }, { key: "lost", dropResponse: true });
  assert.equal(outcome.recovered, true);
  assert.equal(outcome.replayed, true);
  assert.deepEqual(outcome.changed, [id(44)]);
  if (stringWire) {
    const rows = await aip.read({ read: "Inbox", select: ["id"], filter: [{ field: "id", op: "eq", value: "9007199254740993" }] });
    const marked = await aip.apply({ apply: "Inbox.mark", target: { ids: [rows.rows[0].id] } });
    assert.deepEqual(marked.changed, ["9007199254740993"]);
  }
});

test("V14: 실제 커밋 응답의 Id를 손상시켜도 pending 보존 후 원본 replay로 복구", async () => {
  let corrupt = true;
  const fetcher: typeof fetch = async (input, init) => {
    const res = await fetch(input, init);
    const body = await res.json();
    if (corrupt && new URL(String(input)).pathname === "/apply" && body.ok) {
      body.changed = [stringWire ? 45 : "45"];
    }
    return new Response(JSON.stringify(body));
  };
  const client = connectTypedApply(url, token, binding, fetcher);
  await assert.rejects(client.apply({ apply: "Inbox.mark", target: { ids: [id(45)] } }, { key: "malformed", retries: 0 }), WriteUnsettled);
  assert.deepEqual(client.pending(), ["malformed"]);
  const committed = await client.read({ read: "Inbox", select: ["checked"], filter: [{ field: "id", op: "eq", value: id(45) }] });
  assert.equal(committed.rows[0].checked, true);
  assert.equal(committed.stored, false);
  corrupt = false;
  const restored = await client.retryPending();
  assert.equal(restored[0].replayed, true);
  assert.deepEqual(restored[0].changed, [id(45)]);
  assert.deepEqual(client.pending(), []);
});

test("V14: JS binding의 Id mode는 필수이며 연결 뒤 변경해도 검사를 바꾸지 않음", async () => {
  for (const idWire of [undefined, null, "", "nonsense"]) {
    assert.throws(() => connectTypedApply(url, token, { ...binding, idWire } as any), (e: any) => e.code === "PROTOCOL_ERROR");
  }
  const mutable = { ...binding };
  const client = connectTypedApply(url, token, mutable);
  mutable.idWire = stringWire ? "safe-number-v13" : "decimal-string-v13";
  assert.deepEqual((await client.apply({ apply: "Inbox.mark", target: { ids: [id(42)] } })).unchanged, [id(42)]);
});
