import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import * as packager from "../tools/app-package.mjs";

function fixture(t, overrides = {}) {
  const base = mkdtempSync(join(tmpdir(), "aip-app-package-test-"));
  t.after(() => rmSync(base, { recursive: true, force: true }));
  const root = join(base, "engine");
  const app = join(base, "input");
  const out = join(base, "bundle");
  const put = (path, text) => { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, text); };
  put(join(root, "Cargo.toml"), '[workspace]\n');
  put(join(root, "Cargo.lock"), 'version = 4\n');
  put(join(root, "crates/aip-cli/Cargo.toml"), '[features]\ndefault = []\nmysql = []\nexternal-brokers = []\npoc-runtime = []\n');
  put(join(root, "crates/aip-cli/src/main.rs"), 'fn main() {}\n');
  put(join(root, "product/tools/app-deploy.mjs"), readFileSync(new URL("../tools/app-deploy.mjs", import.meta.url), "utf8"));
  for (const name of ["Dockerfile", "dockerignore", "compose.yaml", "aip-app.service"]) put(join(root, "product/deploy", name), readFileSync(new URL(`../deploy/${name}`, import.meta.url), "utf8"));
  put(join(root, "prototype/example/app.aip"), 'resource Embedded { fields { id: Id } }\n');
  put(join(app, "defs/service.aip"), 'resource Member { fields { id: Id } }\nactor Member\n');
  put(join(app, "workers/main.mjs"), 'export { value } from "./helper.mjs";\n');
  put(join(app, "workers/helper.mjs"), 'export const value = 1;\n');
  put(join(app, "public/index.html"), '<h1>Application</h1>\n');
  put(join(app, "package.json"), '{"name":"app-worker","version":"1.0.0"}\n');
  put(join(app, "package-lock.json"), '{"name":"app-worker","lockfileVersion":3,"packages":{}}\n');
  const manifest = {
    name: "app-one", version: "1.2.3", definition: "defs/service.aip",
    extensions: { directory: "workers", language: "node", entrypoints: ["main.mjs"] },
    static_assets: [{ mount: "/", path: "public" }],
    dependencies: { node: { package_json: "package.json", lockfile: "package-lock.json" } },
    ...overrides,
  };
  put(join(app, "aip.app.json"), JSON.stringify(manifest));
  const commands = [];
  const run = (program, args, stage) => {
    commands.push({ program, args, stage });
    if (program === "rustc") return 'rustc 1.94.0\nhost: aarch64-apple-darwin\n';
    if (program === "git") return args[0] === "rev-parse" ? 'abcdef0123456789\n' : '';
    if (program === "cargo") {
      put(join(root, "target/release/aip"), '#!/bin/sh\nexit 0\n');
      chmodSync(join(root, "target/release/aip"), 0o755);
      return '';
    }
    if (stage === "sdk-build") {
      const sdk = args[args.indexOf("--out") + 1];
      put(join(sdk, "package.json"), '{"name":"@aip/sdk","version":"0.1.0","type":"module"}');
      put(join(sdk, "index.js"), 'export const AipClient = class {};\n');
      return '';
    }
    if (stage === "sdk-pack") {
      const result = spawnSync(program, args, { cwd: root, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout;
    }
    if (stage === "bindings") { put(args[args.indexOf("--out") + 1], 'import { AipClient } from "@aip/sdk";\n'); return ''; }
    if (stage === "engine-version") return 'aip 0.1.0\n';
    if (stage === "definition-check") return '{}\n';
    throw new Error(`unexpected command ${program} ${stage}`);
  };
  return { base, root, app, out, put, manifest, commands, run };
}

async function build(f, run = f.run) {
  assert.equal(typeof packager.packageApplication, "function", "the application packaging entrypoint must exist");
  return packager.packageApplication({ manifest: f.app, out: f.out, profile: "release" }, { root: f.root, run });
}

function verifySums(base, name) {
  const lines = readFileSync(join(base, name), "utf8").trim().split("\n");
  assert.ok(lines.length > 0);
  for (const line of lines) {
    const [digest, path] = line.split("  ");
    assert.equal(createHash("sha256").update(readFileSync(join(base, path))).digest("hex"), digest, path);
  }
}

test("two distinct applications remain self-contained after removing their inputs", async (t) => {
  for (const overrides of [{}, { name: "app-two", extensions: undefined, dependencies: undefined, features: ["mysql"] }]) {
    const f = fixture(t, overrides);
    const result = await build(f);
    assert.equal(result.ok, true);
    const metadata = JSON.parse(readFileSync(join(f.out, "BUILD.json")));
    assert.equal(metadata.app.name, f.manifest.name);
    assert.equal(metadata.target, "aarch64-apple-darwin");
    assert.equal(metadata.profile, "release");
    assert.deepEqual(metadata.features, overrides.features ?? []);
    assert.match(metadata.engine_version, /0\.1\.0/);
    assert.ok(f.commands.some(c => c.program === "cargo" && c.args.includes("--locked") && c.args.includes("--release")));
    assert.ok(f.commands.some(c => c.stage === "definition-check"));
    assert.ok(f.commands.some(c => c.stage === "bindings"));
    if (f.manifest.extensions) assert.equal(readFileSync(join(f.out, "app/workers/helper.mjs"), "utf8"), 'export const value = 1;\n');
    const contract = JSON.parse(readFileSync(join(f.out, "APP_CONTRACT.json")));
    assert.equal(existsSync(join(f.out, contract.packageLayout.definition)), true);
    assert.equal(existsSync(join(f.out, contract.packageLayout.manifest)), true);
    assert.equal(readFileSync(join(f.out, "tools/app-deploy.mjs"), "utf8"), readFileSync(new URL("../tools/app-deploy.mjs", import.meta.url), "utf8"));
    assert.equal(readFileSync(join(f.out, "deploy/Dockerfile"), "utf8"), readFileSync(new URL("../deploy/Dockerfile", import.meta.url), "utf8"));
    assert.equal(existsSync(join(f.out, ".dockerignore")), true);
    const serialized = readFileSync(join(f.out, "BUILD.json"), "utf8") + readFileSync(join(f.out, "APP_CONTRACT.json"), "utf8");
    assert.ok(!serialized.includes(f.base), "metadata must not depend on the input checkout");
    rmSync(f.app, { recursive: true });
    rmSync(f.root, { recursive: true });
    verifySums(f.out, "SHA256SUMS");
    verifySums(f.out, "APP_SHA256SUMS");
    assert.equal(readFileSync(join(f.out, "app/defs/service.aip"), "utf8"), 'resource Member { fields { id: Id } }\nactor Member\n');
  }
});

for (const [title, overrides, code] of [
  ["unsupported feature", { features: ["does-not-exist"] }, "UNSUPPORTED_FEATURE"],
  ["foreign target", { targets: ["x86_64-pc-windows-msvc"] }, "TARGET_MISMATCH"],
  ["node dependency without lock", { dependencies: { node: { package_json: "package.json" } } }, "DEPENDENCY_UNLOCKED"],
]) {
  test(`rejects ${title} before building or creating output`, async (t) => {
    const f = fixture(t, overrides);
    await assert.rejects(build(f), e => e.code === code);
    assert.equal(existsSync(f.out), false);
    assert.equal(f.commands.some(c => c.program === "cargo"), false);
  });
}

for (const kind of ["directory", "file", "symlink"]) {
  test(`preserves existing ${kind} output before building`, async (t) => {
    const f = fixture(t);
    f.put(join(f.base, "marker"), "keep");
    if (kind === "directory") f.put(join(f.out, "marker"), "keep");
    if (kind === "file") f.put(f.out, "keep");
    if (kind === "symlink") symlinkSync(join(f.base, "marker"), f.out);
    await assert.rejects(build(f), e => e.code === "OUTPUT_EXISTS");
    assert.equal(readFileSync(kind === "directory" ? join(f.out, "marker") : f.out, "utf8"), "keep");
    assert.equal(f.commands.some(c => c.program === "cargo"), false);
  });
}

test("rejects output nested inside the app without creating files", async (t) => {
  const f = fixture(t);
  f.out = join(f.app, "public/bundle");
  await assert.rejects(build(f), e => e.code === "OUTPUT_IN_APP");
  assert.equal(existsSync(f.out), false);
});

for (const mutation of ["app", "engine", "binary", "embedded-definition", "deploy-template"]) {
  test(`detects concurrent ${mutation} mutation and removes incomplete output`, async (t) => {
    const f = fixture(t);
    const run = (program, args, stage) => {
      const result = f.run(program, args, stage);
      if (stage === "sdk-build") f.put(join(mutation === "app" ? f.app : f.root,
        mutation === "app" ? "workers/helper.mjs" : mutation === "engine" ? "crates/aip-cli/src/main.rs" : mutation === "embedded-definition" ? "prototype/example/app.aip" : mutation === "deploy-template" ? "product/deploy/Dockerfile" : "target/release/aip"), "changed\n");
      return result;
    };
    await assert.rejects(build(f, run), e => e.code === "SOURCE_CHANGED");
    assert.equal(existsSync(f.out), false);
  });
}

test("a failed build preserves the input and removes only its owned output", async (t) => {
  const f = fixture(t);
  await assert.rejects(build(f, (program, args, stage) => {
    if (program === "cargo") throw Object.assign(new Error("failed build"), { code: "COMMAND_FAILED", stage });
    return f.run(program, args, stage);
  }), e => e.code === "COMMAND_FAILED");
  assert.equal(existsSync(f.out), false);
  assert.equal(existsSync(join(f.app, "aip.app.json")), true);
});

test("extension helper secrets cannot enter the bundle", async (t) => {
  const f = fixture(t);
  f.put(join(f.app, "workers/.env"), "SECRET=do-not-package\n");
  await assert.rejects(build(f), e => e.code === "DISALLOWED_FILE");
  assert.equal(existsSync(f.out), false);
});

for (const [title, mutate, code] of [
  ["lockfile drift", f => f.put(join(f.app, "package.json"), '{"dependencies":{"missing":"1.0.0"}}'), "DEPENDENCY_UNLOCKED"],
  ["local dependencies", f => { f.put(join(f.app, "package.json"), '{"dependencies":{"local":"file:../local"}}'); f.put(join(f.app, "package-lock.json"), '{"lockfileVersion":3,"packages":{"":{"dependencies":{"local":"file:../local"}}}}'); }, "DEPENDENCY_LOCAL_PATH"],
  ["private key body", f => f.put(join(f.app, "workers/helper.mjs"), '-----BEGIN PRIVATE KEY-----\nfixture\n'), "CREDENTIAL_LEAK"],
  ["helper symlink escape", f => { rmSync(join(f.app, "workers/helper.mjs")); f.put(join(f.base, "outside.mjs"), 'private input'); symlinkSync(join(f.base, "outside.mjs"), join(f.app, "workers/helper.mjs")); }, "SYMLINK_ESCAPES_APP"],
]) {
  test(`rejects ${title} and cleans up`, async (t) => {
    const f = fixture(t);
    mutate(f);
    await assert.rejects(build(f), e => e.code === code);
    assert.equal(existsSync(f.out), false);
  });
}

test("unlocked Python requirements are rejected", async (t) => {
  const f = fixture(t, { dependencies: { python: { requirements: "requirements.txt" } } });
  f.put(join(f.app, "requirements.txt"), "requests>=2\n");
  await assert.rejects(build(f), e => e.code === "DEPENDENCY_UNLOCKED");
  assert.equal(f.commands.some(c => c.program === "cargo"), false);
});

test("a missing app manifest retains its structured validation error", async (t) => {
  const f = fixture(t);
  f.app = join(f.base, "missing-input");
  await assert.rejects(build(f), e => e.code === "MANIFEST_MISSING");
  assert.equal(existsSync(f.out), false);
});
