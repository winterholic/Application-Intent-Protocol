import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { wheelFileName } from "../tools/version.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const script = join(repo, "scripts/build-python.sh");
const python = process.env.PYTHON ?? "python3";

const run = (args, env = {}) => spawnSync(script, args, { cwd: repo, encoding: "utf8", env: { ...process.env, ...env } });
const scratch = (t) => {
  const dir = mkdtempSync(join(tmpdir(), "aip-python-build-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
};

test("argument errors fail before building anything", (t) => {
  const dir = scratch(t);
  assert.equal(run([]).status, 2);
  assert.equal(run(["--out", join(dir, "o"), "--wheelhouse", dir, "--no-isolation"]).status, 2);
  assert.match(run(["--out", dir]).stderr, /already exists/u);
  assert.match(run(["--out", join(dir, "o"), "--wheelhouse", join(dir, "missing")]).stderr, /not a directory/u);
  assert.equal(existsSync(join(dir, "o")), false);
});

const hasBuildTools = spawnSync(python, ["-c", "import build, setuptools, sys; sys.exit(0 if int(setuptools.__version__.split('.')[0]) >= 77 else 1)"]).status === 0;

// Needs `build` and setuptools>=77 already installed in $PYTHON (the offline requirement, see product/INSTALL.md); without
// them the test is skipped and says so rather than passing.
test("--no-isolation builds the wheel and sdist without network and leaves the source tree clean", { skip: hasBuildTools ? false : "python with build and setuptools>=77 is not available" }, (t) => {
  const dir = scratch(t);
  const before = readdirSync(join(repo, "product/python")).sort();
  const result = run(["--out", join(dir, "dist"), "--no-isolation"], { PYTHON: python, HTTP_PROXY: "http://127.0.0.1:9", HTTPS_PROXY: "http://127.0.0.1:9", PIP_NO_INDEX: "1" });
  assert.equal(result.status, 0, result.stderr + result.stdout);
  assert.ok(readdirSync(join(dir, "dist")).includes(wheelFileName()));
  assert.deepEqual(readdirSync(join(repo, "product/python")).sort(), before, "the build must not add anything to product/python");
});
