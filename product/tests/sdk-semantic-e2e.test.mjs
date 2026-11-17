import assert from "node:assert/strict";
import { generateKeyPairSync, sign, randomUUID } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, test } from "node:test";
import { requireTestDatabase } from "../test-support/postgres-guard.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const binary = join(root, "target/debug/aip");
const database = requireTestDatabase().raw;
const temp = mkdtempSync(join(tmpdir(), "aip-sdk-semantic-"));
const app = `
resource Member {
  fields { id: Id; org: Text }
  rows read when true
  expose read { select id, org; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
actor Member
resource Record {
  fields { id: Id; org: Text; owner: Member; title: Text; note: Text?; amount: Decimal(18,4)?; secret: Text; checked: Bool; applies: Int }
  rows read when org = actor.org
  field secret read when owner = actor
  transition edit { allow owner = actor; from true; patch title, note, amount; to checked = checked, applies = this.applies + 1 }
  expose apply edit { target id; bulk maxRows 1 }
  expose read {
    select id, org, owner, title, note, amount, secret, checked, applies
    traverse owner { select id, org }
    sort id
    budget { rows 20; depth 2; deadline 2s; cost 1000 }
  }
}
`;

function run(program, args, options = {}) {
  const result = spawnSync(program, args, {
    cwd: options.cwd ?? root,
    encoding: "utf8",
    env: { ...process.env, AIP_TEST_DATABASE: database, ...options.env },
    timeout: options.timeout ?? 120_000,
    maxBuffer: 4 * 1024 * 1024,
  });
  assert.equal(result.error, undefined, `${program} could not start: ${result.error?.message}`);
  assert.equal(result.signal, null, `${program} terminated by ${result.signal}`);
  assert.equal(result.status, 0, `${program} ${args.join(" ")} failed:\n${result.stdout}\n${result.stderr}`);
  return result.stdout.trim();
}

function psql(sql) {
  return run("psql", [database, "-X", "-v", "ON_ERROR_STOP=1", "-At", "-c", sql]);
}

function authFiles(dir) {
  const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const jwk = publicKey.export({ format: "jwk" });
  Object.assign(jwk, { kid: "sdk-semantic-e2e", use: "sig", alg: "RS256", key_ops: ["verify"] });
  writeFileSync(join(dir, "jwks.json"), JSON.stringify({ keys: [jwk] }));
  const now = Math.floor(Date.now() / 1000);
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const header = encode({ alg: "RS256", kid: jwk.kid, typ: "at+jwt" });
  const claims = encode({ iss: "https://issuer.example", aud: "aip-api", sub: "sdk-semantic-user", iat: now, nbf: now, exp: now + 300 });
  const input = `${header}.${claims}`;
  return sign("RSA-SHA256", Buffer.from(input), privateKey).toString("base64url").replace(/^/, `${input}.`);
}

async function fixture(language) {
  assert.ok(database, "PG17 test database is configured");
  const dir = join(temp, `${language}-${randomUUID()}`);
  mkdirSync(dir);
  const schema = `aip_sdk_sem_${randomUUID().replaceAll("-", "").slice(0, 24)}`;
  const owned = { dir, schema, child: null };
  let transferred = false;
  try {
    const appPath = join(dir, "semantic.aip");
    const configPath = join(dir, "service.json");
    writeFileSync(appPath, app);
    const token = authFiles(dir);
    writeFileSync(configPath, JSON.stringify({
      schema, database_url_env: "AIP_TEST_DATABASE", listen: "127.0.0.1:0", wire: "decimal",
      auth: { issuer: "https://issuer.example", audience: "aip-api", jwks: { kind: "file", path: join(dir, "jwks.json") } },
    }));
    run(binary, ["service", "check", appPath]);
    run(binary, ["service", "gen", appPath, "--wire", "decimal", "--out", join(dir, "binding.ts"), "--sdk-import", "@aip/sdk"]);
    const generated = readFileSync(join(dir, "binding.ts"), "utf8");
    const fingerprint = generated.match(/export const contractFingerprint = "([a-f0-9]{64})";/u)?.[1];
    assert.ok(fingerprint, "generated TypeScript binding contains the contract fingerprint");
    if (language === "python") {
      run(binary, ["service", "gen", appPath, "--lang", "python", "--wire", "decimal", "--out", join(dir, "binding.py")]);
    }
    run(binary, ["service", "init", appPath, "--config", configPath]);
    psql(`INSERT INTO ${schema}.member(id,org) VALUES (42,'red'),(99,'red'); INSERT INTO ${schema}.record(id,org,owner_id,title,note,amount,secret,checked,applies) VALUES (1,'red',42,'seed','preserve-me','1.2345','owner-only',false,0),(2,'red',42,'nullable',NULL,NULL,'also-private',false,0),(3,'red',99,'peer','peer-note','2.0001','hidden-secret',false,0),(4,'blue',99,'outside','outside-note','3.0001','outside-secret',false,0);`);
    run(binary, ["service", "principal", "--config", configPath, "bind", "--subject", "sdk-semantic-user", "--actor", "42"]);
    const child = spawn(binary, ["service", "serve", appPath, "--config", configPath], {
      cwd: root, env: { ...process.env, AIP_TEST_DATABASE: database }, stdio: ["ignore", "pipe", "pipe"],
    });
    owned.child = child;
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    const listening = await new Promise((resolveListening, reject) => {
      const timeout = setTimeout(() => reject(new Error(`service start timeout: ${stderr}`)), 15_000);
      child.stdout.on("data", () => {
        const line = stdout.split(/\r?\n/u)[0];
        if (!line) return;
        clearTimeout(timeout);
        try {
          const message = JSON.parse(line);
          if (message.ok && message.event === "listening") resolveListening(message);
          else reject(new Error(`service startup rejected: ${line}\n${stderr}`));
        } catch (error) { reject(error); }
      });
      child.once("exit", (code) => { clearTimeout(timeout); reject(new Error(`service exited ${code}: ${stdout}\n${stderr}`)); });
    });
    assert.equal(listening.fingerprint, fingerprint, "service and generated TypeScript binding use the same declared contract");
    if (language === "python") {
      const pythonGenerated = readFileSync(join(dir, "binding.py"), "utf8");
      const pythonFingerprint = pythonGenerated.match(/^CONTRACT_FINGERPRINT = '([a-f0-9]{64})'$/mu)?.[1]
        ?? pythonGenerated.match(/^CONTRACT_FINGERPRINT = "([a-f0-9]{64})"$/mu)?.[1];
      assert.ok(pythonFingerprint, "generated Python binding contains the contract fingerprint");
      assert.equal(pythonFingerprint, listening.fingerprint, "Python generated binding and service use the same declared contract");
    }
    transferred = true;
    return { ...owned, appPath, token, address: listening.address, fingerprint };
  } finally {
    if (!transferred) await closeFixture(owned);
  }
}

async function closeFixture(value) {
  if (value.child && value.child.exitCode === null && value.child.signalCode === null) {
    await new Promise((resolveExit) => {
      value.child.once("exit", resolveExit);
      value.child.kill("SIGTERM");
    });
  }
  psql(`DROP SCHEMA IF EXISTS ${value.schema} CASCADE`);
}

function assertStored(fixtureValue, title) {
  const result = psql(`SELECT title || '|' || coalesce(note,'<null>') || '|' || coalesce(amount::text,'<null>') || '|' || checked::text || '|' || org || '|' || owner_id::text || '|' || secret || '|' || applies::text FROM ${fixtureValue.schema}.record WHERE id=1`);
  assert.equal(result, `${title}|<null>|987654321012.3456|false|red|42|owner-only|2`, "PostgreSQL retains exact values and shows the two distinct successful requests applied exactly once each");
  const forbidden = psql(`SELECT title || '|' || secret FROM ${fixtureValue.schema}.record WHERE id=3`);
  assert.equal(forbidden, "peer|hidden-secret", "rejected SDK request leaves the forbidden row unchanged");
  assert.equal(psql(`SELECT title FROM ${fixtureValue.schema}.record WHERE id=4`), "outside", "row-hidden resource remains unchanged");
  assert.equal(psql(`SELECT count(*) FROM ${fixtureValue.schema}.record WHERE id=404`), "0", "the missing target remains absent");
  assert.equal(psql(`SELECT org FROM ${fixtureValue.schema}.member WHERE id=42`), "red", "the SDK's related Member exists unchanged in PostgreSQL");
}

async function prepareSdk() {
  const packageDir = join(temp, "sdk");
  const packDir = join(temp, "pack");
  const consumer = join(temp, "consumer");
  const npmCache = join(temp, "npm-cache");
  mkdirSync(packDir);
  mkdirSync(consumer);
  run(process.execPath, [join(root, "product/tools/build-sdk.mjs"), "--out", packageDir]);
  const packed = JSON.parse(run("npm", ["pack", "--offline", "--ignore-scripts", "--json", "--pack-destination", packDir, packageDir], { env: { npm_config_cache: npmCache } }));
  const tarball = join(packDir, packed[0].filename);
  writeFileSync(join(consumer, "package.json"), '{"private":true,"type":"module"}\n');
  run("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", "--prefix", consumer, tarball], { env: { npm_config_cache: npmCache } });
  return { consumer, tarball };
}

before(() => {
  assert.ok(database, "set AIP_TEST_DATABASE_URL to the disposable PostgreSQL 17 service");
  assert.ok(readFileSync(binary).byteLength > 0, "build target/debug/aip before running this suite");
});
after(() => rmSync(temp, { recursive: true, force: true }));

test("generated TypeScript SDK preserves nullable, decimal, policy, replay, and error semantics over PG17", { timeout: 180_000 }, async () => {
  const sdk = await prepareSdk();
  const f = await fixture("ts");
  try {
    const generated = readFileSync(join(f.dir, "binding.ts"), "utf8");
    writeFileSync(join(sdk.consumer, "binding.ts"), generated);
    run(join(root, "spikes/spike-0-ts/node_modules/.bin/tsc"), ["--strict", "--target", "ES2022", "--module", "NodeNext", "--moduleResolution", "NodeNext", "--outDir", sdk.consumer, join(sdk.consumer, "binding.ts")], { cwd: sdk.consumer });
    writeFileSync(join(sdk.consumer, "consumer.ts"), `
import { connect } from "@aip/sdk";
import { contract } from "./binding.js";
const client = connect("http://aip.test", "token", contract);
void client.read({ read: "Record", select: ["id", "note", "amount"] });
void client.apply({ apply: "Record.edit", target: { ids: ["1"] }, values: { note: null, amount: "1.2300" } }, { key: "typecheck-key-0001" });
void client.apply({ apply: "Record.edit", target: { ids: ["1"] }, values: { title: "partial" } }, { key: "typecheck-key-0002" });
`);
    run(join(root, "spikes/spike-0-ts/node_modules/.bin/tsc"), ["--noEmit", "--strict", "--target", "ES2022", "--module", "NodeNext", "--moduleResolution", "NodeNext", join(sdk.consumer, "consumer.ts")], { cwd: sdk.consumer });
    writeFileSync(join(sdk.consumer, "run.mjs"), `
import assert from "node:assert/strict";
import { connect } from "@aip/sdk";
import { contract } from "./binding.js";
const client = connect(${JSON.stringify(`http://${f.address}`)}, ${JSON.stringify(f.token)}, contract);
assert.deepEqual(await client.session(), { actorId: "42" });
const initial = await client.read({ read: "Record", select: ["id", "title", "note", "amount", "secret"], sort: [{field:"id",dir:"asc"}] });
assert.deepEqual(initial.rows.map(row => row.id), ["1", "2", "3"], "row indices refer to the expected IDs");
assert.equal(initial.rows[0].note, "preserve-me");
assert.equal(initial.rows[0].amount, "1.2345");
assert.equal(initial.rows[0].secret, "owner-only");
assert.equal(initial.rows[1].note, null);
assert.equal(initial.rows[1].amount, null);
assert.equal(initial.rows[2].secret, null, "policy-hidden field is redacted on a visible row");
const relation = await client.read({ read: "Record", select: ["id", { owner: { select: ["id", "org"] } }], sort: [{field:"id",dir:"asc"}] });
assert.deepEqual(relation.rows[0].owner, { id: "42", org: "red" }, "generated SDK returns the declared related projection");
const omitted = await client.apply({ apply: "Record.edit", target: { ids: ["1"] }, values: { title: "ts-partial" } }, { key: "sdk-ts-partial-0001" });
assert.equal(omitted.ok, true);
const partial = await client.read({ read: "Record", select: ["id", "title", "note", "amount", "applies"], sort: [{field:"id",dir:"asc"}] });
assert.equal(partial.rows[0].id, "1");
assert.equal(partial.rows[0].note, "preserve-me", "an omitted nullable field is preserved");
assert.equal(partial.rows[0].amount, "1.2345", "an omitted Decimal field is preserved");
assert.equal(partial.rows[0].applies, 1, "the first unique apply changes the DB effect counter once");
const write = { apply: "Record.edit", target: { ids: ["1"] }, values: { title: "ts-updated", note: null, amount: "987654321012.3456" } };
const first = await client.apply(write, { key: "sdk-ts-replay-0001" });
const replay = await client.apply(write, { key: "sdk-ts-replay-0001" });
assert.equal(first.ok, true);
assert.equal(replay.replayed, true);
const after = await client.read({ read: "Record", select: ["id", "title", "note", "amount", "secret", "applies"], sort: [{field:"id",dir:"asc"}] });
assert.equal(after.rows[0].id, "1");
assert.equal(after.rows[0].note, null, "explicit null clears nullable field");
assert.equal(after.rows[0].amount, "987654321012.3456", "Decimal is returned without float rounding");
assert.equal(after.rows[0].secret, "owner-only");
assert.equal(after.rows[0].applies, 2, "the replay did not increment the DB effect counter a second time");
const drift = connect(${JSON.stringify(`http://${f.address}`)}, ${JSON.stringify(f.token)}, { ...contract, fingerprint: "0".repeat(64) });
await assert.rejects(drift.read({ read: "Record", select: ["id"] }), error => error.code === "WIRE.CONTRACT_MISMATCH");
for (const [id, code] of [["3", "WIRE.FORBIDDEN"], ["4", "WIRE.MISSING_TARGET"], ["404", "WIRE.MISSING_TARGET"]]) {
  const denied = await client.apply({ apply: "Record.edit", target: { ids: [id] }, values: { title: "forged" } }, { key: "sdk-ts-denial-"+id });
  assert.equal(denied.ok, false);
  assert.equal(denied.code, code);
}
console.log("TypeScript SDK tests: 1 passed, 0 failed");
`);
    // Generated bindings may be emitted beside source, so resolve the consumer copy explicitly.
    assert.ok(readFileSync(join(sdk.consumer, "binding.js")).byteLength > 0);
    console.log(run(process.execPath, [join(sdk.consumer, "run.mjs")], { cwd: sdk.consumer, timeout: 90_000 }));
    assertStored(f, "ts-updated");
  } finally { await closeFixture(f); }
});

test("generated Python SDK binding preserves nullable, decimal, policy, replay, and error semantics over PG17", { timeout: 180_000 }, async () => {
  const f = await fixture("python");
  try {
    const script = join(f.dir, "run.py");
    writeFileSync(script, `
from aip_sdk import AipError, connect
from binding import CONTRACT_FINGERPRINT, apply_record_edit, read_record
assert CONTRACT_FINGERPRINT == ${JSON.stringify(f.fingerprint)}
client = connect(${JSON.stringify(`http://${f.address}`)}, ${JSON.stringify(f.token)}, CONTRACT_FINGERPRINT)
assert client.session() == {"actorId":"42"}
initial = read_record(client, {"read":"Record","select":["id","title","note","amount","secret","applies"],"sort":[{"field":"id","dir":"asc"}]})
assert [row["id"] for row in initial] == ["1", "2", "3"]
assert initial[0]["note"] == "preserve-me" and initial[0]["amount"] == "1.2345"
assert initial[0]["secret"] == "owner-only" and initial[1]["note"] is None and initial[1]["amount"] is None
assert initial[2]["secret"] is None
relation = read_record(client, {"read":"Record","select":["id",{"owner":{"select":["id","org"]}}],"sort":[{"field":"id","dir":"asc"}]})
assert relation[0]["owner"] == {"id":"42","org":"red"}, relation[0]
omitted = apply_record_edit(client, {"ids":["1"]}, {"title":"python-partial"}, key="sdk-python-partial-0001")
assert omitted["ok"] is True
partial = read_record(client, {"read":"Record","select":["id","title","note","amount","applies"],"sort":[{"field":"id","dir":"asc"}]})
assert partial[0]["id"] == "1"
assert partial[0]["note"] == "preserve-me" and partial[0]["amount"] == "1.2345", partial[0]
assert partial[0]["applies"] == 1, partial[0]
write = {"apply":"Record.edit","target":{"ids":["1"]},"values":{"title":"python-updated","note":None,"amount":"987654321012.3456"}}
first = apply_record_edit(client, {"ids":["1"]}, {"title":"python-updated","note":None,"amount":"987654321012.3456"}, key="sdk-python-replay-0001")
replay = apply_record_edit(client, {"ids":["1"]}, {"title":"python-updated","note":None,"amount":"987654321012.3456"}, key="sdk-python-replay-0001")
assert first["ok"] is True and replay["replayed"] is True
after = read_record(client, {"read":"Record","select":["id","title","note","amount","secret","applies"],"sort":[{"field":"id","dir":"asc"}]})
assert after[0]["id"] == "1"
assert after[0]["note"] is None and after[0]["amount"] == "987654321012.3456" and after[0]["secret"] == "owner-only"
assert after[0]["applies"] == 2, after[0]
try:
    connect(${JSON.stringify(`http://${f.address}`)}, ${JSON.stringify(f.token)}, "0" * 64).read({"read":"Record","select":["id"]})
except AipError as error:
    assert error.code == "WIRE.CONTRACT_MISMATCH"
else:
    raise AssertionError("contract drift was not rejected")
for row_id, code in (("3","WIRE.FORBIDDEN"),("4","WIRE.MISSING_TARGET"),("404","WIRE.MISSING_TARGET")):
    denied = apply_record_edit(client, {"ids":[row_id]}, {"title":"forged"}, key="sdk-python-denial-"+row_id)
    assert denied["ok"] is False and denied["code"] == code, denied
print("Generated Python SDK tests: 1 passed, 0 failed")
`);
    console.log(run("python3", [script], { cwd: f.dir, env: { PYTHONPATH: join(root, "product/python") }, timeout: 90_000 }));
    assertStored(f, "python-updated");
  } finally { await closeFixture(f); }
});
