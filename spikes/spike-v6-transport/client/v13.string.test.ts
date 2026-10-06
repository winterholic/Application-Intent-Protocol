import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTyped } from "./typed.ts";
import { contract } from "./generated-v13-string.ts";

const ids=["0","42","9007199254740991","9007199254740992","9007199254740993","999999999999999999","9223372036854775807"];

test("첫 호출이 where 쓰기여도 생성 계약 모드가 다르면 commit 전에 거부", async () => {
  const mismatched = connectTyped(process.env.AIP_SAFE_URL!,process.env.AIP_SAFE_TOKEN!,contract);
  for (const key of ["first-wrong-wire","shared-profile"]) {
    const result = await mismatched.apply({apply:"Item.read",target:{where:[{field:"label",op:"eq",value:"small"}]}},{key});
    assert.equal(result.code,"CONTRACT_MISMATCH");
    assert.equal(result.ok,false);
  }
  assert.deepEqual(mismatched.pending(),[]);
  const correct = connectTyped(process.env.AIP_STRING_URL!,process.env.AIP_STRING_TOKEN!,contract);
  const state = await correct.read({read:"Item",select:["isChecked"],filter:[{field:"id",op:"eq",value:"42"}]});
  assert.equal(state.rows[0].isChecked,false);
});

test("문자열 후보: 루트·Ref·관계 Id를 정확히 읽고 그대로 filter/apply에 사용", async () => {
  const client = connectTyped(process.env.AIP_STRING_URL!,process.env.AIP_STRING_TOKEN!,contract);
  // 다른 모드에서 성공한 같은 요청·키의 결과를 이 모드에서 재생하지 않는다.
  const shared = await client.apply({apply:"Item.read",target:{where:[{field:"label",op:"eq",value:"small"}]}},{key:"shared-profile"});
  assert.equal(shared.ok,true);
  assert.deepEqual(shared.changed,["42"]);
  const all = await client.read({read:"Item",select:["id","member","parent","count","label"],sort:[{field:"id"}]});
  assert.deepEqual(all.rows.map(r=>r.id),ids);
  assert.ok(all.rows.every(r=>r.member==="1" && r.count===17 && typeof r.label==="string"));
  assert.equal(all.rows.find(r=>r.id==="9007199254740993")?.parent,"9007199254740993");
  assert.equal(all.rows.find(r=>r.id==="9223372036854775807")?.parent,null);
  assert.equal((await client.read({read:"Item",select:["id","member","parent","count","label"],sort:[{field:"id"}]})).cached,true);
  const nested = await client.read({read:"Item",select:["id",{parent:{select:["id","title"]}}],sort:[{field:"id"}]});
  assert.equal(nested.rows.find(r=>r.id==="9007199254740993")?.parent?.id,"9007199254740993");
  assert.equal(nested.rows.find(r=>r.id==="0")?.parent,null,"다른 actor 관계는 null");
  for (const row of all.rows) {
    const filtered = await client.read({read:"Item",select:["id"],filter:[{field:"id",op:"eq",value:row.id}]});
    assert.equal(filtered.rows[0].id,row.id);
    const result = await client.apply({apply:"Item.read",target:{ids:[row.id]}},{key:`string:${row.id}`,dropResponse:row.id===ids.at(-1)});
    assert.equal(result.ok,true);
    assert.deepEqual(result.changed,row.id==="42"?[]:[row.id]);
    assert.deepEqual(result.unchanged,row.id==="42"?[row.id]:[]);
    if(row.id===ids.at(-1)) assert.equal(result.recovered,true);
    const replay = await client.apply({apply:"Item.read",target:{ids:[row.id]}},{key:`string:${row.id}`});
    assert.equal(replay.replayed,true);
    assert.deepEqual(replay.changed,row.id==="42"?[]:[row.id]);
    assert.deepEqual(replay.unchanged,row.id==="42"?[row.id]:[]);
  }
  const unchanged = await client.apply({apply:"Item.read",target:{ids:[ids.at(-1)]}},{key:"unchanged-top"});
  assert.deepEqual(unchanged.unchanged,[ids.at(-1)]);
  const whereDone = await client.apply({apply:"Item.read",target:{where:[{field:"label",op:"eq",value:"top"}]}},{key:"where-already-done"});
  assert.deepEqual(whereDone.changed,[]);
  assert.deepEqual(whereDone.unchanged,[]);
});

test("문자열 후보의 형식·권한 검사와 원자성은 raw 요청에도 유지", async () => {
  const client = connectTyped(process.env.AIP_STRING_URL!,process.env.AIP_STRING_TOKEN!,contract);
  for (const value of ["", "00", "01", "-1", "+1", " 1", "1.0", "9223372036854775808",42]) {
    const read = await client.post("/read",{query:{read:"Item",select:["id"],filter:[{field:"id",op:"eq",value}]}});
    assert.equal(read.code,"BAD_VALUE",String(value));
    const write = await client.apply({apply:"Item.read",target:{ids:[value]}},{key:`invalid:${String(value)}`});
    assert.equal(write.code,"BAD_VALUE",String(value));
  }
  const other = connectTyped(process.env.AIP_STRING_URL!,process.env.AIP_OTHER_TOKEN!,contract);
  const denied = await other.apply({apply:"Item.read",target:{ids:["9223372036854775807"]}},{key:"wrong-actor"});
  assert.equal(denied.ok,false);
  const mismatch = connectTyped(process.env.AIP_SAFE_URL!,process.env.AIP_SAFE_TOKEN!,contract);
  await assert.rejects(mismatch.read({read:"Item",select:["id"],filter:[{field:"id",op:"eq",value:"42"}]}),(e:any)=>e.code==="CONTRACT_MISMATCH");
});
