import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

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
