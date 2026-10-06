import { test } from "node:test";
import assert from "node:assert/strict";
import { connect, WriteUnsettled } from "./transport.ts";
import net from "node:net";

const URL = process.env.AIP_URL!;
const T1 = process.env.AIP_TOKEN_1!;
const T2 = process.env.AIP_TOKEN_2!;
const alarms = { read: "MemberAlarm", select: ["id", "isChecked"], sort: [{ field: "id" }] };
const list = { read: "Recruitment", select: ["id", "internalNote"], sort: [{ field: "id" }] };

test("쓰기 태그로 캐시 무효화가 실제 서버 응답에서 동작", async () => {
  const c = connect(URL, T1);
  const a1 = await c.read(alarms);
  assert.equal(a1.cached, false);
  assert.deepEqual(a1.rows.slice(0, 2), [{ id: 1, isChecked: false }, { id: 2, isChecked: false }]);
  assert.equal((await c.read(alarms)).cached, true);
  const l1 = await c.read(list);
  const w = await c.apply({ apply: "MemberAlarm.read", target: { ids: ["1"] } });
  assert.equal(w.ok, true);
  assert.deepEqual(w.tags, ["MemberAlarm"]);
  const a2 = await c.read(alarms);
  assert.equal(a2.cached, false);
  assert.deepEqual(a2.rows.slice(0, 2), [{ id: 1, isChecked: true }, { id: 2, isChecked: false }]);
  assert.equal((await c.read(list)).cached, false, "now 의존 모집 목록은 시간 경계를 증명하기 전까지 캐시하지 않음");
  assert.ok(l1.rows.length > 0);
});

test("쓰기 응답 유실: 상태 확인으로 복구, 두 번 실행되지 않음", async () => {
  const c = connect(URL, T1);
  await c.read(list);
  const key = "approve-300";
  const r = await c.apply({ apply: "Apply.approve", target: { ids: ["300"] } }, { key, dropResponse: true });
  assert.equal(r.ok, true);
  assert.equal(r.recovered, true);
  assert.deepEqual(r.changed, [300]);
  // 같은 키로 다시 보내면 저장된 결과(재실행 없음)
  const again = await c.apply({ apply: "Apply.approve", target: { ids: ["300"] } }, { key });
  assert.equal(again.replayed, true);
  // 새 키로 보내면 실제로 다시 실행되어 이미 승인 상태라 거부 → 처음 한 번만 실행됐다는 뜻
  const fresh = await c.apply({ apply: "Apply.approve", target: { ids: ["300"] } });
  assert.equal(fresh.code, "INVALID_STATE");
  // 승인은 ClubMember를 바꾸므로 모집 목록(internalNote 정책이 ClubMember를 읽음)은 다시 읽는다
  assert.equal((await c.read(list)).cached, false);
});

test("같은 키에 다른 요청은 거부, 없는 키 상태는 NOT_FOUND", async () => {
  const c = connect(URL, T1);
  await c.apply({ apply: "MemberAlarm.read", target: { ids: ["2"] } }, { key: "k-x" });
  const bad = await c.apply({ apply: "MemberAlarm.read", target: { ids: ["1"] } }, { key: "k-x" });
  assert.equal(bad.code, "IDEMPOTENCY_MISMATCH");
  assert.equal((await c.post("/status", { key: "never-sent", request: {} })).code, "NOT_FOUND");
});

test("키는 actor별: 다른 사용자가 같은 키를 써도 남의 결과를 받지 않음", async () => {
  const c2 = connect(URL, T2);
  const r = await c2.post("/status", { key: "approve-300", request: { apply: "Apply.approve", target: { ids: ["300"] } } });
  assert.equal(r.code, "NOT_FOUND");
});

test("키 불일치 응답이 유실돼도 이전 요청의 성공으로 복구하지 않음(r7 R7-01)", async () => {
  const c = connect(URL, T1);
  await c.apply({ apply: "MemberAlarm.read", target: { ids: ["3"] } }, { key: "k-y" });
  const r = await c.apply({ apply: "MemberAlarm.read", target: { ids: ["4"] } }, { key: "k-y", dropResponse: true });
  assert.equal(r.code, "IDEMPOTENCY_MISMATCH");
  assert.equal((await c.post("/status", { key: "k-y", request: { apply: "MemberAlarm.read", target: { ids: ["4"] } } })).code, "IDEMPOTENCY_MISMATCH");
});

test("재요청까지 통신 실패하면 미확정으로 남고, 복구 뒤 같은 키로 확정(r7 R7-02·R7-03)", async () => {
  let down = false;
  const flaky: typeof fetch = (u, o) => (down && String(u).endsWith("/apply") ? Promise.reject(new TypeError("network")) : fetch(u, o));
  const c = connect(URL, T1, flaky);
  down = true;
  await assert.rejects(c.apply({ apply: "MemberAlarm.read", target: { ids: ["5"] } }, { key: "k-z" }), WriteUnsettled);
  assert.deepEqual(c.pending(), ["k-z"]);
  down = false;
  const before = await c.read(alarms);
  assert.equal(before.stored, false, "미확정 동안 캐시에 저장하지 않음");
  const settled = await c.retryPending();
  assert.equal(settled[0].ok, true);
  assert.deepEqual(c.pending(), []);
  assert.equal((await c.read(alarms)).stored, true);
});

function raw(text: string): Promise<string> {
  const { hostname, port } = new globalThis.URL(URL);
  return new Promise((resolve) => {
    const s = net.connect(Number(port), hostname, () => s.end(text));
    let buf = "";
    s.on("data", (d) => (buf += d));
    s.on("close", () => resolve(buf));
    s.on("error", () => resolve(buf));
  });
}

test("HTTP 경계: 초과 본문·잘못된 길이·GET·잘못된 JSON은 처리하지 않음(r7 R7-04·R7-05)", async () => {
  const body = JSON.stringify({ request: { apply: "MemberAlarm.read", target: { ids: ["6"] } }, key: "big" });
  const big = await raw(`POST /apply HTTP/1.1\r\nauthorization: Bearer ${T1}\r\ncontent-length: ${(1 << 20) + 1}\r\n\r\n${body}`);
  assert.match(big, /PAYLOAD_TOO_LARGE/);
  const dup = await raw(`POST /apply HTTP/1.1\r\nauthorization: Bearer ${T1}\r\ncontent-length: x\r\ncontent-length: ${body.length}\r\n\r\n${body}`);
  assert.match(dup, /BAD_REQUEST/);
  const get = await raw(`GET /apply HTTP/1.1\r\nauthorization: Bearer ${T1}\r\ncontent-length: ${body.length}\r\n\r\n${body}`);
  assert.match(get, /BAD_REQUEST/);
  const badJson = await raw(`POST /status HTTP/1.1\r\ncontent-length: 7\r\n\r\n{"key":`);
  assert.match(badJson, /JSON/);
  // 위 요청들은 알림 6을 바꾸지 않았다
  const c = connect(URL, T1);
  const rows = (await c.read(alarms)).rows as { id: number; isChecked: boolean }[];
  assert.equal(rows.find((r) => r.id === 6)?.isChecked, false);
});
