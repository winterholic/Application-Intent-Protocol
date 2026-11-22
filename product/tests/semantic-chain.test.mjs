// D4: 정의 → 실행 기록 → DB 상태 → SDK 응답을 한 시나리오로 잇는 의미 시험. 스냅샷이 아니라 의미를 단언한다.
//   1. 재고는 정확히 한 번 줄고(재전송·동시 같은 key·응답 유실), 동시 요청이 몰려도 음수가 되지 않는다.
//   2. 숨은 필드의 값은 다른 필드로 복사되지 않는다(정의 단계에서 막히고, 허용된 쓰기를 해도 DB에 새지 않는다).
//   3. 원자 생성이 일부만 실패하면 전체가 취소된다(묶음, 확장 모두). 실패한 key는 기록이 없어 고친 요청으로 다시 쓸 수 있다.
// "실행 기록"은 `aip service audit list`가 주는 업무 감사 행이다(어느 경로(via)로 어느 전이를 어느 행에 적용했고 전후 값이 무엇인지).
//
//   cargo build -p aip-cli && node --test --test-concurrency=1 product/tests/semantic-chain.test.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { after, test } from "node:test";
import { startFixture, cleanupTemp, root, run, binary } from "../test-support/service-fixture.mjs";

const sdkEntry = join(root, "product/sdk/index.ts");
const { connect } = await import(sdkEntry);
const fixtures = [];
after(async () => {
  for (const f of fixtures) await f.stop();
  cleanupTemp();
});

const audit = (f) => JSON.parse(run(binary, ["service", "audit", "--config", f.configPath, "list"])).audit;
const clientFor = async (f, token = "alice", fetchImpl) => {
  const { contract } = await import(join(f.dir, "binding.ts") + `?${f.schema}`);
  return connect(f.base, f.tokens[token], contract, fetchImpl);
};

const SHOP = {
  definition: readFileSync(join(root, "product/tests/fixtures/sdk-semantics/app.aip"), "utf8"),
  extensions: { "sdk.mjs": readFileSync(join(root, "product/tests/fixtures/sdk-semantics/extensions/sdk.mjs"), "utf8") },
};
const SHOP_SEED = `
INSERT INTO \${S}.member(id,name) VALUES (1,'alice'),(2,'bob');
INSERT INTO \${S}.product(id,owner_id,stock,price) VALUES (1,1,10,'2.50'),(2,1,10,'2.50'),(3,1,3,'2.50'),(4,1,5,'2.50');
SELECT setval(pg_get_serial_sequence('\${S}.product','id'), 100);
`;

test("stock decreases exactly once: replay, concurrent same key, concurrent distinct keys and a lost response", { timeout: 240_000 }, async () => {
  const f = await startFixture({ ...SHOP, actors: { alice: 1 }, python: false, seed: SHOP_SEED, sdkImport: sdkEntry });
  fixtures.push(f);
  const stock = (id) => Number(f.sql(`SELECT stock FROM \${S}.product WHERE id=${id}`));

  // 정의: 입력 범위와 산술은 생성 계약에 그대로 나타난다.
  const generated = readFileSync(join(f.dir, "binding.ts"), "utf8");
  assert.match(generated, /"Product\.take": \{[^}]*input: \{\s*qty: number; \/\*\* 1\.\.100 \*\/\s*\}/su);

  // 1) 같은 key 재전송: 한 번 실행, 감사 행 한 개.
  const take = (id, qty) => ({ apply: "Product.take", target: { ids: [String(id)] }, input: { qty } });
  const client = await clientFor(f);
  const first = await client.apply(take(1, 3), { key: "once-0001" });
  assert.equal(first.ok, true);
  const replay = await client.apply(take(1, 3), { key: "once-0001" });
  assert.equal(replay.replayed, true);
  assert.equal(stock(1), 7);
  const rowsFor = (key) => audit(f).filter((row) => row.requestKey === key);
  assert.deepEqual(rowsFor("once-0001").map((r) => [r.via, r.transition, r.resource, r.before, r.after]), [["apply", "take", "Product", { stock: 10 }, { stock: 7 }]]);

  // 2) 서로 다른 클라이언트 다섯이 같은 key로 동시에: 하나만 실행되고 나머지는 저장된 결과를 받는다.
  const racers = await Promise.all(Array.from({ length: 5 }, () => clientFor(f)));
  const same = await Promise.all(racers.map((c) => c.apply(take(1, 2), { key: "race-same-0001" })));
  assert.ok(same.every((r) => r.ok === true), JSON.stringify(same));
  assert.equal(same.filter((r) => r.replayed === true).length, 4, "exactly one request executed, four replayed");
  assert.equal(stock(1), 5);
  assert.equal(rowsFor("race-same-0001").length, 1);

  // 3) 서로 다른 key 여덟 개가 재고 10에 qty 2씩: 정확히 다섯이 성공하고 재고는 0이며 음수가 되지 않는다.
  const many = await Promise.all(Array.from({ length: 8 }, async (_, i) => (await clientFor(f)).apply(take(2, 2), { key: `race-many-${i}-0001` })));
  assert.equal(many.filter((r) => r.ok).length, 5, JSON.stringify(many.map((r) => r.code ?? "ok")));
  assert.deepEqual([...new Set(many.filter((r) => !r.ok).map((r) => r.code))], ["WIRE.INVALID_STATE"]);
  assert.equal(stock(2), 0);
  assert.equal(audit(f).filter((r) => r.requestKey?.startsWith("race-many-")).length, 5, "one audit row per executed decrement");
  assert.equal(f.sql("SELECT count(*) FROM ${S}.product WHERE stock < 0"), "0");

  // 4) 묶음 안의 같은 전이: 연산마다 한 번씩 두 번 줄고, 하나가 실패하면 앞 연산도 취소된다(재고 3 -> 1, 실패한 묶음은 1 그대로).
  const bundleOk = await client.bundle({ atomic: [take(3, 1), take(3, 1)] }, { key: "bundle-ok-0001" });
  assert.equal(bundleOk.ok, true);
  assert.equal(stock(3), 1);
  // 묶음 안의 전이도 감사 행에는 via "apply"로 남는다(묶음과 직접 호출은 감사 행으로 구별되지 않는다).
  assert.deepEqual(rowsFor("bundle-ok-0001").map((r) => [r.via, r.transition]), [["apply", "take"], ["apply", "take"]]);
  const bundleBad = await client.bundle({ atomic: [take(3, 1), take(3, 1)] }, { key: "bundle-bad-0001" });
  assert.equal(bundleBad.ok, false);
  assert.equal(bundleBad.code, "WIRE.INVALID_STATE");
  assert.equal(stock(3), 1, "the first take of the failed bundle was rolled back");
  assert.equal(rowsFor("bundle-bad-0001").length, 0, "a rolled-back request leaves no audit row");
  assert.equal(f.sql("SELECT count(*) FROM ${S}.aip_idem WHERE key = 'bundle-bad-0001'"), "0", "...and no idempotency record");

  // 5) 응답 유실: 서버는 커밋했고 클라이언트는 응답을 못 받았다. 같은 key 재전송이 저장된 결과를 주고 재고는 한 번만 줄었다.
  let drop = 1;
  const flaky = async (input, init) => {
    const response = await fetch(input, init);
    if (drop > 0 && String(input).endsWith("/apply")) { drop--; throw new TypeError("response lost"); }
    return response;
  };
  const lossy = await clientFor(f, "alice", flaky);
  const recovered = await lossy.apply(take(4, 2), { key: "lost-0001" });
  assert.deepEqual([recovered.ok, recovered.recovered, recovered.replayed], [true, true, true]);
  assert.equal(stock(4), 3);
  assert.equal(rowsFor("lost-0001").length, 1);
});

// ---------------------------------------------------------------------------------------------------------------------------

const DOC_APP = `
resource Member { fields { id: Id; name: Text(1..20) } rows read when true }
actor Member
resource Doc {
  fields { id: Id; owner: Member; title: Text(1..30); summary: Text?; secret: Text? }
  rows read when true
  field secret read when owner = actor
  transition retitle { allow true; from true; patch title, summary }
  transition stamp { allow true; from true; to summary = title }
  expose read { select id, title, summary, secret; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose apply retitle { target id; bulk maxRows 3 }
  expose apply stamp { target id; bulk maxRows 3 }
}
`;

test("a hidden field never reaches another field: the definition refuses the copy and permitted writes do not leak the value", { timeout: 240_000 }, async () => {
  // 정의: 숨은 필드(`secret`)를 다른 필드로 옮기는 모든 모양은 service check가 거절한다.
  const copies = {
    "to summary = secret": DOC_APP.replace("to summary = title", "to summary = secret"),
    "from secret = \"x\"": DOC_APP.replace("transition stamp { allow true; from true;", 'transition stamp { allow true; from secret = "x";'),
    "repeat unchanged on a hidden target": DOC_APP.replace("to summary = title }", "to secret = secret; repeat unchanged }"),
    "predicate over the hidden field": `predicate leaks(d: Doc) = d.secret = "x"\n${DOC_APP.replace("transition stamp { allow true; from true;", "transition stamp { allow true; from leaks(this);")}`,
  };
  for (const [name, definition] of Object.entries(copies)) {
    const refused = await startFixture({ definition, python: false, serve: false, allowCheckFailure: true });
    fixtures.push(refused);
    assert.equal(refused.checked, false, `service check must refuse: ${name}`);
    assert.match(refused.check.stdout + refused.check.stderr, /DEF\.POLICY_FIELD_NOT_USABLE_IN_TRANSITION/u, name);
  }

  // 허용된 정의: 쓰기는 가능하지만 값은 새지 않는다.
  const f = await startFixture({
    definition: DOC_APP, actors: { alice: 1, bob: 2 }, python: false, sdkImport: sdkEntry,
    seed: "INSERT INTO ${S}.member(id,name) VALUES (1,'alice'),(2,'bob'); INSERT INTO ${S}.doc(id,owner_id,title,summary,secret) VALUES (1,1,'plan',NULL,'alice-only'),(2,1,'plan2','s2','alice-only-2');",
  });
  fixtures.push(f);
  const alice = await clientFor(f, "alice");
  const bob = await clientFor(f, "bob");
  // SDK 응답: bob은 secret을 못 읽는다(null). alice는 읽는다.
  const bobSees = (await bob.read({ read: "Doc", select: ["id", "title", "secret"], sort: [{ field: "id", dir: "asc" }] })).rows;
  assert.deepEqual(bobSees.map((r) => r.secret), [null, null]);
  assert.deepEqual((await alice.read({ read: "Doc", select: ["id", "secret"], sort: [{ field: "id", dir: "asc" }] })).rows.map((r) => r.secret), ["alice-only", "alice-only-2"]);
  // bob이 다른 필드를 쓴다: 제목과 요약을 바꾸고, stamp(summary = title)를 부른다. secret은 그대로고 summary에 secret 값이 없다.
  assert.equal((await bob.apply({ apply: "Doc.retitle", target: { ids: ["1"] }, values: { title: "bob title", summary: "bob summary" } })).ok, true);
  assert.equal((await bob.apply({ apply: "Doc.stamp", target: { ids: ["1", "2"] } })).ok, true);
  assert.equal(f.sql("SELECT secret FROM ${S}.doc WHERE id=1"), "alice-only");
  assert.equal(f.sql("SELECT count(*) FROM ${S}.doc WHERE summary = secret OR title = secret"), "0", "no column was filled from the hidden value");
  assert.equal(f.sql("SELECT summary FROM ${S}.doc WHERE id=1"), "bob title");
  // 감사 기록도 호출자(bob)가 변경한 필드만 담고 secret의 값을 다른 키에 싣지 않는다.
  const records = audit(f).filter((r) => r.actorId === 2);
  assert.ok(records.length >= 2);
  for (const record of records) assert.ok(!JSON.stringify(record).includes("alice-only"), JSON.stringify(record));
  // 필터·정렬에도 숨은 필드는 못 쓴다: 공개 읽기 선언에 filter가 없으므로 계약에 나타나지 않는다.
  const binding = readFileSync(join(f.dir, "binding.ts"), "utf8");
  assert.doesNotMatch(binding, /filter: "[^"]*secret/u);
});

// ---------------------------------------------------------------------------------------------------------------------------

test("a partly failing atomic create cancels everything: bundle and write extension, and the key can be reused with a fixed request", { timeout: 240_000 }, async () => {
  const f = await startFixture({ ...SHOP, actors: { alice: 1 }, python: false, seed: SHOP_SEED, sdkImport: sdkEntry });
  fixtures.push(f);
  const client = await clientFor(f);
  const count = (table) => Number(f.sql(`SELECT count(*) FROM \${S}."${table}"`));
  const before = { order: count("order"), line: count("order_line") };
  const create = (extra) => ({ atomic: [
    { create: "Order", values: { owner: "1", status: "open", total: "3.00" } },
    { create: "OrderLine", values: { product: "1", qty: 1, price: "3.00" }, refs: { order: { created: 0 } } },
    { create: "OrderLine", values: { product: extra, qty: 1, price: "3.00" }, refs: { order: { created: 0 } } },
  ] });
  // 셋째 create가 없는 상품을 가리킨다: 앞의 두 create도 취소된다.
  const failed = await client.bundle(create("999"), { key: "atomic-0001" });
  assert.equal(failed.ok, false);
  assert.equal(failed.code, "WIRE.MISSING_TARGET");
  assert.deepEqual({ order: count("order"), line: count("order_line") }, before, "the order and the first line were cancelled");
  assert.equal(f.sql("SELECT count(*) FROM ${S}.aip_idem WHERE key = 'atomic-0001'"), "0");
  assert.equal(audit(f).filter((r) => r.requestKey === "atomic-0001").length, 0);
  // 같은 key로 고친 요청을 보내면 새로 실행되어 정확히 한 번 만들어진다.
  const fixed = await client.bundle(create("2"), { key: "atomic-0001" });
  assert.equal(fixed.ok, true, JSON.stringify(fixed));
  assert.equal(fixed.replayed, undefined);
  assert.deepEqual({ order: count("order"), line: count("order_line") }, { order: before.order + 1, line: before.line + 2 });

  // 쓰기 확장: 주문과 줄을 만든 뒤 예외로 끝난다. 두 create가 모두 취소된다.
  const exploded = await client.writeExtension("Order.place", { owner: "1", product: "1", explode: true }, { key: "ext-explode-0001" });
  assert.equal(exploded.ok, false);
  assert.equal(exploded.code, "WIRE.EXTENSION_ERROR");
  assert.deepEqual({ order: count("order"), line: count("order_line") }, { order: before.order + 1, line: before.line + 2 }, "the extension's two creates were cancelled");
  const placed = await client.writeExtension("Order.place", { owner: "1", product: "1", explode: false }, { key: "ext-place-0001" });
  assert.equal(placed.ok, true, JSON.stringify(placed));
  assert.deepEqual({ order: count("order"), line: count("order_line") }, { order: before.order + 2, line: before.line + 3 });
});
