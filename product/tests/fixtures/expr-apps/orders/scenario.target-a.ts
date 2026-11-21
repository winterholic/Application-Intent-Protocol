// 주문·재고 변형 A(선언만): bundle refs(P0-A)와 input 산술(P0-B)를 serve + PG17에서 실행한다.
import assert from "node:assert/strict";
import { connect } from "@aip/sdk";
import { contract, type Id } from "./binding.js";
import { baseUrl, scenario, sql, schema, tokens } from "./check.js";

const olga = connect(baseUrl, tokens.olga!, contract);
const pavel = connect(baseUrl, tokens.pavel!, contract);
const OLGA = "1" as Id<"Member">;
const PAVEL = "2" as Id<"Member">;
const pid = (sku: string) => sql(`SELECT id FROM ${schema}.product WHERE sku='${sku}'`) as Id<"Product">;
const stock = (sku: string) => Number(sql(`SELECT stock FROM ${schema}.product WHERE sku='${sku}'`));
const count = (table: string) => Number(sql(`SELECT count(*) FROM ${schema}.${table}`));
const code = (r: { ok: boolean; code?: string }) => (r.ok ? "OK" : (r.code ?? "?"));

await scenario("A1-bundle-refs-atomic-order-lines-and-input-take", async () => {
  const pA = pid("A"), pB = pid("B");
  const r = await olga.bundle((b) => {
    const order = b.create("Order", { owner: OLGA, total: 700, placed: true });
    b.create("OrderLine", { product: pA, qty: 3, unitPrice: 200 }, { order });
    b.create("OrderLine", { product: pB, qty: 2, unitPrice: 50 }, { order });
    b.apply({ apply: "Product.take", target: { ids: [pA] }, input: { qty: 3 } });
    b.apply({ apply: "Product.take", target: { ids: [pB] }, input: { qty: 2 } });
  }, { key: "tgt-a1" });
  assert.equal(r.ok, true, JSON.stringify(r));
  const orderId = r.ok ? r.created[0]!.id : "";
  assert.deepEqual([stock("A"), stock("B")], [7, 3], "입력 수량만큼 재고 차감(서버가 잠근 행에서 계산)");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.order_line WHERE order_id=${orderId}`), "2", "줄 2개가 만든 주문 id를 가리킴");
  assert.equal(count('"order"'), 1);
});

await scenario("A2-insufficient-stock-rolls-back-order-lines-and-earlier-take", async () => {
  const pA = pid("A"), pB = pid("B");
  const before = [count('"order"'), count("order_line"), stock("A"), stock("B")];
  const r = await olga.bundle((b) => {
    const order = b.create("Order", { owner: OLGA, total: 1, placed: true });
    b.create("OrderLine", { product: pA, qty: 1, unitPrice: 200 }, { order });
    b.create("OrderLine", { product: pB, qty: 100, unitPrice: 50 }, { order });
    b.apply({ apply: "Product.take", target: { ids: [pA] }, input: { qty: 1 } });
    b.apply({ apply: "Product.take", target: { ids: [pB] }, input: { qty: 100 } });
  }, { key: "tgt-a2" });
  assert.equal(r.ok, false);
  console.log(`INFO A2-rollback-code: 재고 부족 묶음 거절 코드 ${code(r)}`);
  assert.equal(code(r), "WIRE.INVALID_STATE", "from stock >= input.qty가 판정한다(Int range 사후 검사만이면 BAD_VALUE)");
  assert.deepEqual([count('"order"'), count("order_line"), stock("A"), stock("B")], before, "주문·줄·첫 차감이 모두 남지 않는다");
});

await scenario("A3-ref-to-missing-product-leaves-no-orphan-order", async () => {
  const before = [count('"order"'), count("order_line")];
  const r = await olga.bundle((b) => {
    const order = b.create("Order", { owner: OLGA, total: 1, placed: true });
    b.create("OrderLine", { product: "9999" as Id<"Product">, qty: 1, unitPrice: 1 }, { order });
  }, { key: "tgt-a3" });
  assert.equal(r.ok, false);
  console.log(`INFO A3-missing-product-code: ${code(r)}`);
  assert.deepEqual([count('"order"'), count("order_line")], before, "우회 버전의 고아 주문이 생기지 않는다");
});

await scenario("A4-replay-returns-created-ids-and-decrements-once", async () => {
  const pC = pid("C");
  const place = () => olga.bundle((b) => {
    const order = b.create("Order", { owner: OLGA, total: 10, placed: true });
    b.create("OrderLine", { product: pC, qty: 1, unitPrice: 10 }, { order });
    b.apply({ apply: "Product.take", target: { ids: [pC] }, input: { qty: 1 } });
  }, { key: "tgt-a4" });
  const orders0 = count('"order"'), c0 = stock("C");
  const first = await place();
  const again = await place();
  assert.equal(first.ok && again.ok && again.replayed, true, JSON.stringify([first, again]));
  assert.deepEqual(again.ok && again.created, first.ok && first.created, "재전송이 처음에 만든 id를 그대로 돌려준다");
  assert.equal(stock("C"), c0 - 1);
  assert.equal(count('"order"'), orders0 + 1);
  const other = await olga.bundle((b) => {
    b.apply({ apply: "Product.take", target: { ids: [pC] }, input: { qty: 2 } });
  }, { key: "tgt-a4" });
  assert.equal(other.ok, false);
  console.log(`INFO A4-same-key-different-body-code: ${code(other)}`);
  assert.equal(stock("C"), c0 - 1, "같은 key에 다른 본문은 실행되지 않는다");
});

await scenario("A5-input-validation-before-and-inside-transaction", async () => {
  const pD = pid("D"); // 재고 3
  const take = (qty: unknown, key: string) => olga.bundle({ atomic: [{ apply: "Product.take", target: { ids: [pD] }, input: { qty } }] } as never, { key });
  const results = { zero: await take(0, "tgt-a5-0"), big: await take(5000, "tgt-a5-big"), over: await take(4, "tgt-a5-over"), neg: await take(-1, "tgt-a5-neg") };
  const missing = await olga.bundle({ atomic: [{ apply: "Product.take", target: { ids: [pD] } }] } as never, { key: "tgt-a5-missing" });
  for (const [name, r] of Object.entries({ ...results, missing })) assert.equal((r as { ok: boolean }).ok, false, name);
  console.log(`INFO A5-input-codes: qty0=${code(results.zero)} qty5000=${code(results.big)} qty4>stock3=${code(results.over)} qty-1=${code(results.neg)} 누락=${code(missing)}`);
  assert.equal(code(results.over), "WIRE.INVALID_STATE", "재고보다 큰 수량은 from이 거절한다");
  assert.equal(stock("D"), 3, "거절된 요청은 재고를 건드리지 않는다");
});

await scenario("A6-price-forgery-unit-price-blocked-total-still-client-asserted", async () => {
  const pA = pid("A");
  const before = [count('"order"'), count("order_line")];
  const forged = await olga.bundle((b) => {
    const order = b.create("Order", { owner: OLGA, total: 1, placed: true });
    b.create("OrderLine", { product: pA, qty: 1, unitPrice: 1 }, { order }); // 실제 단가는 200
  }, { key: "tgt-a6-unit" });
  assert.equal(forged.ok, false, "create allow `unitPrice = product.price`가 단가 위조를 막는다");
  console.log(`INFO A6-unit-price-forgery-code: ${code(forged)}`);
  assert.deepEqual([count('"order"'), count("order_line")], before);
  const honestLine = await olga.bundle((b) => {
    const order = b.create("Order", { owner: OLGA, total: 1, placed: true }); // 합계만 위조: 정당한 줄 1개의 합계는 200
    b.create("OrderLine", { product: pA, qty: 1, unitPrice: 200 }, { order });
  }, { key: "tgt-a6-total" });
  assert.equal(honestLine.ok, true, JSON.stringify(honestLine));
  console.log(`INFO A6-total-forgery: total=1로 보낸 주문이 그대로 저장됨(실제 합계 200). 변형 A의 한계`);
  assert.equal(sql(`SELECT max(total) FROM ${schema}."order" WHERE id=${honestLine.ok ? honestLine.created[0]!.id : 0}`), "1");
});

// 두 연산 순서 모두에서 단가 정책을 지키며 가능한 수량만 주문된다.
async function burst(sku: string, label: string, takeFirst: boolean) {
  const p = pid(sku); // 재고 4, 2개짜리 6건
  assert.equal(stock(sku), 4);
  const buyers = [[olga, OLGA], [pavel, PAVEL], [olga, OLGA], [pavel, PAVEL], [olga, OLGA], [pavel, PAVEL]] as const;
  const started = Date.now();
  const results = await Promise.all(buyers.map(([buyer, owner], i) => buyer.bundle((b) => {
    if (takeFirst) b.apply({ apply: "Product.take", target: { ids: [p] }, input: { qty: 2 } });
    const order = b.create("Order", { owner, total: 20, placed: true });
    b.create("OrderLine", { product: p, qty: 2, unitPrice: 10 }, { order });
    if (!takeFirst) b.apply({ apply: "Product.take", target: { ids: [p] }, input: { qty: 2 } });
  }, { key: `tgt-a7-${label}-${i}` })));
  const ok = results.filter((r) => r.ok).length;
  console.log(`INFO A7-${label}: 재고 4개, 2개짜리 주문 6건 동시(${takeFirst ? "take 먼저" : "줄 먼저"}) -> 성공 ${ok}건 (${results.map(code).join("|")}), 최종 재고 ${stock(sku)}, ${Date.now() - started}ms`);
  assert.equal(ok, 2, "재고 4개로 2개짜리 주문은 정확히 2건 성공한다");
  assert.deepEqual(results.filter((result) => !result.ok).map(code), Array(4).fill("WIRE.INVALID_STATE"));
  assert.equal(stock(sku), 4 - 2 * ok, "성공 건수 x 2만큼만 차감");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.order_line WHERE product_id=${p}`), String(ok), "성공한 주문의 줄만 남는다(고아 없음)");
}
await scenario("A7a-concurrent-orders-lines-first-exact-success-count", () => burst("E", "lines-first", false));
await scenario("A7b-concurrent-orders-take-first-exact-success-count", () => burst("F", "take-first", true));
