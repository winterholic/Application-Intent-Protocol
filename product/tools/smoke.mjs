import { spawn, spawnSync } from "node:child_process";
import { generateKeyPairSync, randomUUID, sign } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const productDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = resolve(productDir, "..");
const testDatabase = "host=localhost dbname=postgres";
const issuer = "https://issuer.example";
const audience = "aip-api";

class SmokeFailure extends Error {
  constructor(stage, code, message, exitCode) {
    super(message);
    this.stage = stage;
    this.code = code;
    this.exitCode = exitCode;
  }
}

function argumentsFrom(argv) {
  let binary = join(repoDir, "target", "debug", "aip");
  let sdkPackage;
  let worker;
  let extensionDir;
  let wires = ["decimal", "safe"];
  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") {
      console.log("usage: node product/tools/smoke.mjs [--bin <aip-binary>] [--sdk-package <package.tgz>] [--wire decimal|safe] [--workers node|python] [--extension-dir <directory>]");
      process.exit(0);
    }
    if (arg === "--bin" && argv[index + 1]) {
      binary = resolve(argv[++index]);
      continue;
    }
    if (arg === "--sdk-package" && argv[index + 1]) {
      sdkPackage = resolve(argv[++index]);
      continue;
    }
    if (arg === "--workers" && ["node", "python"].includes(argv[index + 1])) {
      worker = argv[++index];
      continue;
    }
    if (arg === "--extension-dir" && argv[index + 1]) {
      extensionDir = resolve(argv[++index]);
      continue;
    }
    if (arg === "--wire" && ["decimal", "safe"].includes(argv[index + 1])) {
      wires = [argv[++index]];
      continue;
    }
    throw new SmokeFailure("arguments", "BAD_ARGS", "알 수 없는 인자 또는 인자값이 없습니다");
  }
  if (extensionDir && !worker) throw new SmokeFailure("arguments", "BAD_ARGS", "--extension-dir는 --workers와 함께 지정해야 합니다");
  return { binary, sdkPackage, worker, extensionDir, wires };
}

const childEnv = (extra = {}) => ({
  PATH: process.env.PATH,
  HOME: process.env.HOME,
  TMPDIR: process.env.TMPDIR,
  LANG: process.env.LANG,
  AIP_TEST_DATABASE: testDatabase,
  ...extra,
});

function run(program, args, stage, options = {}) {
  const result = spawnSync(program, args, {
    cwd: options.cwd ?? repoDir,
    env: options.env ?? childEnv(),
    encoding: "utf8",
    timeout: options.timeout ?? 60_000,
    maxBuffer: 4 * 1024 * 1024,
  });
  if (result.error) throw new SmokeFailure(stage, "COMMAND_START", "필요한 로컬 명령을 실행할 수 없습니다");
  if (result.signal) throw new SmokeFailure(stage, "COMMAND_SIGNAL", `하위 명령이 ${result.signal} 신호로 종료됐습니다`);
  if (result.status !== 0) {
    const diagnostic = stage === "typecheck" ? `${result.stdout}\n${result.stderr}`.trim().slice(-2000) : "";
    throw new SmokeFailure(stage, "COMMAND_FAILED", diagnostic || "하위 명령이 실패했습니다", result.status);
  }
  return result.stdout;
}

function runJson(program, args, stage, options = {}) {
  const output = run(program, args, stage, options).trim();
  let result;
  try {
    result = JSON.parse(output);
  } catch {
    throw new SmokeFailure(stage, "BAD_COMMAND_OUTPUT", "하위 명령이 JSON 결과를 반환하지 않았습니다");
  }
  if (result?.ok !== true) throw new SmokeFailure(stage, result?.code ?? "COMMAND_REJECTED", "제품 명령이 요청을 거부했습니다");
  return result;
}

function jwtFor(publicJwk, privateKey) {
  const now = Math.floor(Date.now() / 1000);
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const header = encode({ alg: "RS256", kid: publicJwk.kid, typ: "at+jwt" });
  const claims = encode({ iss: issuer, aud: audience, sub: "user-42", iat: now, nbf: now, exp: now + 300 });
  const unsigned = `${header}.${claims}`;
  return `${unsigned}.${sign("RSA-SHA256", Buffer.from(unsigned), privateKey).toString("base64url")}`;
}

async function createAuthFiles(directory) {
  const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const jwk = publicKey.export({ format: "jwk" });
  jwk.kid = "product-sdk-smoke-rsa";
  jwk.use = "sig";
  jwk.alg = "RS256";
  jwk.key_ops = ["verify"];
  const jwksPath = join(directory, "jwks.json");
  await writeFile(jwksPath, JSON.stringify({ keys: [jwk] }) + "\n", { mode: 0o600, flag: "wx" });
  return { jwksPath, token: jwtFor(jwk, privateKey) };
}

async function writeConfig(directory, schema, wire, jwksPath, worker, extensionDir) {
  const configPath = join(directory, "service.json");
  const config = {
    schema,
    database_url_env: "AIP_TEST_DATABASE",
    listen: "127.0.0.1:0",
    wire,
    allowed_origins: ["http://localhost:3000"],
    auth: { issuer, audience, jwks: { kind: "file", path: jwksPath } },
  };
  if (worker) {
    config.workers = {
      directory: extensionDir ?? join(repoDir, "prototype", "example", "extensions"),
      language: worker,
      enable_writes: true,
    };
  }
  await writeFile(configPath, JSON.stringify(config, null, 2) + "\n", { flag: "wx" });
  return configPath;
}

function parseJsonLine(line, stage) {
  try {
    return JSON.parse(line);
  } catch {
    throw new SmokeFailure(stage, "BAD_SERVER_OUTPUT", "서비스가 기동 결과 JSON을 반환하지 않았습니다");
  }
}

async function startService(binary, appPath, configPath) {
  const child = spawn(binary, ["service", "serve", appPath, "--config", configPath], {
    cwd: repoDir,
    env: childEnv(),
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  const listening = new Promise((resolveListening, rejectListening) => {
    const timeout = setTimeout(() => rejectListening(new SmokeFailure("serve", "START_TIMEOUT", "서비스 기동 시간이 초과됐습니다")), 15_000);
    const fail = (error) => {
      clearTimeout(timeout);
      rejectListening(error);
    };
    child.once("error", () => fail(new SmokeFailure("serve", "COMMAND_START", "서비스 실행 파일을 시작하지 못했습니다")));
    child.once("exit", (code) => {
      if (output.trim()) {
        const firstLine = output.split(/\r?\n/u)[0];
        const payload = parseJsonLine(firstLine, "serve");
        if (payload?.ok === true && payload.event === "listening") return;
        fail(new SmokeFailure("serve", payload?.code ?? "SERVER_EXITED", "서비스가 수신 대기 전에 종료됐습니다", code ?? undefined));
      } else {
        fail(new SmokeFailure("serve", "SERVER_EXITED", "서비스가 수신 대기 전에 종료됐습니다", code ?? undefined));
      }
    });
    child.stdout.on("data", (chunk) => {
      output += chunk;
      const newline = output.indexOf("\n");
      if (newline < 0) return;
      clearTimeout(timeout);
      const firstLine = output.slice(0, newline).trim();
      const payload = parseJsonLine(firstLine, "serve");
      if (payload?.ok !== true || payload.event !== "listening" || typeof payload.address !== "string") {
        fail(new SmokeFailure("serve", payload?.code ?? "SERVER_START_FAILED", "서비스가 수신 대기를 시작하지 못했습니다"));
        return;
      }
      resolveListening({ child, address: payload.address });
    });
  });
  try {
    return await listening;
  } catch (error) {
    await stopService(child);
    throw error;
  }
}

async function stopService(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  await new Promise((resolveStop) => {
    const timeout = setTimeout(() => child.kill("SIGKILL"), 5_000);
    child.once("exit", () => {
      clearTimeout(timeout);
      resolveStop();
    });
    child.kill("SIGTERM");
  });
}

function makeSmokeConsumer(wire) {
  const idType = wire === "decimal" ? "string" : "number";
  const idValue = wire === "decimal" ? '"11"' : "11";
  return `
import { connect } from "@aip/sdk";
import { contract } from "./binding.js";

const base = process.env.AIP_SMOKE_BASE;
const token = process.env.AIP_SMOKE_TOKEN;
if (!base || !token) throw new Error("smoke runtime configuration missing");
const client = connect(base, token, contract);
if (false) {
  // @ts-expect-error operation names come from the generated contract
  void client.operation("missing", {});
  // @ts-expect-error operation input is generated from its declaration
  void client.operation("textLength", { text: 42 });
}
const query = { read: "Event", select: ["id", "checked", "title", "phase"] as const } as const;
const first = await client.read(query);
if (first.rows.length !== 1 || first.rows[0].id !== ${idValue} || first.rows[0].checked !== false) throw new Error("initial typed read mismatch");
const second = await client.read(query);
if (!second.cached) throw new Error("second read did not hit the shared cache");
const targetId: ${idType} = ${idValue};
const request = { apply: "Event.mark", target: { ids: [targetId] } } as const;
const applied = await client.apply(request, { key: "product-sdk-smoke-write" });
if (!applied.ok || !applied.changed.includes(targetId) || applied.replayed === true) throw new Error("typed write was not applied");
const afterWrite = await client.read(query);
if (afterWrite.cached || afterWrite.rows[0]?.checked !== true) throw new Error("write did not invalidate the read cache");
const replay = await client.apply(request, { key: "product-sdk-smoke-write" });
if (!replay.ok || replay.replayed !== true) throw new Error("idempotent write replay was not reported");
let extension;
let operation;
if (process.env.AIP_SMOKE_WORKER) {
  const extensionResult = await client.writeExtension("Event.confirm", { id: targetId }, { key: "product-sdk-smoke-extension" });
  if (!extensionResult.ok) throw new Error("Event.confirm extension write failed");
  if (extensionResult.output.id !== targetId || extensionResult.output.count !== 0) throw new Error("Event.confirm did not preserve the unchanged transition result");
  const extensionReplay = await client.writeExtension("Event.confirm", { id: targetId }, { key: "product-sdk-smoke-extension" });
  if (!extensionReplay.ok || extensionReplay.replayed !== true || extensionReplay.output.count !== 0) throw new Error("Event.confirm idempotent replay failed");
  extension = { writesApplied: 1, unchangedCount: extensionResult.output.count, replayedWrites: Number(extensionReplay.replayed === true) };
  const calculated = await client.operation("textLength", { text: "가😀A" });
  if (calculated.length !== 3) throw new Error("operation Unicode calculation failed");
  let rejected = false;
  try { await client.operation("textLength", { text: "" }); }
  catch (error) { rejected = (error as {code?:string}).code === "BAD_VALUE"; }
  if (!rejected) throw new Error("operation range validation failed");
  operation = { length: calculated.length, invalidInputRejected: Number(rejected) };
}
const report: Record<string, unknown> = { wire: ${JSON.stringify(wire)}, firstReadRows: first.rows.length, cacheHits: Number(second.cached), writesApplied: Number(applied.ok && applied.changed.length === 1), postWriteCacheMisses: Number(!afterWrite.cached), replayedWrites: Number(replay.ok && replay.replayed === true) };
if (extension) report.extension = extension;
if (operation) report.operation = operation;
if (process.env.AIP_SMOKE_WORKER) report.worker = process.env.AIP_SMOKE_WORKER;
console.log(JSON.stringify(report));
`;
}

async function typecheckAndInstall(workDir, tarball, wire, bindingPath) {
  const consumerDir = join(workDir, "consumer");
  const outputDir = join(consumerDir, "out");
  await mkdir(consumerDir, { recursive: true });
  await writeFile(join(consumerDir, "package.json"), JSON.stringify({ type: "module" }) + "\n", { flag: "wx" });
  run("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", tarball], "install", { cwd: consumerDir });
  const bindingInConsumer = join(consumerDir, "binding.ts");
  await writeFile(bindingInConsumer, await readFile(bindingPath), { flag: "wx" });
  const runnerPath = join(consumerDir, "smoke.ts");
  await writeFile(runnerPath, makeSmokeConsumer(wire), { flag: "wx" });
  const compiler = join(repoDir, "spikes", "spike-0-ts", "node_modules", ".bin", "tsc");
  run(compiler, [
    "--strict", "--noEmitOnError", "--module", "NodeNext", "--moduleResolution", "NodeNext", "--target", "ES2022",
    "--lib", "ES2022,DOM", "--types", "node", "--typeRoots", join(repoDir, "spikes", "spike-0-ts", "node_modules", "@types"),
    "--rootDir", consumerDir, "--outDir", outputDir, runnerPath, bindingInConsumer,
  ], "typecheck", { cwd: consumerDir });
  return { consumerDir, runnerPath: join(outputDir, "smoke.js") };
}

async function runWire({ binary, wire, workRoot, tarball, worker, extensionDir }) {
  const wireDir = join(workRoot, wire);
  await mkdir(wireDir, { recursive: true });
  const schema = `aip_product_${randomUUID().replaceAll("-", "")}`;
  const appPath = join(wireDir, "app.aip");
  await writeFile(appPath, await readFile(join(repoDir, "prototype", "example", "app.aip")), { flag: "wx" });
  const { jwksPath, token } = await createAuthFiles(wireDir);
  const configPath = await writeConfig(wireDir, schema, wire, jwksPath, worker, extensionDir);
  const init = runJson(binary, ["service", "init", appPath, "--config", configPath], "init");
  if (init.schema !== schema) throw new SmokeFailure("init", "SCHEMA_OWNERSHIP", "초기화 결과 schema가 요청한 이름과 다릅니다");
  let ownsSchema = true;
  let server;
  let failure;
  try {
    const seed = `INSERT INTO ${schema}.member(id) VALUES (42); INSERT INTO ${schema}.event(id, member_id, checked, title, link, phase, date) VALUES (11, 42, false, 'SDK smoke event', 'https://example.test', 'READY', '2026-01-01T00:00:00Z');`;
    run("psql", ["-X", "-v", "ON_ERROR_STOP=1", "--dbname", testDatabase, "-c", seed], "seed");
    const principal = runJson(binary, ["service", "principal", "--config", configPath, "bind", "--subject", "user-42", "--actor", "42"], "principal-bind");
    if (principal.affected !== 1) throw new SmokeFailure("principal-bind", "PRINCIPAL_NOT_BOUND", "주체와 actor 연결이 생성되지 않았습니다");
    const bindingPath = join(wireDir, "binding.ts");
    runJson(binary, ["service", "gen", appPath, "--wire", wire, "--out", bindingPath, "--sdk-import", "@aip/sdk"], "gen");
    const consumer = await typecheckAndInstall(wireDir, tarball, wire, bindingPath);
    server = await startService(binary, appPath, configPath);
    const base = `http://${server.address}`;
    const runtime = run(process.execPath, [consumer.runnerPath], "consumer-runtime", {
      cwd: consumer.consumerDir,
      env: childEnv({ AIP_SMOKE_BASE: base, AIP_SMOKE_TOKEN: token, ...(worker ? { AIP_SMOKE_WORKER: worker } : {}) }),
    }).trim();
    let results;
    try {
      results = JSON.parse(runtime);
    } catch {
      throw new SmokeFailure("consumer-runtime", "BAD_CONSUMER_OUTPUT", "설치 소비자가 smoke 결과 JSON을 반환하지 않았습니다");
    }
    if (results.wire !== wire || results.firstReadRows !== 1 || results.cacheHits !== 1 || results.writesApplied !== 1 || results.postWriteCacheMisses !== 1 || results.replayedWrites !== 1) {
      throw new SmokeFailure("consumer-runtime", "SDK_BEHAVIOR", "설치된 SDK의 read/cache/write 결과가 기대와 다릅니다");
    }
    if (worker && (results.worker !== worker || results.extension?.writesApplied !== 1 || results.extension.unchangedCount !== 0 || results.extension.replayedWrites !== 1)) {
      throw new SmokeFailure("consumer-runtime", "EXTENSION_BEHAVIOR", "Event.confirm 확장 쓰기 결과가 기대와 다릅니다");
    }
    if (worker && (results.operation?.length !== 3 || results.operation.invalidInputRejected !== 1)) {
      throw new SmokeFailure("consumer-runtime", "OPERATION_BEHAVIOR", "설치 SDK의 operation 호출과 범위 검증 결과가 기대와 다릅니다");
    }
    const revoked = runJson(binary, ["service", "principal", "--config", configPath, "revoke", "--subject", "user-42"], "principal-revoke");
    if (revoked.affected !== 1) throw new SmokeFailure("principal-revoke", "PRINCIPAL_NOT_REVOKED", "principal 폐기가 적용되지 않았습니다");
    const response = await fetch(`${base}/session`, {
      method: "POST",
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
      body: "{}",
    });
    const session = await response.json();
    if (session.ok !== false || session.code !== "UNAUTHENTICATED") throw new SmokeFailure("revocation", "REVOCATION_ACCEPTED", "폐기한 JWT가 service endpoint에서 계속 허용됩니다");
    return {
      wire,
      results: {
        firstReadRows: results.firstReadRows,
        cacheHits: results.cacheHits,
        writesApplied: results.writesApplied,
        postWriteCacheMisses: results.postWriteCacheMisses,
        replayedWrites: results.replayedWrites,
        revokedRequestsDenied: 1,
      },
      ...(worker ? { extension: results.extension } : {}),
      ...(worker ? { operation: results.operation } : {}),
    };
  } catch (error) {
    failure = error;
    throw error;
  } finally {
    if (server) await stopService(server.child);
    if (ownsSchema) {
      try {
        run("psql", ["-X", "-v", "ON_ERROR_STOP=1", "--dbname", testDatabase, "-c", `DROP SCHEMA ${schema} CASCADE`], "cleanup");
        ownsSchema = false;
      } catch {
        if (failure) failure.message = `${failure.message}; smoke가 소유한 임시 schema cleanup도 실패했습니다`;
        else throw new SmokeFailure("cleanup", "SCHEMA_CLEANUP_FAILED", "smoke가 소유한 임시 schema를 정리하지 못했습니다");
      }
    }
  }
}

async function buildAndPack(workRoot, suppliedPackage) {
  if (suppliedPackage) {
    const info = await stat(suppliedPackage).catch(() => undefined);
    if (!info?.isFile() || !suppliedPackage.endsWith(".tgz")) {
      throw new SmokeFailure("sdk-package", "PACKAGE_MISSING", "--sdk-package는 존재하는 .tgz 파일이어야 합니다");
    }
    const metadataText = run("tar", ["-xOf", suppliedPackage, "package/package.json"], "sdk-package-metadata");
    let metadata;
    try {
      metadata = JSON.parse(metadataText);
    } catch {
      throw new SmokeFailure("sdk-package-metadata", "BAD_PACKAGE_METADATA", "SDK tarball package.json을 읽지 못했습니다");
    }
    if (metadata.name !== "@aip/sdk" || metadata.version !== "0.1.0") {
      throw new SmokeFailure("sdk-package-metadata", "PACKAGE_IDENTITY", "SDK tarball이 @aip/sdk@0.1.0이 아닙니다");
    }
    return { tarball: suppliedPackage, metadata };
  }

  const output = join(workRoot, "sdk-package");
  const packDir = join(workRoot, "pack");
  await mkdir(packDir, { recursive: true });
  run(process.execPath, [join(productDir, "tools", "build-sdk.mjs"), "--out", output], "sdk-build");
  const packed = run("npm", ["pack", "--offline", "--ignore-scripts", "--json", "--pack-destination", packDir, output], "pack");
  let result;
  try {
    result = JSON.parse(packed);
  } catch {
    throw new SmokeFailure("pack", "BAD_PACK_OUTPUT", "npm pack 결과를 읽지 못했습니다");
  }
  if (result.length !== 1 || result[0].name !== "@aip/sdk" || result[0].version !== "0.1.0") {
    throw new SmokeFailure("pack", "PACKAGE_IDENTITY", "로컬 tarball의 package 이름 또는 버전이 다릅니다");
  }
  return { tarball: join(packDir, result[0].filename), metadata: result[0] };
}

async function smoke() {
  const { binary, sdkPackage, worker, extensionDir, wires } = argumentsFrom(process.argv.slice(2));
  const workRoot = await mkdtemp(join(tmpdir(), "aip-product-sdk-smoke-"));
  try {
    const sdk = await buildAndPack(workRoot, sdkPackage);
    const results = [];
    for (const wire of wires) results.push(await runWire({ binary, wire, workRoot, tarball: sdk.tarball, worker, extensionDir }));
    return {
      ok: true,
      package: { name: sdk.metadata.name, version: sdk.metadata.version, packed: true, installed: true },
      wires: results,
      ...(worker ? { worker } : {}),
    };
  } finally {
    await rm(workRoot, { recursive: true, force: true });
  }
}

try {
  console.log(JSON.stringify(await smoke()));
} catch (error) {
  const safeMessage = error instanceof Error
    ? error.message.replaceAll(testDatabase, "[redacted database]").replace(/Bearer\s+\S+/gu, "Bearer [redacted]").replace(/[A-Za-z0-9_-]{96,}/gu, "[redacted token]")
    : "제품 SDK 통합 smoke가 실패했습니다";
  const failure = error instanceof SmokeFailure ? error : new SmokeFailure("internal", "SMOKE_FAILED", safeMessage);
  console.error(JSON.stringify({ ok: false, stage: failure.stage, code: failure.code, message: failure.message, exitCode: failure.exitCode }));
  process.exitCode = 1;
}
