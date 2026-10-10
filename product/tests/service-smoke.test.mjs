import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const testDir = dirname(fileURLToPath(import.meta.url));
const productDir = resolve(testDir, "..");
const repoDir = resolve(productDir, "..");
const smokeScript = join(productDir, "tools", "smoke.mjs");
const packageScript = join(productDir, "tools", "package.mjs");

test("the installed SDK completes authenticated reads, cache, writes and operations for both Id wires", { timeout: 240_000 }, () => {
  const testRoot = mkdtempSync(join(tmpdir(), `aip-release-test-${process.pid}-`));
  const outputDir = join(testRoot, "artifact");
  try {
    const packaged = spawnSync(process.execPath, [packageScript, "--out", outputDir], {
      cwd: repoDir,
      encoding: "utf8",
      timeout: 120_000,
      maxBuffer: 4 * 1024 * 1024,
    });
    assert.equal(packaged.error, undefined, `package process error: ${packaged.error?.message}`);
    assert.equal(packaged.signal, null, `package was terminated by ${packaged.signal}`);
    assert.equal(packaged.status, 0, `package failed:\n${packaged.stdout}\n${packaged.stderr}`);
    const packageResult = JSON.parse(packaged.stdout);
    assert.equal(packageResult.ok, true);
    assert.equal(resolve(packageResult.out), outputDir);

    const binaryPath = join(outputDir, "bin", "aip");
    const sdkPath = join(outputDir, "sdk", "aip-sdk-0.1.0.tgz");
    const configPath = join(outputDir, "config.example.json");
    const definitionPath = join(outputDir, "example.aip");
    const nodeExtension = join(outputDir, "extensions", "event.mjs");
    const pythonExtension = join(outputDir, "extensions", "event.py");
    const readmePath = join(outputDir, "README.md");
    const sumsPath = join(outputDir, "SHA256SUMS");
    const sourceSumsPath = join(outputDir, "SOURCE_SHA256SUMS");
    for (const path of [binaryPath, sdkPath, configPath, definitionPath, nodeExtension, pythonExtension, readmePath, sourceSumsPath, sumsPath]) assert.ok(existsSync(path), `${path} exists in the artifact`);
    assert.ok((statSync(binaryPath).mode & 0o111) !== 0, "the relocated CLI remains executable");
    const config = JSON.parse(readFileSync(configPath, "utf8"));
    assert.equal(config.database_url_env, "AIP_DATABASE_URL");
    assert.equal(Object.hasOwn(config, "database_url"), false);
    assert.equal(Object.hasOwn(config, "workers"), false, "workers stay disabled in the default config");
    assert.equal(config.auth.jwks.kind, "file");
    assert.ok(!readFileSync(configPath, "utf8").includes("private_key"));
    assert.equal(readFileSync(nodeExtension, "utf8"), readFileSync(join(repoDir, "prototype", "example", "extensions", "event.mjs"), "utf8"));
    assert.equal(readFileSync(pythonExtension, "utf8"), readFileSync(join(repoDir, "prototype", "example", "extensions", "event.py"), "utf8"));
    const readme = readFileSync(readmePath, "utf8");
    assert.match(readme, /local build|로컬 빌드/iu);
    assert.match(readme, /MacNetDeny/u);
    assert.match(readme, /language.*node/u);
    assert.match(readme, /language.*python/u);
    assert.match(readme, /enable_writes/u);

    const entries = readFileSync(sumsPath, "utf8").trim().split("\n");
    assert.equal(entries.length, 8);
    for (const entry of entries) {
      const match = /^([a-f0-9]{64})  (.+)$/u.exec(entry);
      assert.ok(match, `valid SHA256 line: ${entry}`);
      const actual = createHash("sha256").update(readFileSync(join(outputDir, match[2]))).digest("hex");
      assert.equal(actual, match[1], `${match[2]} checksum matches`);
    }

    const sourceEntries = readFileSync(sourceSumsPath, "utf8").trim().split("\n");
    assert.ok(sourceEntries.length > 20, "source manifest covers the CLI and shared SDK implementation inputs");
    for (const entry of sourceEntries) {
      const match = /^([a-f0-9]{64})  (.+)$/u.exec(entry);
      assert.ok(match, `valid source SHA256 line: ${entry}`);
      const actual = createHash("sha256").update(readFileSync(join(repoDir, match[2]))).digest("hex");
      assert.equal(actual, match[1], `${match[2]} source checksum matches the current workspace`);
    }

    const occupied = join(testRoot, "occupied");
    mkdirSync(occupied);
    const directoryMarker = join(occupied, "marker.txt");
    writeFileSync(directoryMarker, "keep directory\n");
    const fileMarker = join(testRoot, "marker-file.txt");
    writeFileSync(fileMarker, "keep file\n");
    const linkedOutput = join(testRoot, "linked-output");
    symlinkSync(fileMarker, linkedOutput);
    for (const path of [occupied, fileMarker, linkedOutput]) {
      const rejected = spawnSync(process.execPath, [packageScript, "--out", path], { cwd: repoDir, encoding: "utf8" });
      assert.notEqual(rejected.status, 0);
      assert.equal(JSON.parse(rejected.stderr).code, "OUTPUT_EXISTS");
      assert.deepEqual(readdirSync(occupied), ["marker.txt"]);
      assert.equal(readFileSync(directoryMarker, "utf8"), "keep directory\n");
      assert.equal(readFileSync(fileMarker, "utf8"), "keep file\n");
      assert.equal(readFileSync(linkedOutput, "utf8"), "keep file\n");
    }

    const smoke = spawnSync(process.execPath, [smokeScript, "--bin", binaryPath, "--sdk-package", sdkPath], {
      cwd: repoDir,
      encoding: "utf8",
      timeout: 100_000,
      maxBuffer: 4 * 1024 * 1024,
    });
    assert.equal(smoke.error, undefined, `smoke process error: ${smoke.error?.message}`);
    assert.equal(smoke.signal, null, `smoke was terminated by ${smoke.signal}`);
    assert.equal(smoke.status, 0, `relocated artifact smoke failed:\n${smoke.stdout}\n${smoke.stderr}`);
    const report = JSON.parse(smoke.stdout);
    assert.equal(report.ok, true);
    assert.deepEqual(report.package, { name: "@aip/sdk", version: "0.1.0", packed: true, installed: true });
    assert.deepEqual(report.wires.map((item) => item.wire), ["decimal", "safe"]);
    for (const item of report.wires) {
      assert.deepEqual(item.results, {
        firstReadRows: 1,
        cacheHits: 1,
        writesApplied: 1,
        postWriteCacheMisses: 1,
        replayedWrites: 1,
        revokedRequestsDenied: 1,
      });
    }
    assert.ok(!JSON.stringify(report).includes("eyJ"), "the report must not contain the generated JWT");

    for (const language of ["node", "python"]) {
      const extensionSmoke = spawnSync(process.execPath, [
        smokeScript, "--bin", binaryPath, "--sdk-package", sdkPath,
        "--workers", language, "--extension-dir", join(outputDir, "extensions"),
      ], {
        cwd: repoDir,
        encoding: "utf8",
        timeout: 100_000,
        maxBuffer: 4 * 1024 * 1024,
      });
      assert.equal(extensionSmoke.error, undefined, `${language} worker smoke process error: ${extensionSmoke.error?.message}`);
      assert.equal(extensionSmoke.signal, null, `${language} worker smoke was terminated by ${extensionSmoke.signal}`);
      assert.equal(extensionSmoke.status, 0, `${language} worker smoke failed:\n${extensionSmoke.stdout}\n${extensionSmoke.stderr}`);
      const extensionReport = JSON.parse(extensionSmoke.stdout);
      assert.equal(extensionReport.ok, true);
      assert.equal(extensionReport.worker, language);
      assert.deepEqual(extensionReport.wires.map((item) => item.wire), ["decimal", "safe"]);
      for (const item of extensionReport.wires) {
        assert.deepEqual(item.results, {
          firstReadRows: 1,
          cacheHits: 1,
          writesApplied: 1,
          postWriteCacheMisses: 1,
          replayedWrites: 1,
          revokedRequestsDenied: 1,
        });
        assert.deepEqual(item.extension, { writesApplied: 1, unchangedCount: 0, replayedWrites: 1 });
        assert.deepEqual(item.operation, { length: 3, invalidInputRejected: 1 });
      }
      assert.ok(!JSON.stringify(extensionReport).includes("eyJ"), "worker smoke report must not contain the generated JWT");
    }
  } finally {
    rmSync(testRoot, { recursive: true, force: true });
  }
});
