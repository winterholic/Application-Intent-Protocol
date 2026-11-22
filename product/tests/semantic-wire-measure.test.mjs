// D6·D1 실측 시험.
//  (1) 쓰기 확장 `/apply`를 포함한 모든 `/apply` 경로에서 `x-aip-contract` 헤더가 없을 때·형식이 틀릴 때·지문이 다를 때의 코드 표.
//      product/SUPPORT.md "계약 헤더 오류 코드"가 마지막 줄을 "코드 확인만"으로 남겨 둔 것을 실행으로 바꾼다.
//  (2) 단독 집계(`expose aggregate`)의 시간 한도가 정의의 `budget deadline`이 아니라 고정 2초임을 잠금으로 실측한다.
//      원본 표를 ACCESS EXCLUSIVE 잠금으로 막아 질의가 끝나지 못하게 하고 응답까지 걸린 시간을 잰다.
//
//   cargo build -p aip-cli && node --test --test-concurrency=1 product/tests/semantic-wire-measure.test.mjs
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { after, test } from "node:test";
import { startFixture, cleanupTemp } from "../test-support/service-fixture.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtures = [];
after(async () => {
  for (const f of fixtures) await f.stop();
  cleanupTemp();
});

test("contract header codes are the same for transition, bundle, compose and status, and the write extension answers CONTRACT_MISMATCH", { timeout: 120_000 }, async () => {
  const definition = readFileSync(join(root, "product/tests/fixtures/sdk-semantics/app.aip"), "utf8");
  const extension = readFileSync(join(root, "product/tests/fixtures/sdk-semantics/extensions/sdk.mjs"), "utf8");
  const f = await startFixture({ definition, actors: { alice: 1 }, extensions: { "sdk.mjs": extension }, python: false, seed: "INSERT INTO ${S}.member(id,name) VALUES (1,'alice');" });
  fixtures.push(f);
  const requests = {
    transition: { apply: "Order.confirm", target: { ids: ["1"] } },
    bundle: { atomic: [{ apply: "Order.confirm", target: { ids: ["1"] } }] },
    compose: { compose: "Order", targets: { ids: ["1"] }, steps: [{ transition: "confirm" }] },
    extension: { extension: "Account.adjust", input: { a: "00000000-0000-4000-8000-000000000001", amount: "1.00", fail: false } },
  };
  const variants = {
    missing: { contract: false },
    upperHex: { contract: f.fingerprint.toUpperCase() },
    tooShort: { contract: "abc" },
    wrongFingerprint: { contract: "0".repeat(64) },
  };
  // 전이·묶음·조합은 "없음"과 "다름"을 구분하지만 쓰기 확장 /apply는 둘 다 CONTRACT_MISMATCH다.
  const table = {};
  for (const [kind, request] of Object.entries(requests)) {
    table[kind] = {};
    for (const [name, variant] of Object.entries(variants)) {
      const reply = await f.post("/apply", { request, key: `header-${kind}-${name}-0001` }, { token: "alice", ...variant });
      table[kind][name] = reply.body.code;
    }
  }
  const plain = { missing: "WIRE.CONTRACT_REQUIRED", upperHex: "WIRE.BAD_REQUEST", tooShort: "WIRE.BAD_REQUEST", wrongFingerprint: "WIRE.CONTRACT_MISMATCH" };
  assert.deepEqual(table.transition, plain);
  assert.deepEqual(table.bundle, plain);
  assert.deepEqual(table.compose, plain);
  assert.deepEqual(table.extension, { ...plain, missing: "WIRE.CONTRACT_MISMATCH" }, "write extension /apply has no CONTRACT_REQUIRED: a missing header is CONTRACT_MISMATCH");
  const missing = await f.post("/apply", { request: requests.extension, key: "header-extension-message-0001" }, { token: "alice", contract: false });
  assert.equal(missing.body.retry, "never");
  // 헤더가 없거나 다를 때 서버는 아무것도 실행하지 않는다. 같은 key로 올바른 헤더를 붙이면 새 실행이다(저장된 결과가 없다).
  assert.equal(f.sql("SELECT count(*) FROM ${S}.aip_idem"), "0", "a refused contract header leaves no idempotency record");
  for (const [name, variant] of Object.entries({ missing: variants.missing, wrongFingerprint: variants.wrongFingerprint })) {
    const status = await f.post("/status", { request: requests.transition, key: `header-status-${name}-0001` }, { token: "alice", ...variant });
    assert.equal(status.body.code, plain[name]);
  }
});

test("a standalone aggregate answers WIRE.DEADLINE_EXCEEDED after a fixed 2 s, while an exposed read honours its declared 1 s budget", { timeout: 120_000 }, async () => {
  const definition = `
resource Member { fields { id: Id } rows read when id = actor }
actor Member
access anyone(a: Member) = true
resource Visit {
  fields { id: Id; owner: Member; n: Int(0..1000) }
  rows read when owner = actor
  aggregate total: Int { sourceAccess anyone(actor); callerFilter none; rowOutput none; release count }
  expose aggregate total
  expose read { select id, n; sort id; budget { rows 5; depth 1; deadline 1s; cost 100 } }
}
`;
  const f = await startFixture({ definition, actors: { alice: 1 }, python: false, seed: "INSERT INTO ${S}.member(id) VALUES (1); INSERT INTO ${S}.visit(id,owner_id,n) VALUES (1,1,5);" });
  fixtures.push(f);
  const healthy = await f.post("/read", { query: { aggregate: "Visit.total", input: {} } }, { token: "alice" });
  assert.equal(healthy.body.ok, true, JSON.stringify(healthy.body));
  assert.deepEqual(healthy.body.rows, [1]);

  // 다른 연결이 표를 잠근다. 질의는 잠금을 기다리다 statement_timeout에서 끊긴다.
  const locker = spawn("psql", [process.env.AIP_TEST_DATABASE_URL, "-X", "-q", "-v", "ON_ERROR_STOP=1", "-c", `BEGIN; LOCK TABLE ${f.schema}.visit IN ACCESS EXCLUSIVE MODE; SELECT pg_sleep(7); COMMIT;`], { stdio: "ignore" });
  try {
    await new Promise((r) => setTimeout(r, 800));
    const timed = async (path, body) => {
      const started = performance.now();
      const reply = await f.post(path, body, { token: "alice" });
      return { reply, ms: performance.now() - started };
    };
    const read = await timed("/read", { query: { read: "Visit", select: ["id", "n"] } });
    const aggregate = await timed("/read", { query: { aggregate: "Visit.total", input: {} } });
    assert.equal(read.reply.body.code, "WIRE.DEADLINE_EXCEEDED", JSON.stringify(read.reply.body));
    assert.equal(aggregate.reply.body.code, "WIRE.DEADLINE_EXCEEDED", JSON.stringify(aggregate.reply.body));
    console.log(`measured deadline: declared read budget 1s -> ${Math.round(read.ms)} ms; standalone aggregate -> ${Math.round(aggregate.ms)} ms`);
    assert.ok(read.ms >= 900 && read.ms < 1800, `read honours its 1 s budget (${read.ms} ms)`);
    assert.ok(aggregate.ms >= 1900 && aggregate.ms < 3200, `standalone aggregate times out after the fixed 2 s (${aggregate.ms} ms)`);
  } finally {
    locker.kill("SIGTERM");
    await new Promise((r) => setTimeout(r, 300));
  }
});
