import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, test } from "node:test";

const testDir = dirname(fileURLToPath(import.meta.url));
const prototypeDir = resolve(testDir, "..");
const repoDir = resolve(prototypeDir, "..");
const buildScript = join(prototypeDir, "tools", "build-sdk.mjs");
const cli = join(prototypeDir, "target", "debug", "aip-prototype");
const definition = join(prototypeDir, "example", "app.aip");
const tsc = join(repoDir, "spikes", "spike-0-ts", "node_modules", ".bin", "tsc");

const suiteRoot = mkdtempSync(join(tmpdir(), `aip-sdk-package-${process.pid}-`));
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
    const relative = join(prefix, entry.name);
    const absolute = join(directory, entry.name);
    return entry.isDirectory() ? walkFiles(absolute, relative) : [relative];
  });
}

function generatedContract(consumerDir) {
  const output = run(cli, [
    "gen", definition,
    "--wire", "decimal",
    "--out", join(consumerDir, "contract.ts"),
    "--sdk-import", "@aip/prototype-sdk",
  ]);
  assertCommandOk(output, "prototype gen");
  const result = JSON.parse(output.stdout);
  assert.equal(result.ok, true);
  assert.match(result.fingerprint, /^[a-f0-9]{64}$/);
  return result.fingerprint;
}

before(() => {
  mkdirSync(packDir, { recursive: true });
  const built = run(process.execPath, [buildScript, "--out", packageDir]);
  assertCommandOk(built, "SDK package build");

  const packed = run("npm", [
    "pack", "--offline", "--ignore-scripts", "--json",
    "--pack-destination", packDir,
    packageDir,
  ]);
  assertCommandOk(packed, "offline npm pack");
  const result = JSON.parse(packed.stdout);
  assert.equal(result.length, 1);
  tarballPath = join(packDir, result[0].filename);
  packedFiles = result[0].files.map((file) => file.path);
});

after(() => {
  rmSync(suiteRoot, { recursive: true, force: true });
});

test("build emits a private self-contained JS and declaration package that packs offline", () => {
  const manifest = JSON.parse(readFileSync(join(packageDir, "package.json"), "utf8"));
  assert.equal(manifest.name, "@aip/prototype-sdk");
  assert.equal(manifest.version, "0.0.0");
  assert.equal(manifest.private, true);
  assert.equal(manifest.type, "module");
  assert.deepEqual(manifest.exports["."], {
    types: "./prototype/sdk/index.d.ts",
    import: "./prototype/sdk/index.js",
  });
  assert.ok(Array.isArray(manifest.files), "the package explicitly lists its shipped files");

  const files = walkFiles(packageDir);
  assert.ok(files.includes("prototype/sdk/index.js"));
  assert.ok(files.includes("prototype/sdk/index.d.ts"));
  const javascriptFiles = files.filter((file) => file.endsWith(".js"));
  const declarationFiles = files.filter((file) => file.endsWith(".d.ts"));
  assert.equal(javascriptFiles.length, 5, "the runtime closure contains the five compiled SDK modules");
  assert.equal(declarationFiles.length, 5, "the type closure contains the five SDK declarations");

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
      if (file.endsWith(".js")) {
        assert.ok(specifier.endsWith(".js"), `${file} runtime imports use compiled JavaScript: ${specifier}`);
      } else if (specifier.endsWith(".ts")) {
        dependency = resolved.replace(/\.ts$/u, ".d.ts");
      } else if (specifier.endsWith(".js") && !existsSync(resolved)) {
        dependency = resolved.replace(/\.js$/u, ".d.ts");
      }
      assert.ok(existsSync(dependency), `${file} relative dependency is packaged: ${specifier}`);
    }
  }

  assert.equal(packedFiles.filter((file) => file.endsWith(".js")).length, 5);
  assert.equal(packedFiles.filter((file) => file.endsWith(".d.ts")).length, 5);
  assert.ok(packedFiles.some((file) => file.endsWith("prototype/sdk/index.js")));
  assert.ok(packedFiles.some((file) => file.endsWith("prototype/sdk/index.d.ts")));
  assert.ok(readFileSync(tarballPath).byteLength > 0, "npm pack writes the package tarball");
});

test("build refuses to overwrite an existing directory and preserves its marker", () => {
  const existingDir = join(suiteRoot, "existing-output");
  mkdirSync(existingDir);
  const marker = join(existingDir, "marker.txt");
  writeFileSync(marker, "keep this file\n");

  const output = run(process.execPath, [buildScript, "--out", existingDir]);
  assert.notEqual(output.status, 0, "an existing output directory is not replaced");
  assert.equal(readFileSync(marker, "utf8"), "keep this file\n");
  assert.deepEqual(readdirSync(existingDir), ["marker.txt"]);
});

test("an installed consumer imports the SDK, reads through its cache, and typechecks generated contracts", () => {
  const consumerDir = join(suiteRoot, "consumer");
  mkdirSync(consumerDir);
  writeFileSync(join(consumerDir, "package.json"), JSON.stringify({ type: "module" }));
  const installed = run("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", tarballPath], { cwd: consumerDir });
  assertCommandOk(installed, "offline consumer npm install");

  const contractFingerprint = generatedContract(consumerDir);
  const runtime = `
import assert from "node:assert/strict";
import { connect, WriteUnsettled } from "@aip/prototype-sdk";
import { contract } from "./contract.ts";

assert.equal(typeof WriteUnsettled, "function");
const calls = [];
const fingerprint = ${JSON.stringify(contractFingerprint)};
const fetchMock = async (url, init) => {
  const path = new URL(url).pathname;
  calls.push(path);
  if (path === "/session") {
    return new Response(JSON.stringify({ ok: true, principal: { actorId: "1" }, remainingMs: 60_000 }), { status: 200 });
  }
  assert.equal(path, "/read");
  assert.equal(init.headers["x-aip-contract"], fingerprint);
  const request = JSON.parse(init.body);
  assert.equal(request.query.read, "Event");
  return new Response(JSON.stringify({
    ok: true,
    rows: [{ id: "1", title: "hello", phase: "READY", link: "https://example.test" }],
    deps: ["Event"],
    maxAgeMs: 5_000,
    contractFingerprint: fingerprint,
  }), { status: 200 });
};
const client = connect("http://aip.test", "test-token", contract, fetchMock);
const query = { read: "Event", select: ["id", "title", "phase", "link"] };
const first = await client.read(query);
assert.equal(first.cached, false);
assert.deepEqual(first.rows, [{ id: "1", title: "hello", phase: "READY", link: "https://example.test" }]);
const second = await client.read(query);
assert.equal(second.cached, true);
assert.deepEqual(calls, ["/session", "/read"]);
`;
  const runtimePath = join(consumerDir, "runtime.mjs");
  writeFileSync(runtimePath, runtime);
  assertCommandOk(run(process.execPath, [runtimePath], { cwd: consumerDir }), "installed SDK runtime consumer");

  const positive = `
import { connect, type ApplyRequest } from "@aip/prototype-sdk";
import { contract } from "./contract.js";
type Actions = NonNullable<typeof contract.__apply>;
const request: ApplyRequest<Actions> = { apply: "Event.mark", target: { where: [{ field: "phase", op: "eq", value: "READY" }] } };
const client = connect("http://localhost", "token", contract);
async function consume() {
  const result = await client.read({ read: "Event", select: ["id", "title", "phase", "link"] as const });
  const title: string = result.rows[0].title;
  const phase: "READY" | "DONE" = result.rows[0].phase;
  const link: string = result.rows[0].link;
  // @ts-expect-error read rows are deeply readonly
  result.rows[0].title = "changed";
  // @ts-expect-error declared Enum values are closed
  const invalid: ApplyRequest<Actions> = { apply: "Event.mark", target: { where: [{ field: "phase", op: "eq", value: "UNKNOWN" }] } };
  // @ts-expect-error a field outside the read projection is absent
  const hidden: string = result.rows[0].internalNote;
  await client.apply(request);
  return [title, phase, link, invalid, hidden];
}
void consume;
`;
  const positivePath = join(consumerDir, "consumer.ts");
  writeFileSync(positivePath, positive);
  const tscArgs = ["--noEmit", "--strict", "--module", "NodeNext", "--moduleResolution", "NodeNext", "--target", "ES2022", "--lib", "ES2022,DOM", positivePath, join(consumerDir, "contract.ts")];
  assertCommandOk(run(tsc, tscArgs, { cwd: consumerDir, timeout: 20_000 }), "strict NodeNext positive type consumer");

  const withoutMarker = positive.replace("  // @ts-expect-error declared Enum values are closed\n", "");
  assert.notEqual(withoutMarker, positive, "negative-control marker must be present in the source");
  const negativePath = join(consumerDir, "consumer-negative.ts");
  writeFileSync(negativePath, withoutMarker);
  const negative = run(tsc, [...tscArgs.slice(0, -2), negativePath, join(consumerDir, "contract.ts")], { cwd: consumerDir, timeout: 20_000 });
  assert.notEqual(negative.status, 0, "removing the Enum negative marker makes the consumer fail typechecking");
  assert.match(`${negative.stdout}\n${negative.stderr}`, /UNKNOWN/u);
});

test("invalid build options and existing files or symlinks never delete caller files", () => {
  for (const args of [[], ["--out", ""], ["--out", join(suiteRoot,"unused"), "--unknown"]]) {
    const output=run(process.execPath,[buildScript,...args]);
    assert.notEqual(output.status,0);
    assert.equal(JSON.parse(output.stderr).code,"BAD_ARGS");
  }
  const marker=join(suiteRoot,"owned-marker");
  writeFileSync(marker,"preserve");
  const link=join(suiteRoot,"linked-output");
  symlinkSync(marker,link);
  for (const out of [marker,link]) {
    const output=run(process.execPath,[buildScript,"--out",out]);
    assert.notEqual(output.status,0);
    assert.equal(JSON.parse(output.stderr).code,"OUTPUT_EXISTS");
    assert.equal(readFileSync(marker,"utf8"),"preserve");
    assert.equal(readFileSync(link,"utf8"),"preserve");
  }
});
