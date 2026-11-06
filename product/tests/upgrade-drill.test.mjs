import assert from "node:assert/strict";
import {test} from "node:test";
import {finalizeDrill, rollbackBinaryOutcome} from "../tools/upgrade-drill.mjs";

test("rollback accepts only the schema mismatch refusal while preserving a successful old-binary path", async () => {
  await assert.rejects(
    rollbackBinaryOutcome({started: false, code: "WIRE.INTERNAL"}),
    /WIRE\.SCHEMA_MISMATCH/,
  );
  assert.deepEqual(
    await rollbackBinaryOutcome({started: false, code: "WIRE.SCHEMA_MISMATCH"}),
    {started: false, failedClosedWith: "WIRE.SCHEMA_MISMATCH"},
  );
  const successful = await rollbackBinaryOutcome(
    {started: true, address: "127.0.0.1:1", child: {}},
    {call: async () => ({ok: true, rows: [{id: 11, checked: true}, {id: 12, checked: true}], replayed: true})},
  );
  assert.deepEqual(successful, {started: true, readsUpgradedData: true, replaysNewBinaryWrite: true});
});

test("cleanup failure makes a successful drill nonzero and preserves an earlier failure", async () => {
  const report = {ok: true, stages: {restore: {ok: true}}};
  await finalizeDrill(report, {
    cleanup: async () => { throw Object.assign(new Error("permission denied"), {stage: "cleanup-schema"}); },
    removeWorkDir: async () => {},
  });
  assert.equal(report.ok, false);
  assert.deepEqual(report.cleanupFailed, [{stage: "cleanup-schema", message: "permission denied"}]);
  assert.deepEqual(report.failure, report.cleanupFailed[0]);
  assert.equal(report.exitCode, 1);

  const failed = {ok: false, failure: {stage: "restore-read", message: "original failure"}};
  await finalizeDrill(failed, {
    cleanup: async () => { throw new Error("secondary cleanup failure"); },
    removeWorkDir: async () => {},
  });
  assert.deepEqual(failed.failure, {stage: "restore-read", message: "original failure"});
  assert.equal(failed.cleanupFailed[0].message, "secondary cleanup failure");
  assert.equal(failed.exitCode, 1);
});
