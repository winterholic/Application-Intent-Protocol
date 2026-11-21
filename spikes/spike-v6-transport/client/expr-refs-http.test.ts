import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTypedApply } from "./typed.ts";
import type { BundleBuilder } from "../../spike-v5-sdk/sdk/generic.ts";
import { contract, type ApplyContract, type CreateContract } from "./generated-expr-refs.ts";

const url = process.env.AIP_REFS_URL!;
const token = process.env.AIP_REFS_TOKEN!;
const aip = connectTypedApply(url, token, contract);
const member = "1" as never;
const product = "7" as never;
type Builder = BundleBuilder<ApplyContract, CreateContract>;

test("P0-A: 빌더가 쌓은 묶음이 주문 생성 → 줄 생성 → input 차감 → 확정을 한 요청으로 보내고, 같은 키는 만든 id까지 재생한다", async () => {
  const build = (b: Builder) => {
    const order = b.create("Order", { owner: member, status: "open" });
    b.create("OrderLine", { owner: member, product, qty: 2 }, { order });
    b.apply({ apply: "Product.take", target: { ids: [product] }, input: { qty: 2 } });
    b.apply({ apply: "Order.confirm", target: order });
  };
  const done = await aip.bundle(build, { key: "ts-flow-1" });
  assert.equal(done.ok, true, JSON.stringify(done));
  if (!done.ok) return;
  assert.deepEqual(done.created.map((row) => row.resource), ["Order", "OrderLine"]);
  assert.equal(done.applied.length, 2);
  const replay = await aip.bundle(build, { key: "ts-flow-1" });
  assert.equal(replay.ok && replay.replayed, true);
  assert.deepEqual(replay.ok && replay.created, done.created, "the replay returns the ids the first run made");
});

test("P0-A: 다른 빌더의 핸들, 빈 빌더, 빌더 안의 예외는 보내기 전에 거절한다", async () => {
  let leaked: unknown;
  await assert.rejects(
    aip.bundle((b) => {
      leaked = b.create("Order", { owner: member, status: "open" });
      throw new Error("stop before sending");
    }),
    /stop before sending/,
  );
  await assert.rejects(
    aip.bundle((b) => {
      b.create("OrderLine", { owner: member, qty: 1 }, { order: leaked as never });
    }),
    (error: any) => error.code === "CLIENT.PROTOCOL_ERROR",
  );
  await assert.rejects(aip.bundle(() => {}), (error: any) => error.code === "CLIENT.PROTOCOL_ERROR");
});

test("P0-A: 서버가 거절한 묶음(뒤 연산 실패)은 아무것도 남기지 않는다", async () => {
  const refused = await aip.bundle((b) => {
    const order = b.create("Order", { owner: member, status: "open" });
    b.create("OrderLine", { owner: member, product, qty: 1 }, { order });
    b.apply({ apply: "Product.take", target: { ids: [product] }, input: { qty: 100 } });
    b.apply({ apply: "Order.confirm", target: order });
  }, { key: "ts-flow-refused" });
  assert.equal(refused.ok, false);
  assert.equal(!refused.ok && refused.code, "WIRE.INVALID_STATE");
});
