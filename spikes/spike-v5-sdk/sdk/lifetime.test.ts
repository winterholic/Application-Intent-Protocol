import { test } from "node:test";
import assert from "node:assert/strict";
import { createCache } from "./cache.ts";

const query = { read: "MemberAlarm", select: ["id"] };

test("캐시 수명 경계에서는 서버를 다시 호출", async () => {
  let now = 0;
  let calls = 0;
  const c = createCache({ now: () => now });
  const fetcher = async () => ({ rows: [++calls], deps: ["MemberAlarm"], maxAgeMs: 100 });
  await c.read(query, fetcher);
  now = 99;
  assert.equal((await c.read(query, fetcher)).cached, true);
  now = 100;
  assert.deepEqual((await c.read(query, fetcher)).rows, [2]);
});

test("왕복시간이 수명을 넘긴 응답은 저장하지 않음", async () => {
  let now = 0;
  const c = createCache({ now: () => now });
  const fetcher = async () => {
    now += 101;
    return { rows: [1], deps: ["MemberAlarm"], maxAgeMs: 100 };
  };
  assert.equal((await c.read(query, fetcher)).stored, false);
  assert.equal(c.size(), 0);
  assert.equal((await c.read(query, fetcher)).cached, false);
});

test("수명 정보 누락·음수·무한대·문자열은 캐시 권한을 부여하지 않음", async () => {
  for (const maxAgeMs of [undefined, -1, Infinity, NaN, "100"]) {
    const c = createCache();
    const fetcher = async () => ({ rows: [1], deps: ["MemberAlarm"], maxAgeMs } as any);
    assert.equal((await c.read(query, fetcher)).stored, false);
    assert.equal(c.size(), 0);
  }
});

test("벽시계 변경은 단조 시계로 측정하는 수명을 늘리지 않음", async ({ mock }) => {
  let now = 0;
  const c = createCache({ now: () => now });
  let calls = 0;
  const fetcher = async () => ({ rows: [++calls], deps: ["MemberAlarm"], maxAgeMs: 10 });
  await c.read(query, fetcher);
  mock.method(Date, "now", () => 0);
  now = 9;
  assert.equal((await c.read(query, fetcher)).cached, true);
  now = 10;
  assert.equal((await c.read(query, fetcher)).cached, false);
});
