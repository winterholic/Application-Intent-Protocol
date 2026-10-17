import assert from "node:assert/strict";
import {test} from "node:test";
import {validateTestDsn} from "../test-support/postgres-guard.mjs";

test("accepts explicit isolated URI and keyword DSNs", () => {
  assert.equal(validateTestDsn("postgresql://runner@127.0.0.1:55432/aip_test_run").port, 55432);
  assert.equal(validateTestDsn("host='127.0.0.1' port=55432 dbname=aip_test_run user=runner").dbname, "aip_test_run");
});

test("rejects missing fields, shared endpoints and libpq service fallback", () => {
  for (const dsn of [
    "host=localhost dbname=postgres",
    "host=127.0.0.1 port=5432 dbname=aip_test_run user=runner",
    "host=127.0.0.1 port=55432 dbname=postgres user=runner",
    "postgresql://runner@localhost/aip_test_run",
    "host=127.0.0.1 port=55432 dbname=aip_test_run service=test",
  ]) assert.throws(() => validateTestDsn(dsn));
});

test("rejects flaskbb bypass vectors: hostaddr, partial DSN, service fallback", () => {
  for (const dsn of [
    "host=db.example hostaddr=127.0.0.1 port=5432 dbname=postgres user=synthetic",
    "host=db.example hostaddr=127.0.0.1 port=55432 dbname=aip_test_run user=synthetic",
    "postgresql://synthetic@db.example:55432/aip_test_run?hostaddr=127.0.0.1",
    "host=127.0.0.1",
    "service=synthetic_fixture",
    "host=127.0.0.1 port=55432 dbname=aip_test_run user=runner service=synthetic_fixture",
    "postgresql://runner@127.0.0.1:55432/aip_test_run?service=synthetic_fixture",
  ]) assert.throws(() => validateTestDsn(dsn));
});

