import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { npmDistTag, npmTarballName, pythonVersion, releaseConfig, wheelFileName, workspaceVersion } from "../tools/version.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

// check-versions.mjs resolves its repository from its own location, so a copy of just the files it reads is a sandbox.
function sandbox(t) {
  const root = mkdtempSync(join(tmpdir(), "aip-version-check-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const copy = (name) => {
    mkdirSync(dirname(join(root, name)), { recursive: true });
    cpSync(join(repo, name), join(root, name), { recursive: true });
  };
  for (const name of [
    "Cargo.toml", "Cargo.lock", "prototype/Cargo.lock", "LICENSE", "COMPATIBILITY.md", "scripts/check-versions.mjs", "product/tools/version.mjs",
    "product/release.config.json", "product/python/pyproject.toml", "product/python/README.md", "product/python/LICENSE", "product/python/aip_sdk/__init__.py",
    "crates/aip-contract/src/lib.rs", "crates/aip-ir/src/lib.rs",
  ]) copy(name);
  for (const entry of readdirSync(join(repo, "crates"), { withFileTypes: true })) {
    if (entry.isDirectory()) copy(`crates/${entry.name}/Cargo.toml`);
  }
  for (const entry of readdirSync(join(repo, "spikes"), { withFileTypes: true })) {
    if (entry.isDirectory() && existsSync(join(repo, "spikes", entry.name, "Cargo.lock"))) copy(`spikes/${entry.name}/Cargo.lock`);
  }
  return root;
}

function check(root, args = [], env = {}) {
  const result = spawnSync(process.execPath, [join(root, "scripts/check-versions.mjs"), ...args], {
    cwd: root, encoding: "utf8", env: { ...process.env, AIP_NPM_PACKAGE: "", AIP_PYPI_PACKAGE: "", ...env },
  });
  assert.equal(result.error, undefined);
  return { status: result.status, out: result.stdout, err: result.stderr, report: result.status === 0 ? JSON.parse(result.stdout) : JSON.parse(result.stderr) };
}

function edit(root, name, from, to) {
  const path = join(root, name);
  const text = readFileSync(path, "utf8");
  assert.ok(text.includes(from), `${name} contains ${from}`);
  writeFileSync(path, text.replace(from, to));
}

test("the checked-in tree is consistent with the single version source", () => {
  const version = workspaceVersion();
  const result = spawnSync(process.execPath, [join(repo, "scripts/check-versions.mjs"), "--tag", `v${version}`], { cwd: repo, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.equal(report.ok, true);
  assert.equal(report.version, version);
});

test("version conversions: PEP 440 for rc, npm tarball for scoped names, dist tag, wheel file name", () => {
  assert.equal(pythonVersion("1.0.0"), "1.0.0");
  assert.equal(pythonVersion("1.0.0-rc.1"), "1.0.0rc1");
  assert.throws(() => pythonVersion("1.0"), /지원하지 않는/u);
  assert.equal(npmTarballName("@aip/sdk", "1.0.0"), "aip-sdk-1.0.0.tgz");
  assert.equal(npmTarballName("plain-name", "1.0.0-rc.2"), "plain-name-1.0.0-rc.2.tgz");
  assert.equal(npmDistTag("1.0.0"), "latest");
  assert.equal(npmDistTag("1.0.0-rc.1"), "next");
  assert.equal(wheelFileName("application-intent-protocol-sdk", "1.0.0-rc.1"), "application_intent_protocol_sdk-1.0.0rc1-py3-none-any.whl");
});

test("package names come from the one config and can be overridden by environment, with validation", () => {
  assert.equal(releaseConfig({}).npmPackage, "@aip/sdk");
  assert.equal(releaseConfig({}).pypiDistribution, "application-intent-protocol-sdk");
  assert.equal(releaseConfig({}).pythonImport, "aip_sdk");
  const override = releaseConfig({ AIP_NPM_PACKAGE: "@example/aip", AIP_PYPI_PACKAGE: "example-aip" });
  assert.equal(override.npmPackage, "@example/aip");
  assert.equal(override.pypiDistribution, "example-aip");
  assert.equal(override.namesApprovedByOwner, false);
  assert.throws(() => releaseConfig({ AIP_NPM_PACKAGE: "Bad Name" }), /npm 패키지 이름/u);
  assert.throws(() => releaseConfig({ AIP_PYPI_PACKAGE: "-bad" }), /PyPI 배포 이름/u);
  assert.equal(releaseConfig({}).namesApprovedByOwner, false, "final names are pending owner approval");
});

test("an untouched sandbox copy passes", (t) => {
  const result = check(sandbox(t));
  assert.equal(result.status, 0, result.err);
  assert.equal(result.report.ok, true);
});

for (const [label, file, from, to, pattern] of [
  ["Cargo.toml version moves alone", "Cargo.toml", `\nversion = "${workspaceVersion()}"`, '\nversion = "9.9.9"', /Cargo\.lock의 .* 9\.9\.9|pyproject version/u],
  ["pyproject version drifts", "product/python/pyproject.toml", `\nversion = "${pythonVersion(workspaceVersion())}"`, '\nversion = "0.0.1"', /pyproject version 0\.0\.1/u],
  ["python __version__ drifts", "product/python/aip_sdk/__init__.py", `__version__ = "${pythonVersion(workspaceVersion())}"`, '__version__ = "0.0.1"', /__version__ 0\.0\.1/u],
  ["python PROTOCOL drifts", "product/python/aip_sdk/__init__.py", 'PROTOCOL = "aip-protocol/1.0"', 'PROTOCOL = "aip-protocol/0.1"', /PROTOCOL aip-protocol\/0\.1/u],
  ["pyproject name differs from the config", "product/python/pyproject.toml", 'name = "application-intent-protocol-sdk"', 'name = "aip-sdk"', /pyproject name aip-sdk/u],
  ["a stale Cargo.lock entry", "prototype/Cargo.lock", `name = "aip-crypto"\nversion = "${workspaceVersion()}"`, 'name = "aip-crypto"\nversion = "0.1.0"', /prototype\/Cargo\.lock의 aip-crypto은 0\.1\.0/u],
  ["the compatibility table lacks this version", "COMPATIBILITY.md", `| ${workspaceVersion()} |`, "| 0.0.0 |", /COMPATIBILITY\.md에 .* 행이 없습니다/u],
  ["python LICENSE differs from the root LICENSE", "product/python/LICENSE", "MIT License", "Not MIT", /product\/python\/LICENSE/u],
]) {
  test(`check-versions fails when ${label}`, (t) => {
    const root = sandbox(t);
    assert.equal(check(root).status, 0, "baseline sandbox is consistent");
    edit(root, file, from, to);
    const result = check(root);
    assert.equal(result.status, 1, result.out);
    assert.equal(result.report.ok, false);
    assert.match(result.report.problems.join("\n"), pattern);
  });
}

test("check-versions rejects a tag that does not match the workspace version", (t) => {
  const root = sandbox(t);
  const version = workspaceVersion();
  assert.equal(check(root, ["--tag", `v${version}`]).status, 0);
  const wrong = check(root, ["--tag", "v9.9.9"]);
  assert.equal(wrong.status, 1);
  assert.match(wrong.report.problems.join("\n"), /태그 v9\.9\.9는 workspace 버전 v/u);
  assert.equal(check(root, ["--tag", `${version}`]).status, 1, "the leading v is required");
});

test("check-versions inspects built artifacts: tarball identity, spikes paths, wheel metadata", (t) => {
  const root = sandbox(t);
  const bad = join(root, "bad");
  mkdirSync(join(bad, "package/spikes/x"), { recursive: true });
  writeFileSync(join(bad, "package/package.json"), JSON.stringify({ name: "@aip/sdk", version: "0.0.1", private: true }));
  writeFileSync(join(bad, "package/spikes/x/index.js"), "");
  const tarball = join(bad, "aip-sdk-0.0.1.tgz");
  assert.equal(spawnSync("tar", ["-czf", tarball, "-C", bad, "package"]).status, 0);
  const result = check(root, ["--npm-tarball", tarball]);
  assert.equal(result.status, 1);
  const problems = result.report.problems.join("\n");
  assert.match(problems, /tarball 버전 0\.0\.1/u);
  assert.match(problems, /private/u);
  assert.match(problems, /spikes 내부 경로/u);
  assert.match(problems, /package\/LICENSE가 없습니다/u);
});
