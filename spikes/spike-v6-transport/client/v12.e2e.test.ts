import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTyped } from "./typed.ts";
import { contract } from "./generated-v12.ts";

const URL = process.env.AIP_URL!;
const T1 = process.env.AIP_TOKEN_1!;
const T1_NEW = process.env.AIP_TOKEN_1_NEW!;
const T2 = process.env.AIP_TOKEN_2!;
const alarmQuery = { read: "MemberAlarm", select: ["id", "isChecked"], sort: [{ field: "id", dir: "asc" }] } as const;

function isCode(code: string) {
  return (error: any) => error?.code === code;
}

test("생성 계약 typed connect가 HTTP read/cache/apply/세션과 관계 타입을 잇는다", async () => {
  const client = connectTyped(URL, T1, contract);
  const alarms = await client.read(alarmQuery);
  const first: boolean = alarms.rows[0].isChecked;
  assert.equal(first, false);
  assert.equal(alarms.cached, false);
  assert.equal((await client.read(alarmQuery)).cached, true);

  const written = await client.apply({ apply: "MemberAlarm.read", target: { ids: ["1"] } });
  assert.equal(written.ok, true);
  assert.deepEqual(written.tags, ["MemberAlarm"]);
  const afterWrite = await client.read(alarmQuery);
  assert.equal(afterWrite.cached, false);
  assert.equal(afterWrite.rows[0].isChecked, true);

  const changed = await client.replaceSession(T1_NEW);
  assert.deepEqual(changed, []);
  const afterSession = await client.read(alarmQuery);
  assert.equal(afterSession.cached, false);
  assert.equal(afterSession.rows[0].isChecked, true);

  const cards = await client.read({
    read: "Recruitment",
    select: ["id", "title", { club: { select: ["name", "logo"] } }],
  });
  const cardTitle: string = cards.rows[0].title;
  const clubName: string | undefined = cards.rows[0].club?.name;
  assert.equal(cardTitle, "A 모집");
  assert.equal(clubName, "A");

  const otherActor = connectTyped(URL, T2, contract);
  const otherAlarms = await otherActor.read(alarmQuery);
  assert.deepEqual(otherAlarms.rows, [], "다른 actor의 같은 typed query는 자기 행만 반환");

  const raw = await client.post("/read", { query: { read: "Recruitment", select: ["status"] } });
  assert.equal(raw.ok, false);
  assert.equal(raw.code, "FIELD_NOT_EXPOSED", "타입 SDK를 건너뛴 raw 요청도 서버 정책 검사를 받아야 함");

  const wrongBinding = { ...contract, fingerprint: "0".repeat(64) };
  const wrong = connectTyped(URL, T1, wrongBinding);
  await assert.rejects(wrong.read(alarmQuery), isCode("CONTRACT_MISMATCH"));
  void cardTitle;
  void clubName;
});
