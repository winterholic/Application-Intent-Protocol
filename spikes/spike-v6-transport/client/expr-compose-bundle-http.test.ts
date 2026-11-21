import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTypedApply } from "./typed.ts";
import type { BundleBuilder } from "../../spike-v5-sdk/sdk/generic.ts";
import { contract, type ApplyContract, type CreateContract, type ComposeContract } from "./generated-expr-compose.ts";

const url = process.env.AIP_COMPOSE_URL!;
const token = process.env.AIP_COMPOSE_TOKEN!;
const aip = connectTypedApply(url, token, contract);
type Builder = BundleBuilder<ApplyContract, CreateContract, ComposeContract>;
const id = (n: number) => String(n) as never;

test("P0-D: 빌더의 compose 연산이 input 전이와 한 요청으로 가고 연산별 경계가 돌아온다", async () => {
  const build = (b: Builder) => {
    b.apply({ apply: "Product.take", target: { ids: [id(4)] }, input: { qty: 2 } });
    b.compose({ compose: "Order", targets: { ids: [id(5)] }, steps: [{ transition: "confirm" }, { create: "Log", values: { owner: { item: "owner" }, note: { const: "ts" } } }] });
  };
  const done = await aip.bundle(build, { key: "ts-compose-1" });
  assert.equal(done.ok, true, JSON.stringify(done));
  if (!done.ok) return;
  assert.deepEqual(done.ops, [
    { kind: "apply", applied: [0, 1], created: [0, 0] },
    { kind: "compose", applied: [1, 2], created: [0, 1] },
  ]);
  assert.deepEqual(done.tags, ["Log", "Order", "Product"]);
  const replay = await aip.bundle(build, { key: "ts-compose-1" });
  assert.equal(replay.ok && replay.replayed, true);
  assert.deepEqual(replay.ok && replay.ops, done.ops);
});
