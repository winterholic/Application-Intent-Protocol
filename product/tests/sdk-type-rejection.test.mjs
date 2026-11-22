// D3: 컴파일 타임에 거부할 수 있는 잘못된 요청은 생성 binding의 타입이 거부해야 한다.
// 이 시험은 정의에서 binding을 만들고 tsc --strict로
//   (1) 올바른 요청 모음이 오류 없이 컴파일되는지,
//   (2) 잘못된 요청 한 줄 한 줄이 각각 컴파일 오류를 내는지(오류 줄과 사례 줄이 정확히 일치)
// 를 확인한다. 타입은 런타임 권한을 보장하지 않는다. 서버가 최종 권한이고, 여기서는 호출 모양만 본다.
// DB가 필요 없다: `aip service gen`만 쓴다.
//
//   cargo build -p aip-cli && node --test product/tests/sdk-type-rejection.test.mjs
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const binary = process.env.AIP_BIN ?? join(root, "target/debug/aip");
const tsc = join(root, "spikes/spike-0-ts/node_modules/.bin/tsc");
const sdk = join(root, "product/sdk/index.ts");
const definition = join(root, "product/tests/fixtures/sdk-semantics/app.aip");

function sh(program, args, cwd) {
  const r = spawnSync(program, args, { cwd, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
  assert.equal(r.error, undefined);
  return r;
}

const PRELUDE = `import { connect } from ${JSON.stringify(sdk)};
import { contract } from "./binding.ts";
import type { Id } from "./binding.ts";
const client = connect("http://aip.test", "token", contract);
declare const accountId: Id<"Account">;
declare const productId: Id<"Product">;
declare const orderId: Id<"Order">;
declare const memberId: Id<"Member">;
`;

const OK = `
async function ok() {
  const page = await client.read({ read: "Account", select: ["id", "label", "memo", "balance", "cap"], filter: [{ field: "label", op: "eq", value: "x" }], sort: [{ field: "id", dir: "desc" }], limit: 5 });
  const row = page.rows[0];
  const memoLength: number | undefined = row.memo?.length;
  const balance: string = row.balance;
  const cap: string | null = row.cap;
  void [memoLength, balance, cap];
  await client.read({ read: "OrderLine", select: ["id", "qty", "price", "note", "order", "product"] });
  await client.apply({ apply: "Account.deposit", target: { ids: [accountId] }, input: { amount: "1.00" } });
  await client.apply({ apply: "Account.rename", target: { ids: [accountId] }, values: { label: "l", memo: null, cap: null } });
  await client.apply({ apply: "Account.rename", target: { ids: [accountId] }, values: { label: "only" } });
  await client.apply({ apply: "Product.take", target: { ids: [productId] }, input: { qty: 2 } });
  await client.apply({ apply: "Order.confirm", target: { ids: [orderId] } });
  await client.bundle((b) => {
    const order = b.create("Order", { owner: memberId, status: "open", total: "0.00" });
    b.create("OrderLine", { product: productId, qty: 1, price: "1.00" }, { order });
    b.create("OrderLine", { product: productId, qty: 2, price: "2.00", note: null }, { order });
    b.apply({ apply: "Order.confirm", target: order });
    b.apply({ apply: "Product.take", target: { ids: [productId] }, input: { qty: 1 } });
  });
  await client.bundle({ atomic: [{ create: "Order", values: { owner: memberId, status: "open", total: "0.00" } }, { apply: "Order.confirm", target: { ids: [orderId] } }] });
  await client.compose({ compose: "Order", targets: { ids: [orderId] }, steps: [{ transition: "confirm" }] });
  const unsettled = await client.apply({ apply: "Order.confirm", target: { ids: [orderId] } });
  if (unsettled.ok) { const changed: readonly Id<"Order">[] = unsettled.changed; void changed; } else { const code: string = unsettled.code; void code; }
  const ext = await client.writeExtension("Account.adjust", { a: accountId, amount: "1.00", fail: false });
  if (ext.ok) { const moved: string = ext.output.moved; const note: string | null = ext.output.note; void [moved, note]; } else { const app: string | undefined = ext.appCode; void app; }
}
void ok;
`;

// 한 사례는 정확히 한 줄이다. 사례 줄마다 오류가 하나 이상 있어야 하고, 사례 아닌 줄에는 오류가 없어야 한다.
const NEGATIVE = [
  ["read unknown resource", `client.read({ read: "Nope", select: ["id"] });`],
  ["read a non-root value", `client.read({ read: "Account", select: ["nope"] });`],
  ["select a field of another resource", `client.read({ read: "Account", select: ["qty"] });`],
  ["empty select", `client.read({ read: "Account", select: [] });`],
  ["filter op the definition did not allow", `client.read({ read: "Account", select: ["id"], filter: [{ field: "label", op: "gte", value: "x" }] });`],
  ["filter value of the wrong type", `client.read({ read: "Account", select: ["id"], filter: [{ field: "label", op: "eq", value: 5 }] });`],
  ["filter on a resource without a filter list", `client.read({ read: "Order", select: ["id"], filter: [{ field: "status", op: "eq", value: "open" }] });`],
  ["sort on a field outside the sort list", `client.read({ read: "Account", select: ["id"], sort: [{ field: "label" }] });`],
  ["offset without an opt-in budget", `client.read({ read: "Account", select: ["id"], offset: 1 });`],
  ["cursor without an opt-in budget", `client.read({ read: "Account", select: ["id"], after: { id: accountId } });`],
  ["limit as text", `client.read({ read: "Account", select: ["id"], limit: "5" });`],
  ["relation on a resource without traverse", `client.read({ read: "Account", select: [{ owner: { select: ["id"] } }] });`],
  ["use an unselected field", `async () => { const p = await client.read({ read: "Account", select: ["id"] }); return p.rows[0].balance; };`],
  ["use a nullable field as non-null", `async () => { const p = await client.read({ read: "Account", select: ["memo"] }); return p.rows[0].memo.length; };`],
  ["assign to a result row", `async () => { const p = await client.read({ read: "Account", select: ["id"] }); p.rows[0].id = accountId; };`],
  ["apply an unknown transition", `client.apply({ apply: "Account.nope", target: { ids: [accountId] } });`],
  ["apply an internal-only name", `client.apply({ apply: "Order.create", target: { ids: [orderId] } });`],
  ["input transition without input", `client.apply({ apply: "Account.deposit", target: { ids: [accountId] } });`],
  ["input Decimal as a number", `client.apply({ apply: "Account.deposit", target: { ids: [accountId] }, input: { amount: 5 } });`],
  ["input transition with values", `client.apply({ apply: "Account.deposit", target: { ids: [accountId] }, input: { amount: "1.00" }, values: { label: "x" } });`],
  ["unknown input key", `client.apply({ apply: "Product.take", target: { ids: [productId] }, input: { qty: 1, extra: 1 } });`],
  ["input Int as text", `client.apply({ apply: "Product.take", target: { ids: [productId] }, input: { qty: "3" } });`],
  ["patch transition with input", `client.apply({ apply: "Account.rename", target: { ids: [accountId] }, values: { label: "x" }, input: { amount: "1.00" } });`],
  ["null into a non-null patch field", `client.apply({ apply: "Account.rename", target: { ids: [accountId] }, values: { label: null } });`],
  ["patch a field outside the patch list", `client.apply({ apply: "Account.rename", target: { ids: [accountId] }, values: { balance: "1.00" } });`],
  ["plain transition with values", `client.apply({ apply: "Order.confirm", target: { ids: [orderId] }, values: { status: "x" } });`],
  ["where target on an id-only transition", `client.apply({ apply: "Order.confirm", target: { where: [] } });`],
  ["id of another resource as target", `client.apply({ apply: "Order.confirm", target: { ids: [productId] } });`],
  ["id and where together", `client.apply({ apply: "Order.confirm", target: { ids: [orderId], where: [] } });`],
  ["created handle outside a bundle", `client.apply({ apply: "Order.confirm", target: { created: 0 } });`],
  ["create unknown resource", `client.bundle((b) => { b.create("Nope", {}); });`],
  ["create missing a required field", `client.bundle((b) => { b.create("Order", { owner: memberId, status: "open" }); });`],
  ["create with a number for Decimal", `client.bundle((b) => { b.create("Order", { owner: memberId, status: "open", total: 0 }); });`],
  ["create with an unexposed field", `client.bundle((b) => { b.create("Order", { owner: memberId, status: "open", total: "0.00", id: orderId }); });`],
  ["ref handle of the wrong resource", `client.bundle((b) => { const p = b.create("Order", { owner: memberId, status: "open", total: "0.00" }); b.create("OrderLine", { product: productId, qty: 1, price: "1.00" }, { product: p }); });`],
  ["ref for a field that is not a reference", `client.bundle((b) => { const o = b.create("Order", { owner: memberId, status: "open", total: "0.00" }); b.create("OrderLine", { product: productId, qty: 1, price: "1.00" }, { order: o, qty: o }); });`],
  ["handle of the wrong resource as apply target", `client.bundle((b) => { const o = b.create("Order", { owner: memberId, status: "open", total: "0.00" }); b.apply({ apply: "Product.take", target: o, input: { qty: 1 } }); });`],
  ["bundle with no operation", `client.bundle({ atomic: [] });`],
  ["compose step outside the transitions", `client.compose({ compose: "Order", targets: { ids: [orderId] }, steps: [{ transition: "cancel" }] });`],
  ["compose without steps", `client.compose({ compose: "Order", targets: { ids: [orderId] }, steps: [] });`],
  ["compose on self when the definition has no selfRow", `client.compose({ compose: "Order", targets: { ids: [orderId] }, steps: [{ transition: "confirm", on: "self" }] });`],
  ["compose create step the definition did not declare", `client.compose({ compose: "Order", targets: { ids: [orderId] }, steps: [{ create: "Order", values: {} }] });`],
  ["compose target of another resource", `client.compose({ compose: "Order", targets: { ids: [productId] }, steps: [{ transition: "confirm" }] });`],
  ["write extension unknown name", `client.writeExtension("Account.nope", {});`],
  ["write extension input with a number for Decimal", `client.writeExtension("Account.adjust", { a: accountId, amount: 1, fail: false });`],
  ["write extension missing input", `client.writeExtension("Account.adjust", { a: accountId, fail: false });`],
  ["write extension wrong id resource", `client.writeExtension("Account.adjust", { a: orderId, amount: "1.00", fail: false });`],
  ["read the output before narrowing the result", `async () => { const r = await client.writeExtension("Account.adjust", { a: accountId, amount: "1.00", fail: false }); return r.output; };`],
  ["read the changed ids before narrowing the result", `async () => { const r = await client.apply({ apply: "Order.confirm", target: { ids: [orderId] } }); return r.changed; };`],
  ["bundle op with an apply name that does not exist", `client.bundle({ atomic: [{ apply: "Order.nope", target: { ids: [orderId] } }] });`],
];

test("the generated binding types accept valid requests and reject each invalid one at compile time", () => {
  const dir = mkdtempSync(join(tmpdir(), "aip-type-reject-"));
  try {
    const gen = sh(binary, ["service", "gen", definition, "--wire", "decimal", "--out", join(dir, "binding.ts"), "--sdk-import", sdk], root);
    assert.equal(gen.status, 0, gen.stderr + gen.stdout);
    const compile = (file) => sh(tsc, ["--noEmit", "--strict", "--target", "esnext", "--module", "nodenext", "--moduleResolution", "nodenext", "--allowImportingTsExtensions", "--skipLibCheck", "--pretty", "false", file], dir);

    writeFileSync(join(dir, "ok.ts"), PRELUDE + OK);
    const ok = compile("ok.ts");
    assert.equal(ok.status, 0, `valid requests must compile:\n${ok.stdout}`);

    const lines = NEGATIVE.map(([, code]) => code);
    for (const [name, code] of NEGATIVE) assert.ok(!code.includes("\n"), `${name} must be one line`);
    const preludeLines = PRELUDE.split("\n").length - 1;
    writeFileSync(join(dir, "neg.ts"), PRELUDE + lines.join("\n") + "\n");
    const neg = compile("neg.ts");
    const failing = new Set([...neg.stdout.matchAll(/^neg\.ts\((\d+),\d+\): error/gmu)].map((m) => Number(m[1]) - preludeLines - 1));
    const missed = NEGATIVE.map(([name], index) => [name, index]).filter(([, index]) => !failing.has(index)).map(([name]) => name);
    assert.deepEqual(missed, [], `these invalid requests compiled but must be rejected by the type system:\n${missed.join("\n")}`);
    const stray = [...failing].filter((index) => index < 0 || index >= NEGATIVE.length);
    assert.deepEqual(stray, [], `unexpected errors outside the negative cases:\n${neg.stdout}`);
    assert.equal(neg.status !== 0, true);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
