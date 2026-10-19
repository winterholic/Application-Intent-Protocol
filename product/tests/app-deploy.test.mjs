import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmod, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { createServer } from "node:http";
import { test } from "node:test";

import * as module from "../tools/app-deploy.mjs";
async function fixture(t) {
  const base = await realpath(await mkdtemp(join(tmpdir(), "aip-app-deploy-test-")));
  t.after(() => rm(base, { recursive: true, force: true }));
  const root = join(base, "package");
  const put = async (name, value) => { const path = join(root, name); await mkdir(dirname(path), { recursive: true }); await writeFile(path, typeof value === "string" ? value : JSON.stringify(value)); };
  const extension = { directory: "workers", language: "node", entrypoints: ["event.mjs"], enable_writes: true };
  const app = { name: "fixture", version: "1.0.0", definition: "app.aip", extensions: extension };
  const contract = { app, contractDigest: "a".repeat(64), packageLayout: { bin: "bin/aip", definition: "app/app.aip", extensions: "app/workers", manifest: "app/aip.app.json" } };
  const targets = { "darwin-arm64": "aarch64-apple-darwin", "darwin-x64": "x86_64-apple-darwin", "linux-x64": "x86_64-unknown-linux-gnu", "linux-arm64": "aarch64-unknown-linux-gnu" };
  const build = { format: 1, target: targets[`${process.platform}-${process.arch}`], app: { name: app.name, version: app.version }, contract_digest: contract.contractDigest };
  const files = { "BUILD.json": build, "APP_CONTRACT.json": contract, "app/aip.app.json": app, "app/app.aip": "resource Item {}", "app/workers/event.mjs": "export {};", "bin/aip": '#!/bin/sh\nprintf \'{"ok":true}\\n\'\n' };
  const seal = async () => { const lines = []; for (const name of Object.keys(files)) lines.push(`${createHash("sha256").update(await readFile(join(root, name))).digest("hex")}  ${name}`); await put("SHA256SUMS", lines.join("\n") + "\n"); };
  for (const [name, value] of Object.entries(files)) await put(name, value);
  await chmod(join(root, "bin/aip"), 0o755);
  await seal();
  const configPath = join(base, "operator.json");
  const config = { schema: "aip_fixture", database_url_env: "FIXTURE_RUNTIME_DSN", migrate_database_url_env: "FIXTURE_MIGRATE_DSN", listen: "127.0.0.1:23199", auth: { jwks: { kind: "file", path: "jwks.json" } }, workers: { directory: join(root, "app/workers"), language: "node", enable_writes: true, isolation: "auto" } };
  await writeFile(configPath, JSON.stringify(config));
  const environment = { ...process.env, FIXTURE_RUNTIME_DSN: "host=127.0.0.1 port=25432 dbname=aip_test_fixture", FIXTURE_MIGRATE_DSN: "host=127.0.0.1 port=25432 dbname=aip_test_fixture" };
  return { base, root, put, seal, files, config, configPath, build, contract, environment };
}

test("install verifies and copies a self-contained package to a fresh destination", async t => {
  assert.equal(typeof module.installPackage, "function", "install entrypoint is required");
  const f = await fixture(t); const out = join(f.base, "installed");
  await module.installPackage(f.root, out);
  await rm(f.root, { recursive: true });
  assert.equal((await module.verifyPackage(out)).contract.app.name, "fixture");
  await assert.rejects(module.installPackage(out, out), e => e.code === "OUTPUT_EXISTS");
});

test("installation rejects a destination aliased inside its source package", async t => {
  const f = await fixture(t);
  await symlink(f.root, join(f.base, "alias"));
  await assert.rejects(module.installPackage(f.root, join(f.base, "alias/nested")), e => e.code === "OUTPUT_IN_PACKAGE");
  assert.equal((await module.verifyPackage(f.root)).contract.app.name, "fixture");
});

for (const mutation of ["tamper", "extra", "symlink", "traversal", "missing-inventory"]) test(`package refuses ${mutation} before installation`, async t => {
  assert.equal(typeof module.installPackage, "function");
  const f = await fixture(t);
  if (mutation === "tamper") await f.put("bin/aip", "bad");
  if (mutation === "extra") await f.put(".env", "secret");
  if (mutation === "symlink") { await rm(join(f.root, "bin/aip")); await symlink(process.execPath, join(f.root, "bin/aip")); }
  if (mutation === "traversal") await f.put("SHA256SUMS", `${"a".repeat(64)}  ../operator.json\n`);
  if (mutation === "missing-inventory") await f.put("SHA256SUMS", "");
  await assert.rejects(module.installPackage(f.root, join(f.base, "out")), e => typeof e.code === "string");
  await assert.rejects(readFile(join(f.base, "out/BUILD.json")), { code: "ENOENT" });
});

test("migration preserves digest gate and separates migrate and runtime environment", async t => {
  assert.equal(typeof module.prepareCommand, "function");
  const f = await fixture(t);
  const plan = await module.prepareCommand({ root: f.root, action: "plan", config: f.configPath }, f.environment);
  assert.deepEqual(plan.args, ["service", "migrate", join(f.root, "app/app.aip"), "--config", f.configPath]);
  await assert.rejects(module.prepareCommand({ root: f.root, action: "migrate", config: f.configPath }, f.environment), e => e.code === "DIGEST_REQUIRED");
  const apply = await module.prepareCommand({ root: f.root, action: "migrate", config: f.configPath, digest: "b".repeat(64) }, f.environment);
  assert.deepEqual(apply.args.slice(-2), ["--apply", "b".repeat(64)]);
  delete f.environment.FIXTURE_MIGRATE_DSN;
  await assert.rejects(module.prepareCommand({ root: f.root, action: "init", config: f.configPath }, f.environment), e => e.code === "ENV_MISSING");
});

test("wrong platform is rejected before executing binary", async t => {
  assert.equal(typeof module.verifyPackage, "function");
  const f = await fixture(t); f.build.target = "x86_64-pc-windows-msvc";
  await f.put("BUILD.json", f.build); await f.seal();
  await assert.rejects(module.verifyPackage(f.root), e => e.code === "TARGET_MISMATCH");
});

for (const mutation of ["worker-path", "worker-language", "worker-runtime", "isolation", "secret", "schema", "listen"]) test(`configuration refuses ${mutation}`, async t => {
  assert.equal(typeof module.prepareCommand, "function");
  const f = await fixture(t);
  if (mutation === "worker-path") f.config.workers.directory = f.base;
  if (mutation === "worker-language") f.config.workers.language = "python";
  if (mutation === "worker-runtime") f.config.workers.runtime_executable = "/nonexistent-aip-worker-runtime";
  if (mutation === "isolation") f.config.workers.isolation = "none";
  if (mutation === "secret") f.config.password = "NEVER_PRINT_THIS";
  if (mutation === "schema") f.config.schema = "public";
  if (mutation === "listen") f.config.listen = "0.0.0.0:8080";
  await writeFile(f.configPath, JSON.stringify(f.config));
  await assert.rejects(module.prepareCommand({ root: f.root, action: "serve", config: f.configPath }, f.environment), e => e.code && !e.message.includes("NEVER_PRINT_THIS"));
});

test("readiness requires successful status and refuses redirects", async t => {
  assert.equal(typeof module.probe, "function");
  let status = 503;
  const server = createServer((request, response) => { response.writeHead(status, { Location: "http://example.com" }); response.end('{"ok":true}'); });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise(resolve => server.close(resolve)));
  const url = `http://127.0.0.1:${server.address().port}/ready`;
  await assert.rejects(module.probe(url), e => e.code === "NOT_READY");
  status = 302; await assert.rejects(module.probe(url), e => e.code === "NOT_READY");
  status = 200; assert.equal((await module.probe(url)).ok, true);
});
