import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, test } from "node:test";

const testDir = dirname(fileURLToPath(import.meta.url));
const productDir = resolve(testDir, "..");
const repoDir = resolve(productDir, "..");
const buildScript = join(productDir, "tools", "build-sdk.mjs");
const tsc = join(repoDir, "spikes", "spike-0-ts", "node_modules", ".bin", "tsc");
const suiteRoot = mkdtempSync(join(tmpdir(), `aip-product-sdk-${process.pid}-`));
const packageDir = join(suiteRoot, "dist", "sdk");
const packDir = join(suiteRoot, "pack");
let tarballPath;
let packedFiles;

function run(program, args, options = {}) {
  return spawnSync(program, args, {
    cwd: options.cwd ?? repoDir,
    encoding: "utf8",
    timeout: options.timeout ?? 20_000,
    maxBuffer: 4 * 1024 * 1024,
    ...options,
  });
}

function assertCommandOk(result, label) {
  assert.equal(result.error, undefined, `${label} process error: ${result.error?.message}`);
  assert.equal(result.signal, null, `${label} was terminated by ${result.signal}`);
  assert.equal(result.status, 0, `${label} failed:\n${result.stdout}\n${result.stderr}`);
}

function walkFiles(directory, prefix = "") {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const relativePath = join(prefix, entry.name);
    const absolutePath = join(directory, entry.name);
    return entry.isDirectory() ? walkFiles(absolutePath, relativePath) : [relativePath];
  });
}

before(() => {
  mkdirSync(packDir, { recursive: true });
  const built = run(process.execPath, [buildScript, "--out", packageDir]);
  assertCommandOk(built, "SDK package build");

  const packed = run("npm", ["pack", "--offline", "--ignore-scripts", "--json", "--pack-destination", packDir, packageDir]);
  assertCommandOk(packed, "offline npm pack");
  const result = JSON.parse(packed.stdout);
  assert.equal(result.length, 1);
  tarballPath = join(packDir, result[0].filename);
  packedFiles = result[0].files.map((file) => file.path);
});

after(() => rmSync(suiteRoot, { recursive: true, force: true }));

test("build emits the installable @aip/sdk 0.1.0 JS and declaration closure", () => {
  const manifest = JSON.parse(readFileSync(join(packageDir, "package.json"), "utf8"));
  assert.equal(manifest.name, "@aip/sdk");
  assert.equal(manifest.version, "0.1.0");
  assert.equal(manifest.private, true);
  assert.equal(manifest.type, "module");
  assert.deepEqual(manifest.exports["."], {
    types: "./product/sdk/index.d.ts",
    import: "./product/sdk/index.js",
  });
  assert.ok(Array.isArray(manifest.files));

  const files = walkFiles(packageDir);
  assert.ok(files.includes("product/sdk/index.js"));
  assert.ok(files.includes("product/sdk/index.d.ts"));
  assert.ok(files.includes("spikes/spike-v6-transport/client/typed.js"));
  assert.ok(files.includes("spikes/spike-v6-transport/client/transport.js"));
  assert.ok(files.includes("spikes/spike-v5-sdk/sdk/cache.js"));
  assert.ok(files.includes("spikes/spike-v5-sdk/sdk/generic.d.ts"));

  for (const file of files.filter((name) => name.endsWith(".js") || name.endsWith(".d.ts"))) {
    const contents = readFileSync(join(packageDir, file), "utf8");
    assert.ok(!contents.includes(repoDir), `${file} must not embed an absolute checkout path`);
    const specifiers = [...contents.matchAll(/(?:\bfrom\s*|\bimport\s*)["']([^"']+)["']/gu)].map((match) => match[1]);
    for (const specifier of specifiers) {
      assert.ok(!isAbsolute(specifier), `${file} must not import an absolute path`);
      if (!specifier.startsWith(".")) continue;
      const resolved = resolve(dirname(join(packageDir, file)), specifier);
      const insidePackage = relative(packageDir, resolved);
      assert.ok(insidePackage === "" || (!insidePackage.startsWith(`..${sep}`) && insidePackage !== ".."), `${file} import escapes the package: ${specifier}`);
      let dependency = resolved;
      if (file.endsWith(".js")) assert.ok(specifier.endsWith(".js"), `${file} runtime import must use compiled JavaScript: ${specifier}`);
      else if (specifier.endsWith(".ts")) dependency = resolved.replace(/\.ts$/u, ".d.ts");
      else if (specifier.endsWith(".js") && !existsSync(resolved)) dependency = resolved.replace(/\.js$/u, ".d.ts");
      assert.ok(existsSync(dependency), `${file} relative dependency is packaged: ${specifier}`);
    }
  }

  assert.ok(packedFiles.includes("product/sdk/index.js"));
  assert.ok(packedFiles.includes("product/sdk/index.d.ts"));
  assert.ok(readFileSync(tarballPath).byteLength > 0);
});

test("an offline-installed consumer forwards the JWT and reuses the shared read cache", () => {
  const consumerDir = join(suiteRoot, "consumer");
  mkdirSync(consumerDir);
  writeFileSync(join(consumerDir, "package.json"), JSON.stringify({ type: "module" }));
  assertCommandOk(run("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", tarballPath], { cwd: consumerDir }), "offline consumer npm install");

  const runtimePath = join(consumerDir, "runtime.mjs");
  writeFileSync(runtimePath, `
import assert from "node:assert/strict";
import { connect, WriteUnsettled } from "@aip/sdk";

assert.equal(typeof WriteUnsettled, "function");
const fingerprint = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
const binding = {
  fingerprint,
  idWire: "decimal-string-v13",
  readDescriptors: { Event: { root: true, maxRows: 10, fields: { id: { type: "Id", nullable: false }, title: { type: "Text", nullable: false } }, traverse: {} } },
};
const calls = [];
const fetchMock = async (url, init) => {
  const path = new URL(url).pathname;
  calls.push({ path, authorization: init.headers.authorization });
  if (path === "/session") return new Response(JSON.stringify({ ok: true, principal: { actorId: "17" }, remainingMs: 60_000 }), { status: 200 });
  assert.equal(path, "/read");
  assert.equal(init.headers["x-aip-contract"], fingerprint);
  assert.deepEqual(JSON.parse(init.body), { query: { read: "Event", select: ["id", "title"] } });
  return new Response(JSON.stringify({ ok: true, rows: [{ id: "1", title: "cached" }], deps: ["Event"], maxAgeMs: 5_000, contractFingerprint: fingerprint }), { status: 200 });
};
const client = connect("http://aip.test", "production-access-token", binding, fetchMock);
const query = { read: "Event", select: ["id", "title"] };
const first = await client.read(query);
assert.equal(first.cached, false);
assert.deepEqual(first.rows, [{ id: "1", title: "cached" }]);
const second = await client.read(query);
assert.equal(second.cached, true);
assert.deepEqual(calls, [
  { path: "/session", authorization: "Bearer production-access-token" },
  { path: "/read", authorization: "Bearer production-access-token" },
]);
`);
  assertCommandOk(run(process.execPath, [runtimePath], { cwd: consumerDir }), "installed SDK runtime consumer");
});

test("the installed declarations preserve strict typed reads and reject hidden fields", () => {
  const consumerDir = join(suiteRoot, "type-consumer");
  mkdirSync(consumerDir);
  writeFileSync(join(consumerDir, "package.json"), JSON.stringify({ type: "module" }));
  assertCommandOk(run("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", tarballPath], { cwd: consumerDir }), "offline type consumer npm install");

  const source = `
import { connect, type ContractBinding } from "@aip/sdk";
const contract = {
  Event: { root: true, fields: { id: "" as string, title: "" as string }, traverse: {}, filterFields: {}, filter: "", sort: "", maxRows: 10 },
} as const;
const binding = {
  fingerprint: "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
  readDescriptors: { Event: { root: true, maxRows: 10, fields: { id: { type: "Id", nullable: false }, title: { type: "Text", nullable: false } }, traverse: {} } },
  idWire: "decimal-string-v13",
  __contract: undefined as unknown as typeof contract,
  __apply: {},
} as const satisfies ContractBinding<typeof contract> & { readonly idWire: "decimal-string-v13"; readonly __apply: {} };
const client = connect("https://aip.test", "jwt-access-token", binding);
async function check() {
  const result = await client.read({ read: "Event", select: ["title"] as const });
  const title: string = result.rows[0].title;
  // @ts-expect-error fields omitted from the projection are inaccessible
  const absent: string = result.rows[0].id;
  return [title, absent];
}
void check;
`;
  const consumerPath = join(consumerDir, "consumer.ts");
  writeFileSync(consumerPath, source);
  const compilerArgs = ["--noEmit", "--strict", "--module", "NodeNext", "--moduleResolution", "NodeNext", "--target", "ES2022", "--lib", "ES2022,DOM", consumerPath];
  assertCommandOk(run(tsc, compilerArgs, { cwd: consumerDir }), "strict NodeNext positive type consumer");

  const invalidSource = source.replace("  // @ts-expect-error fields omitted from the projection are inaccessible\n", "");
  assert.notEqual(invalidSource, source);
  const negativePath = join(consumerDir, "consumer-negative.ts");
  writeFileSync(negativePath, invalidSource);
  const negative = run(tsc, [...compilerArgs.slice(0, -1), negativePath], { cwd: consumerDir });
  assert.notEqual(negative.status, 0, "the hidden field access must fail without its negative-control marker");
  assert.match(`${negative.stdout}\n${negative.stderr}`, /Property 'id' does not exist/u);
});

test("invalid arguments and existing directories, files, or symlinks preserve caller data", () => {
  for (const args of [[], ["--out", ""], ["--out", join(suiteRoot, "unused"), "--unknown"]]) {
    const output = run(process.execPath, [buildScript, ...args]);
    assert.notEqual(output.status, 0);
    assert.equal(JSON.parse(output.stderr).code, "BAD_ARGS");
  }
  const directory = join(suiteRoot, "existing-output");
  mkdirSync(directory);
  const directoryMarker = join(directory, "marker.txt");
  writeFileSync(directoryMarker, "preserve directory\n");
  const marker = join(suiteRoot, "owned-marker");
  writeFileSync(marker, "preserve file\n");
  const link = join(suiteRoot, "linked-output");
  symlinkSync(marker, link);
  for (const out of [directory, marker, link]) {
    const output = run(process.execPath, [buildScript, "--out", out]);
    assert.notEqual(output.status, 0);
    assert.equal(JSON.parse(output.stderr).code, "OUTPUT_EXISTS");
    assert.equal(readFileSync(directoryMarker, "utf8"), "preserve directory\n");
    assert.equal(readFileSync(marker, "utf8"), "preserve file\n");
    assert.equal(readFileSync(link, "utf8"), "preserve file\n");
  }
  assert.deepEqual(readdirSync(directory), ["marker.txt"]);
});

test("a checkout without the SDK compiler reports the bootstrap command and creates no output", () => {
  const checkout = join(suiteRoot, "fresh-checkout");
  const isolatedBuilder = join(checkout, "product", "tools", "build-sdk.mjs");
  mkdirSync(dirname(isolatedBuilder), { recursive: true });
  writeFileSync(isolatedBuilder, readFileSync(buildScript));
  const outputDir = join(checkout, "output-parent", "sdk");

  const output = run(process.execPath, [isolatedBuilder, "--out", outputDir], { cwd: checkout });

  assert.notEqual(output.status, 0);
  const diagnostic = JSON.parse(output.stderr);
  assert.equal(diagnostic.code, "SDK_TOOLCHAIN_MISSING");
  assert.match(diagnostic.msg, /npm ci --prefix spikes\/spike-0-ts/u);
  assert.equal(existsSync(dirname(outputDir)), false);
});
