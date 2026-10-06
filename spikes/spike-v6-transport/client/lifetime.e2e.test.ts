import { test } from "node:test";
import assert from "node:assert/strict";
import { connect } from "./transport.ts";

const URL = process.env.AIP_URL!;
const TOKEN = process.env.AIP_TOKEN!;
const EXPIRING_TOKEN = process.env.AIP_EXPIRING_TOKEN!;
const alarms = { read: "MemberAlarm", select: ["id", "isChecked"], sort: [{ field: "id" }] };
const recruitment = { read: "Recruitment", select: ["id"], sort: [{ field: "id" }] };
const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

// Run first: Rust issues this token immediately before starting the Node test process.
test("만료 뒤에는 캐시 hit로 토큰 만료를 숨기지 않음", async () => {
  const expiring = connect(URL, EXPIRING_TOKEN);
  const first = await expiring.read(alarms);
  assert.equal(first.cached, false);
  assert.equal(first.stored, true);
  await pause(2100);
  await assert.rejects(expiring.read(alarms), (error: any) => error.code === "TOKEN_EXPIRED");
});

test("알림 캐시 hit 뒤 다른 연결의 쓰기는 최대 수명 안에 반영", async () => {
  const reader = connect(URL, TOKEN);
  const writer = connect(URL, TOKEN);
  const first = await reader.read(alarms);
  assert.equal(first.stored, true);
  assert.equal((await reader.read(alarms)).cached, true);

  const written = await writer.post("/apply", {
    request: { apply: "MemberAlarm.read", target: { ids: ["1"] } },
    key: "v9-lifetime-alarm-1",
  });
  assert.equal(written.ok, true);
  assert.deepEqual(written.tags, ["MemberAlarm"]);

  await pause(1100);
  const refreshed = await reader.read(alarms);
  assert.equal(refreshed.cached, false);
  assert.equal(refreshed.rows.find((row: any) => row.id === 1).isChecked, true);
});

test("시간 의존 모집 조회는 저장하지 않고 현재 마감 상태를 다시 계산", async () => {
  const client = connect(URL, TOKEN);
  const first = await client.read(recruitment);
  assert.deepEqual(first.rows, [{ id: 100 }]);
  assert.equal(first.stored, false);

  const second = await client.read(recruitment);
  assert.deepEqual(second.rows, [{ id: 100 }]);
  assert.equal(second.cached, false);
  assert.equal(second.stored, false);
});

test("다른 연결의 권한 회수는 유효 수명 뒤 서버 정책을 다시 적용", async () => {
  const reader = connect(URL, TOKEN);
  const writer = connect(URL, TOKEN);
  const query = { read: "ManagerNote", select: ["id", "note"] };
  assert.deepEqual((await reader.read(query)).rows, [{ id: 1, note: "secret" }]);
  const response = await writer.apply({ apply: "Permission.revoke", target: { ids: ["1"] } }, { key: "revoke" });
  assert.equal(response.ok, true);
  assert.deepEqual(response.tags, ["Permission"]);
  // 현재 구조의 지연을 명시적으로 측정한다. 다른 연결의 태그는 reader에게 통보되지 않는다.
  assert.equal((await reader.read(query)).cached, true);
  await pause(1100);
  const after = await reader.read(query);
  assert.equal(after.cached, false);
  assert.deepEqual(after.rows, []);
});
