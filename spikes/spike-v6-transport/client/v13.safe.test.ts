import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTyped } from "./typed.ts";
import { contract } from "./generated-v13-safe.ts";

test("숫자 후보: 안전범위 안은 같은 행, 밖은 조회·where 쓰기를 명시 거부", async () => {
  const client = connectTyped(process.env.AIP_SAFE_URL!,process.env.AIP_SAFE_TOKEN!,contract);
  const shared = await client.apply({apply:"Item.read",target:{where:[{field:"label",op:"eq",value:"small"}]}},{key:"shared-profile"});
  assert.equal(shared.ok,true);
  assert.deepEqual(shared.changed,[42]);
  for (const id of [0,42,Number.MAX_SAFE_INTEGER]) {
    const read = await client.read({ read: "Item", select: ["id"], filter: [{field:"id",op:"eq",value:id}] });
    assert.equal(read.rows[0].id,id);
    const result = await client.apply({apply:"Item.read",target:{ids:[read.rows[0].id]}},{key:`safe:${id}`});
    assert.equal(result.ok,true);
    assert.deepEqual(result.changed,id===42?[]:[id]);
    assert.deepEqual(result.unchanged,id===42?[id]:[]);
  }
  await assert.rejects(client.read({read:"Item",select:["id"]}),(e:any)=>e.code==="ID_OUT_OF_RANGE");
  const where = await client.apply({apply:"Item.read",target:{where:[{field:"label",op:"eq",value:"top"}]}},{key:"unsafe-where"});
  assert.equal(where.ok,false);
  assert.equal(where.code,"ID_OUT_OF_RANGE");
  for (const value of [9007199254740992,9007199254740993]) {
    const rejected = await client.post("/read",{query:{read:"Item",select:["id"],filter:[{field:"id",op:"eq",value}]}});
    assert.equal(rejected.code,"ID_OUT_OF_RANGE");
    const denied = await client.apply({apply:"Item.read",target:{ids:[value]}},{key:`bad-safe:${value}`});
    assert.equal(denied.code,"ID_OUT_OF_RANGE");
  }
  const mismatch = connectTyped(process.env.AIP_LEGACY_URL!,process.env.AIP_LEGACY_TOKEN!,contract);
  await assert.rejects(mismatch.read({read:"Item",select:["id"]}),(e:any)=>e.code==="CONTRACT_MISMATCH");
});
