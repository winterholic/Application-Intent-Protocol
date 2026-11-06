import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const checker = resolve(repo, "scripts/check-postgres-toolchain.sh");
const serverVersionNum = "170011";
const clients = {
  psql: "psql (PostgreSQL) 17.11 (Ubuntu 17.11-1.pgdg24.04+2)",
  pg_dump: "pg_dump (PostgreSQL) 17.11 (Ubuntu 17.11-1.pgdg24.04+2)",
  pg_restore: "pg_restore (PostgreSQL) 17.11 (Ubuntu 17.11-1.pgdg24.04+2)",
};

function check(versions) {
  return spawnSync("bash", [checker, serverVersionNum, versions.psql, versions.pg_dump, versions.pg_restore], {
    cwd: repo,
    encoding: "utf8",
  });
}

test("release gate rejects any PostgreSQL client major that differs from the server", () => {
  for (const name of Object.keys(clients)) {
    const mismatched = { ...clients, [name]: clients[name].replace("17.11", "16.15").replace("17.11-", "16.15-") };
    const result = check(mismatched);
    assert.notEqual(result.status, 0, name + " mismatch should fail");
    assert.match(result.stderr, new RegExp(name + " client major 16 does not match PostgreSQL server major 17"));
  }
});

test("release gate accepts PostgreSQL 17 server and all three PostgreSQL 17 clients", () => {
  const result = check(clients);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /PostgreSQL server major 17 matches psql, pg_dump and pg_restore major 17/);
});

const releaseGate = resolve(repo, "scripts/release-gate.sh");
const workflow = resolve(repo, ".github/workflows/aip-release-gate.yml");
const stubRoot = resolve(repo, ".aip-harness/tmp/postgres17-cli-compat-20261107");

function runReleaseGate({ override = true, missingTool, mismatchTool } = {}) {
  mkdirSync(stubRoot, { recursive: true });
  const testRoot = mkdtempSync(join(stubRoot, "case-"));
  const bin = join(testRoot, "custom-client-bin");
  mkdirSync(bin);
  const versions = {
    psql: mismatchTool === "psql" ? "16.15" : "17.11",
    pg_dump: mismatchTool === "pg_dump" ? "16.15" : "17.11",
    pg_restore: mismatchTool === "pg_restore" ? "16.15" : "17.11",
  };
  for (const [name, version] of Object.entries(versions)) {
    if (name === missingTool) continue;
    const versionOutput = `${name} (PostgreSQL) ${version} (stub)`;
    writeFileSync(join(bin, name), `#!/bin/sh\nif [ "$1" = "-XAt" ]; then echo 170011; else echo '${versionOutput}'; fi\n`, { mode: 0o755 });
  }
  writeFileSync(join(bin, "git"), "#!/bin/sh\necho stub-stop-after-toolchain >&2\nexit 77\n", { mode: 0o755 });
  const env = {
    ...process.env,
    PATH: `${bin}:${process.env.PATH}`,
    AIP_TEST_DATABASE_URL: "host=127.0.0.1 port=55432 dbname=aip_test_stub user=stub",
    AIP_TEST_ADMIN_URL: "host=127.0.0.1 port=55432 dbname=postgres user=stub",
  };
  if (override) env.AIP_PG_BIN_DIR = bin;
  else delete env.AIP_PG_BIN_DIR;
  try {
    return spawnSync("bash", [releaseGate, "--baseline", "HEAD"], { cwd: repo, env, encoding: "utf8" });
  } finally {
    rmSync(testRoot, { recursive: true, force: true });
  }
}

test("release gate accepts an absolute custom PostgreSQL bin directory without a versioned suffix", () => {
  const result = runReleaseGate();
  assert.equal(result.status, 77, result.stderr || result.stdout);
  assert.match(result.stdout, /matches psql, pg_dump and pg_restore major 17/);
  assert.match(result.stderr, /stub-stop-after-toolchain/);
});

test("release gate rejects a missing tool in a custom PostgreSQL bin directory", () => {
  const result = runReleaseGate({ missingTool: "pg_restore" });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /missing PostgreSQL client tool: .*\/pg_restore/);
});

test("release gate rejects a custom PostgreSQL bin directory with a mismatched client major", () => {
  const result = runReleaseGate({ mismatchTool: "pg_dump" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /pg_dump client major 16 does not match PostgreSQL server major 17/);
});

test("release gate keeps PATH resolution when AIP_PG_BIN_DIR is unset", () => {
  const result = runReleaseGate({ override: false });
  assert.equal(result.status, 77, result.stderr || result.stdout);
  assert.match(result.stdout, /matches psql, pg_dump and pg_restore major 17/);
  assert.match(result.stderr, /stub-stop-after-toolchain/);
});

test("release workflow invokes PGDG setup noninteractively and keeps PostgreSQL 17 bin override", async () => {
  const text = readFileSync(workflow, "utf8");
  assert.match(text, /apt\.postgresql\.org\.sh\s+-y/);
  assert.match(text, /AIP_PG_BIN_DIR:\s*\/usr\/lib\/postgresql\/17\/bin/);
});
