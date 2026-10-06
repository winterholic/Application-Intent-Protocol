import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createCache, ScopeChanged } from "./cache.ts";

// Rust 테스트가 실제 facts·계획에서 만든 deps와 쓰기 태그.
const fx = JSON.parse(readFileSync(new URL("./cache-fixture.json", import.meta.url), "utf8"));
const list = { read: "Recruitment", select: ["id", "internalNote"] };
const alarms = { read: "MemberAlarm", select: ["id"] };

function server() {
  let calls = 0;
  return {
    calls: () => calls,
    fetch: (deps: string[]) => async () => {
      calls++;
      return { rows: [calls], deps, maxAgeMs: 60_000 };
    },
  };
}

test("쓰기 태그와 deps가 겹치면 무효화, 아니면 유지", async () => {
  const c = createCache();
  const s = server();
  c.setActor("1");
  await c.read(list, s.fetch(fx.listDeps));
  await c.read(alarms, s.fetch(fx.alarmDeps));
  assert.equal((await c.read(list, s.fetch(fx.listDeps))).cached, true);
  c.onWrite({ status: "ok", changed: fx.writeTags["MemberAlarm.read"] });
  assert.equal((await c.read(list, s.fetch(fx.listDeps))).cached, true, "알림 읽음은 모집 목록에 영향 없음");
  assert.equal((await c.read(alarms, s.fetch(fx.alarmDeps))).cached, false, "알림 목록은 다시 읽음");
  // 승인(W2)은 Apply·ClubMember를 바꾼다. 모집 목록은 internalNote 정책이 ClubMember를 읽으므로 무효화.
  c.onWrite({ status: "ok", changed: fx.writeTags["Apply.approve"] });
  assert.equal((await c.read(list, s.fetch(fx.listDeps))).cached, false);
});

test("계정 전환·결과 미확정은 전부 버림", async () => {
  const c = createCache();
  const s = server();
  c.setActor("1");
  await c.read(list, s.fetch(fx.listDeps));
  c.setActor("2");
  assert.equal((await c.read(list, s.fetch(fx.listDeps))).cached, false, "다른 actor는 캐시 공유 안 함");
  // 로그아웃하면 앞 사용자의 데이터가 메모리에 남지 않는다(키 분리와 별개).
  c.setActor(null);
  assert.equal(c.size(), 0);
  await c.read(list, s.fetch(fx.listDeps));
  c.onWrite({ status: "unknown" });
  assert.equal(c.size(), 0);
});

test("읽는 도중 관련 쓰기가 있으면 옛 응답 대신 다시 읽은 값을 돌려줌(r5 R5-01)", async () => {
  const c = createCache();
  c.setActor("1");
  let db = "쓰기 전 값";
  let release!: () => void;
  const gate = new Promise<void>((r) => (release = r));
  let first = true;
  const pending = c.read(list, async () => {
    if (first) {
      first = false;
      const v = db;
      await gate;
      return { rows: [v], deps: fx.listDeps, maxAgeMs: 60_000 };
    }
    return { rows: [db], deps: fx.listDeps, maxAgeMs: 60_000 };
  });
  db = "쓰기 뒤 값";
  c.onWrite({ status: "ok", changed: ["Recruitment"] });
  release();
  const r = await pending;
  assert.deepEqual(r.rows, ["쓰기 뒤 값"]);
});

test("읽는 도중 사용자가 바뀌면 옛 사용자 결과를 돌려주지 않음(r5 R5-01)", async () => {
  const c = createCache();
  c.setActor("manager");
  let release!: () => void;
  const gate = new Promise<void>((r) => (release = r));
  const pending = c.read(list, async () => {
    await gate;
    return { rows: ["manager secret"], deps: fx.listDeps, maxAgeMs: 60_000 };
  });
  c.setActor("member");
  release();
  await assert.rejects(pending, ScopeChanged);
  assert.equal(c.size(), 0);
});

test("결과 미확정 쓰기가 풀리기 전에는 새 결과를 저장하지 않음(r5 R5-02)", async () => {
  const c = createCache();
  c.setActor("1");
  let db = "old";
  c.onWrite({ status: "unknown" });
  const before = await c.read(list, async () => ({ rows: [db], deps: fx.listDeps, maxAgeMs: 60_000 }));
  assert.equal(before.stored, false);
  db = "committed";
  c.resolveUnknown();
  const after = await c.read(list, async () => ({ rows: [db], deps: fx.listDeps, maxAgeMs: 60_000 }));
  assert.deepEqual(after.rows, ["committed"]);
});

test("화면이 받은 행을 고쳐도 캐시는 바뀌지 않음(r5 R5-03)", async () => {
  const c = createCache();
  c.setActor("1");
  const r = await c.read(list, async () => ({ rows: [{ id: 1, title: "server" }], deps: fx.listDeps, maxAgeMs: 60_000 }));
  assert.throws(() => ((r.rows[0] as { title: string }).title = "screen edit"));
  const again = await c.read(list, async () => ({ rows: [], deps: [], maxAgeMs: 60_000 }));
  assert.deepEqual(again.rows, [{ id: 1, title: "server" }]);
});
