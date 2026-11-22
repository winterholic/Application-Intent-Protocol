#!/usr/bin/env node
// 소비자 앱 3유형 표현력 검증 실행기. 한 명령으로 PG 17 컨테이너를 띄우고, 앱마다 새 schema를 init하고,
// `aip service serve`를 띄운 뒤 TypeScript·Python SDK 시나리오를 실행하고 PASS/FAIL 표를 낸다.
//
//   node product/tests/fixtures/expr-apps/run.mjs [--app saas|community|orders] [--lang ts|py] [--keep-pg] [--json out.json]
//
// 환경 변수
//   AIP_BIN                  aip 바이너리(기본: $CARGO_TARGET_DIR/debug/aip 또는 <repo>/target/debug/aip, 없으면 cargo build)
//   AIP_TEST_DATABASE_URL    이미 있는 1회용 PG 17 DSN(keyword 형식). 없으면 docker로 전용 컨테이너를 띄운다
//   AIP_PG_IMAGE             기본 postgres:17
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { generateKeyPairSync, randomUUID, sign } from "node:crypto";
import { createServer } from "node:net";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, copyFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { validateTestDsn } from "../../../test-support/postgres-guard.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../../..");
const tsc = join(root, "spikes/spike-0-ts/node_modules/.bin/tsc");

// 로컬 셸의 PGHOST·PGPORT가 psql·테스트 DSN 검증을 오염시키지 않게 한다.
for (const key of Object.keys(process.env)) if (key.startsWith("PG")) delete process.env[key];

const argv = process.argv.slice(2);
const flag = (name) => argv.includes(name);
const option = (name) => (argv.includes(name) ? argv[argv.indexOf(name) + 1] : undefined);

const subjects = {
  saas: [["alice", 1], ["bob", 2], ["vera", 3], ["carol", 4], ["dave", 5]],
  community: [["mia", 1], ["noah", 2], ["mod", 3]],
  orders: [["olga", 1], ["pavel", 2]],
  teamdocs: [["ann", 1], ["ben", 2], ["cat", 3], ["dan", 4], ["eve", 5], ["fay", 6]],
};

// 한 단위 = (앱, 언어). 단위마다 새 schema·새 serve 프로세스를 쓴다.
const units = [
  { app: "saas", lang: "ts", definition: "workspace.aip", scenario: "scenario.ts" },
  { app: "saas", lang: "py", definition: "workspace.aip", scenario: "scenario.py" },
  { app: "community", lang: "ts", definition: "community.aip", scenario: "scenario.ts" },
  { app: "community", lang: "py", definition: "community.aip", scenario: "scenario.py" },
  { app: "orders", lang: "ts", definition: "orders.current.aip", scenario: "scenario.current.ts", workers: true },
  // P0-A·B·C 병합(2e94daa0) 뒤 목표 정의. 변형 A는 선언만(bundle refs + input 산술), 변형 B는 확장(ctx.data.create).
  { app: "orders", lang: "ts", label: "orders/target-a", definition: "orders.target.aip", scenario: "scenario.target-a.ts" },
  // 한 사용자가 여러 조직에 다른 역할로 속하고, 문서 공유·필드 제한·관계 조회·집계를 쓰는 다중 사용자 업무(C3).
  { app: "teamdocs", lang: "ts", definition: "teamdocs.aip", scenario: "scenario.ts" },
  { app: "orders", lang: "ts", label: "orders/target-b", definition: "orders.target-b.aip", scenario: "scenario.target-b.ts", workers: true },
];

function run(program, args, { cwd = root, env = {}, timeout = 180_000, allowFail = false } = {}) {
  const result = spawnSync(program, args, { cwd, encoding: "utf8", env: { ...process.env, ...env }, timeout, maxBuffer: 16 * 1024 * 1024 });
  assert.equal(result.error, undefined, `${program} 시작 실패: ${result.error?.message}`);
  if (!allowFail) assert.equal(result.status, 0, `${program} ${args.join(" ")} 실패:\n${result.stdout}\n${result.stderr}`);
  return allowFail ? result : result.stdout.trim();
}

function freePort() {
  return new Promise((ok, fail) => {
    const server = createServer();
    server.listen(0, "127.0.0.1", () => { const { port } = server.address(); server.close(() => ok(port)); });
    server.on("error", fail);
  });
}

async function startPostgres() {
  const configured = process.env.AIP_TEST_DATABASE_URL;
  if (configured) {
    validateTestDsn(configured);
    return { dsn: configured, container: null };
  }
  const port = await freePort();
  const name = `aip-expr-apps-${process.pid}`;
  run("docker", ["run", "-d", "--name", name, "-p", `127.0.0.1:${port}:5432`, "-e", "POSTGRES_HOST_AUTH_METHOD=trust",
    "-e", "POSTGRES_DB=aip_test_expr_apps", process.env.AIP_PG_IMAGE ?? "postgres:17"]);
  const dsn = `host=127.0.0.1 port=${port} user=postgres dbname=aip_test_expr_apps`;
  validateTestDsn(dsn);
  for (let i = 0; i < 60; i++) {
    // 이미지의 초기화 단계가 임시 서버를 먼저 띄우므로 같은 질의가 연속 두 번 성공해야 준비된 것으로 본다.
    const a = run("psql", [dsn, "-X", "-Atc", "select 1"], { allowFail: true });
    await new Promise((r) => setTimeout(r, 700));
    const b = run("psql", [dsn, "-X", "-Atc", "select 1"], { allowFail: true });
    if (a.status === 0 && b.status === 0) return { dsn, container: name };
  }
  throw new Error("PG 컨테이너가 준비되지 않음");
}

const psql = (dsn, sql) => run("psql", [dsn, "-X", "-v", "ON_ERROR_STOP=1", "-At", "-c", sql]);

function authMaterial(dir, names) {
  const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const jwk = publicKey.export({ format: "jwk" });
  Object.assign(jwk, { kid: "expr-apps", use: "sig", alg: "RS256", key_ops: ["verify"] });
  writeFileSync(join(dir, "jwks.json"), JSON.stringify({ keys: [jwk] }));
  const now = Math.floor(Date.now() / 1000);
  const enc = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const tokens = {};
  for (const name of names) {
    const head = enc({ alg: "RS256", kid: jwk.kid, typ: "at+jwt" });
    const body = enc({ iss: "https://issuer.example", aud: "aip-api", sub: name, iat: now, nbf: now, exp: now + 300 });
    tokens[name] = `${head}.${body}.${sign("RSA-SHA256", Buffer.from(`${head}.${body}`), privateKey).toString("base64url")}`;
  }
  return tokens;
}

let sdkPrepared;
function prepareSdk(temp) {
  if (sdkPrepared) return sdkPrepared;
  const packageDir = join(temp, "sdk");
  const packDir = join(temp, "pack");
  const consumer = join(temp, "consumer");
  const cache = join(temp, "npm-cache");
  mkdirSync(packDir);
  mkdirSync(consumer);
  run(process.execPath, [join(root, "product/tools/build-sdk.mjs"), "--out", packageDir]);
  const packed = JSON.parse(run("npm", ["pack", "--offline", "--ignore-scripts", "--json", "--pack-destination", packDir, packageDir], { env: { npm_config_cache: cache } }));
  writeFileSync(join(consumer, "package.json"), '{"private":true,"type":"module"}\n');
  run("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", "--prefix", consumer, join(packDir, packed[0].filename)], { env: { npm_config_cache: cache } });
  return (sdkPrepared = consumer);
}

function startService(binary, appPath, configPath, env) {
  const child = spawn(binary, ["service", "serve", appPath, "--config", configPath], { cwd: root, env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"] });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (c) => { stderr += c; });
  return new Promise((ok, fail) => {
    const timer = setTimeout(() => fail(new Error(`serve 시작 시간 초과: ${stderr}`)), 20_000);
    child.stdout.on("data", (c) => {
      stdout += c;
      const line = stdout.split(/\r?\n/u)[0];
      if (!line || !stdout.includes("\n")) return;
      clearTimeout(timer);
      try {
        const message = JSON.parse(line);
        if (message.ok && message.event === "listening") ok({ child, message, log: () => stderr });
        else fail(new Error(`serve 거부: ${line}\n${stderr}`));
      } catch (error) { fail(error); }
    });
    child.once("exit", (code) => { clearTimeout(timer); fail(new Error(`serve 종료 ${code}: ${stdout}\n${stderr}`)); });
  });
}

const stopService = (child) => new Promise((done) => {
  if (child.exitCode !== null || child.signalCode !== null) return done();
  child.once("exit", done);
  child.kill("SIGTERM");
});

function parseResults(unit, output) {
  const rows = [];
  for (const line of output.split(/\r?\n/u)) {
    const m = line.match(/^RESULT (PASS|FAIL|SKIP) (\S+)(?: :: (.*))?$/u);
    if (m) rows.push({ unit, status: m[1], name: m[2], detail: m[3] ?? "" });
    const info = line.match(/^INFO (\S+): (.*)$/u);
    if (info) rows.push({ unit, status: "INFO", name: info[1], detail: info[2] });
  }
  return rows;
}

async function runUnit(unit, ctx) {
  const label = unit.label ?? `${unit.app}/${unit.lang}`;
  const appDir = join(here, unit.app);
  if (unit.targetOnly) {
    const checked = run(ctx.binary, ["service", "check", join(appDir, unit.definition)], { allowFail: true });
    if (checked.status === 0) {
      return [{ unit: label, status: "INFO", name: "target-definition", detail: "현재 바이너리가 목표 정의를 받아들인다. P0가 들어온 것이니 scenario.target.ts를 실행 대상으로 올린다" }];
    }
    const code = JSON.parse(checked.stdout || checked.stderr || "{}").diagnostics?.[0] ?? {};
    return [{ unit: label, status: "SKIP", name: "target-scenarios(P0-A/B/C)", detail: `미실행: P0 미병합. 현재 check 결과 ${code.code ?? "?"} ${code.message ?? ""}`.trim() }];
  }
  const work = join(ctx.temp, `${unit.app}-${unit.lang}-${randomUUID().slice(0, 8)}`);
  mkdirSync(work);
  const schema = `aip_expr_${unit.app}_${unit.lang}_${randomUUID().replaceAll("-", "").slice(0, 12)}`;
  const names = subjects[unit.app].map(([n]) => n);
  const tokens = authMaterial(work, names);
  const appPath = join(appDir, unit.definition);
  const configPath = join(work, "service.json");
  const config = {
    schema, database_url_env: "AIP_TEST_DATABASE", listen: "127.0.0.1:0", wire: "decimal",
    auth: { issuer: "https://issuer.example", audience: "aip-api", jwks: { kind: "file", path: join(work, "jwks.json") } },
  };
  if (unit.workers) config.workers = { directory: join(appDir, "extensions"), language: "node", enable_writes: true };
  writeFileSync(configPath, JSON.stringify(config));
  const env = { AIP_TEST_DATABASE: ctx.dsn };
  let service;
  const out = [];
  try {
    run(ctx.binary, ["service", "check", appPath]);
    run(ctx.binary, ["service", "gen", appPath, "--wire", "decimal", "--out", join(work, "binding.ts"), "--sdk-import", "@aip/sdk"]);
    if (unit.lang === "py") run(ctx.binary, ["service", "gen", appPath, "--lang", "python", "--wire", "decimal", "--out", join(work, "binding.py")]);
    run(ctx.binary, ["service", "init", appPath, "--config", configPath], { env });
    const seed = readFileSync(join(appDir, "seed.sql"), "utf8").replaceAll("${SCHEMA}", schema);
    writeFileSync(join(work, "seed.sql"), seed);
    run("psql", [ctx.dsn, "-X", "-v", "ON_ERROR_STOP=1", "-q", "-f", join(work, "seed.sql")]);
    for (const [subject, actor] of subjects[unit.app]) {
      run(ctx.binary, ["service", "principal", "--config", configPath, "bind", "--subject", subject, "--actor", String(actor)], { env });
    }
    service = await startService(ctx.binary, appPath, configPath, env);
    const scenarioEnv = {
      AIP_ADDR: service.message.address, AIP_TOKENS: JSON.stringify(tokens), AIP_DB: ctx.dsn, AIP_SCHEMA: schema,
      AIP_FINGERPRINT: service.message.fingerprint, AIP_REPO: root,
    };
    let result;
    if (unit.lang === "ts") {
      const sdk = prepareSdk(ctx.temp);
      const dir = join(sdk, unit.app);
      mkdirSync(dir, { recursive: true });
      copyFileSync(join(work, "binding.ts"), join(dir, "binding.ts"));
      copyFileSync(join(here, "lib/check.ts"), join(dir, "check.ts"));
      copyFileSync(join(appDir, unit.scenario), join(dir, unit.scenario));
      const compiled = run(tsc, ["--strict", "--target", "ES2022", "--module", "NodeNext", "--moduleResolution", "NodeNext",
        "--types", "node", "--typeRoots", join(root, "spikes/spike-0-ts/node_modules/@types"), "--outDir", dir,
        join(dir, "binding.ts"), join(dir, "check.ts"), join(dir, unit.scenario)], { cwd: sdk, allowFail: true });
      if (compiled.status !== 0) {
        out.push({ unit: label, status: "FAIL", name: "typescript-typecheck", detail: `${compiled.stdout}${compiled.stderr}`.trim().split("\n").slice(0, 8).join(" | ") });
        return out;
      }
      out.push({ unit: label, status: "PASS", name: "typescript-typecheck", detail: "scenario가 생성 binding에 대해 --strict 컴파일 통과" });
      result = run(process.execPath, [join(dir, unit.scenario.replace(/\.ts$/u, ".js"))], { cwd: dir, env: scenarioEnv, timeout: 240_000, allowFail: true });
    } else {
      copyFileSync(join(here, "lib/check.py"), join(work, "check.py"));
      copyFileSync(join(appDir, unit.scenario), join(work, unit.scenario));
      result = run("python3", [join(work, unit.scenario)], { cwd: work, env: { ...scenarioEnv, PYTHONPATH: `${join(root, "product/python")}:${work}` }, timeout: 240_000, allowFail: true });
    }
    out.push(...parseResults(label, result.stdout));
    if (result.status !== 0 && !out.some((r) => r.status === "FAIL")) {
      out.push({ unit: label, status: "FAIL", name: "scenario-process", detail: `exit ${result.status}: ${(result.stderr || "").trim().split("\n").slice(-4).join(" | ")}` });
    }
    if (process.env.AIP_EXPR_VERBOSE) console.error(result.stderr);
    return out;
  } catch (error) {
    out.push({ unit: label, status: "FAIL", name: "harness", detail: String(error.message).split("\n").slice(0, 6).join(" | ") });
    return out;
  } finally {
    if (service) {
      await stopService(service.child);
      if (process.env.AIP_EXPR_VERBOSE) console.error(service.log());
    }
    run("psql", [ctx.dsn, "-X", "-q", "-c", `DROP SCHEMA IF EXISTS ${schema} CASCADE`], { allowFail: true });
  }
}

const only = option("--app");
const onlyLang = option("--lang");
const temp = mkdtempSync(join(tmpdir(), "aip-expr-apps-"));
let pg;
let exitCode = 0;
try {
  const targetDir = process.env.CARGO_TARGET_DIR ?? join(root, "target");
  const binary = process.env.AIP_BIN ?? join(targetDir, "debug/aip");
  if (!existsSync(binary)) run("cargo", ["build", "-p", "aip-cli", "--bin", "aip"], { timeout: 600_000 });
  assert.ok(existsSync(tsc), `${tsc} 없음: npm ci --prefix spikes/spike-0-ts --ignore-scripts 필요`);
  pg = await startPostgres();
  const version = psql(pg.dsn, "select version()");
  const revision = run("git", ["rev-parse", "--short=12", "HEAD"]);
  const dirty = run("git", ["status", "--porcelain", "--untracked-files=no"]) ? " (+dirty)" : "";
  const results = [];
  for (const unit of units.filter((u) => (!only || u.app === only) && (!onlyLang || u.lang === onlyLang))) {
    const rows = await runUnit(unit, { binary, dsn: pg.dsn, temp });
    for (const row of rows) console.log(`${row.status.padEnd(4)} ${row.unit.padEnd(14)} ${row.name}${row.detail ? `  -- ${row.detail}` : ""}`);
    results.push(...rows);
  }
  const count = (s) => results.filter((r) => r.status === s).length;
  console.log("");
  console.log(`실제 PostgreSQL: ${version}`);
  console.log(`DSN 대상: ${pg.container ? `전용 컨테이너 ${pg.container}` : "AIP_TEST_DATABASE_URL"} (${pg.dsn.replace(/user=\S+/u, "user=…")})`);
  console.log(`revision: ${revision}${dirty}, ${run(binary, ["--version"])}, HTTP=aip service serve, SDK=@aip/sdk(tarball 설치)+python aip_sdk`);
  console.log(`합계: PASS ${count("PASS")} / FAIL ${count("FAIL")} / SKIP ${count("SKIP")}`);
  if (option("--json")) writeFileSync(option("--json"), JSON.stringify({ revision, version, results }, null, 2));
  if (count("FAIL") > 0 || results.length === 0) exitCode = 1;
} catch (error) {
  console.error(error.stack ?? error);
  exitCode = 2;
} finally {
  if (pg?.container && !flag("--keep-pg")) run("docker", ["rm", "-f", pg.container], { allowFail: true });
  rmSync(temp, { recursive: true, force: true });
}
process.exit(exitCode);
