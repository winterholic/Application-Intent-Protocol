// 주문·결제 대역·재고 반환·상태 머신 시나리오(정의 storefront.aip, 시드 seed.sql).
// 모든 의미 단언은 응답과 함께 psql로 DB 상태(재고, 주문, 줄, 결제 원장)를 직접 읽어 확인한다.
import assert from "node:assert/strict";
import { connect } from "@aip/sdk";
import { contract } from "./binding.js";
import { baseUrl, scenario, sql, schema, tokens } from "./check.js";

const as = (who: string) => connect(baseUrl, tokens[who]!, contract);
const olga = () => as("olga"), pavel = () => as("pavel"), sam = () => as("sam"), gate = () => as("gate");
type Client = ReturnType<typeof as>;
const OLGA = "1", PAVEL = "2";
const P = { A: "1", B: "2", C: "3", D: "4", E: "5", F: "6" } as const;

const stock = (sku: keyof typeof P) => Number(sql(`SELECT stock FROM ${schema}.product WHERE sku='${sku}'`));
const count = (table: string, where = "true") => Number(sql(`SELECT count(*) FROM ${schema}.${table} WHERE ${where}`));
const orders = () => count('"order"');
const lines = () => count("order_line");
const payments = () => count("payment");
const orderState = (id: string) => sql(`SELECT state||'/'||total||'/'||paid FROM ${schema}."order" WHERE id=${id}`);
const code = (r: any) => (r.ok ? "OK" : r.appCode ? `${r.code}:${r.appCode}` : (r.code ?? "?"));
const snapshot = () => ({ orders: orders(), lines: lines(), payments: payments(), stock: sql(`SELECT string_agg(sku||'='||stock, ',' ORDER BY id) FROM ${schema}.product`) });

const rawApply = async (who: string, request: unknown, key: string): Promise<any> => {
  const response = await fetch(`${baseUrl}/apply`, {
    method: "POST",
    headers: { authorization: `Bearer ${tokens[who]!}`, "x-aip-contract": process.env.AIP_FINGERPRINT!, "content-type": "application/json" },
    body: JSON.stringify({ request, key }),
  });
  return response.json();
};
// 줄이 3개까지인 checkout 입력. 빈 슬롯은 null이다.
const slots = (owner: string, items: Array<{ product: string; qty: number }>) => {
  const input: Record<string, unknown> = { owner };
  for (const slot of [1, 2, 3]) {
    input[`product${slot}`] = items[slot - 1]?.product ?? null;
    input[`qty${slot}`] = items[slot - 1]?.qty ?? null;
  }
  return input;
};
const checkout = (who: string, owner: string, items: Array<{ product: string; qty: number }>, key: string): Promise<any> =>
  as(who).writeExtension("Order.checkout" as never, slots(owner, items) as never, { key } as never);
const checkoutRaw = (who: string, input: Record<string, unknown>, key: string): Promise<any> =>
  rawApply(who, { extension: "Order.checkout", input }, key);
const cancel = (who: Client, order: string, key: string): Promise<any> =>
  who.writeExtension("Order.cancelAndRestock" as never, { order } as never, { key } as never);
const webhook = (order: string, amount: number, ref: string, key: string, paymentAmount = amount): Promise<any> =>
  gate().bundle({ atomic: [
    { create: "Payment", values: { order, amount: paymentAmount, providerRef: ref } },
    { apply: "Order.pay", target: { ids: [order] }, input: { amount } },
  ] } as never, { key } as never);
const s: { o1?: string; o2?: string; o3?: string } = {};

await scenario("S1-checkout-multi-line-server-total-and-exact-stock", async () => {
  const before = { A: stock("A"), B: stock("B") };
  const r = await checkout("olga", OLGA, [{ product: P.A, qty: 3 }, { product: P.B, qty: 2 }], "sf-checkout-1");
  assert.equal(r.ok, true, JSON.stringify(r));
  assert.equal(r.output.total, 700, "3×200 + 2×50를 서버가 계산");
  s.o1 = r.output.id;
  assert.deepEqual([stock("A"), stock("B")], [before.A - 3, before.B - 2], "재고가 정확히 주문 수량만큼 한 번 줄었다");
  assert.equal(orderState(s.o1!), "PLACED/700/0");
  assert.equal(sql(`SELECT string_agg(qty||'x'||unit_price, ',' ORDER BY product_id) FROM ${schema}.order_line WHERE order_id=${s.o1}`), "3x200,2x50", "단가는 서버가 상품에서 읽은 값");
});

await scenario("S2-input-validation-leaves-no-trace", async () => {
  const before = snapshot();
  const A = P.A;
  const bad: Array<[string, Record<string, unknown>]> = [
    ["qty 0", { ...slots(OLGA, [{ product: A, qty: 1 }]), qty1: 0 }],
    ["qty 1001", { ...slots(OLGA, [{ product: A, qty: 1 }]), qty1: 1001 }],
    ["qty 1.5", { ...slots(OLGA, [{ product: A, qty: 1 }]), qty1: 1.5 }],
    ["qty as string", { ...slots(OLGA, [{ product: A, qty: 1 }]), qty1: "2" }],
    ["qty negative", { ...slots(OLGA, [{ product: A, qty: 1 }]), qty1: -3 }],
    ["slot 2 product without qty", { ...slots(OLGA, [{ product: A, qty: 1 }]), product2: P.B }],
    ["slot 2 qty without product", { ...slots(OLGA, [{ product: A, qty: 1 }]), qty2: 1 }],
    ["gap: slot 3 without slot 2", { ...slots(OLGA, [{ product: A, qty: 1 }]), product3: P.B, qty3: 1 }],
    ["duplicate product", slots(OLGA, [{ product: A, qty: 1 }, { product: A, qty: 1 }])],
    ["unknown product", slots(OLGA, [{ product: "9999", qty: 1 }])],
    ["owner forged", slots(PAVEL, [{ product: A, qty: 1 }])],
  ];
  const codes: string[] = [];
  for (const [name, input] of bad) {
    const r = await checkoutRaw("olga", input, `sf-bad-${name}`);
    assert.equal(r.ok, false, `${name}: ${JSON.stringify(r)}`);
    codes.push(`${name}=${code(r)}`);
  }
  console.log(`INFO S2-codes: ${codes.join(" | ")}`);
  assert.deepEqual(snapshot(), before, "거절된 요청은 주문·줄·결제·재고를 하나도 바꾸지 않는다");
});

await scenario("S3-insufficient-stock-on-second-line-rolls-back-everything", async () => {
  const before = snapshot();
  const r = await checkout("olga", OLGA, [{ product: P.A, qty: 1 }, { product: P.C, qty: 5 }], "sf-short");
  assert.equal(code(r), "WIRE.EXTENSION_REJECTED:INSUFFICIENT_STOCK");
  assert.deepEqual(snapshot(), before, "첫 줄(A)의 재고도 줄지 않았다");
});

await scenario("S4-same-key-replay-decrements-once", async () => {
  const d0 = stock("D"), o0 = orders();
  const first = await checkout("olga", OLGA, [{ product: P.D, qty: 1 }], "sf-replay");
  const again = await checkout("olga", OLGA, [{ product: P.D, qty: 1 }], "sf-replay");
  assert.equal(first.ok && again.ok && again.replayed, true, JSON.stringify([first, again]));
  assert.equal(again.output.id, first.output.id, "같은 주문 id를 재생");
  assert.equal(stock("D"), d0 - 1);
  assert.equal(orders(), o0 + 1);
  const other = await checkout("olga", OLGA, [{ product: P.D, qty: 2 }], "sf-replay");
  assert.equal(other.ok, false, "같은 key에 다른 본문");
  console.log(`INFO S4-same-key-different-body: ${code(other)}`);
  assert.equal(stock("D"), d0 - 1);
});

await scenario("S5-payment-double-confirm-amount-match-and-atomic-ledger", async () => {
  assert.equal(orderState(s.o1!), "PLACED/700/0");
  const asCustomer = await olga().bundle({ atomic: [{ create: "Payment", values: { order: s.o1!, amount: 700, providerRef: "fake-1" } }] } as never, { key: "sf-pay-olga" } as never);
  assert.equal(code(asCustomer), "WIRE.FORBIDDEN", "고객은 결제 원장을 못 쓴다");
  const selfPay = await olga().apply({ apply: "Order.pay", target: { ids: [s.o1!] }, input: { amount: 700 } } as never, { key: "sf-selfpay" } as never);
  assert.equal(code(selfPay), "WIRE.FORBIDDEN", "고객이 스스로 PAID로 만들 수 없다");
  const wrongLedger = await webhook(s.o1!, 699, "fake-2", "sf-pay-699", 700);
  assert.equal(wrongLedger.ok, false);
  console.log(`INFO S5-amount-mismatch: 결제 원장 700 + pay 699 -> ${code(wrongLedger)}`);
  assert.equal(payments(), 0, "pay가 실패하면 같은 묶음의 결제 원장 행도 남지 않는다");
  assert.equal(orderState(s.o1!), "PLACED/700/0");
  const wrongAmount = await webhook(s.o1!, 1, "fake-3", "sf-pay-1");
  assert.equal(wrongAmount.ok, false, "원장 금액이 주문 합계와 다르면 create allow가 거절");
  assert.equal(payments(), 0);
  const paid = await webhook(s.o1!, 700, "fake-4", "sf-pay-ok");
  assert.equal(paid.ok, true, JSON.stringify(paid));
  assert.equal(orderState(s.o1!), "PAID/700/700");
  assert.equal(payments(), 1);
  const replay = await webhook(s.o1!, 700, "fake-4", "sf-pay-ok");
  assert.equal(replay.ok && replay.replayed, true);
  const duplicateWebhook = await webhook(s.o1!, 700, "fake-4", "sf-pay-dup-key");
  assert.equal(code(duplicateWebhook), "WIRE.ALREADY_EXISTS", "같은 providerRef의 다른 key 콜백은 유일 제약이 막는다");
  const paidAgain = await webhook(s.o1!, 700, "fake-5", "sf-pay-again");
  assert.equal(code(paidAgain), "WIRE.INVALID_STATE", "이미 PAID인 주문은 다시 결제되지 않는다");
  assert.equal(orderState(s.o1!), "PAID/700/700", "paid가 두 번 더해지지 않았다");
  assert.equal(payments(), 1, "실패한 콜백의 원장 행은 롤백");
});

await scenario("S6-state-machine-roles-and-visibility", async () => {
  const r = await checkout("olga", OLGA, [{ product: P.E, qty: 1 }], "sf-checkout-2");
  assert.equal(r.ok, true, JSON.stringify(r));
  s.o2 = r.output.id;
  assert.equal(code(await olga().apply({ apply: "Order.ship", target: { ids: [s.o1!] } } as never, { key: "sf-ship-olga" } as never)), "WIRE.FORBIDDEN", "고객은 발송 못 한다");
  assert.equal(code(await sam().apply({ apply: "Order.ship", target: { ids: [s.o2!] } } as never, { key: "sf-ship-unpaid" } as never)), "WIRE.INVALID_STATE", "결제 전 발송 불가");
  assert.equal(orderState(s.o2!), "PLACED/100/0");
  assert.equal((await sam().apply({ apply: "Order.ship", target: { ids: [s.o1!] } } as never, { key: "sf-ship-ok" } as never)).ok, true);
  assert.equal(orderState(s.o1!), "SHIPPED/700/700");
  assert.equal(code(await webhook(s.o1!, 700, "fake-6", "sf-pay-shipped")), "WIRE.INVALID_STATE", "발송 뒤 결제 불가");
  assert.equal((await pavel().read({ read: "Order", select: ["id"], filter: [{ field: "id", op: "eq", value: s.o1! }] })).rows.length, 0, "다른 고객의 주문은 안 보인다");
  assert.equal((await sam().read({ read: "Order", select: ["id"], filter: [{ field: "id", op: "eq", value: s.o1! }] })).rows.length, 1, "직원은 본다");
  const withLines = (await olga().read({ read: "Order", select: ["id", "total", "state", { OrderLine: { select: ["id", "qty", "unitPrice"], limit: 10 } }], filter: [{ field: "id", op: "eq", value: s.o1! }] } as never)).rows[0] as any;
  assert.equal(withLines.OrderLine.length, 2);
  assert.equal(withLines.state, "SHIPPED");
});

await scenario("S7-cancel-restores-stock-exactly-once-and-only-before-shipping", async () => {
  const beforeShipped = snapshot();
  const shipped = await cancel(olga(), s.o1!, "sf-cancel-shipped");
  assert.equal(shipped.ok, false, "발송된 주문은 취소 불가");
  console.log(`INFO S7-cancel-shipped: ${code(shipped)}`);
  assert.deepEqual(snapshot(), beforeShipped, "실패한 취소는 상태·재고를 안 바꾼다");
  assert.equal(orderState(s.o1!), "SHIPPED/700/700");
  // 결제된 주문 취소 -> 재고 정확히 한 번 반환
  const made = await checkout("olga", OLGA, [{ product: P.A, qty: 2 }, { product: P.B, qty: 1 }], "sf-checkout-3");
  s.o3 = made.output.id;
  const base = { A: stock("A") + 2, B: stock("B") + 1 };
  assert.equal((await webhook(s.o3!, 450, "fake-7", "sf-pay-o3")).ok, true);
  const cancelled = await cancel(olga(), s.o3!, "sf-cancel-paid");
  assert.equal(cancelled.ok, true, JSON.stringify(cancelled));
  assert.equal(cancelled.output.restored, 3);
  assert.deepEqual([stock("A"), stock("B")], [base.A, base.B], "A, B 재고가 주문 전 값으로 돌아왔다");
  assert.equal(orderState(s.o3!), "CANCELLED/450/450");
  const replay = await cancel(olga(), s.o3!, "sf-cancel-paid");
  assert.equal(replay.ok && replay.replayed, true);
  assert.deepEqual([stock("A"), stock("B")], [base.A, base.B], "재전송이 재고를 두 번 돌려주지 않는다");
  const again = await cancel(olga(), s.o3!, "sf-cancel-paid-2");
  assert.equal(again.ok, false, "이미 취소된 주문은 다른 key로도 취소 불가");
  assert.deepEqual([stock("A"), stock("B")], [base.A, base.B]);
  // 직원은 남의 PLACED 주문을 취소할 수 있고, 다른 고객은 못 한다.
  const byOther = await cancel(pavel(), s.o2!, "sf-cancel-pavel");
  assert.equal(byOther.ok, false, "다른 고객은 남의 주문 취소 불가");
  console.log(`INFO S7-other-customer-cancel: ${code(byOther)}`);
  const e0 = stock("E");
  const bySam = await cancel(sam(), s.o2!, "sf-cancel-sam");
  assert.equal(bySam.ok, true, JSON.stringify(bySam));
  assert.equal(stock("E"), e0 + 1);
  // 직접 호출 통로는 닫혀 있다.
  const direct = [
    await olga().apply({ apply: "Product.giveBack", target: { ids: [P.A] }, input: { qty: 5 } } as never, { key: "sf-direct-giveback" } as never),
    await olga().apply({ apply: "Order.cancel", target: { ids: [s.o1!] } } as never, { key: "sf-direct-cancel" } as never),
    await olga().apply({ apply: "Product.receive", target: { ids: [P.A] }, input: { qty: 5 } } as never, { key: "sf-direct-receive" } as never),
  ];
  assert.deepEqual(direct.map(code), ["WIRE.NOT_EXPOSED", "WIRE.NOT_EXPOSED", "WIRE.FORBIDDEN"]);
});

await scenario("S8a-concurrent-checkouts-never-oversell", async () => {
  const c0 = stock("C"); // 4
  const who = ["olga", "pavel", "olga", "pavel"];
  const owner = [OLGA, PAVEL, OLGA, PAVEL];
  const started = Date.now();
  const results = await Promise.all(who.map((w, i) => checkout(w, owner[i]!, [{ product: P.C, qty: 2 }], `sf-race-${i}`)));
  const ok = results.filter((r) => r.ok).length;
  console.log(`INFO S8a-concurrent-checkout: 재고 ${c0}, 2개짜리 4건 동시 -> 성공 ${ok}건 (${results.map(code).join("|")}), 최종 재고 ${stock("C")}, ${Date.now() - started}ms`);
  assert.ok(stock("C") >= 0, "재고는 음수가 되지 않는다");
  assert.equal(stock("C"), c0 - 2 * ok, "재고 감소량 = 성공한 주문의 수량 합");
  assert.equal(count("order_line", `product_id=${P.C}`), ok, "성공한 주문의 줄만 남는다");
  assert.ok(ok >= 1 && ok <= 2);
  assert.equal(sql(`SELECT count(*) FROM ${schema}."order" o WHERE NOT EXISTS (SELECT 1 FROM ${schema}.order_line l WHERE l.order_id=o.id)`), "0", "줄 없는 고아 주문이 없다");
});

await scenario("S8b-concurrent-double-cancel-restores-once", async () => {
  const made = await checkout("olga", OLGA, [{ product: P.E, qty: 2 }], "sf-checkout-dc");
  const id = made.output.id;
  const afterOrder = stock("E");
  const results = await Promise.all([cancel(olga(), id, "sf-dc-1"), cancel(sam(), id, "sf-dc-2")]);
  console.log(`INFO S8b-double-cancel: ${results.map(code).join("|")}, 재고 ${afterOrder} -> ${stock("E")}`);
  assert.equal(results.filter((r) => r.ok).length, 1, "동시 취소 두 건 중 정확히 한 건만 성공");
  assert.equal(stock("E"), afterOrder + 2, "재고는 정확히 한 번 돌아온다");
  assert.equal(orderState(id), "CANCELLED/200/0");
});

await scenario("S8c-concurrent-duplicate-payment-callback-pays-once", async () => {
  const made = await checkout("olga", OLGA, [{ product: P.F, qty: 1 }], "sf-checkout-dp");
  const id = made.output.id;
  const p0 = payments();
  const results = await Promise.all([webhook(id, 100, "fake-race", "sf-race-pay-1"), webhook(id, 100, "fake-race", "sf-race-pay-2")]);
  console.log(`INFO S8c-duplicate-callback: ${results.map(code).join("|")}`);
  assert.equal(results.filter((r) => r.ok).length, 1);
  assert.equal(payments(), p0 + 1, "결제 원장 한 건");
  assert.equal(orderState(id), "PAID/100/100", "paid는 한 번만 더해졌다");
});

await scenario("S9-field-limits-and-staff-only-restock", async () => {
  const cost = async (who: Client) => (await who.read({ read: "Product", select: ["id", "cost"], filter: [{ field: "id", op: "eq", value: P.A }] })).rows[0]?.cost;
  assert.equal(await cost(olga()), null, "고객에게 원가는 null");
  assert.equal(await cost(sam()), 120, "직원은 원가를 본다");
  const a0 = stock("A");
  assert.equal((await olga().apply({ apply: "Product.receive", target: { ids: [P.A] }, input: { qty: 5 } } as never, { key: "sf-recv-olga" } as never)).ok, false);
  assert.equal(stock("A"), a0);
  assert.equal((await sam().apply({ apply: "Product.receive", target: { ids: [P.A] }, input: { qty: 5 } } as never, { key: "sf-recv-sam" } as never)).ok, true);
  assert.equal(stock("A"), a0 + 5);
  const zero = await sam().apply({ apply: "Product.receive", target: { ids: [P.A] }, input: { qty: 0 } } as never, { key: "sf-recv-zero" } as never);
  assert.equal(code(zero), "WIRE.BAD_VALUE");
  assert.equal(stock("A"), a0 + 5, "잘못된 입력은 재고를 안 바꾼다");
});

await scenario("S10-direct-create-cannot-forge-total-paid-state-or-lines", async () => {
  const before = snapshot();
  const create = (values: Record<string, unknown>, key: string) => olga().bundle({ atomic: [{ create: "Order", values }] } as never, { key } as never);
  const forged = [
    await create({ owner: OLGA, total: 1, paid: 0, state: "PLACED" }, "sf-forge-total"),
    await create({ owner: OLGA, total: 0, paid: 700, state: "PLACED" }, "sf-forge-paid"),
    await create({ owner: OLGA, total: 0, paid: 0, state: "PAID" }, "sf-forge-state"),
    await create({ owner: PAVEL, total: 0, paid: 0, state: "PLACED" }, "sf-forge-owner"),
  ];
  // 남의 owner는 Member 행이 보이지 않아 FORBIDDEN이 아니라 존재 은폐(MISSING_TARGET)다.
  assert.deepEqual(forged.map(code), ["WIRE.FORBIDDEN", "WIRE.FORBIDDEN", "WIRE.FORBIDDEN", "WIRE.MISSING_TARGET"]);
  const toPaid = await olga().bundle({ atomic: [{ create: "OrderLine", values: { order: s.o1!, product: P.B, qty: 1, unitPrice: 50 } }] } as never, { key: "sf-late-line" } as never);
  assert.equal(code(toPaid), "WIRE.FORBIDDEN", "이미 합계가 확정된 주문에 줄을 더할 수 없다");
  assert.equal(code(await olga().apply({ apply: "Order.setTotal", target: { ids: [s.o1!] }, values: { total: 1 } } as never, { key: "sf-set-total" } as never)), "WIRE.NOT_EXPOSED");
  assert.deepEqual(snapshot(), before);
  // 정상 create는 빈 주문(합계 0)만 만들고, 합계 0 주문은 결제할 수 없다.
  const empty = await create({ owner: OLGA, total: 0, paid: 0, state: "PLACED" }, "sf-empty-order");
  assert.equal(empty.ok, true, JSON.stringify(empty));
  const emptyId = (empty as any).created[0].id;
  const pay = await webhook(emptyId, 1, "fake-empty", "sf-pay-empty");
  assert.equal(pay.ok, false, "합계 0 주문은 어떤 금액으로도 결제 불가");
  console.log(`INFO S10-empty-order-pay: ${code(pay)}`);
  assert.equal(orderState(emptyId), "PLACED/0/0");
});
