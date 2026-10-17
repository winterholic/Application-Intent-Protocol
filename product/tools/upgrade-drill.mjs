// Upgrade and rollback drill between two real `aip` binaries against one PostgreSQL schema (WBS 8.7).
//
//   node product/tools/upgrade-drill.mjs --old-bin <aip> --new-bin <aip> [--workers python]
//
// The old binary initializes and serves a deployment, a caller writes through it and a durable job finishes. The new binary then
// serves the same schema: the committed row, the idempotency record and the finished job must survive. Two rollbacks follow:
// the old binary against the schema the new one touched, and a restore of the pre-upgrade dump. Every stage is asserted and the
// observed outcome is printed as one JSON report. The database must be the dedicated test PostgreSQL (see postgres-guard.mjs).
import { spawn, spawnSync } from "node:child_process";
import { createHash, generateKeyPairSync, randomUUID, sign } from "node:crypto";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { requireTestDatabase } from "../test-support/postgres-guard.mjs";

const productDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = resolve(productDir, "..");
const testDb = requireTestDatabase();
const database = testDb.raw;
const issuer = "https://issuer.example";
const audience = "aip-api";

class DrillFailure extends Error {
  constructor(stage, message) {
    super(`${stage}: ${message}`);
    this.stage = stage;
  }
}

function assert(condition, stage, message) {
  if (!condition) throw new DrillFailure(stage, message);
}

function parseArgs(argv) {
  const options = { oldBin: undefined, newBin: undefined, workers: "python", app: join(repoDir, "prototype", "example", "app.aip") };
  for (let index = 0; index < argv.length; index++) {
    const [name, value] = [argv[index], argv[index + 1]];
    if (name === "--old-bin" && value) options.oldBin = resolve(argv[++index]);
    else if (name === "--new-bin" && value) options.newBin = resolve(argv[++index]);
    else if (name === "--workers" && ["node", "python", "none"].includes(value)) options.workers = argv[++index];
    else if (name === "--app" && value) options.app = resolve(argv[++index]);
    else throw new DrillFailure("arguments", "usage: node product/tools/upgrade-drill.mjs --old-bin <aip> --new-bin <aip> [--workers node|python|none] [--app <file.aip>]");
  }
  assert(options.oldBin && options.newBin, "arguments", "--old-bin and --new-bin are required");
  return options;
}

const childEnv = () => ({ PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: process.env.TMPDIR, LANG: process.env.LANG, AIP_TEST_DATABASE: database });

function run(program, args, stage, { allowFailure = false, cwd = repoDir } = {}) {
  const result = spawnSync(program, args, { cwd, env: childEnv(), encoding: "utf8", timeout: 120_000, maxBuffer: 8 * 1024 * 1024 });
  if (result.error) throw new DrillFailure(stage, `cannot start ${program}`);
  if (!allowFailure && result.status !== 0) throw new DrillFailure(stage, `${program} exited ${result.status}: ${(result.stdout + result.stderr).slice(-800)}`);
  return result;
}

function runJson(binary, args, stage) {
  const result = run(binary, args, stage, { allowFailure: true });
  let parsed;
  try {
    parsed = JSON.parse(result.stdout.trim().split("\n")[0]);
  } catch {
    throw new DrillFailure(stage, `no JSON result: ${(result.stdout + result.stderr).slice(-400)}`);
  }
  return { status: result.status, ...parsed };
}

const describe = (outcome) => JSON.stringify({ code: outcome.code, errors: outcome.errors, line: outcome.line });

const sql = (statement, stage) => run("psql", ["-X", "-v", "ON_ERROR_STOP=1", "-At", "--dbname", database, "-c", statement], stage).stdout.trim();

function infraSchema(schema) {
  return `aip_infra_${createHash("sha256").update(schema).digest("hex").slice(0, 52)}`;
}

function tokenFor(jwk, privateKey) {
  const now = Math.floor(Date.now() / 1000);
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const unsigned = `${encode({ alg: "RS256", kid: jwk.kid, typ: "at+jwt" })}.${encode({ iss: issuer, aud: audience, sub: "user-42", iat: now, nbf: now, exp: now + 600 })}`;
  return `${unsigned}.${sign("RSA-SHA256", Buffer.from(unsigned), privateKey).toString("base64url")}`;
}

async function startService(binary, appPath, configPath, stage) {
  const child = spawn(binary, ["service", "serve", appPath, "--config", configPath], { cwd: repoDir, env: childEnv(), stdio: ["ignore", "pipe", "pipe"] });
  let output = "";
  let errors = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => (errors += chunk));
  child.stdout.setEncoding("utf8");
  const outcome = await new Promise((resolveOutcome) => {
    const timer = setTimeout(() => resolveOutcome({ started: false, code: "START_TIMEOUT" }), 20_000);
    child.stdout.on("data", (chunk) => {
      output += chunk;
      const newline = output.indexOf("\n");
      if (newline < 0) return;
      clearTimeout(timer);
      try {
        const line = JSON.parse(output.slice(0, newline));
        resolveOutcome(line.ok === true && line.event === "listening" ? { started: true, address: line.address } : { started: false, code: line.code ?? "UNKNOWN", line });
      } catch {
        resolveOutcome({ started: false, code: "BAD_OUTPUT" });
      }
    });
    child.once("exit", () => {
      clearTimeout(timer);
      if (!output.trim()) resolveOutcome({ started: false, code: "EXITED_WITHOUT_OUTPUT", errors: errors.slice(-400) });
    });
  });
  if (!outcome.started) await stopService(child);
  return { child, ...outcome, stage };
}

async function stopService(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  await new Promise((resolveStop) => {
    const timer = setTimeout(() => child.kill("SIGKILL"), 5_000);
    child.once("exit", () => {
      clearTimeout(timer);
      resolveStop();
    });
    child.kill("SIGTERM");
  });
}

async function call(base, token, fingerprint, path, body) {
  const headers = { "content-type": "application/json", authorization: `Bearer ${token}`, "x-aip-contract": fingerprint };
  const response = await fetch(`${base}${path}`, { method: "POST", headers, body: JSON.stringify(body) });
  return response.json();
}

const summarize = (response) => (response?.ok === true ? response.rows.map((row) => `${row.id}:${row.checked}`).join(",") : `error:${response?.code}`);

const eventQuery = { query: { read: "Event", select: ["id", "checked"] } };
const markRequest = (id) => ({ apply: "Event.mark", target: { ids: [id] } });

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const schema = `aip_drill_${randomUUID().replaceAll("-", "")}`;
  const infra = infraSchema(schema);
  const workDir = await mkdtemp(join(tmpdir(), "aip-upgrade-drill-"));
  const report = { schema: "<run-owned>", oldBin: options.oldBin, newBin: options.newBin, workers: options.workers, stages: {} };
  let server;
  const dumpPath = join(workDir, "pre-upgrade.dump");
  try {
    await mkdir(workDir, { recursive: true });
    const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
    const jwk = { ...publicKey.export({ format: "jwk" }), kid: "upgrade-drill", use: "sig", alg: "RS256", key_ops: ["verify"] };
    const jwksPath = join(workDir, "jwks.json");
    await writeFile(jwksPath, JSON.stringify({ keys: [jwk] }) + "\n", { mode: 0o600 });
    const config = {
      schema,
      database_url_env: "AIP_TEST_DATABASE",
      listen: "127.0.0.1:0",
      wire: "decimal",
      auth: { issuer, audience, jwks: { kind: "file", path: jwksPath } },
    };
    if (options.workers !== "none") {
      config.workers = { directory: join(repoDir, "prototype", "example", "extensions"), language: options.workers, enable_writes: true };
    }
    const configPath = join(workDir, "service.json");
    await writeFile(configPath, JSON.stringify(config, null, 2) + "\n");
    const token = tokenFor(jwk, privateKey);
    const fingerprintOf = (binary) => {
      const generated = runJson(binary, ["service", "gen", options.app, "--wire", "decimal", "--out", join(workDir, `binding-${createHash("sha1").update(binary).digest("hex").slice(0, 6)}.ts`), "--sdk-import", "@aip/sdk"], "gen");
      assert(generated.ok === true, "gen", "service gen failed");
      return generated.fingerprint;
    };

    // 1. The old binary creates the deployment and takes real traffic.
    const init = runJson(options.oldBin, ["service", "init", options.app, "--config", configPath], "old-init");
    assert(init.ok === true && init.schema === schema, "old-init", `init failed: ${JSON.stringify(init)}`);
    sql(`INSERT INTO ${schema}.member(id) VALUES (42); INSERT INTO ${schema}.event(id, member_id, checked, title, link, phase, date) VALUES (11, 42, false, 'drill event one', 'https://example.test', 'READY', '2026-01-01T00:00:00Z'), (12, 42, false, 'drill event two', 'https://example.test', 'READY', '2026-01-01T00:00:00Z')`, "seed");
    const bound = runJson(options.oldBin, ["service", "principal", "--config", configPath, "bind", "--subject", "user-42", "--actor", "42"], "old-bind");
    assert(bound.affected === 1, "old-bind", "principal was not bound");
    const oldFingerprint = fingerprintOf(options.oldBin);
    server = await startService(options.oldBin, options.app, configPath, "old-serve");
    assert(server.started, "old-serve", `old binary did not start: ${describe(server)}`);
    let base = `http://${server.address}`;
    const ready = await fetch(`${base}/ready`);
    assert(ready.status === 200, "old-ready", `/ready answered ${ready.status}`);
    const written = await call(base, token, oldFingerprint, "/apply", { request: markRequest("11"), key: "drill-write-1" });
    assert(written.ok === true && written.replayed !== true, "old-write", `write failed: ${JSON.stringify(written)}`);
    let jobId;
    if (options.workers !== "none") {
      const enqueued = await call(base, token, oldFingerprint, "/jobs/enqueue", { operation: "textLengthJob", input: { text: "가😀A" }, key: "drill-job-1", delayMs: 0 });
      assert(enqueued.ok === true, "old-job", `enqueue failed: ${JSON.stringify(enqueued)}`);
      jobId = enqueued.run.id;
      for (let attempt = 0; attempt < 200; attempt++) {
        const status = await call(base, token, oldFingerprint, "/jobs/status", { id: jobId });
        if (status.run?.state === "SUCCEEDED" || status.run?.state === "FAILED") break;
        await new Promise((resolveWait) => setTimeout(resolveWait, 50));
      }
      const finished = await call(base, token, oldFingerprint, "/jobs/status", { id: jobId });
      assert(finished.run?.state === "SUCCEEDED", "old-job", `job ended in ${finished.run?.state}`);
    }
    await stopService(server.child);
    const before = {
      rows: sql(`SELECT string_agg(id || ':' || checked, ',' ORDER BY id) FROM ${schema}.event`, "snapshot"),
      idempotency: Number(sql(`SELECT count(*) FROM ${schema}.aip_idem`, "snapshot")),
      jobs: options.workers === "none" ? 0 : Number(sql(`SELECT count(*) FROM ${schema}.aip_jobs`, "snapshot")),
    };
    assert(before.rows === "11:true,12:false" && before.idempotency === 1, "old-snapshot", `unexpected pre-upgrade state ${JSON.stringify(before)}`);
    report.stages.old = { fingerprint: oldFingerprint, ...before };

    // 2. Backup before the upgrade: the application schema and, when it exists, the infrastructure schema.
    const infraExisted = sql(`SELECT count(*) FROM pg_namespace WHERE nspname='${infra}'`, "dump") === "1";
    const dumpSchemas = ["--schema", schema, ...(infraExisted ? ["--schema", infra] : [])];
    run("pg_dump", ["-Fc", "--no-owner", ...dumpSchemas, "--file", dumpPath, "--dbname", database], "dump");
    report.stages.backup = { infraSchemaPresent: infraExisted };

    // 3. Upgrade: the new binary must not serve the old schema until its plan is applied, and the plan must be exactly the
    // reviewed change. Applying it keeps every row, record and job.
    const plan = runJson(options.newBin, ["service", "migrate", options.app, "--config", configPath], "new-plan");
    assert(plan.ok === true && plan.blocked === false, "new-plan", `migrate plan refused or blocked: ${JSON.stringify(plan).slice(0, 400)}`);
    const sqlSteps = (plan.steps ?? []).map((step) => step.sql ?? step.kind ?? "step");
    report.stages.plan = { blocked: plan.blocked, requiresReview: plan.requiresReview, changes: (plan.changes ?? []).map((change) => change.class ?? change), steps: sqlSteps.length };
    assert(sqlSteps.every((statement) => !/DROP|TRUNCATE|DELETE/iu.test(String(statement))), "new-plan", `the upgrade plan contains a destructive step: ${JSON.stringify(sqlSteps)}`);
    const refused = await startService(options.newBin, options.app, configPath, "new-serve-before-apply");
    assert(!refused.started, "new-serve-before-apply", "the new binary served an old schema before the plan was applied");
    assert(String(refused.errors ?? "").includes("WIRE.SCHEMA_MISMATCH") || refused.code === "WIRE.SCHEMA_MISMATCH", "new-serve-before-apply", `unexpected refusal: ${describe(refused)}`);
    assert(String(refused.errors ?? "").includes("aip service migrate"), "new-serve-before-apply", `the refusal does not tell the operator how to upgrade: ${describe(refused)}`);
    const journalBefore = Number(sql(`SELECT count(*) FROM ${schema}.aip_migrate_journal`, "journal"));
    const applied = runJson(options.newBin, ["service", "migrate", options.app, "--config", configPath, "--apply", plan.digest], "new-apply");
    assert(applied.ok === true, "new-apply", `applying the reviewed plan failed: ${JSON.stringify(applied).slice(0, 400)}`);
    const journalAfter = Number(sql(`SELECT count(*) FROM ${schema}.aip_migrate_journal`, "journal"));
    assert(journalAfter === journalBefore + 1, "new-apply", "the applied upgrade is not recorded in the migration journal");
    const provisioned = runJson(options.newBin, ["service", "provision", "--config", configPath], "new-provision");
    assert(provisioned.ok === true, "new-provision", `provision failed: ${JSON.stringify(provisioned).slice(0, 300)}`);
    report.stages.apply = { refusedBeforeApply: String(refused.errors ?? "").trim().slice(0, 160), journalRows: [journalBefore, journalAfter], provisioned: true };
    server = await startService(options.newBin, options.app, configPath, "new-serve");
    assert(server.started, "new-serve", `new binary did not start after the upgrade was applied: ${describe(server)}`);
    base = `http://${server.address}`;
    const newFingerprint = fingerprintOf(options.newBin);
    assert((await fetch(`${base}/ready`)).status === 200, "new-ready", "/ready not 200 on the upgraded schema");
    const rows = await call(base, token, newFingerprint, "/read", eventQuery);
    assert(summarize(rows) === "11:true,12:false", "new-read", `rows after upgrade: ${JSON.stringify(rows)}`);
    const replay = await call(base, token, newFingerprint, "/apply", { request: markRequest("11"), key: "drill-write-1" });
    assert(replay.ok === true && replay.replayed === true, "new-replay", `old idempotency record not honored: ${JSON.stringify(replay)}`);
    const status = await call(base, token, newFingerprint, "/status", { request: markRequest("11"), key: "drill-write-1" });
    assert(status.ok === true && status.replayed === true, "new-status", `status after upgrade: ${JSON.stringify(status)}`);
    const fresh = await call(base, token, newFingerprint, "/apply", { request: markRequest("12"), key: "drill-write-2" });
    assert(fresh.ok === true && fresh.replayed !== true, "new-write", `new write after upgrade failed: ${JSON.stringify(fresh)}`);
    if (jobId) {
      const oldJob = await call(base, token, newFingerprint, "/jobs/status", { id: jobId });
      assert(oldJob.run?.state === "SUCCEEDED", "new-job-status", `job from the old binary: ${JSON.stringify(oldJob).slice(0, 300)}`);
      const oldResult = await call(base, token, newFingerprint, "/jobs/result", { id: jobId });
      assert(oldResult.output?.length === 3, "new-job-result", `job result after upgrade: ${JSON.stringify(oldResult).slice(0, 300)}`);
      const newJob = await call(base, token, newFingerprint, "/jobs/enqueue", { operation: "textLengthJob", input: { text: "가😀A" }, key: "drill-job-2", delayMs: 0 });
      assert(newJob.ok === true, "new-job", `enqueue on the upgraded schema failed: ${JSON.stringify(newJob).slice(0, 300)}`);
      let state;
      for (let attempt = 0; attempt < 200 && state !== "SUCCEEDED" && state !== "FAILED"; attempt++) {
        state = (await call(base, token, newFingerprint, "/jobs/status", { id: newJob.run.id })).run?.state;
        if (state !== "SUCCEEDED") await new Promise((resolveWait) => setTimeout(resolveWait, 50));
      }
      assert(state === "SUCCEEDED", "new-job", `job on the upgraded schema ended in ${state}`);
    }
    await stopService(server.child);
    const infraTables = sql(`SELECT string_agg(tablename, ',' ORDER BY tablename) FROM pg_tables WHERE schemaname='${infra}'`, "after-upgrade");
    const jobColumns = options.workers === "none" ? "" : sql(`SELECT string_agg(column_name, ',' ORDER BY column_name) FROM information_schema.columns WHERE table_schema='${schema}' AND table_name='aip_jobs' AND column_name='trace_id'`, "after-upgrade");
    report.stages.upgrade = {
      fingerprintUnchanged: newFingerprint === oldFingerprint,
      oldRowsAndIdempotencyPreserved: true,
      oldJobReadable: Boolean(jobId),
      infraTablesAfter: infraTables,
      jobTraceColumn: jobColumns || null,
    };

    // 4. Rollback A: only the binary goes back. The old binary meets a schema the new one has already touched.
    const reverted = await startService(options.oldBin, options.app, configPath, "rollback-binary");
    let rollbackBinary;
    if (reverted.started) {
      const revertedBase = `http://${reverted.address}`;
      const seen = await call(revertedBase, token, oldFingerprint, "/read", eventQuery);
      const sawRows = summarize(seen) === "11:true,12:true";
      const replayOld = await call(revertedBase, token, oldFingerprint, "/apply", { request: markRequest("12"), key: "drill-write-2" });
      rollbackBinary = { started: true, readsUpgradedData: sawRows, replaysNewBinaryWrite: replayOld.ok === true && replayOld.replayed === true };
      assert(sawRows && rollbackBinary.replaysNewBinaryWrite, "rollback-binary", `the old binary started but disagrees with the data: ${JSON.stringify({ seen, replayOld }).slice(0, 400)}`);
      await stopService(reverted.child);
    } else {
      const refusal = String(reverted.errors ?? "").match(/"code":"([^"]+)"/u)?.[1] ?? reverted.code;
      assert(refusal && refusal !== "START_TIMEOUT" && refusal !== "EXITED_WITHOUT_OUTPUT", "rollback-binary", `the old binary neither served nor failed closed: ${describe(reverted)}`);
      rollbackBinary = { started: false, failedClosedWith: refusal };
    }
    report.stages.rollbackBinaryOnly = rollbackBinary;

    // 5. Rollback B: restore the pre-upgrade dump and serve with the old binary.
    sql(`DROP SCHEMA ${schema} CASCADE`, "restore");
    sql(`DROP SCHEMA IF EXISTS ${infra} CASCADE`, "restore");
    run("pg_restore", ["--no-owner", "--dbname", database, dumpPath], "restore");
    const restored = await startService(options.oldBin, options.app, configPath, "restore-serve");
    assert(restored.started, "restore-serve", `old binary did not start on the restored schema: ${describe(restored)}`);
    const restoredBase = `http://${restored.address}`;
    const back = await call(restoredBase, token, oldFingerprint, "/read", eventQuery);
    assert(summarize(back) === "11:true,12:false", "restore-read", `rows after restore: ${JSON.stringify(back)}`);
    const backReplay = await call(restoredBase, token, oldFingerprint, "/apply", { request: markRequest("11"), key: "drill-write-1" });
    assert(backReplay.ok === true && backReplay.replayed === true, "restore-replay", "idempotency record missing after restore");
    const redo = await call(restoredBase, token, oldFingerprint, "/apply", { request: markRequest("12"), key: "drill-write-2" });
    assert(redo.ok === true && redo.replayed !== true, "restore-redo", "the write undone by the restore must run again as new");
    await stopService(restored.child);
    report.stages.rollbackRestore = { rowsRestored: true, idempotencyRestored: true, undoneWriteRunsAgain: true };
    report.ok = true;
  } catch (error) {
    report.ok = false;
    report.failure = error instanceof DrillFailure ? { stage: error.stage, message: error.message } : { message: String(error?.message ?? error) };
  } finally {
    if (server) await stopService(server.child);
    try {
      sql(`DROP SCHEMA IF EXISTS ${schema} CASCADE`, "cleanup");
      sql(`DROP SCHEMA IF EXISTS ${infra} CASCADE`, "cleanup");
    } catch {
      report.cleanupFailed = true;
    }
    await rm(workDir, { recursive: true, force: true });
  }
  console.log(JSON.stringify(report, null, 2));
  process.exit(report.ok ? 0 : 1);
}

await main();
