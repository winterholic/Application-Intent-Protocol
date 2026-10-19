import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = fileURLToPath(new URL("../../", import.meta.url));
const packager = join(root, "product/tools/package.mjs");
function execute(program, args, cwd, timeout = 90_000) {
  const result = spawnSync(program, args, { cwd, encoding: "utf8", timeout, maxBuffer: 8 * 1024 * 1024 });
  assert.equal(result.error, undefined, result.error?.message);
  assert.equal(result.signal, null, result.stderr);
  assert.equal(result.status, 0, `${program} ${args.join(" ")}\n${result.stderr}\n${result.stdout}`);
  return result.stdout;
}

test("release CLI packages two apps; moved packages check, regenerate and install SDK without original inputs", { timeout: 1_800_000 }, () => {
  const base = mkdtempSync(join(tmpdir(), "aip-app-package-e2e-"));
  try {
    for (const [name, language] of [["event-app", "node"], ["review-app", "python"]]) {
      const app = join(base, `${name}-input`);
      const out = join(base, `${name}-bundle`);
      mkdirSync(join(app, "workers"), { recursive: true });
      mkdirSync(join(app, "web"));
      const original = readFileSync(join(root, "prototype/example/app.aip"), "utf8");
      writeFileSync(join(app, "service.aip"), language === "node" ? original : original.replaceAll("Event", "Review").replaceAll("Member", "Reviewer"));
      const extension = language === "node" ? "mjs" : "py";
      writeFileSync(join(app, `workers/event.${extension}`), readFileSync(join(root, `prototype/example/extensions/event.${extension}`)));
      writeFileSync(join(app, "web/index.html"), `<h1>${name}</h1>\n`);
      // Unreferenced application files must stay out of the archive.
      writeFileSync(join(app, ".env"), "DO_NOT_SHIP=fixture_only\n");
      writeFileSync(join(app, "aip.app.json"), JSON.stringify({
        name, version: "1.0.0", definition: "service.aip",
        extensions: { directory: "workers", language, entrypoints: [`event.${extension}`], enable_writes: true },
        static_assets: [{ mount: "/", path: "web" }],
      }));
      const result = JSON.parse(execute(process.execPath, [packager, "--manifest", app, "--out", out], root, 1_200_000));
      assert.equal(result.profile, "release");
      assert.equal(result.app, name);
      const moved = join(base, `${name}-moved`);
      renameSync(out, moved);
      rmSync(app, { recursive: true });
      const installed = join(base, `${name}-installed`);
      execute(process.execPath, [join(moved, "tools/app-deploy.mjs"), "install", "--out", installed], moved);
      execute(process.execPath, [join(installed, "tools/app-deploy.mjs"), "check"], base);
      rmSync(installed, { recursive: true });
      assert.equal(existsSync(join(moved, "app/.env")), false);
      const contract = JSON.parse(readFileSync(join(moved, "APP_CONTRACT.json")));
      const binary = join(moved, "bin/aip");
      execute("shasum", ["-a", "256", "-c", "SHA256SUMS"], moved);
      execute("shasum", ["-a", "256", "-c", "APP_SHA256SUMS"], moved);
      execute(binary, ["service", "check", contract.packageLayout.definition], moved);
      execute(binary, ["service", "gen", contract.packageLayout.definition, "--wire", "decimal", "--out", "regenerated.ts", "--sdk-import", "@aip/sdk"], moved);
      assert.equal(readFileSync(join(moved, "regenerated.ts"), "utf8"), readFileSync(join(moved, "bindings.ts"), "utf8"));
      const consumer = join(moved, "consumer");
      mkdirSync(consumer);
      writeFileSync(join(consumer, "package.json"), '{"type":"module","private":true}\n');
      execute("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", "--cache", join(base, "npm-cache"), "--prefix", consumer, resolve(moved, "sdk/aip-sdk-0.1.0.tgz")], moved);
      writeFileSync(join(consumer, "binding.ts"), readFileSync(join(moved, "bindings.ts")));
      execute(join(root, "spikes/spike-0-ts/node_modules/.bin/tsc"), ["--noEmit", "--strict", "--module", "NodeNext", "--moduleResolution", "NodeNext", "--target", "ES2022", join(consumer, "binding.ts")], consumer);
      execute(process.execPath, ["--input-type=module", "-e", 'await import("@aip/sdk");'], consumer);
      console.log(`${name}: release, relocated checksums, service check/gen, installed SDK and strict bindings PASS`);
    }
  } finally {
    rmSync(base, { recursive: true, force: true });
  }
});
