// StockDesk 소비자 e2e 두 번째 언어: TypeScript SDK(tarball 설치)와 생성된 binding으로 같은 서버를 호출한다.
// Python 시나리오(e2e.py)와 같은 업무(테넌트 격리, 주문+줄, 가격 위조 거부, 확장 주문 확정, 재고 롤백, 멱등)를 TypeScript 경로로 확인한다.
import { connect } from "@aip/sdk";
import { contract } from "./binding.ts";
import assert from "node:assert/strict";

const URL_ = process.env.AIP_URL!;
const run = process.env.RUN_ID ?? String(Date.now());
const as = (token: string) => connect(URL_, token, contract);
const mgr1 = as(process.env.TOKEN_MGR1!), clerk = as(process.env.TOKEN_CLERK1!), mgr2 = as(process.env.TOKEN_MGR2!);
const anon = connect(URL_, null, contract);
let n = 0;
const step = (name: string, extra?: unknown) => console.log(`PASS ${String(++n).padStart(2, "0")} ${name}${extra === undefined ? "" : " " + JSON.stringify(extra)}`);
const code = (r: unknown) => (r as { code?: string }).code;
const stocks = async (prefix: string) => Object.fromEntries(
  (await mgr1.read({ read: "Product", select: ["sku", "stock"], filter: [{ field: "sku", op: "prefix", value: prefix }] } as any) as any).rows.map((p: any) => [p.sku, p.stock]));

assert.equal((await mgr1.session()).actorId, "1"); step("session mgr1 (typescript)");
const tenants = await mgr1.read({ read: "Tenant", select: ["id", "name"] } as any) as any;
assert.deepEqual(tenants.rows.map((t: any) => t.name), ["Gangnam Store"]); step("tenant isolation: only own tenant visible");

const sku = `TS-${run}`;
const made = await mgr1.bundle({ atomic: [
  { create: "Product", values: { tenant: "1", sku: `${sku}-A`, name: "Widget", unitPrice: "10.50", stock: 100, reorderLevel: 10 } },
  { create: "Product", values: { tenant: "1", sku: `${sku}-B`, name: "Gadget", unitPrice: "3.25", stock: 5, reorderLevel: 2 } },
] } as any, { key: `ts-prod-${run}` }) as any;
assert.ok(made.ok, JSON.stringify(made)); const [pa, pb] = made.created.map((c: any) => c.id); step("manager creates 2 products atomically", { pa, pb });
const clerkProduct = await clerk.bundle({ atomic: [{ create: "Product", values: { tenant: "1", sku: `${sku}-C`, name: "x", unitPrice: "1.00", stock: 1, reorderLevel: 0 } }] } as any, { key: `ts-prodc-${run}` }) as any;
assert.equal(clerkProduct.ok, false); step("clerk create product -> denied", code(clerkProduct));
const dup = await mgr1.bundle({ atomic: [{ create: "Product", values: { tenant: "1", sku: `${sku}-A`, name: "dup", unitPrice: "1.00", stock: 1, reorderLevel: 0 } }] } as any, { key: `ts-dup-${run}` }) as any;
assert.equal(dup.code, "WIRE.ALREADY_EXISTS"); step("duplicate sku in tenant -> WIRE.ALREADY_EXISTS", dup.code);

const o = await clerk.bundle({ atomic: [{ create: "Order", values: { tenant: "1", placedBy: "2", number: `TO-${run}-1`, status: "OPEN", total: "0.00" } }] } as any, { key: `ts-ord-${run}` }) as any;
assert.ok(o.ok, JSON.stringify(o)); const oid = o.created[0].id;
const forged = await clerk.bundle({ atomic: [{ create: "OrderLine", values: { order: oid, product: pa, qty: 2, unitPrice: "0.01" } }] } as any, { key: `ts-forged-${run}` }) as any;
assert.equal(forged.ok, false); step("order line with tampered price -> denied", code(forged));
const lines = await clerk.bundle({ atomic: [
  { create: "OrderLine", values: { order: oid, product: pa, qty: 4, unitPrice: "10.50" } },
  { create: "OrderLine", values: { order: oid, product: pb, qty: 2, unitPrice: "3.25" } },
  { create: "OrderLine", values: { order: oid, product: pa, qty: 1, unitPrice: "10.50" } },
] } as any, { key: `ts-lines-${run}` }) as any;
assert.ok(lines.ok, JSON.stringify(lines)); step("clerk creates order + 3 lines", { order: oid });

const placed = await clerk.writeExtension("Order.place", { order: oid }, { key: `ts-place-${run}` }) as any;
assert.ok(placed.ok, JSON.stringify(placed)); assert.equal(placed.output.total, "59.00"); step("python write extension Order.place from typescript", placed.output);
assert.deepEqual(await stocks(sku), { [`${sku}-A`]: 95, [`${sku}-B`]: 3 }); step("stock decremented atomically");
const replay = await clerk.writeExtension("Order.place", { order: oid }, { key: `ts-place-${run}` }) as any;
assert.equal(replay.replayed, true);
assert.deepEqual(await stocks(sku), { [`${sku}-A`]: 95, [`${sku}-B`]: 3 }); step("same key replay does not double-decrement", { replayed: true });

const o2 = await clerk.bundle({ atomic: [{ create: "Order", values: { tenant: "1", placedBy: "2", number: `TO-${run}-2`, status: "OPEN", total: "0.00" } }] } as any, { key: `ts-ord2-${run}` }) as any;
const oid2 = o2.created[0].id;
await clerk.bundle({ atomic: [
  { create: "OrderLine", values: { order: oid2, product: pa, qty: 1, unitPrice: "10.50" } },
  { create: "OrderLine", values: { order: oid2, product: pb, qty: 99, unitPrice: "3.25" } },
] } as any, { key: `ts-lines2-${run}` });
const short = await clerk.writeExtension("Order.place", { order: oid2 }, { key: `ts-place2-${run}` }) as any;
assert.equal(short.ok, false);
assert.deepEqual(await stocks(sku), { [`${sku}-A`]: 95, [`${sku}-B`]: 3 }); step("insufficient stock -> rejected, nothing changed", { code: code(short) });

const orders = await mgr1.read({ read: "Order", select: ["id", "number", "status", "total", "lineCount", { placedBy: { select: ["id", "name"] } }, { OrderLine: { select: ["id", "qty", "unitPrice"], limit: 10 } }],
  filter: [{ field: "status", op: "eq", value: "CONFIRMED" }], sort: [{ field: "id", dir: "desc" }] } as any) as any;
const top = orders.rows.find((r: any) => r.id === oid);
assert.ok(top && top.lineCount === 3 && top.placedBy.name === "Joo" && top.OrderLine.length === 3 && top.total === "59.00", JSON.stringify(top));
step("relation read: order + placedBy + lines + lineCount aggregate", { id: top.id, lineCount: top.lineCount });

assert.equal(((await mgr2.read({ read: "Order", select: ["id"] } as any)) as any).rows.length, 0); step("other tenant manager sees no orders");
const cross = await mgr2.apply({ apply: "Order.cancel", target: { ids: [oid2] } } as any, { key: `ts-x-${run}` }) as any;
assert.equal(cross.ok, false); step("other tenant cancel -> denied", code(cross));
const clerkRestock = await clerk.apply({ apply: "Product.restock", target: { ids: [pa] }, values: { stock: 500 }, expect: { stock: 95 } } as any, { key: `ts-cr-${run}` }) as any;
assert.equal(clerkRestock.ok, false); step("clerk restock -> denied", code(clerkRestock));
const internal = await mgr1.apply({ apply: "Product.take", target: { ids: [pa] }, values: { stock: 0 }, expect: { stock: 95 } } as any, { key: `ts-take-${run}` }) as any;
assert.equal(internal.ok, false); step("internal transition Product.take not callable publicly", code(internal));
assert.equal(((await anon.read({ read: "Product", select: ["id"] } as any)) as any).rows.length, 0); step("anonymous read -> no rows");

const restock = await mgr1.apply({ apply: "Product.restock", target: { ids: [pa] }, values: { stock: 195 }, expect: { stock: 95 } } as any, { key: `ts-restock-${run}` }) as any;
assert.ok(restock.ok, JSON.stringify(restock));
const stale = await mgr1.apply({ apply: "Product.restock", target: { ids: [pa] }, values: { stock: 300 }, expect: { stock: 95 } } as any, { key: `ts-restock2-${run}` }) as any;
assert.equal(stale.code, "WIRE.CONFLICT"); step("restock then stale expect -> WIRE.CONFLICT", stale.code);
const cancel = await mgr1.apply({ apply: "Order.cancel", target: { ids: [oid2] } } as any, { key: `ts-cancel-${run}` }) as any;
assert.ok(cancel.ok); step("manager cancels open order");
console.log("ALL PASS");
