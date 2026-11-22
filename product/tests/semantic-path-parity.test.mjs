// D2: 같은 공개 전이를 직접 apply, bundle 안 apply, compose 단계, 쓰기 확장의 ctx.data.apply 네 경로로 실행해
// 타입·권한·검증·트랜잭션 결과를 PG17에서 비교한다. 경로마다 새 행을 쓰고, 오류 코드와 최종 DB 상태를 표로 단언한다.
// 의도된 예외(확장은 안쪽 ctx 거절을 `WIRE.EXTENSION_ERROR`로 가린다, compose 단계는 input·patch·expect 전이를 못 쓴다)는
// plan-docs/stabilization-20261122/track-d.md의 "의도된 예외 계약" 표와 같은 값이다.
//
//   cargo build -p aip-cli && node --test --test-concurrency=1 product/tests/semantic-path-parity.test.mjs
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { startFixture, cleanupTemp } from "../test-support/service-fixture.mjs";

const ORDER_APP = `
resource Member { fields { id: Id; name: Text(1..20) } rows read when id = actor }
actor Member
resource Order {
  fields { id: Id; owner: Member; status: Text(1..12); paid: Int(0..1000); shared: Bool }
  rows read when owner = actor or shared = true
  transition confirm { from status = "open"; allow owner = actor; to status = "confirmed" }
  check confirmNeedsPay when status != "confirmed" or paid >= 10
  expose read { select id, status, paid; sort id; budget { rows 50; depth 1; deadline 2s; cost 1000 } }
  expose apply confirm { target id; bulk maxRows 5 }
  expose compose { bulk maxRows 5; transitions confirm }
  extension write confirmOne { input { a: Order.Id }; output { n: Int }; access Order.confirm; effect db; deadline 3s; implementation "ext.confirmOne" }
  extension write confirmTwo { input { a: Order.Id; b: Order.Id }; output { n: Int }; access Order.confirm; effect db; deadline 3s; implementation "ext.confirmTwo" }
}
`;
const ORDER_EXT = `
export async function confirmOne(input, ctx) { await ctx.data.apply("Order", "confirm", [input.a]); return { n: 1 }; }
export async function confirmTwo(input, ctx) { await ctx.data.apply("Order", "confirm", [input.a, input.b]); return { n: 2 }; }
`;

const PRODUCT_APP = `
resource Member { fields { id: Id; name: Text(1..20) } rows read when id = actor }
actor Member
resource Product {
  fields { id: Id; owner: Member; stock: Int(0..50); price: Decimal(10,2); title: Text(1..5); note: Text?; version: Int(0..1000) }
  rows read when owner = actor
  transition take { input { qty: Int(1..50) }; from stock >= input.qty; allow owner = actor; to stock = stock - input.qty }
  transition restock { input { qty: Int(1..50) }; from true; allow owner = actor; to stock = stock + input.qty }
  transition edit { from true; allow owner = actor; patch title, note, price; expect version; to version = version + 1 }
  expose read { select id, stock, price, title, note, version; sort id; budget { rows 50; depth 1; deadline 2s; cost 1000 } }
  expose apply take { target id; bulk maxRows 5 }
  expose apply restock { target id; bulk maxRows 5 }
  expose apply edit { target id; bulk maxRows 5 }
  extension write takeVia { input { a: Product.Id; qty: Int(1..50) }; output { n: Int }; access Product.take; effect db; deadline 3s; implementation "ext.takeVia" }
  extension write restockVia { input { a: Product.Id; qty: Int(1..50) }; output { n: Int }; access Product.restock; effect db; deadline 3s; implementation "ext.restockVia" }
  extension write editVia {
    input { a: Product.Id; title: Text(1..20)?; note: Text?; price: Decimal(10,2)?; version: Int; which: Text }
    output { n: Int }; access Product.edit; effect db; deadline 3s; implementation "ext.editVia"
  }
}
`;
const PRODUCT_EXT = `
export async function takeVia(input, ctx) { await ctx.data.apply("Product", "take", [input.a], undefined, undefined, undefined, { qty: input.qty }); return { n: 1 }; }
export async function restockVia(input, ctx) { await ctx.data.apply("Product", "restock", [input.a], undefined, undefined, undefined, { qty: input.qty }); return { n: 1 }; }
export async function editVia(input, ctx) {
  const values = {};
  for (const k of input.which.split(",")) if (k) values[k] = input[k];
  await ctx.data.apply("Product", "edit", [input.a], undefined, values, { version: input.version });
  return { n: 1 };
}
`;

const SEED = "INSERT INTO ${S}.member(id,name) VALUES (1,'alice'),(2,'bob');";
const EXT = "WIRE.EXTENSION_ERROR";
const fixtures = [];
before(() => {});
after(async () => {
  for (const f of fixtures) await f.stop();
  cleanupTemp();
});

const label = (r) => (r.body.ok ? "OK" : r.body.code);

test("Order.confirm: direct, bundle, compose and extension agree on permission, state, check and rollback outcomes", { timeout: 240_000 }, async () => {
  const f = await startFixture({ definition: ORDER_APP, actors: { alice: 1, bob: 2 }, extensions: { "ext.mjs": ORDER_EXT }, python: false, seed: SEED });
  fixtures.push(f);
  const paths = ["direct", "bundle", "compose", "extension"];
  // [id, owner, status, paid, shared]
  const rows = [
    [1, 1, "open", 20, false], [2, 1, "open", 20, false], [3, 1, "open", 20, false], [4, 1, "confirmed", 20, false],
    [5, 1, "open", 5, false], [6, 2, "open", 20, true], [7, 2, "open", 20, false], [8, 1, "open", 20, false],
    [9, 1, "open", 20, false], [10, 1, "open", 20, false],
  ];
  const scenarios = [
    ["success", [1]], ["multi", [2, 3]], ["state", [4]], ["check", [5]], ["forbidden", [6]], ["hidden", [7]], ["missing", [99]],
    ["rollback-state", [8, 4]], ["rollback-check", [9, 5]], ["rollback-forbidden", [10, 6]],
  ];
  // 직접·bundle·compose의 기대 코드. 확장은 EXTENSION_EXPECTED가 따로 정한다.
  const EXPECTED = {
    success: "OK", multi: "OK", state: "WIRE.INVALID_STATE", check: "WIRE.CHECK_FAILED", forbidden: "WIRE.FORBIDDEN",
    hidden: "WIRE.MISSING_TARGET", missing: "WIRE.MISSING_TARGET", "rollback-state": "WIRE.INVALID_STATE",
    "rollback-check": "WIRE.CHECK_FAILED", "rollback-forbidden": "WIRE.FORBIDDEN",
  };
  // 확장 안의 ctx.data 거절은 호출자에게 일반 EXTENSION_ERROR로만 보인다(spec/wire-errors.md). 커밋 시점 check는 확장 밖이라 코드가 그대로다.
  const EXTENSION_EXPECTED = { ...EXPECTED, state: EXT, forbidden: EXT, hidden: EXT, missing: EXT, "rollback-state": EXT, "rollback-forbidden": EXT };
  const states = {};
  for (const [index, path] of paths.entries()) {
    const base = (index + 1) * 100;
    f.sql(`INSERT INTO \${S}."order"(id,owner_id,status,paid,shared) VALUES ${rows.map(([i, o, s, p, sh]) => `(${base + i},${o},'${s}',${p},${sh})`).join(",")}`);
    for (const [name, relative] of scenarios) {
      const ids = relative.map((r) => String(base + r));
      const request = {
        direct: { apply: "Order.confirm", target: { ids } },
        bundle: { atomic: ids.map((id) => ({ apply: "Order.confirm", target: { ids: [id] } })) },
        compose: { compose: "Order", targets: { ids }, steps: [{ transition: "confirm" }] },
        extension: ids.length === 1 ? { extension: "Order.confirmOne", input: { a: ids[0] } } : { extension: "Order.confirmTwo", input: { a: ids[0], b: ids[1] } },
      }[path];
      const reply = await f.post("/apply", { request, key: `parity-${path}-${name}-0001` }, { token: "alice" });
      const want = (path === "extension" ? EXTENSION_EXPECTED : EXPECTED)[name];
      assert.equal(label(reply), want, `${name} via ${path}: ${JSON.stringify(reply.body)}`);
    }
    states[path] = f.sql(`SELECT string_agg((id-${base})::text||'='||status, ',' ORDER BY id) FROM \${S}."order" WHERE id BETWEEN ${base + 1} AND ${base + 10}`);
  }
  // 실패한 묶음·조합·확장은 앞선 대상까지 되돌렸다: 8, 9, 10은 open, 4는 처음부터 confirmed, 5는 open.
  assert.equal(states.direct, "1=confirmed,2=confirmed,3=confirmed,4=confirmed,5=open,6=open,7=open,8=open,9=open,10=open");
  for (const path of paths) assert.equal(states[path], states.direct, `${path} leaves the same DB state as the direct apply`);
});

test("Product input arithmetic, patch/expect and Decimal: direct, bundle and extension agree; compose refuses these transitions at check time", { timeout: 240_000 }, async () => {
  const f = await startFixture({ definition: PRODUCT_APP, actors: { alice: 1 }, extensions: { "ext.mjs": PRODUCT_EXT }, python: false, seed: SEED });
  fixtures.push(f);
  const paths = ["direct", "bundle", "extension"];
  // [이름, 전이, 행(1=재고, 2=편집), 요청 조각, 직접·bundle 기대, 확장 기대]
  const cases = [
    ["take-ok", "take", 1, { qty: 3 }, "OK", "OK"],
    ["take-over-stock", "take", 1, { qty: 11 }, "WIRE.INVALID_STATE", EXT],
    ["take-zero", "take", 1, { qty: 0 }, "WIRE.BAD_VALUE", "WIRE.BAD_VALUE"],
    ["take-string", "take", 1, { qty: "3" }, "WIRE.BAD_VALUE", "WIRE.BAD_VALUE"],
    ["take-fraction", "take", 1, { qty: 1.5 }, "WIRE.BAD_VALUE", "WIRE.BAD_VALUE"],
    ["take-missing-key", "take", 1, {}, "WIRE.BAD_VALUE", "WIRE.BAD_VALUE"],
    ["take-above-range", "take", 1, { qty: 51 }, "WIRE.BAD_VALUE", "WIRE.BAD_VALUE"],
    // 7 + 50이 필드 범위 50을 넘는다. 감싸기나 포화 없이 거절된다.
    ["restock-overflow", "restock", 1, { qty: 50 }, "WIRE.BAD_VALUE", EXT],
    ["edit-ok", "edit", 2, { values: { title: "abc", note: null, price: "9.99" }, expect: { version: 0 } }, "OK", "OK"],
    ["edit-stale-expect", "edit", 2, { values: { title: "zzz" }, expect: { version: 0 } }, "WIRE.CONFLICT", "WIRE.CONFLICT"],
    ["edit-too-long", "edit", 2, { values: { title: "abcdef" }, expect: { version: 1 } }, "WIRE.BAD_VALUE", EXT],
    ["edit-null-required", "edit", 2, { values: { title: null }, expect: { version: 1 } }, "WIRE.BAD_VALUE", EXT],
    ["edit-decimal-scale", "edit", 2, { values: { price: "1.234" }, expect: { version: 1 } }, "WIRE.BAD_VALUE", "WIRE.BAD_VALUE"],
    ["edit-decimal-number", "edit", 2, { values: { price: 1.5 }, expect: { version: 1 } }, "WIRE.BAD_VALUE", "WIRE.BAD_VALUE"],
  ];
  const finalStates = {};
  for (const [index, path] of paths.entries()) {
    const base = (index + 1) * 1000;
    f.sql(`INSERT INTO \${S}.product(id,owner_id,stock,price,title,note,version) VALUES (${base + 1},1,10,'1.00','t',NULL,0),(${base + 2},1,10,'1.00','t','keep',0)`);
    for (const [name, transition, row, payload, wantDirect, wantExtension] of cases) {
      const id = String(base + row);
      const direct = { apply: `Product.${transition}`, target: { ids: [id] }, ...(transition === "edit" ? payload : { input: payload }) };
      let request = direct;
      if (path === "bundle") request = { atomic: [direct] };
      if (path === "extension") {
        if (transition === "take") request = { extension: "Product.takeVia", input: { a: id, ...payload } };
        else if (transition === "restock") request = { extension: "Product.restockVia", input: { a: id, ...payload } };
        else {
          const values = payload.values;
          request = { extension: "Product.editVia", input: { a: id, title: values.title ?? null, note: values.note ?? null, price: values.price ?? null, version: payload.expect.version, which: Object.keys(values).join(",") } };
        }
      }
      const reply = await f.post("/apply", { request, key: `parity-${path}-${name}-0001` }, { token: "alice" });
      assert.equal(label(reply), path === "extension" ? wantExtension : wantDirect, `${name} via ${path}: ${JSON.stringify(reply.body)}`);
    }
    // 같은 key 재전송은 재고를 한 번만 줄인다(7 그대로).
    const id = String(base + 1);
    const replayRequest = path === "extension" ? { extension: "Product.takeVia", input: { a: id, qty: 3 } }
      : path === "bundle" ? { atomic: [{ apply: "Product.take", target: { ids: [id] }, input: { qty: 3 } }] }
      : { apply: "Product.take", target: { ids: [id] }, input: { qty: 3 } };
    const replay = await f.post("/apply", { request: replayRequest, key: `parity-${path}-take-ok-0001` }, { token: "alice" });
    assert.equal(replay.body.ok, true, `${path} replay: ${JSON.stringify(replay.body)}`);
    assert.equal(replay.body.replayed, true, `${path} replay is marked replayed`);
    finalStates[path] = f.sql(`SELECT string_agg(stock||'/'||price||'/'||title||'/'||coalesce(note,'~')||'/v'||version, ' ' ORDER BY id) FROM \${S}.product WHERE id BETWEEN ${base + 1} AND ${base + 2}`);
  }
  // 재고 10 - 3 = 7. 거절된 모든 시도는 아무것도 바꾸지 않았다. 편집은 한 번만 반영되어 v1이다.
  assert.equal(finalStates.direct, "7/1.00/t/~/v0 10/9.99/abc/~/v1");
  for (const path of paths) assert.equal(finalStates[path], finalStates.direct, `${path} leaves the same DB state as the direct apply`);
});

test("compose refuses transitions that take input, patch or expect (decision C0, P0-D) before touching the database", { timeout: 120_000 }, async () => {
  const withCompose = PRODUCT_APP.replace("expose apply take", "expose compose { bulk maxRows 1; transitions take }\n  expose apply take");
  const rejected = await startFixture({ definition: withCompose, actors: { alice: 1 }, python: false, serve: false, allowCheckFailure: true });
  fixtures.push(rejected);
  assert.equal(rejected.checked, false, "service check rejects a compose step that needs input");
  assert.match(rejected.check.stdout + rejected.check.stderr, /DEF\.UNSUPPORTED/u);
  for (const transition of ["edit"]) {
    const patchCompose = PRODUCT_APP.replace("expose apply take", `expose compose { bulk maxRows 1; transitions ${transition} }\n  expose apply take`);
    const patch = await startFixture({ definition: patchCompose, actors: { alice: 1 }, python: false, serve: false, allowCheckFailure: true });
    fixtures.push(patch);
    assert.equal(patch.checked, false, `service check rejects a compose step that patches or expects (${transition})`);
    assert.match(patch.check.stdout + patch.check.stderr, /DEF\.UNSUPPORTED/u);
  }
});
