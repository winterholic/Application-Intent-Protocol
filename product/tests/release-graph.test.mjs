import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function tree(edges, env = {}) {
  const result = spawnSync("cargo", ["tree", "-p", "aip-cli", "--locked", "-e", edges, "-i", "aip-test-support"], {
    cwd: repo, encoding: "utf8", timeout: 120_000, env: { ...process.env, ...env },
  });
  assert.equal(result.error, undefined);
  return result;
}

// aip-test-support carries the test-database guard and a build.rs that rejects PG* variables. A product (release) build
// must not link it: a developer shell with PGHOST then still builds `aip`.
test("aip-test-support is not in the release dependency graph of aip-cli", () => {
  const release = tree("normal,build");
  assert.equal(release.status, 0, release.stderr);
  assert.equal(release.stdout.trim(), "", `aip-test-support leaked into the release graph:\n${release.stdout}`);
});

test("the test dependency graph still contains aip-test-support, so test builds keep the guard", () => {
  const withDev = tree("normal,build,dev");
  assert.equal(withDev.status, 0, withDev.stderr);
  assert.match(withDev.stdout, /^aip-test-support v/u);
});
