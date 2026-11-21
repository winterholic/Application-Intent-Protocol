import { connect, protocolWarning } from "@aip/sdk";
import * as sdk from "@aip/sdk";
import { contract } from "./binding.ts";
import fs from "node:fs";
const TOKEN = fs.readFileSync("token.txt", "utf8").trim();
const log = (l: string, x: unknown) => console.log(l, JSON.stringify(x));
const client = connect("http://127.0.0.1:18471", TOKEN, contract);
console.log("exports:", Object.keys(sdk).sort().join(","));
log("session", await client.session());
// builder
const r1: any = await client.bundle((b: any) => {
  const order = b.create("Order", { owner: "42", total: "9.00", status: "open" });
  b.create("OrderLine", { product: "2", qty: 1, amount: "2.50" }, { order });
  b.apply({ apply: "Product.take", target: { ids: ["2"] }, input: { qty: 1 } });
}, { key: "ts-b1-" + Date.now() });
log("builder bundle", r1);
// direct ops
const r2: any = await client.bundle({ atomic: [
  { create: "Order", values: { owner: "42", total: "1.00", status: "open" } },
  { apply: "Order.confirm", target: { created: 0 } },
] } as any, { key: "ts-b2-" + Date.now() });
log("direct target.created", r2);
// apply reject => result object
const r3: any = await client.apply({ apply: "Product.take", target: { ids: ["1"] }, input: { qty: 1000 } } as any, { key: "ts-r3-" + Date.now() });
log("apply reject (INVALID_STATE)", r3);
// read reject => throws
try { await client.read({ read: "Product", select: ["nope"] } as any); } catch (e: any) { log("read reject throws", { name: e?.name, code: e?.code, retry: e?.retry, requestId: e?.requestId, keys: Object.keys(e) }); }
// anonymous
const anon = connect("http://127.0.0.1:18471", null, contract);
log("anon session", await anon.session());
for (const t of ["", "  ", undefined as any]) { try { connect("http://127.0.0.1:18471", t, contract); log("empty token", "NO THROW at connect"); } catch (e: any) { log("empty token", { code: e?.code }); } }
try { await connect("http://127.0.0.1:18471", "" as any, contract).session(); } catch (e: any) { log("empty token session", { code: e?.code }); }
log("protocolWarning", [protocolWarning("aip-protocol/1.3"), protocolWarning("aip-protocol/2.0"), protocolWarning("garbage")]);
// compose
const r4: any = await client.compose({ compose: "Order", targets: { ids: ["1"] }, steps: [{ transition: "confirm" }, { create: "Note", values: { owner: { item: "owner" } } }] } as any, { key: "ts-c1-" + Date.now() });
log("compose", r4);
// wrong contract
const bad = connect("http://127.0.0.1:18471", TOKEN, { ...contract, fingerprint: "0".repeat(64) } as any);
try { const x = await bad.read({ read: "Product", select: ["id"] } as any); log("bad fp read", x);} catch (e: any) { log("bad fp read throws", { code: e?.code, retry: e?.retry }); }
