// D3: TypeScript·Python SDK가 같은 서비스(PG17)에서 같은 의미를 내는지 확인한다.
// nullable/optional, Decimal·Id·Uuid, input 산술, refs와 target.created, bundle·compose, 쓰기 확장 결과, 오류 코드와 재시도 표지,
// 응답 유실 뒤 복구, 계약 지문 불일치를 같은 순서로 두 언어에서 실행하고, 정규화한 결과가 두 언어에서 같고 기대표와도 같은지 단언한다.
// TypeScript 쪽 스크립트는 tsc --strict로도 컴파일해 생성 binding 타입이 이 호출들을 받는지 함께 본다(타입은 권한을 보장하지 않는다).
//
//   cargo build -p aip-cli && node --test --test-concurrency=1 product/tests/sdk-contract-semantics.test.mjs
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, test } from "node:test";
import { startFixture, cleanupTemp, root, run, tsc } from "../test-support/service-fixture.mjs";

const definition = readFileSync(join(root, "product/tests/fixtures/sdk-semantics/app.aip"), "utf8");
const extension = readFileSync(join(root, "product/tests/fixtures/sdk-semantics/extensions/sdk.mjs"), "utf8");
const sdkEntry = join(root, "product/sdk/index.ts");
const pythonPath = join(root, "product/python");

const A1 = "aaaaaaaa-0000-4000-8000-000000000001"; // alice, label mine: secret visible
const A2 = "aaaaaaaa-0000-4000-8000-000000000002"; // alice, label other: secret hidden by the field policy
const A3 = "aaaaaaaa-0000-4000-8000-000000000003"; // bob: invisible to alice
const SEED = `
INSERT INTO \${S}.member(id,name) VALUES (1,'alice'),(2,'bob');
INSERT INTO \${S}.account(id,owner_id,label,balance,secret) VALUES
  ('${A1}',1,'mine','20.00','sec-1'),('${A2}',1,'other','1.00','sec-2'),('${A3}',2,'bobs','7.00','sec-3');
INSERT INTO \${S}.product(id,owner_id,stock,price) VALUES (1,1,10,'2.50');
INSERT INTO \${S}."order"(id,owner_id,status,total) VALUES (1,1,'open','1.00'),(2,1,'open','1.00');
SELECT setval(pg_get_serial_sequence('\${S}."order"','id'), 10);
SELECT setval(pg_get_serial_sequence('\${S}.product','id'), 10);
`;

const fixtures = [];
after(async () => {
  for (const f of fixtures) await f.stop();
  cleanupTemp();
});

// ---------------------------------------------------------------------------------------------------------------------------
// 두 언어가 같은 단계를 같은 이름으로 기록한다. 값은 정규화한다: 요청 ID는 "<req>", 새로 만든 uuid는 "<uuid>".
// ---------------------------------------------------------------------------------------------------------------------------
const tsScript = (f) => `
import { connect, WriteUnsettled } from ${JSON.stringify(sdkEntry)};
import { contract } from "./binding.ts";
import type { Id } from "./binding.ts";

const base = ${JSON.stringify(f.base)};
const token = ${JSON.stringify(f.tokens.alice)};
const client = connect(base, token, contract);
const id = <R extends string>(value: string) => value as Id<R>;
const A1 = id<"Account">(${JSON.stringify(A1)});
const A2 = id<"Account">(${JSON.stringify(A2)});
const A3 = id<"Account">(${JSON.stringify(A3)});
const P1 = id<"Product">("1");
const alice = id<"Member">("1");
const out: Record<string, unknown> = {};
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
function norm(value: unknown): unknown {
  if (typeof value === "string") return UUID.test(value) && ![A1, A2, A3].includes(value as never) ? "<uuid>" : /^req_[0-9a-f]+$/.test(value) ? "<req>" : value;
  if (Array.isArray(value)) return value.map(norm);
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, k === "msg" ? "<msg>" : norm(v)]));
  return value;
}
const rec = async (step: string, work: () => Promise<unknown>) => {
  try { out[step] = norm(await work()); } catch (error: any) { out[step] = norm({ threw: error.code ?? error.name, retry: error.retry, requestId: error.requestId }); }
};
const row = async (resource: "Account", select: readonly ("id" | "label" | "memo" | "cap" | "balance" | "secret")[], target: Id<"Account">) =>
  (await client.read({ read: resource, select, filter: [{ field: "id", op: "eq", value: target }] })).rows[0];
// 읽기 캐시는 클라이언트마다 따로이고 자기 쓰기로만 무효화된다. 다른 클라이언트(lossy)의 쓰기를 보려면 새 클라이언트로 읽는다.
const stock = async () => (await connect(base, token, contract).read({ read: "Product", select: ["stock"] })).rows[0].stock;

// nullable / optional
let created: any;
await rec("n01 create without the optional memo", async () => (created = await client.bundle({ atomic: [{ create: "Account", values: { owner: alice, label: "n1", balance: "5.00" } }] })));
const N = id<"Account">(created.created[0].id);
await rec("n02 read the new row", () => row("Account", ["id", "label", "memo", "cap", "balance"], N));
await rec("n03 patch memo and cap", () => client.apply({ apply: "Account.rename", target: { ids: [N] }, values: { memo: "m", cap: "3.00" } }));
await rec("n04 read after patch", () => row("Account", ["label", "memo", "cap"], N));
await rec("n05 patch only the label", () => client.apply({ apply: "Account.rename", target: { ids: [N] }, values: { label: "n1b" } }));
await rec("n06 omitted fields kept", () => row("Account", ["label", "memo", "cap"], N));
await rec("n07 explicit null clears", () => client.apply({ apply: "Account.rename", target: { ids: [N] }, values: { memo: null, cap: null } }));
await rec("n08 read after null", () => row("Account", ["label", "memo", "cap"], N));
await rec("n09 null into a required field", () => client.apply({ apply: "Account.rename", target: { ids: [N] }, values: { label: null as never } }));
await rec("n10 field policy redaction", async () => ({ mine: (await row("Account", ["secret"], A1)).secret, other: (await row("Account", ["secret"], A2)).secret }));

// Decimal
for (const [i, amount] of ["0.01", "0.01", "0.01"].entries()) await rec(\`d0\${i + 1} deposit \${amount}\`, () => client.apply({ apply: "Account.deposit", target: { ids: [N] }, input: { amount } }, { key: \`dec-\${i}-0001\` }));
await rec("d04 balance is exact", async () => (await row("Account", ["balance"], N)).balance);
await rec("d05 result over the column precision", () => client.apply({ apply: "Account.deposit", target: { ids: [N] }, input: { amount: "9999999999.99" } }));
await rec("d06 three decimals", () => client.apply({ apply: "Account.deposit", target: { ids: [N] }, input: { amount: "0.001" } }));
await rec("d07 withdraw more than the balance", () => client.apply({ apply: "Account.withdraw", target: { ids: [N] }, input: { amount: "5.04" } }));
await rec("d08 withdraw exactly the balance", () => client.apply({ apply: "Account.withdraw", target: { ids: [N] }, input: { amount: "5.03" } }));
await rec("d09 balance after", async () => (await row("Account", ["balance"], N)).balance);
await rec("d10 non-positive amount", () => client.apply({ apply: "Account.deposit", target: { ids: [N] }, input: { amount: "0.00" } }));

// Id / Uuid
await rec("u01 numeric id on a Uuid resource", () => client.apply({ apply: "Account.deposit", target: { ids: [id<"Account">("1")] }, input: { amount: "1.00" } }));
await rec("u02 malformed uuid", () => client.apply({ apply: "Account.deposit", target: { ids: [id<"Account">("not-a-uuid")] }, input: { amount: "1.00" } }));
await rec("u03 another actor's row is missing", () => client.apply({ apply: "Account.deposit", target: { ids: [A3] }, input: { amount: "1.00" } }));
await rec("u04 reference to a Uuid resource", async () => {
  const made = await client.bundle({ atomic: [{ create: "Note", values: { account: A1, body: "n", reviewer: null } }] });
  const page = await client.read({ read: "Note", select: ["id", "account", "reviewer"] });
  return { made, rows: page.rows };
});
await rec("u05 create against a row of another owner", () => client.bundle({ atomic: [{ create: "Note", values: { account: A3, body: "x", reviewer: null } }] }));

// input arithmetic and replay
const take = { apply: "Product.take", target: { ids: [P1] }, input: { qty: 3 } } as const;
await rec("i01 take 3", () => client.apply(take, { key: "take-3-0001" }));
await rec("i02 stock after", async () => stock());
await rec("i03 take more than the stock", () => client.apply({ apply: "Product.take", target: { ids: [P1] }, input: { qty: 8 } }));
await rec("i04 qty 0", () => client.apply({ apply: "Product.take", target: { ids: [P1] }, input: { qty: 0 } }));
await rec("i05 qty above the declared range", () => client.apply({ apply: "Product.take", target: { ids: [P1] }, input: { qty: 101 } }));
await rec("i06 replay of the same key", () => client.apply(take, { key: "take-3-0001" }));
await rec("i07 stock unchanged by the replay", async () => stock());
await rec("i08 same key, other body", () => client.apply({ apply: "Product.take", target: { ids: [P1] }, input: { qty: 4 } }, { key: "take-3-0001" }));

// refs / target.created, bundle
await rec("r01 order, two lines, confirm and take in one bundle", () => client.bundle((b) => {
  const order = b.create("Order", { owner: alice, status: "open", total: "5.00" });
  b.create("OrderLine", { product: P1, qty: 1, price: "2.50" }, { order });
  b.create("OrderLine", { product: P1, qty: 1, price: "2.50", note: null }, { order });
  b.apply({ apply: "Order.confirm", target: order });
  b.apply({ apply: "Product.take", target: { ids: [P1] }, input: { qty: 2 } });
}));
await rec("r02 stock after the bundle", async () => stock());
await rec("r03 lines point at the new order", async () => (await client.read({ read: "OrderLine", select: ["qty", "price", "note", "order", "product"] })).rows);
await rec("r04 a failing step cancels the earlier creates", () => client.bundle((b) => {
  const order = b.create("Order", { owner: alice, status: "open", total: "9.00" });
  b.create("OrderLine", { product: P1, qty: 1, price: "9.00" }, { order });
  b.apply({ apply: "Product.take", target: { ids: [P1] }, input: { qty: 99 } });
}));
await rec("r05 nothing was kept", async () => ({ stock: await stock(), totals: (await client.read({ read: "Order", select: ["total"], sort: [{ field: "id", dir: "asc" }] })).rows.map((r) => r.total) }));
let stale: any;
await client.bundle((b) => { stale = b.create("Order", { owner: alice, status: "open", total: "1.00" }); });
await rec("r06 a handle from another bundle", () => client.bundle((b) => { b.apply({ apply: "Order.confirm", target: stale }); }));
await rec("r07 an empty builder", () => client.bundle(() => {}));

// compose, also inside a bundle with a Uuid resource
await rec("c01 compose confirm", () => client.compose({ compose: "Order", targets: { ids: [id<"Order">("1")] }, steps: [{ transition: "confirm" }] }));
await rec("c02 compose again on a confirmed order", () => client.compose({ compose: "Order", targets: { ids: [id<"Order">("1")] }, steps: [{ transition: "confirm" }] }));
await rec("c03 bundle with a compose over a Uuid resource and another apply", () => client.bundle({ atomic: [
  { compose: "Account", targets: { ids: [A1] }, steps: [{ transition: "touch" }, { transition: "touch" }] },
  { apply: "Product.take", target: { ids: [P1] }, input: { qty: 1 } },
] }));
await rec("c04 stock after the bundle", async () => stock());
await rec("c05 pending is empty", async () => client.pending());

// write extension result
await rec("x01 adjust", () => client.writeExtension("Account.adjust", { a: A1, amount: "2.50", fail: false }));
await rec("x02 business refusal", () => client.writeExtension("Account.adjust", { a: A1, amount: "2.50", fail: true }));
await rec("x03 balance after", async () => (await row("Account", ["balance"], A1)).balance);
await rec("x04 invalid Decimal input", async () => { try { return await client.writeExtension("Account.adjust", { a: A1, amount: "2.505", fail: false }); } catch (error: any) { return { ok: false, code: error.code }; } });

// lost response and unsettled
let drop = 0;
const flaky: typeof fetch = async (input, init) => { const response = await fetch(input, init); if (drop > 0 && String(input).endsWith("/apply")) { drop--; throw new TypeError("response lost"); } return response; };
const lossy = connect(base, token, contract, flaky);
const before = await stock();
drop = 1;
await rec("l01 one lost response is recovered with the same key", () => lossy.apply({ apply: "Product.take", target: { ids: [P1] }, input: { qty: 1 } }, { key: "lost-0001" }));
await rec("l02 executed once", async () => before - (await stock()));
drop = 3;
await rec("l03 every attempt lost", () => lossy.apply({ apply: "Product.take", target: { ids: [P1] }, input: { qty: 1 } }, { key: "lost-0002", retries: 2 }));
await rec("l04 key kept as pending", async () => lossy.pending());
await rec("l05 recovery replays the stored result", async () => (await lossy.retryPending()).map((r: any) => ({ ok: r.ok, replayed: r.replayed })));
await rec("l06 executed once more", async () => before - (await stock()));
drop = 3;
await rec("l07 keyless call after a lost write", () => lossy.apply({ apply: "Product.take", target: { ids: [P1] }, input: { qty: 1 } }, { retries: 0 }));
await rec("l08 the same keyless call reuses the pending key", () => lossy.apply({ apply: "Product.take", target: { ids: [P1] }, input: { qty: 1 } }));
await rec("l09 executed once more", async () => before - (await stock()));

// contract drift
const drift = connect(base, token, { ...contract, fingerprint: "0".repeat(64) });
const stockBefore = await stock();
await rec("f01 read", () => drift.read({ read: "Account", select: ["id"] }));
await rec("f02 apply", () => drift.apply({ apply: "Product.take", target: { ids: [P1] }, input: { qty: 1 } }));
await rec("f03 bundle", () => drift.bundle({ atomic: [{ apply: "Product.take", target: { ids: [P1] }, input: { qty: 1 } }] }));
await rec("f04 compose", () => drift.compose({ compose: "Order", targets: { ids: [id<"Order">("2")] }, steps: [{ transition: "confirm" }] }));
await rec("f05 write extension", () => drift.writeExtension("Account.adjust", { a: A1, amount: "1.00", fail: false }));
await rec("f06 nothing executed", async () => stockBefore - (await stock()));
await rec("f07 retry guidance on a business failure", () => client.apply({ apply: "Order.confirm", target: { ids: [id<"Order">("1")] } }));

console.log(JSON.stringify(out));
`;

const pyScript = (f) => `
import json, re, sys
sys.path.insert(0, ${JSON.stringify(pythonPath)})
import aip_sdk
from aip_sdk import connect, created, AipError, WriteUnsettled, _urllib_transport
import binding

base = ${JSON.stringify(f.base)}
token = ${JSON.stringify(f.tokens.alice)}
client = connect(base, token, binding.CONTRACT_FINGERPRINT)
A1, A2, A3 = ${JSON.stringify(A1)}, ${JSON.stringify(A2)}, ${JSON.stringify(A3)}
KNOWN = {A1, A2, A3}
UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
out = {}

def norm(value):
    if isinstance(value, str):
        if UUID.match(value) and value not in KNOWN: return "<uuid>"
        if re.match(r"^req_[0-9a-f]+$", value): return "<req>"
        return value
    if isinstance(value, list): return [norm(v) for v in value]
    if isinstance(value, dict): return {k: ("<msg>" if k == "msg" else norm(v)) for k, v in value.items()}
    return value

def rec(step, work):
    try:
        out[step] = norm(work())
    except AipError as error:
        entry = {"threw": error.code}
        if error.retry: entry["retry"] = error.retry
        if error.request_id: entry["requestId"] = error.request_id
        out[step] = norm(entry)

def row(select, target):
    return client.read({"read": "Account", "select": select, "filter": [{"field": "id", "op": "eq", "value": target}]})[0]

def stock():
    return client.read({"read": "Product", "select": ["stock"]})[0]["stock"]

def apply(request, **options):
    return client.apply(request, **options)

# nullable / optional
created_account = client.create("Account", {"owner": "1", "label": "n1", "balance": "5.00"})
out["n01 create without the optional memo"] = norm(created_account)
N = created_account["created"][0]["id"]
rec("n02 read the new row", lambda: row(["id", "label", "memo", "cap", "balance"], N))
rec("n03 patch memo and cap", lambda: binding.apply_account_rename(client, {"ids": [N]}, {"memo": "m", "cap": "3.00"}))
rec("n04 read after patch", lambda: row(["label", "memo", "cap"], N))
rec("n05 patch only the label", lambda: binding.apply_account_rename(client, {"ids": [N]}, {"label": "n1b"}))
rec("n06 omitted fields kept", lambda: row(["label", "memo", "cap"], N))
rec("n07 explicit null clears", lambda: binding.apply_account_rename(client, {"ids": [N]}, {"memo": None, "cap": None}))
rec("n08 read after null", lambda: row(["label", "memo", "cap"], N))
rec("n09 null into a required field", lambda: binding.apply_account_rename(client, {"ids": [N]}, {"label": None}))
rec("n10 field policy redaction", lambda: {"mine": row(["secret"], A1)["secret"], "other": row(["secret"], A2)["secret"]})

# Decimal
for i, amount in enumerate(["0.01", "0.01", "0.01"]):
    rec(f"d0{i + 1} deposit {amount}", lambda i=i, amount=amount: binding.apply_account_deposit(client, {"ids": [N]}, input={"amount": amount}, key=f"dec-{i}-0001"))
rec("d04 balance is exact", lambda: row(["balance"], N)["balance"])
rec("d05 result over the column precision", lambda: binding.apply_account_deposit(client, {"ids": [N]}, input={"amount": "9999999999.99"}))
rec("d06 three decimals", lambda: binding.apply_account_deposit(client, {"ids": [N]}, input={"amount": "0.001"}))
rec("d07 withdraw more than the balance", lambda: binding.apply_account_withdraw(client, {"ids": [N]}, input={"amount": "5.04"}))
rec("d08 withdraw exactly the balance", lambda: binding.apply_account_withdraw(client, {"ids": [N]}, input={"amount": "5.03"}))
rec("d09 balance after", lambda: row(["balance"], N)["balance"])
rec("d10 non-positive amount", lambda: binding.apply_account_deposit(client, {"ids": [N]}, input={"amount": "0.00"}))

# Id / Uuid
rec("u01 numeric id on a Uuid resource", lambda: binding.apply_account_deposit(client, {"ids": ["1"]}, input={"amount": "1.00"}))
rec("u02 malformed uuid", lambda: binding.apply_account_deposit(client, {"ids": ["not-a-uuid"]}, input={"amount": "1.00"}))
rec("u03 another actor's row is missing", lambda: binding.apply_account_deposit(client, {"ids": [A3]}, input={"amount": "1.00"}))
def u04():
    made = client.create("Note", {"account": A1, "body": "n", "reviewer": None})
    return {"made": made, "rows": client.read({"read": "Note", "select": ["id", "account", "reviewer"]})}
rec("u04 reference to a Uuid resource", u04)
rec("u05 create against a row of another owner", lambda: client.create("Note", {"account": A3, "body": "x", "reviewer": None}))

# input arithmetic and replay
take = {"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 3}}
rec("i01 take 3", lambda: apply(take, key="take-3-0001"))
rec("i02 stock after", stock)
rec("i03 take more than the stock", lambda: binding.apply_product_take(client, {"ids": ["1"]}, input={"qty": 8}))
rec("i04 qty 0", lambda: binding.apply_product_take(client, {"ids": ["1"]}, input={"qty": 0}))
rec("i05 qty above the declared range", lambda: binding.apply_product_take(client, {"ids": ["1"]}, input={"qty": 101}))
rec("i06 replay of the same key", lambda: apply(take, key="take-3-0001"))
rec("i07 stock unchanged by the replay", stock)
rec("i08 same key, other body", lambda: apply({"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 4}}, key="take-3-0001"))

# refs / target.created, bundle
rec("r01 order, two lines, confirm and take in one bundle", lambda: client.bundle([
    {"create": "Order", "values": {"owner": "1", "status": "open", "total": "5.00"}},
    {"create": "OrderLine", "values": {"product": "1", "qty": 1, "price": "2.50"}, "refs": {"order": created(0)}},
    {"create": "OrderLine", "values": {"product": "1", "qty": 1, "price": "2.50", "note": None}, "refs": {"order": created(0)}},
    {"apply": "Order.confirm", "target": created(0)},
    {"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 2}},
]))
rec("r02 stock after the bundle", stock)
rec("r03 lines point at the new order", lambda: client.read({"read": "OrderLine", "select": ["qty", "price", "note", "order", "product"]}))
rec("r04 a failing step cancels the earlier creates", lambda: client.bundle([
    {"create": "Order", "values": {"owner": "1", "status": "open", "total": "9.00"}},
    {"create": "OrderLine", "values": {"product": "1", "qty": 1, "price": "9.00"}, "refs": {"order": created(0)}},
    {"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 99}},
]))
rec("r05 nothing was kept", lambda: {"stock": stock(), "totals": [r["total"] for r in client.read({"read": "Order", "select": ["total"], "sort": [{"field": "id", "dir": "asc"}]})]})
client.bundle([{"create": "Order", "values": {"owner": "1", "status": "open", "total": "1.00"}}])
# A Python handle is only an index, so a handle from "another bundle" is a forward or self reference to the server.
rec("r06 a handle from another bundle", lambda: client.bundle([{"apply": "Order.confirm", "target": created(0)}]))
rec("r07 an empty builder", lambda: client.bundle([]))

# compose, also inside a bundle with a Uuid resource
rec("c01 compose confirm", lambda: client.compose("Order", ["1"], [{"transition": "confirm"}]))
rec("c02 compose again on a confirmed order", lambda: client.compose("Order", ["1"], [{"transition": "confirm"}]))
rec("c03 bundle with a compose over a Uuid resource and another apply", lambda: client.bundle([
    {"compose": "Account", "targets": {"ids": [A1]}, "steps": [{"transition": "touch"}, {"transition": "touch"}]},
    {"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 1}},
]))
rec("c04 stock after the bundle", stock)
rec("c05 pending is empty", lambda: client.pending())

# write extension result
rec("x01 adjust", lambda: binding.write_account_adjust(client, {"a": A1, "amount": "2.50", "fail": False}))
rec("x02 business refusal", lambda: binding.write_account_adjust(client, {"a": A1, "amount": "2.50", "fail": True}))
rec("x03 balance after", lambda: row(["balance"], A1)["balance"])
rec("x04 invalid Decimal input", lambda: binding.write_account_adjust(client, {"a": A1, "amount": "2.505", "fail": False}))
refusal = aip_sdk.extension_rejection(binding.write_account_adjust(client, {"a": A1, "amount": "2.50", "fail": True}))
out["x05 typed refusal"] = {"app_code": refusal.app_code, "code": refusal.code, "text": str(refusal)}

# lost response and unsettled
drop = [0]
def flaky(url, headers, body, timeout):
    result = _urllib_transport(url, headers, body, timeout)
    if drop[0] > 0 and url.endswith("/apply"):
        drop[0] -= 1
        raise ConnectionResetError()
    return result
lossy = connect(base, token, binding.CONTRACT_FINGERPRINT, transport=flaky)
before = stock()
drop[0] = 1
rec("l01 one lost response is recovered with the same key", lambda: lossy.apply({"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 1}}, key="lost-0001"))
rec("l02 executed once", lambda: before - stock())
drop[0] = 3
rec("l03 every attempt lost", lambda: lossy.apply({"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 1}}, key="lost-0002", retries=2))
rec("l04 key kept as pending", lambda: lossy.pending())
rec("l05 recovery replays the stored result", lambda: [{"ok": r["ok"], "replayed": r.get("replayed")} for r in lossy.retry_pending()])
rec("l06 executed once more", lambda: before - stock())
drop[0] = 3
rec("l07 keyless call after a lost write", lambda: lossy.apply({"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 1}}, retries=0))
rec("l08 the same keyless call reuses the pending key", lambda: lossy.apply({"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 1}}))
rec("l09 executed once more", lambda: before - stock())

# contract drift
drift = connect(base, token, "0" * 64)
stock_before = stock()
rec("f01 read", lambda: drift.read({"read": "Account", "select": ["id"]}))
rec("f02 apply", lambda: drift.apply({"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 1}}))
rec("f03 bundle", lambda: drift.bundle([{"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 1}}]))
rec("f04 compose", lambda: drift.compose("Order", ["2"], [{"transition": "confirm"}]))
rec("f05 write extension", lambda: drift.write_extension("Account.adjust", {"a": A1, "amount": "1.00", "fail": False}))
rec("f06 nothing executed", lambda: stock_before - stock())
rec("f07 retry guidance on a business failure", lambda: client.apply({"apply": "Order.confirm", "target": {"ids": ["1"]}}))

print(json.dumps(out))
`;

async function runLanguage(language) {
  const f = await startFixture({ definition, actors: { alice: 1 }, extensions: { "sdk.mjs": extension }, seed: SEED, sdkImport: sdkEntry });
  fixtures.push(f);
  if (language === "ts") {
    writeFileSync(join(f.dir, "package.json"), '{"private":true,"type":"module"}\n');
    writeFileSync(join(f.dir, "run.ts"), tsScript(f));
    // 타입 검사: 생성 binding 타입이 이 시나리오의 모든 호출을 받는다.
    run(tsc, ["--noEmit", "--strict", "--target", "esnext", "--module", "nodenext", "--moduleResolution", "nodenext", "--allowImportingTsExtensions", "--skipLibCheck", join(f.dir, "run.ts")], { cwd: f.dir });
    const stdout = run(process.execPath, [join(f.dir, "run.ts")], { cwd: f.dir, timeout: 120_000 });
    return { f, results: JSON.parse(stdout.split("\n").at(-1)) };
  }
  writeFileSync(join(f.dir, "run.py"), pyScript(f));
  const stdout = run("python3", [join(f.dir, "run.py")], { cwd: f.dir, timeout: 120_000 });
  return { f, results: JSON.parse(stdout.split("\n").at(-1)) };
}

const UUID_TOKEN = "<uuid>";
const ok = { ok: true };
const business = (code) => ({ ok: false, code, requestId: "<req>", msg: "<msg>", retry: "never" });
const drifted = (code = "WIRE.CONTRACT_MISMATCH") => ({ ok: false, code, requestId: "<req>", msg: "<msg>", retry: "never" });
// 두 언어의 결과가 같아야 하는 단계의 기대 값. 언어별 표현이 다른 단계(예외 대 결과 객체)는 아래 LANGUAGE_SPECIFIC가 따로 단언한다.
const EXPECTED = {
  "n02 read the new row": { balance: "5.00", cap: null, id: UUID_TOKEN, label: "n1", memo: null },
  "n04 read after patch": { cap: "3.00", label: "n1", memo: "m" },
  "n06 omitted fields kept": { cap: "3.00", label: "n1b", memo: "m" },
  "n08 read after null": { cap: null, label: "n1b", memo: null },
  "n09 null into a required field": business("WIRE.BAD_VALUE"),
  "n10 field policy redaction": { mine: "sec-1", other: null },
  "d04 balance is exact": "5.03",
  "d05 result over the column precision": business("WIRE.BAD_VALUE"),
  "d06 three decimals": business("WIRE.BAD_VALUE"),
  "d07 withdraw more than the balance": business("WIRE.INVALID_STATE"),
  "d09 balance after": "0.00",
  "d10 non-positive amount": business("WIRE.BAD_VALUE"),
  "u01 numeric id on a Uuid resource": business("WIRE.BAD_VALUE"),
  "u02 malformed uuid": business("WIRE.BAD_VALUE"),
  "u03 another actor's row is missing": business("WIRE.MISSING_TARGET"),
  "u05 create against a row of another owner": business("WIRE.MISSING_TARGET"),
  "i02 stock after": 7,
  "i03 take more than the stock": business("WIRE.INVALID_STATE"),
  "i04 qty 0": business("WIRE.BAD_VALUE"),
  "i05 qty above the declared range": business("WIRE.BAD_VALUE"),
  "i07 stock unchanged by the replay": 7,
  "i08 same key, other body": business("WIRE.IDEMPOTENCY_MISMATCH"),
  "r02 stock after the bundle": 5,
  "r04 a failing step cancels the earlier creates": business("WIRE.INVALID_STATE"),
  "r05 nothing was kept": { stock: 5, totals: ["1.00", "1.00", "5.00"] },
  "c02 compose again on a confirmed order": business("WIRE.INVALID_STATE"),
  "c04 stock after the bundle": 4,
  "c05 pending is empty": [],
  "x03 balance after": "22.50",
  "l02 executed once": 1,
  "l04 key kept as pending": ["lost-0002"],
  "l05 recovery replays the stored result": [{ ok: true, replayed: true }],
  "l06 executed once more": 2,
  "l09 executed once more": 3,
  "f06 nothing executed": 0,
  "f07 retry guidance on a business failure": business("WIRE.INVALID_STATE"),
};

let tsRun;
let pyRun;
test("TypeScript SDK on PG17: nullable, Decimal, Uuid, input arithmetic, refs, compose, extension, lost responses and contract drift", { timeout: 300_000 }, async () => {
  tsRun = await runLanguage("ts");
  const r = tsRun.results;
  for (const [step, want] of Object.entries(EXPECTED)) assert.deepEqual(r[step], want, `${step}\n${JSON.stringify(r, null, 1)}`);
  assert.equal(r["n01 create without the optional memo"].ok, true);
  assert.deepEqual(r["n01 create without the optional memo"].created, [{ id: UUID_TOKEN, resource: "Account" }]);
  for (const step of ["n03 patch memo and cap", "n05 patch only the label", "n07 explicit null clears", "i01 take 3"]) assert.equal(r[step].ok, true, step);
  assert.equal(r["i06 replay of the same key"].replayed, true, "the same key is replayed, not executed twice");
  assert.deepEqual(r["u04 reference to a Uuid resource"].rows, [{ account: A1, id: "1", reviewer: null }]);
  // r01: 주문 하나, 줄 둘, 확정, 재고 차감. created와 applied의 id와 개수.
  const bundle = r["r01 order, two lines, confirm and take in one bundle"];
  assert.equal(bundle.ok, true);
  assert.deepEqual(bundle.created.map((c) => c.resource), ["Order", "OrderLine", "OrderLine"]);
  assert.deepEqual(bundle.applied.map((a) => a.changed.length), [1, 1]);
  assert.deepEqual(r["r03 lines point at the new order"].map((l) => [l.qty, l.price, l.note, l.order === bundle.created[0].id]), [[1, "2.50", null, true], [1, "2.50", null, true]]);
  // c01·c03
  assert.equal(r["c01 compose confirm"].ok, true);
  const mixed = r["c03 bundle with a compose over a Uuid resource and another apply"];
  assert.equal(mixed.ok, true, "a committed bundle with a compose over a Uuid resource is a result, not a protocol error");
  assert.deepEqual(mixed.ops.map((o) => o.kind), ["compose", "apply"]);
  // 확장
  assert.deepEqual(r["x01 adjust"], { contractFingerprint: tsRun.f.fingerprint === undefined ? undefined : r["x01 adjust"].contractFingerprint, ok: true, output: { id: A1, moved: "2.50", note: null }, tags: ["Account"] });
  assert.deepEqual(r["x02 business refusal"], { appCode: "NOT_TODAY", code: "WIRE.EXTENSION_REJECTED", msg: "<msg>", ok: false, requestId: "<req>", retry: "never" });
  // TS는 계약에 맞지 않는 확장 입력(소수 셋째 자리)을 보내기 전에 거절해 예외로 올리고, 아래 래퍼가 그 코드를 결과 모양으로 바꾼 것이다.
  assert.deepEqual(r["x04 invalid Decimal input"], { ok: false, code: "WIRE.BAD_VALUE" });
  // 응답 유실
  const recovered = r["l01 one lost response is recovered with the same key"];
  assert.deepEqual([recovered.ok, recovered.recovered, recovered.replayed], [true, true, true]);
  assert.deepEqual(r["l03 every attempt lost"], { threw: "CLIENT.WRITE_UNSETTLED" });
  assert.deepEqual(r["l07 keyless call after a lost write"], { threw: "CLIENT.WRITE_UNSETTLED" });
  assert.equal(r["l08 the same keyless call reuses the pending key"].ok, true);
  assert.equal(r["l08 the same keyless call reuses the pending key"].replayed, true);
  // 계약 지문
  assert.deepEqual(r["f01 read"], { threw: "WIRE.CONTRACT_MISMATCH", retry: "never", requestId: "<req>" });
  for (const step of ["f02 apply", "f03 bundle", "f04 compose"]) assert.deepEqual(r[step], drifted(), step);
  assert.equal(r["f05 write extension"].code, "WIRE.CONTRACT_MISMATCH");
  assert.deepEqual(r["r06 a handle from another bundle"].code, "WIRE.BAD_VALUE");
  assert.deepEqual(r["r07 an empty builder"], { threw: "CLIENT.PROTOCOL_ERROR" });
  // DB가 최종 근거다.
  assert.equal(tsRun.f.sql("SELECT stock FROM ${S}.product WHERE id=1"), "1", "10 - 3 - 2 - 1 (compose bundle) - 1 - 1 - 1");
  assert.equal(tsRun.f.sql("SELECT balance FROM ${S}.account WHERE id='" + A1 + "'"), "22.50");
  assert.equal(tsRun.f.sql("SELECT count(*) FROM ${S}.\"order\" WHERE total = '9.00'"), "0", "the cancelled bundle left no order");
});

test("Python SDK on PG17: the same journey gives the same results", { timeout: 300_000 }, async () => {
  pyRun = await runLanguage("python");
  const r = pyRun.results;
  for (const [step, want] of Object.entries(EXPECTED)) assert.deepEqual(r[step], want, step);
  assert.deepEqual(r["n01 create without the optional memo"].created, [{ id: UUID_TOKEN, resource: "Account" }]);
  assert.equal(r["i06 replay of the same key"].replayed, true);
  assert.deepEqual(r["u04 reference to a Uuid resource"].rows, [{ account: A1, id: "1", reviewer: null }]);
  const bundle = r["r01 order, two lines, confirm and take in one bundle"];
  assert.equal(bundle.ok, true);
  assert.deepEqual(bundle.created.map((c) => c.resource), ["Order", "OrderLine", "OrderLine"]);
  const mixed = r["c03 bundle with a compose over a Uuid resource and another apply"];
  assert.equal(mixed.ok, true);
  assert.deepEqual(mixed.ops.map((o) => o.kind), ["compose", "apply"]);
  assert.deepEqual(r["x02 business refusal"], { appCode: "NOT_TODAY", code: "WIRE.EXTENSION_REJECTED", msg: "<msg>", ok: false, requestId: "<req>", retry: "never" });
  assert.deepEqual(r["x04 invalid Decimal input"], business("WIRE.BAD_VALUE"), "Python sends the input and the server refuses it");
  assert.deepEqual(r["x05 typed refusal"], { app_code: "NOT_TODAY", code: "WIRE.EXTENSION_REJECTED", text: "오늘은 안 됩니다" });
  const recovered = r["l01 one lost response is recovered with the same key"];
  assert.deepEqual([recovered.ok, recovered.recovered, recovered.replayed], [true, true, true]);
  assert.deepEqual(r["l03 every attempt lost"], { threw: "CLIENT.WRITE_UNSETTLED" });
  assert.deepEqual(r["l07 keyless call after a lost write"], { threw: "CLIENT.WRITE_UNSETTLED" });
  assert.equal(r["l08 the same keyless call reuses the pending key"].replayed, true);
  assert.deepEqual(r["f01 read"], { threw: "WIRE.CONTRACT_MISMATCH", retry: "never", requestId: "<req>" });
  for (const step of ["f02 apply", "f03 bundle", "f04 compose"]) assert.deepEqual(r[step], drifted(), step);
  assert.equal(r["f05 write extension"].code, "WIRE.CONTRACT_MISMATCH");
  assert.equal(r["r06 a handle from another bundle"].code, "WIRE.BAD_VALUE");
  assert.deepEqual(r["r07 an empty builder"], { threw: "WIRE.BAD_REQUEST" });
  assert.equal(pyRun.f.sql("SELECT stock FROM ${S}.product WHERE id=1"), "1");
  assert.equal(pyRun.f.sql("SELECT balance FROM ${S}.account WHERE id='" + A1 + "'"), "22.50");
});

test("both SDKs agree step by step except where the languages differ by design", () => {
  assert.ok(tsRun && pyRun, "the two language tests above ran first");
  // 설계상 표현이 다른 단계: 빈 묶음(TS는 CLIENT 오류, Python은 WIRE.BAD_REQUEST), 확장 입력 검사(TS는 전송 전, Python은 서버), 확장 응답의 지문 필드와 타입 있는 거절 객체.
  const BY_DESIGN = new Set(["r07 an empty builder", "x01 adjust", "x04 invalid Decimal input", "x05 typed refusal", "n01 create without the optional memo"]);
  const different = Object.keys(tsRun.results).filter((step) => !BY_DESIGN.has(step) && JSON.stringify(tsRun.results[step]) !== JSON.stringify(pyRun.results[step]));
  assert.deepEqual(different.map((step) => [step, tsRun.results[step], pyRun.results[step]]), []);
  assert.equal(Object.keys(tsRun.results).filter((step) => !BY_DESIGN.has(step) && !(step in pyRun.results)).length, 0);
});
