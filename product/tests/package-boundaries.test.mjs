import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const sourceDirectories = [
  "crates/aip-cli", "crates/aip-service", "crates/aip-auth", "crates/aip-migrate", "crates/aip-contract",
  "crates/aip-syntax", "crates/aip-sema", "crates/aip-ir", "crates/aip-pg", "crates/aip-plan", "crates/aip-runtime",
  "spikes/spike-v1-fixture", "spikes/spike-v2-read", "spikes/spike-v3-write", "spikes/spike-v4-worker",
  "spikes/spike-v5-sdk", "spikes/spike-v6-transport", "spikes/spike-v11-dev-checks",
];

function fixture(t, mutation = "none", realBuilder = false) {
  const root = mkdtempSync(join(tmpdir(), "aip-package-boundary-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const copy = (name) => {
    mkdirSync(dirname(join(root, name)), { recursive: true });
    cpSync(join(repo, name), join(root, name), { recursive: true });
  };
  for (const directory of sourceDirectories) {
    for (const part of ["Cargo.toml", "src", "sdk", "client", "workers"]) {
      if (existsSync(join(repo, directory, part))) copy(join(directory, part));
    }
  }
  for (const name of ["Cargo.toml", "Cargo.lock", "prototype/src/schema_catalog.sql", "prototype/example", "product/sdk", "product/tools"]) copy(name);
  mkdirSync(join(root, "target/debug"), { recursive: true });
  writeFileSync(join(root, "target/debug/aip"), "fixture executable\n");
  chmodSync(join(root, "target/debug/aip"), 0o755);
  if (realBuilder) {
    mkdirSync(join(root, "spikes/spike-0-ts"), { recursive: true });
    symlinkSync(join(repo, "spikes/spike-0-ts/node_modules"), join(root, "spikes/spike-0-ts/node_modules"), "dir");
  } else {
    // A controlled build subprocess makes concurrent edits deterministic without changing the shared checkout.
    writeFileSync(join(root, "product/tools/build-sdk.mjs"), `
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
const out = process.argv[3];
mkdirSync(out);
writeFileSync(join(out, "package.json"), JSON.stringify({ name: "@aip/sdk", version: "0.1.0", files: ["index.js"] }));
writeFileSync(join(out, "index.js"), "export const source = " + JSON.stringify(readFileSync("product/sdk/index.ts", "utf8")) + ";\\n");
const mutation = ${JSON.stringify(mutation)};
if (mutation === "edit") writeFileSync("product/sdk/index.ts", "export const changed = true;\\n");
if (mutation === "add") writeFileSync("spikes/spike-v5-sdk/sdk/added.ts", "export const added = true;\\n");
if (mutation === "remove") rmSync("spikes/spike-v5-sdk/sdk/cache.ts");
if (mutation === "example") writeFileSync("prototype/example/app.aip", "changed example\\n");
if (mutation === "binary") writeFileSync("target/debug/aip", "replacement executable\\n");
`);
  }
  return root;
}

function runPackage(root) {
  return spawnSync(process.execPath, [join(root, "product/tools/package.mjs"), "--out", join(root, "bundle")], {
    cwd: root, encoding: "utf8", timeout: 45_000, maxBuffer: 1024 * 1024,
  });
}

function assertManifest(root, file, base) {
  const lines = readFileSync(join(root, "bundle", file), "utf8").trim().split("\n");
  assert.ok(lines.length > 0);
  for (const line of lines) {
    const [digest, name] = line.split("  ");
    assert.match(digest, /^[a-f0-9]{64}$/u);
    assert.equal(createHash("sha256").update(readFileSync(join(base, name))).digest("hex"), digest, name);
  }
}

test("unchanged inputs produce a real SDK tarball and verifiable bundle manifests", (t) => {
  const root = fixture(t, "none", true);
  const result = runPackage(root);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).files, 8);
  assertManifest(root, "SOURCE_SHA256SUMS", root);
  assertManifest(root, "SHA256SUMS", join(root, "bundle"));
  const tar = spawnSync("tar", ["-tzf", join(root, "bundle/sdk/aip-sdk-0.1.0.tgz")], { encoding: "utf8" });
  assert.equal(tar.status, 0, tar.stderr);
  assert.ok(tar.stdout.includes("package/product/sdk/index.js"));
  assert.ok(tar.stdout.includes("package/product/sdk/index.d.ts"));
});

for (const [mutation, culprit] of [
  ["edit", "product/sdk/index.ts"],
  ["add", "spikes/spike-v5-sdk/sdk/added.ts"],
  ["remove", "spikes/spike-v5-sdk/sdk/cache.ts"],
  ["example", "prototype/example/app.aip"],
  ["binary", "target/debug/aip"],
]) {
  test(`packaging rejects ${mutation} during SDK build and removes its incomplete output`, (t) => {
    const root = fixture(t, mutation);
    const result = runPackage(root);
    assert.equal(result.error, undefined);
    assert.equal(result.signal, null);
    assert.equal(result.status, 1, result.stdout);
    const failure = JSON.parse(result.stderr);
    assert.equal(failure.code, "SOURCE_CHANGED");
    assert.equal(failure.stage, "source-manifest");
    assert.ok(failure.message.includes(culprit), failure.message);
    assert.equal(existsSync(join(root, "bundle")), false);
    assert.equal(existsSync(join(root, "Cargo.toml")), true);
  });
}

for (const missing of ["Cargo.lock", "prototype/src/schema_catalog.sql", "crates/aip-cli/Cargo.toml"]) {
  test(`missing source input ${missing} fails before starting the SDK build`, (t) => {
    const root = fixture(t, "edit");
    const original = readFileSync(join(root, "product/sdk/index.ts"), "utf8");
    rmSync(join(root, missing));
    const result = runPackage(root);
    assert.equal(result.status, 1, result.stdout);
    const failure = JSON.parse(result.stderr);
    assert.equal(failure.code, "SOURCE_MISSING");
    assert.equal(failure.stage, "source-manifest");
    assert.ok(failure.message.includes(missing), failure.message);
    assert.equal(readFileSync(join(root, "product/sdk/index.ts"), "utf8"), original);
    assert.equal(existsSync(join(root, "bundle")), false);
  });
}
