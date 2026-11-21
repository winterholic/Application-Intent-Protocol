// 목표 시나리오(미실행). P0-A·B·C 병합 뒤 orders.target.aip로 실행한다. 지금 binding을 만들 수 없어 타입 검사도 못 한다.
// 병합 뒤 후속 작업이 할 일: (1) 이 파일을 scenario.current.ts와 같은 방식으로 run.mjs에 올린다,
// (2) `refs`·`input`·`ctx.data.apply` 인자 이름을 실제 구현에 맞춘다, (3) 아래 기대값을 실행으로 확인한다.
import assert from "node:assert/strict";
import { connect } from "@aip/sdk";
import { contract } from "./binding.js";
import { baseUrl, scenario, sql, schema, tokens } from "./check.js";

const olga = connect(baseUrl, tokens.olga!, contract);
const pavel = connect(baseUrl, tokens.pavel!, contract);
const productId = (sku: string) => sql(`SELECT id FROM ${schema}.product WHERE sku='${sku}'`);
const stock = (sku: string) => Number(sql(`SELECT stock FROM ${schema}.product WHERE sku='${sku}'`));

// T1 (P0-A): 주문+줄 원자 생성. 호출 한 번이고 중간 실패 시 줄 없는 주문이 남지 않는다.
await scenario("target-A1-bundle-refs-atomic-order-and-lines", async () => {
  const a = productId("A"), b = productId("B");
  const r = await (olga as any).bundle({ atomic: [
    { create: "Order", values: { owner: "1", total: 700, placed: true } },
    { create: "OrderLine", values: { product: a, qty: 3, unitPrice: 200 }, refs: { order: { created: 0 } } },
    { create: "OrderLine", values: { product: b, qty: 2, unitPrice: 50 }, refs: { order: { created: 0 } } },
  ] }, { key: "tgt-1" });
  assert.equal(r.ok, true);
  assert.equal(sql(`SELECT count(*) FROM ${schema}.order_line`), "2");
  // 두 번째 줄이 존재하지 않는 상품이면 부모도 남지 않아야 한다
  const bad = await (olga as any).bundle({ atomic: [
    { create: "Order", values: { owner: "1", total: 1, placed: true } },
    { create: "OrderLine", values: { product: "9999", qty: 1, unitPrice: 1 }, refs: { order: { created: 0 } } },
  ] }, { key: "tgt-1-bad" });
  assert.equal(bad.ok, false);
  assert.equal(sql(`SELECT count(*) FROM ${schema}."order"`), "1");
});

// T2 (P0-A + P0-B): 한 요청 안에서 줄 생성과 입력 수량 재고 차감. 단, total과 unitPrice를 클라이언트가 보낸다(가격 위조 가능). 변형 B가 이를 막는다.
await scenario("target-A2-bundle-create-and-take-with-input", async () => {
  const c = productId("C");
  const r = await (olga as any).bundle({ atomic: [
    { create: "Order", values: { owner: "1", total: 20, placed: true } },
    { create: "OrderLine", values: { product: c, qty: 2, unitPrice: 10 }, refs: { order: { created: 0 } } },
    { apply: "Product.take", target: { ids: [c] }, input: { qty: 2 } },
  ] }, { key: "tgt-2" });
  assert.equal(r.ok, true);
  assert.equal(stock("C"), 3);
  const over = await (olga as any).bundle({ atomic: [{ apply: "Product.take", target: { ids: [c] }, input: { qty: 99 } }] }, { key: "tgt-2-over" });
  assert.equal(over.ok, false); // from stock >= input.qty
  assert.equal(stock("C"), 3);
});

// T3 (P0-B): 동시 주문. 행 잠금으로 직렬화되므로 재고 5, 수량 2 요청 3건이면 정확히 2건 성공한다(현재 우회는 1건).
await scenario("target-B1-concurrent-take-exact", async () => {
  const d = productId("D"); // 시드 재고 3으로 가정, 수량 1 요청 5건 -> 정확히 3건 성공
  const results = await Promise.all(Array.from({ length: 5 }, (_, i) => (i % 2 ? pavel : olga).bundle({ atomic: [{ apply: "Product.take", target: { ids: [d] }, input: { qty: 1 } }] } as any, { key: `tgt-cc-${i}` })));
  assert.equal(results.filter((r: any) => r.ok).length, 3);
  assert.equal(stock("D"), 0);
});

// T4 (P0-A): 멱등 재전송. 같은 key 재전송은 저장 결과(만든 id 포함)를 재생하고 재고를 다시 줄이지 않는다.
await scenario("target-A3-idempotent-replay-of-atomic-bundle", async () => {
  const a = productId("A");
  const req = { atomic: [
    { create: "Order", values: { owner: "1", total: 200, placed: true } },
    { create: "OrderLine", values: { product: a, qty: 1, unitPrice: 200 }, refs: { order: { created: 0 } } },
    { apply: "Product.take", target: { ids: [a] }, input: { qty: 1 } },
  ] };
  const before = stock("A");
  const first = await (olga as any).bundle(req, { key: "tgt-4" });
  const again = await (olga as any).bundle(req, { key: "tgt-4" });
  assert.equal(first.ok && again.replayed, true);
  assert.deepEqual(again.created, first.created);
  assert.equal(stock("A"), before - 1);
});

// T5 (P0-C + P0-B): 변형 B. 서버가 단가로 합계를 계산하고 주문·줄·재고 차감이 원자적이다. 호출자는 가격을 보내지 않는다.
await scenario("target-B2-extension-create-and-take-server-priced", async () => {
  const a = productId("A");
  const r = await (olga as any).writeExtension("Order.placeAtomic", { product: a, qty: 2 }, { key: "tgt-5" });
  assert.equal(r.ok, true);
  assert.equal(r.output.total, 400);
  assert.equal(sql(`SELECT unit_price FROM ${schema}.order_line WHERE order_id=${r.output.id}`), "200");
  const bad = await (olga as any).writeExtension("Order.placeAtomic", { product: a, qty: 100000 }, { key: "tgt-5-over" });
  assert.equal(bad.ok, false);
});
