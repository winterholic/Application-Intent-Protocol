import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

test("smoke refuses a missing explicit packaged definition before SDK build or DB access", () => {
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("PG")));
  env.AIP_TEST_DATABASE_URL = "host=127.0.0.1 port=65432 dbname=aip_test_unused user=fixture";
  const result = spawnSync(process.execPath, [fileURLToPath(new URL("../tools/smoke.mjs", import.meta.url)), "--app-definition", "/nonexistent-aip-packaged-definition/app.aip"], { env, encoding: "utf8", timeout: 5000 });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 1);
  assert.equal(JSON.parse(result.stderr).code, "APP_MISSING");
});
