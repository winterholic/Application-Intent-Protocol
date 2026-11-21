// 주문·재고 변형 B(확장): 서버가 단가를 읽어 합계를 계산한다. P0-C ctx.data.create와 P0-B take를 serve + PG17에서 실행한다.
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
const code = (r: { ok: boolean; code?: string; appCode?: string }) => (r.ok ? "OK" : `${r.code}${r.appCode ? ":" + r.appCode : ""}`);

await scenario("B1-server-priced-total-and-atomic-create-take", async () => {
  const r = await olga.writeExtension("Order.placeAtomic", { owner: OLGA, product: pid("A"), qty: 2 }, { key: "tgt-b1" });
  assert.equal(r.ok, true, JSON.stringify(r));
  if (!r.ok) return;
  assert.equal(r.output.total, 400, "호출자가 보내지 않은 단가 200으로 서버가 계산");
  assert.equal(sql(`SELECT total||'|'||placed FROM ${schema}."order" WHERE id=${r.output.id}`), "400|true");
  assert.equal(sql(`SELECT unit_price||'|'||qty FROM ${schema}.order_line WHERE order_id=${r.output.id}`), "200|2");
  assert.equal(stock("A"), 8);
});

await scenario("B2-insufficient-stock-rejected-nothing-left", async () => {
  const before = [count('"order"'), count("order_line"), stock("B")];
  const r = await olga.writeExtension("Order.placeAtomic", { owner: OLGA, product: pid("B"), qty: 100 }, { key: "tgt-b2" });
  assert.equal(r.ok, false);
  assert.equal(code(r as never), "WIRE.EXTENSION_REJECTED:INSUFFICIENT_STOCK");
  assert.deepEqual([count('"order"'), count("order_line"), stock("B")], before);
});

await scenario("B3-forgery-paths-closed", async () => {
  const pA = pid("A");
  const create = (values: Record<string, unknown>, resource: string, key: string) => olga.bundle({ atomic: [{ create: resource, values }] } as never, { key });
  const forgedTotal = await create({ owner: OLGA, total: 999, placed: true }, "Order", "tgt-b3-total");
  const forgedLine = await olga.bundle((b) => {
    const order = b.create("Order", { owner: OLGA, total: 0, placed: false });
    b.create("OrderLine", { product: pA, qty: 1, unitPrice: 1 }, { order });
  }, { key: "tgt-b3-line" });
  const directTake = await olga.apply({ apply: "Product.take", target: { ids: [pA] }, input: { qty: 1 } } as never, { key: "tgt-b3-take" });
  const directSettle = await olga.apply({ apply: "Order.settle", target: { ids: ["1"] }, values: { total: 1 } } as never, { key: "tgt-b3-settle" });
  const spoofOwner = await olga.writeExtension("Order.placeAtomic", { owner: PAVEL, product: pA, qty: 1 }, { key: "tgt-b3-owner" });
  for (const [name, r] of Object.entries({ forgedTotal, forgedLine, directTake, directSettle, spoofOwner })) assert.equal((r as { ok: boolean }).ok, false, `${name} 거부돼야 함: ${JSON.stringify(r)}`);
  console.log(`INFO B3-forgery-codes: 합계999 직접 create=${code(forgedTotal as never)} 단가1 줄=${code(forgedLine as never)} take 직접=${code(directTake as never)} settle 직접=${code(directSettle as never)} 남의 owner=${code(spoofOwner as never)}`);
  assert.equal(stock("A"), 8, "차감 통로는 확장뿐");
  assert.equal(sql(`SELECT count(*) FROM ${schema}."order" WHERE total=999 OR owner_id=2`), "0");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.order_line WHERE unit_price=1`), "0");
  const emptyOrder = await create({ owner: OLGA, total: 0, placed: false }, "Order", "tgt-b3-empty");
  console.log(`INFO B3-direct-empty-order: 합계 0의 미확정 주문 직접 create=${code(emptyOrder as never)}. 줄 없는 미확정 주문은 클라이언트가 스스로 만들 수 있다(재고·합계에는 영향 없음)`);
});

await scenario("B4-replay-same-key-one-order", async () => {
  const orders0 = count('"order"'), s0 = stock("C");
  const req = { owner: OLGA, product: pid("C"), qty: 1 };
  const first = await olga.writeExtension("Order.placeAtomic", req, { key: "tgt-b4" });
  const again = await olga.writeExtension("Order.placeAtomic", req, { key: "tgt-b4" });
  assert.equal(first.ok && again.ok && again.replayed, true, JSON.stringify([first, again]));
  assert.deepEqual(again.ok && again.output, first.ok && first.output, "재전송이 처음 만든 주문 id를 돌려준다");
  assert.equal(count('"order"'), orders0 + 1);
  assert.equal(stock("C"), s0 - 1);
});

// 동시 호출 측정. 한 사용자의 동기 worker 동시 한도(기본 2)를 넘지 않게 사용자당 1건씩 2건, 그리고 6건(사용자당 3건)을 본다.
async function burst(sku: string, label: string, n: number) {
  const p = pid(sku);
  const buyers = Array.from({ length: n }, (_, i) => (i % 2 === 0 ? [olga, OLGA] as const : [pavel, PAVEL] as const));
  const before = count('"order"');
  const started = Date.now();
  const results = await Promise.all(buyers.map(([buyer, owner], i) => buyer.writeExtension("Order.placeAtomic", { owner, product: p, qty: 2 }, { key: `tgt-b5-${label}-${i}` })));
  const ok = results.filter((r) => r.ok).length;
  console.log(`INFO B5-${label}: 재고 4개, 2개짜리 ${n}건 동시 -> 성공 ${ok}건 (${results.map((r) => code(r as never)).join("|")}), 최종 재고 ${stock(sku)}, ${Date.now() - started}ms`);
  assert.equal(stock(sku), 4 - 2 * ok, "성공 x 2만큼만 차감");
  assert.equal(count('"order"') - before, ok, "실패한 호출이 만든 주문은 롤백되어 고아가 없다");
  return ok;
}
await scenario("B5a-two-concurrent-extension-orders-invariants", async () => { await burst("G", "two", 2); });
await scenario("B5b-six-concurrent-extension-orders-invariants", async () => { await burst("F", "six", 6); });
