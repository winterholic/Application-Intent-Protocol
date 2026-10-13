import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTypedApply } from "./typed.ts";
import { WriteUnsettled } from "./transport.ts";
import { contract as decimal } from "./generated-v16-string.ts";
import { contract as numeric } from "./generated-v16-safe.ts";
import { contract as oldDecimal } from "./generated-v16-string-stale.ts";
import { contract as oldNumeric } from "./generated-v16-safe-stale.ts";

const url = process.env.AIP_V16_URL!;
const token = process.env.AIP_V16_TOKEN!;
const stringWire = process.env.AIP_V16_WIRE === "string";
const binding = stringWire ? decimal : numeric;
const id = (n: number) => (stringWire ? String(n) : n) as never;
const aip = connectTypedApply(url, token, binding);

const task = (project: number, title: string, points = 1) =>
  ({ create: "Task", values: { project: id(project), author: id(1), title, points } }) as const;

test("V16: 공개 create와 전이를 한 요청에 묶고 새 id를 받으며 같은 키는 재생한다", async () => {
  const request = { atomic: [task(10, "ts-1", 3), { apply: "Project.archive", target: { ids: [id(10)] } }] } as const;
  const done = await aip.bundle(request, { key: "ts-bundle" });
  assert.equal(done.ok, true, JSON.stringify(done));
  if (!done.ok) return;
  assert.equal(done.created[0].resource, "Task");
  assert.deepEqual(done.applied, [{ changed: [id(10)], unchanged: [] }]);
  assert.deepEqual(done.tags, ["Project", "Task"]);
  const replay = await aip.bundle(request, { key: "ts-bundle" });
  assert.equal(replay.ok && replay.replayed, true);
  assert.deepEqual(replay.ok && replay.created, done.created, "the replay returns the stored created id");
  assert.deepEqual(aip.pending(), []);
  // the same key with another request is refused locally and by the server
  const other = await aip.bundle({ atomic: [task(11, "other")] }, { key: "ts-bundle" });
  assert.equal(other.ok, false);
  assert.equal(!other.ok && other.code, "WIRE.IDEMPOTENCY_MISMATCH");
});

test("V16: 한 연산이 실패하면 묶음 전체가 취소되고 확정 응답이라 pending이 남지 않는다", async () => {
  // project 10 is archived by the first test, so the transition after the create is out of state
  const rolled = await aip.bundle({ atomic: [task(11, "ghost"), { apply: "Project.archive", target: { ids: [id(10)] } }] }, { key: "ts-rollback" });
  assert.equal(rolled.ok, false);
  assert.equal(!rolled.ok && rolled.code, "WIRE.INVALID_STATE");
  const checked = await aip.bundle({ atomic: [task(11, "check-1", 2), task(11, "check-2", 99)] }, { key: "ts-check" });
  assert.equal(!checked.ok && checked.code, "WIRE.CHECK_FAILED", "the commit-time check sees every row the bundle wrote");
  assert.deepEqual(aip.pending(), []);
});

test("V16: 응답이 유실돼도 같은 키로 복구하고 행은 한 번만 만들어진다", async () => {
  const outcome = await aip.bundle({ atomic: [task(12, "lost", 4)] }, { key: "ts-lost", dropResponse: true });
  assert.equal(outcome.ok && outcome.recovered, true);
  assert.equal(outcome.ok && outcome.replayed, true);
  assert.equal(outcome.ok && outcome.created[0].resource, "Task");
});

test("V16: 손상된 커밋 응답은 pending으로 남고 같은 키 재생으로 원본을 받는다", async () => {
  const fetcher: typeof fetch = async (input, init) => {
    const res = await fetch(input, init);
    const body = await res.json();
    if (new URL(String(input)).pathname === "/apply" && body.ok && Array.isArray(body.created)) {
      body.created = [{ resource: "Task", id: stringWire ? 45 : "45" }];
    }
    return new Response(JSON.stringify(body));
  };
  const client = connectTypedApply(url, token, binding, fetcher);
  await assert.rejects(client.bundle({ atomic: [task(13, "malformed")] }, { key: "ts-malformed", retries: 0 }), WriteUnsettled);
  assert.deepEqual(client.pending(), ["ts-malformed"]);
  const healthy = connectTypedApply(url, token, binding);
  const replay = await healthy.bundle({ atomic: [task(13, "malformed")] }, { key: "ts-malformed" });
  assert.equal(replay.ok && replay.replayed, true, "the commit happened once; the key replays its stored result");
});

test("V16: compose는 서버가 읽은 대상 행의 값으로 행을 만들고 같은 키를 재생한다", async () => {
  const request = { compose: "Project", targets: { ids: [id(14)] }, steps: [
    { transition: "archive" },
    { create: "Log", values: { owner: { item: "owner" }, name: { item: "name" } } },
  ] } as const;
  const composed = await aip.compose(request, { key: "ts-compose" });
  assert.equal(composed.ok, true, JSON.stringify(composed));
  assert.deepEqual(composed.ok && composed.applied, [{ changed: [id(14)], unchanged: [] }]);
  assert.equal(composed.ok && composed.created[0].resource, "Log");
  const replay = await aip.compose(request, { key: "ts-compose" });
  assert.equal(replay.ok && replay.replayed, true);
  const refused = await aip.post("/apply", { key: "ts-compose-bad", request: { compose: "Project", targets: { ids: [id(15)] }, steps: [{ create: "Log", values: { owner: { item: "id" }, name: { item: "name" } } }] } });
  assert.equal(refused.code, "WIRE.VALUE_SOURCE_NOT_ALLOWED");
});

test("V16: 호출자가 못 읽는 참조 대상은 없는 행과 같은 MISSING_TARGET이고 크기 상한이 있다", async () => {
  const theirs = await aip.bundle({ atomic: [task(20, "into-theirs")] }, { key: "ts-hidden" });
  const nothing = await aip.bundle({ atomic: [task(424242, "into-nothing")] }, { key: "ts-missing" });
  assert.equal(!theirs.ok && theirs.code, "WIRE.MISSING_TARGET");
  assert.equal(!theirs.ok && !nothing.ok && theirs.msg, nothing.msg);
  const many = Array.from({ length: 21 }, (_, i) => task(15, `many-${i}`)) as unknown as readonly [ReturnType<typeof task>, ...ReturnType<typeof task>[]];
  const capped = await aip.bundle({ atomic: many }, { key: "ts-many" });
  assert.equal(!capped.ok && capped.code, "WIRE.BULK_LIMIT");
});

test("V16: 공개 create 계약이 달라진 이전 binding은 첫 쓰기를 DB 전에 거부한다", async () => {
  const old = connectTypedApply(url, token, stringWire ? oldDecimal : oldNumeric);
  const rejected = await old.bundle({ atomic: [task(15, "stale")] }, { key: "ts-stale" });
  assert.equal(!rejected.ok && rejected.code, "WIRE.CONTRACT_MISMATCH");
  assert.deepEqual(old.pending(), []);
});
