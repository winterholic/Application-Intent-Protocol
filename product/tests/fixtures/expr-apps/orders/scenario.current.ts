// 주문·재고, 현재 문법으로 가능한 부분과 우회 경로의 실행. P0 병합 뒤 목표 정의는 scenario.target.ts(미실행)에 있다.
import assert from "node:assert/strict";
import { connect } from "@aip/sdk";
import { contract, type Id } from "./binding.js";
import { baseUrl, scenario, sql, schema, tokens } from "./check.js";

const olga = connect(baseUrl, tokens.olga!, contract);
const pavel = connect(baseUrl, tokens.pavel!, contract);
type Buyer = typeof olga;

const stock = (sku: string) => Number(sql(`SELECT stock FROM ${schema}.product WHERE sku='${sku}'`));
const productId = (sku: string) => sql(`SELECT id FROM ${schema}.product WHERE sku='${sku}'`) as Id<"Product">;

/** E1 우회: 호출 두 번(부모, 줄 묶음). 두 호출 사이에 끊기면 줄 없는 주문이 남는다. */
async function createOrder(client: Buyer, owner: string, key: string, lines: { sku: string; qty: number; price: number }[]) {
  const parent = await client.bundle({ atomic: [{ create: "Order", values: { owner, total: 0, placed: false } }] }, { key });
  assert.equal(parent.ok, true, JSON.stringify(parent));
  const orderId = (parent.ok ? parent.created[0]!.id : "") as Id<"Order">; // created[]의 id는 resource별 타입이 아니라 합집합이라 직접 좁힌다
  if (lines.length > 0) {
    const ops = lines.map((l) => ({ create: "OrderLine" as const, values: { order: orderId, product: productId(l.sku), qty: l.qty, unitPrice: l.price } }));
    const made = await client.bundle({ atomic: ops as unknown as [(typeof ops)[number], ...(typeof ops)[number][]] }, { key: `${key}-lines` });
    assert.equal(made.ok, true, JSON.stringify(made));
  }
  return orderId;
}

let first = "" as Id<"Order">;

await scenario("declarative-fixed-decrement-takeOne", async () => {
  const p = productId("D"); // stock 3
  for (const k of ["t1", "t2", "t3"]) assert.equal((await olga.apply({ apply: "Product.takeOne", target: { ids: [p] } }, { key: `ord-take-${k}` })).ok, true);
  assert.equal(stock("D"), 0);
  const empty = await olga.apply({ apply: "Product.takeOne", target: { ids: [p] } }, { key: "ord-take-t4" });
  assert.equal(empty.ok, false, "from stock >= 1이 음수 재고를 막는다");
  assert.equal(stock("D"), 0);
  const a = await olga.apply({ apply: "Product.takeOne", target: { ids: [productId("C")] } }, { key: "ord-take-replay" });
  const b = await olga.apply({ apply: "Product.takeOne", target: { ids: [productId("C")] } }, { key: "ord-take-replay" });
  assert.equal(a.ok, true);
  assert.equal((b as { replayed?: boolean }).replayed, true);
  assert.equal(stock("C"), 4, "같은 key 재전송은 한 번만 차감");
  // 확장 전용 전이는 호출자가 직접 못 부른다(재고에 임의 값을 쓰는 통로가 아니다)
  const direct = await olga.apply({ apply: "Product.setStock", target: { ids: [p] }, values: { stock: 999 }, expect: { stock: 0 } } as never, // internal 전이는 계약 타입에 없어 우회 캐스트가 필요하다
     { key: "ord-direct-set" });
  assert.equal((direct as { ok: boolean; code?: string }).ok, false);
  assert.equal(stock("D"), 0);
});

await scenario("workaround-place-order-and-lines-then-confirm", async () => {
  first = await createOrder(olga, "1", "ord-1", [{ sku: "A", qty: 3, price: 200 }, { sku: "B", qty: 2, price: 50 }]);
  const placed = await olga.writeExtension("Order.place", { id: first}, { key: "ord-1-place" });
  assert.equal(placed.ok, true, JSON.stringify(placed));
  assert.deepEqual([stock("A"), stock("B")], [7, 3], "입력 수량만큼 재고 차감(확장 계산)");
  assert.equal(sql(`SELECT total||'|'||placed FROM ${schema}."order" WHERE id=${first}`), "700|true", "E3: 합계 3*200+2*50는 확장이 계산");
});

await scenario("workaround-idempotent-replay-no-double-decrement", async () => {
  const again = await olga.writeExtension("Order.place", { id: first}, { key: "ord-1-place" });
  assert.equal((again as { replayed?: boolean }).replayed, true);
  assert.deepEqual([stock("A"), stock("B")], [7, 3], "같은 key 재전송은 재고를 다시 줄이지 않는다");
  // 확정된 주문을 다른 key로 다시 확정하려는 시도는 from placed = false가 막는다
  const rePlace = await olga.writeExtension("Order.place", { id: first}, { key: "ord-1-place-new-key" });
  assert.equal(rePlace.ok, false, `이중 확정 거부: ${JSON.stringify(rePlace)}`);
  assert.deepEqual([stock("A"), stock("B")], [7, 3]);
  // 부모 생성 재전송도 중복 주문을 만들지 않는다
  const before = sql(`SELECT count(*) FROM ${schema}."order"`);
  const dup = await olga.bundle({ atomic: [{ create: "Order", values: { owner: "1", total: 0, placed: false } }] }, { key: "ord-1" });
  assert.equal((dup as { replayed?: boolean }).replayed, true);
  assert.equal(sql(`SELECT count(*) FROM ${schema}."order"`), before);
});

await scenario("workaround-insufficient-stock-rolls-back-all-lines", async () => {
  const o = await createOrder(olga, "1", "ord-2", [{ sku: "A", qty: 2, price: 200 }, { sku: "B", qty: 100, price: 50 }]);
  const r = await olga.writeExtension("Order.place", { id: o}, { key: "ord-2-place" });
  assert.equal(r.ok, false);
  const failure = r as unknown as { code: string; appCode?: string; msg?: string };
  assert.equal(failure.code, "WIRE.EXTENSION_REJECTED", JSON.stringify(r));
  assert.equal(failure.appCode, "INSUFFICIENT_STOCK", "업무 거절 코드가 호출자에게 전달된다");
  assert.deepEqual([stock("A"), stock("B")], [7, 3], "첫 줄의 차감도 함께 되돌아간다");
  assert.equal(sql(`SELECT placed FROM ${schema}."order" WHERE id=${o}`), "f");
});

await scenario("workaround-empty-order-rejected-and-orphan-left", async () => {
  const o = await createOrder(olga, "1", "ord-3", []); // 줄을 만들기 전에 끊긴 상황
  const r = await olga.writeExtension("Order.place", { id: o}, { key: "ord-3-place" });
  assert.equal((r as { appCode?: string }).appCode, "EMPTY_ORDER");
  assert.equal(sql(`SELECT count(*) FROM ${schema}."order" WHERE placed=false AND id NOT IN (SELECT order_id FROM ${schema}.order_line)`), "1", "E1: 줄 없는 미확정 주문이 남는다. 정리는 앱이 정해야 한다");
});

await scenario("ownership-other-buyer-cannot-place", async () => {
  const o = await createOrder(olga, "1", "ord-4", [{ sku: "C", qty: 1, price: 10 }]);
  const r = await pavel.writeExtension("Order.place", { id: o}, { key: "ord-4-pavel" });
  assert.equal(r.ok, false, `다른 구매자의 확정 거부: ${JSON.stringify(r)}`);
  assert.equal(sql(`SELECT placed FROM ${schema}."order" WHERE id=${o}`), "f");
  const spoof = await pavel.bundle({ atomic: [{ create: "OrderLine", values: { order: o, product: productId("C"), qty: 1, unitPrice: 10 } }] }, { key: "ord-4-spoof" });
  assert.equal(spoof.ok, false, "남의 주문에 줄을 붙일 수 없다");
});

await scenario("workaround-concurrent-orders-never-oversell", async () => {
  // 재고 C는 4개 남았다. 두 구매자가 동시에 주문 5개(각자 2개씩 x 여러 주문)를 넣는다.
  const start = stock("C");
  const orders: { buyer: Buyer; id: Id<"Order">; n: number }[] = [];
  for (let i = 0; i < 3; i++) {
    orders.push({ buyer: olga, id: await createOrder(olga, "1", `ord-cc-o${i}`, [{ sku: "C", qty: 2, price: 10 }]), n: i });
    orders.push({ buyer: pavel, id: await createOrder(pavel, "2", `ord-cc-p${i}`, [{ sku: "C", qty: 2, price: 10 }]), n: i });
  }
  const results = await Promise.all(orders.map((o, i) => o.buyer.writeExtension("Order.place", { id: o.id}, { key: `ord-cc-place-${i}` })));
  const okCount = results.filter((r) => r.ok).length;
  const outcomes = results.map((r) => (r.ok ? "OK" : `${(r as { code: string }).code}${(r as { appCode?: string }).appCode ? ":" + (r as { appCode?: string }).appCode : ""}`));
  console.error(`concurrent outcomes: ${outcomes.join(", ")}`);
  const end = stock("C");
  assert.ok(end >= 0, "재고는 음수가 되지 않는다");
  assert.equal(start - end, okCount * 2, "성공한 주문 수 x 2 만큼만 줄었다(이중 차감·유실 없음)");
  assert.equal(Number(sql(`SELECT count(*) FROM ${schema}."order" WHERE placed=true AND id IN (SELECT order_id FROM ${schema}.order_line WHERE product_id=${productId("C")})`)) - 0, okCount + 0, "확정된 주문 수와 성공 수가 같다");
  console.log(`INFO concurrent-orders: 재고 ${start}개, 주문 6건(각 2개) 동시 -> 성공 ${okCount}건, 결과 ${outcomes.join("|")}, 최종 재고 ${end}`);
});

// 우회 정의에는 줄의 단가를 서버가 검사하는 곳이 없다. 호출자가 단가를 낮춰 보내면 확장은 저장된 값으로 합계를 계산한다.
await scenario("workaround-price-forgery-is-accepted", async () => {
  const o = await createOrder(olga, "1", "ord-forge", [{ sku: "A", qty: 1, price: 1 }]); // 실제 단가는 200
  const r = await olga.writeExtension("Order.place", { id: o }, { key: "ord-forge-place" });
  assert.equal(r.ok, true, JSON.stringify(r));
  assert.equal(sql(`SELECT total FROM ${schema}."order" WHERE id=${o}`), "1", "단가를 1로 보낸 주문이 합계 1로 확정된다");
  console.log("INFO workaround-price-forgery: 우회 정의에서는 호출자가 보낸 unitPrice 1이 그대로 합계 1이 됨(실제 단가 200)");
});
