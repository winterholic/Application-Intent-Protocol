import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { npmTarballName, releaseConfig, wheelFileName, workspaceVersion } from "../tools/version.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const script = join(repo, "scripts/make-release-manifest.mjs");
const version = workspaceVersion();
const targets = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-apple-darwin"];

function dist(t, omit = []) {
  const dir = mkdtempSync(join(tmpdir(), "aip-release-manifest-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const names = [
    ...targets.map((target) => `aip-${version}-${target}.tar.gz`), npmTarballName(releaseConfig().npmPackage, version),
    wheelFileName(), `${releaseConfig().pypiDistribution.replace(/-/gu, "_")}-${version}.tar.gz`,
  ];
  for (const name of names.filter((item) => !omit.includes(item))) writeFileSync(join(dir, name), `content of ${name}\n`);
  return { dir, names };
}

const run = (dir, ...extra) => spawnSync(process.execPath, [script, dir, ...extra], { encoding: "utf8" });

test("manifest and SHA256SUMS cover every artifact and verify with shasum", (t) => {
  const { dir, names } = dist(t);
  const result = run(dir, "--tag", `v${version}`, "--git-sha", "abc123");
  assert.equal(result.status, 0, result.stderr);
  const manifest = JSON.parse(readFileSync(join(dir, `aip-${version}-manifest.json`), "utf8"));
  assert.equal(manifest.version, version);
  assert.equal(manifest.tag, `v${version}`);
  assert.equal(manifest.git_sha, "abc123");
  assert.match(manifest.protocol, /^aip-protocol\/\d+\.\d+$/u);
  assert.match(manifest.core_ir, /^aip-core\//u);
  assert.deepEqual(manifest.files.map((file) => file.name).sort(), [...names].sort());
  assert.deepEqual(manifest.files.filter((file) => file.kind === "cli-archive").map((file) => file.target).sort(), [...targets].sort());
  const check = spawnSync("shasum", ["-a", "256", "-c", "SHA256SUMS"], { cwd: dir, encoding: "utf8" });
  assert.equal(check.status, 0, check.stdout + check.stderr);
  assert.equal(readFileSync(join(dir, "SHA256SUMS"), "utf8").trim().split("\n").length, names.length);
});

test("a tampered file fails the SHA256SUMS check", (t) => {
  const { dir, names } = dist(t);
  assert.equal(run(dir).status, 0);
  writeFileSync(join(dir, names[0]), "tampered\n");
  assert.notEqual(spawnSync("shasum", ["-a", "256", "-c", "SHA256SUMS"], { cwd: dir, encoding: "utf8" }).status, 0);
});

test("a missing platform archive or duplicate package fails before anything is written", (t) => {
  const missing = dist(t, [`aip-${version}-x86_64-apple-darwin.tar.gz`]);
  const result = run(missing.dir);
  assert.equal(result.status, 1);
  assert.match(JSON.parse(result.stderr).message, /x86_64-apple-darwin/u);
  assert.throws(() => readFileSync(join(missing.dir, "SHA256SUMS")));
  const missingWheel = dist(t, [wheelFileName()]);
  assert.match(JSON.parse(run(missingWheel.dir).stderr).message, /python-wheel/u);
});
