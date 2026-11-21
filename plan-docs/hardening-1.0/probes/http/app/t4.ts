import { connect, WriteUnsettled } from "@aip/sdk";
import { contract } from "./binding.ts";
import fs from "node:fs";
const TOKEN = fs.readFileSync("token.txt", "utf8").trim();
const log = (l: string, x: unknown) => console.log(l, JSON.stringify(x));
let n = 0;
const flaky: typeof fetch = async (input: any, init: any) => {
  if (String(input).endsWith("/apply")) { n++; throw new TypeError("network down"); }
  return fetch(input, init);
};
const client = connect("http://127.0.0.1:18471", TOKEN, contract, flaky);
try { await client.apply({ apply: "Product.restock", target: { ids: ["2"] }, input: { amount: "1.00" } } as any, { key: "ts-un-" + Date.now() }); log("no throw", n); }
catch (e: any) { log("unsettled", { isWU: e instanceof WriteUnsettled, name: e?.name, code: e?.code, retry: e?.retry, requestId: e?.requestId, keys: Object.keys(e), n }); }
// COMMIT_UNKNOWN-like server body
const unknown: typeof fetch = async (input: any, init: any) => {
  if (String(input).endsWith("/apply")) return new Response(JSON.stringify({ ok: false, code: "WIRE.COMMIT_UNKNOWN", msg: "x", retry: "check-first", requestId: "req_abcdef123456" }), { status: 200, headers: { "content-type": "application/json", "x-request-id": "req_abcdef123456" } });
  return fetch(input, init);
};
const c2 = connect("http://127.0.0.1:18471", TOKEN, contract, unknown);
try { const r = await c2.apply({ apply: "Product.restock", target: { ids: ["2"] }, input: { amount: "1.00" } } as any, { key: "ts-un2-" + Date.now() }); log("c2 no throw", r); }
catch (e: any) { log("unsettled2", { isWU: e instanceof WriteUnsettled, code: e?.code, retry: e?.retry, requestId: e?.requestId }); }
// retry value normalization: unknown retry string
const odd: typeof fetch = async (input: any, init: any) => {
  if (String(input).endsWith("/read")) return new Response(JSON.stringify({ ok: false, code: "WIRE.RATE_LIMITED", msg: "x", retry: "sometime", requestId: "req_abcdef123456" }), { status: 200 });
  return fetch(input, init);
};
const c3 = connect("http://127.0.0.1:18471", TOKEN, contract, odd);
try { await c3.read({ read: "Product", select: ["id"] } as any); } catch (e: any) { log("odd retry", { code: e?.code, retry: e?.retry }); }
