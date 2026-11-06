import assert from "node:assert/strict";
import {EventEmitter} from "node:events";
import {test} from "node:test";
import {finalizeDrill, rollbackBinaryOutcome, startService, stopOwnedService} from "../tools/upgrade-drill.mjs";

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

test("finalization stops every started drill child after a rollback request error and continues cleanup", async () => {
  const rollbackChild = {name: "rollback-a-old-service"};
  const refusedChild = {name: "unexpected-pre-apply-service"};
  const restoreChild = {name: "restore-b-old-service"};
  await assert.rejects(
    rollbackBinaryOutcome(
      {started: true, address: "127.0.0.1:1", child: rollbackChild},
      {call: async () => { throw new Error("read connection reset"); }},
    ),
    /read connection reset/,
  );

  const ownedChildren = new Set([rollbackChild, refusedChild, restoreChild]);
  const stopAttempts = [];
  const cleanupAttempts = [];
  const report = {ok: false, failure: {stage: "rollback-binary", message: "read connection reset"}};
  await finalizeDrill(report, {
    ownedChildren,
    stopService: async (child) => {
      stopAttempts.push(child);
      if (child === rollbackChild) throw new Error("SIGTERM wait failed");
    },
    cleanup: async () => cleanupAttempts.push("schema"),
    cleanupInfra: async () => cleanupAttempts.push("infra"),
    removeWorkDir: async () => cleanupAttempts.push("tempdir"),
  });

  assert.deepEqual(stopAttempts, [rollbackChild, refusedChild, restoreChild]);
  assert.deepEqual(cleanupAttempts, ["schema", "infra", "tempdir"]);
  assert.deepEqual(report.cleanupFailed, [{stage: "cleanup-service", message: "SIGTERM wait failed"}]);
  assert.deepEqual(report.failure, {stage: "rollback-binary", message: "read connection reset"});
  assert.equal(report.exitCode, 1);
});

test("started rollback child stays owned across request and stop errors until main finalization retries it", async () => {
  const ownedChildren = new Set();
  const stdout = new EventEmitter();
  const stderr = new EventEmitter();
  stdout.setEncoding = () => {};
  stderr.setEncoding = () => {};
  const child = Object.assign(new EventEmitter(), {stdout, stderr, exitCode: null, signalCode: null});
  const startPromise = startService("old-aip", "app.aip", "service.json", "rollback-binary", {
    ownedChildren,
    spawnProcess: () => child,
  });
  queueMicrotask(() => stdout.emit("data", '{"ok":true,"event":"listening","address":"127.0.0.1:44123"}\n'));
  const started = await startPromise;
  assert.equal(started.started, true);
  assert.equal(ownedChildren.has(child), true);

  const report = {ok: false, failure: {stage: "rollback-binary", message: "read connection reset"}};
  await assert.rejects(
    rollbackBinaryOutcome(started, {call: async () => { throw new Error("read connection reset"); }}),
    /read connection reset/,
  );
  let stopAttempts = 0;
  await assert.rejects(
    stopOwnedService(ownedChildren, child, async () => {
      stopAttempts++;
      throw new Error("SIGTERM wait failed");
    }),
    /SIGTERM wait failed/,
  );
  assert.equal(ownedChildren.has(child), true);

  const finalStopAttempts = [];
  const cleanupAttempts = [];
  await finalizeDrill(report, {
    ownedChildren,
    stopService: async (ownedChild) => finalStopAttempts.push(ownedChild),
    cleanup: async () => cleanupAttempts.push("schema"),
    cleanupInfra: async () => cleanupAttempts.push("infra"),
    removeWorkDir: async () => cleanupAttempts.push("tempdir"),
  });
  assert.equal(stopAttempts, 1);
  assert.deepEqual(finalStopAttempts, [child]);
  assert.equal(ownedChildren.has(child), false);
  assert.deepEqual(cleanupAttempts, ["schema", "infra", "tempdir"]);
  assert.equal(report.failure.message, "read connection reset");
  assert.equal(report.exitCode, 1);
});

test("successful rollback request checks stop the exact started child and leave no owned process", async () => {
  const ownedChildren = new Set();
  const stdout = new EventEmitter();
  const stderr = new EventEmitter();
  stdout.setEncoding = () => {};
  stderr.setEncoding = () => {};
  const child = Object.assign(new EventEmitter(), {stdout, stderr, exitCode: null, signalCode: null});
  const startPromise = startService("old-aip", "app.aip", "service.json", "rollback-binary", {
    ownedChildren,
    spawnProcess: () => child,
  });
  queueMicrotask(() => stdout.emit("data", '{"ok":true,"event":"listening","address":"127.0.0.1:44124"}\n'));
  const started = await startPromise;
  const result = await rollbackBinaryOutcome(started, {
    call: async (_base, _token, _fingerprint, path) => path === "/read"
      ? {ok: true, rows: [{id: 11, checked: true}, {id: 12, checked: true}]}
      : {ok: true, replayed: true},
  });
  const stopAttempts = [];
  await stopOwnedService(ownedChildren, child, async (ownedChild) => stopAttempts.push(ownedChild));
  assert.deepEqual(result, {started: true, readsUpgradedData: true, replaysNewBinaryWrite: true});
  assert.deepEqual(stopAttempts, [child]);
  assert.equal(ownedChildren.size, 0);
  const report = await finalizeDrill({ok: true}, {
    ownedChildren,
    cleanup: async () => {},
    cleanupInfra: async () => {},
    removeWorkDir: async () => {},
  });
  assert.equal(report.exitCode, 0);
});
