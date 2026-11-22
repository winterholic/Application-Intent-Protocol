// 의미 정합 시험(D2·D4)과 정의 반례 탐색(D1)이 함께 쓰는 서비스 픽스처.
// 정의 텍스트 하나로 check → gen → init → seed → serve(PG17)를 띄우고, 주체별 JWT와 HTTP 도우미를 돌려준다.
// 시험 전용 DB(AIP_TEST_DATABASE_URL, postgres-guard가 검증)만 쓰고 schema는 시험마다 새로 만들어 끝에서 지운다.
import assert from "node:assert/strict";
import { generateKeyPairSync, sign, randomUUID } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { requireTestDatabase } from "./postgres-guard.mjs";

export const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
export const binary = process.env.AIP_BIN ?? join(root, "target/debug/aip");
const database = requireTestDatabase().raw;
const temp = mkdtempSync(join(tmpdir(), "aip-semantic-"));
export const tsc = join(root, "spikes/spike-0-ts/node_modules/.bin/tsc");

export function cleanupTemp() {
  rmSync(temp, { recursive: true, force: true });
}

export function run(program, args, options = {}) {
  const result = spawnSync(program, args, {
    cwd: options.cwd ?? root,
    encoding: "utf8",
    env: { ...process.env, AIP_TEST_DATABASE: database, ...options.env },
    timeout: options.timeout ?? 120_000,
    maxBuffer: 8 * 1024 * 1024,
  });
  assert.equal(result.error, undefined, `${program} could not start: ${result.error?.message}`);
  if (options.allowFail) return result;
  assert.equal(result.signal, null, `${program} terminated by ${result.signal}`);
  assert.equal(result.status, 0, `${program} ${args.join(" ")} failed:\n${result.stdout}\n${result.stderr}`);
  return result.stdout.trim();
}

export function psql(sql) {
  return run("psql", [database, "-X", "-v", "ON_ERROR_STOP=1", "-At", "-c", sql]);
}

function mintToken(dir, subjects) {
  const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const jwk = publicKey.export({ format: "jwk" });
  Object.assign(jwk, { kid: "semantic-fixture", use: "sig", alg: "RS256", key_ops: ["verify"] });
  writeFileSync(join(dir, "jwks.json"), JSON.stringify({ keys: [jwk] }));
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const tokens = {};
  const now = Math.floor(Date.now() / 1000);
  for (const subject of Object.keys(subjects)) {
    const header = encode({ alg: "RS256", kid: jwk.kid, typ: "at+jwt" });
    const claims = encode({ iss: "https://issuer.example", aud: "aip-api", sub: subject, iat: now, nbf: now, exp: now + 600 });
    const input = `${header}.${claims}`;
    tokens[subject] = `${input}.${sign("RSA-SHA256", Buffer.from(input), privateKey).toString("base64url")}`;
  }
  return tokens;
}

/**
 * 정의로 서비스를 띄운다.
 * @param {object} spec
 * @param {string} spec.definition   .aip 텍스트
 * @param {string} [spec.seed]       `${S}`를 schema 이름으로 치환하는 SQL
 * @param {Record<string, number>} [spec.actors]  JWT subject → actor id (기본 {user: 1})
 * @param {Record<string, string>} [spec.extensions]  확장 파일 이름 → 소스. 있으면 node worker와 쓰기 확장을 켠다
 * @param {string} [spec.wire]       "decimal"(기본) | "safe"
 * @param {boolean} [spec.serve]     false면 check·gen·init까지만 한다
 */
export async function startFixture(spec) {
  const dir = join(temp, randomUUID());
  mkdirSync(dir);
  const schema = `aip_sem_${randomUUID().replaceAll("-", "").slice(0, 24)}`;
  const actors = spec.actors ?? { user: 1 };
  const wire = spec.wire ?? "decimal";
  const owned = { dir, schema, child: null, stderr: "" };
  try {
    const appPath = join(dir, "app.aip");
    const configPath = join(dir, "service.json");
    writeFileSync(appPath, spec.definition);
    const tokens = mintToken(dir, actors);
    const config = {
      schema, database_url_env: "AIP_TEST_DATABASE", listen: "127.0.0.1:0", wire,
      auth: { issuer: "https://issuer.example", audience: "aip-api", jwks: { kind: "file", path: join(dir, "jwks.json") } },
    };
    if (spec.extensions) {
      const extDir = join(dir, "extensions");
      mkdirSync(extDir);
      for (const [name, source] of Object.entries(spec.extensions)) writeFileSync(join(extDir, name), source);
      config.workers = { directory: extDir, language: "node", enable_writes: true };
    }
    writeFileSync(configPath, JSON.stringify(config));
    const check = run(binary, ["service", "check", appPath, ...(spec.checkArgs ?? [])], { allowFail: spec.allowCheckFailure });
    if (spec.allowCheckFailure && check.status !== 0) {
      return { ...owned, checked: false, check, appPath, configPath, stop: () => closeFixture(owned) };
    }
    run(binary, ["service", "gen", appPath, "--wire", wire, "--out", join(dir, "binding.ts"), "--sdk-import", spec.sdkImport ?? "@aip/sdk"]);
    if (spec.python !== false) run(binary, ["service", "gen", appPath, "--lang", "python", "--wire", wire, "--out", join(dir, "binding.py")]);
    const generated = readFileSync(join(dir, "binding.ts"), "utf8");
    const fingerprint = generated.match(/export const contractFingerprint = "([a-f0-9]{64})";/u)?.[1];
    assert.ok(fingerprint, "generated binding carries the contract fingerprint");
    run(binary, ["service", "init", appPath, "--config", configPath]);
    if (spec.seed) psql(spec.seed.replaceAll("${S}", schema));
    for (const [subject, actor] of Object.entries(actors)) {
      run(binary, ["service", "principal", "--config", configPath, "bind", "--subject", subject, "--actor", String(actor)]);
    }
    const listening = spec.serve === false ? undefined : await serve(owned, appPath, configPath);
    const base = listening ? `http://${listening.address}` : undefined;
    const fixture = {
      ...owned, checked: true, appPath, configPath, tokens, fingerprint, base, address: listening?.address,
      serviceFingerprint: listening?.fingerprint,
      /** 계약 헤더를 기본으로 붙이는 원시 HTTP 호출. `contract: false`면 헤더를 뺀다. */
      async post(path, body, { token = "user", contract = fingerprint, headers = {} } = {}) {
        const h = { "content-type": "application/json", ...headers };
        const bearer = tokens[token] ?? token;
        if (bearer) h.authorization = `Bearer ${bearer}`;
        if (contract) h["x-aip-contract"] = contract;
        const response = await fetch(base + path, { method: "POST", headers: h, body: JSON.stringify(body) });
        const text = await response.text();
        let json;
        try { json = JSON.parse(text); } catch { json = { notJson: text }; }
        return { status: response.status, headers: response.headers, body: json };
      },
      sql: (statement) => psql(statement.replaceAll("${S}", schema)),
      stop: () => closeFixture(owned),
    };
    return fixture;
  } catch (error) {
    await closeFixture(owned);
    throw error;
  }
}

async function serve(owned, appPath, configPath) {
  const child = spawn(binary, ["service", "serve", appPath, "--config", configPath], {
    cwd: root, env: { ...process.env, AIP_TEST_DATABASE: database }, stdio: ["ignore", "pipe", "pipe"],
  });
  owned.child = child;
  let stdout = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { owned.stderr += chunk; });
  return new Promise((resolveListening, reject) => {
    const timeout = setTimeout(() => reject(new Error(`service start timeout: ${owned.stderr}`)), 20_000);
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      const line = stdout.split(/\r?\n/u)[0];
      if (!line || !stdout.includes("\n")) return;
      clearTimeout(timeout);
      try {
        const message = JSON.parse(line);
        if (message.ok && message.event === "listening") resolveListening(message);
        else reject(new Error(`service startup rejected: ${line}\n${owned.stderr}`));
      } catch (error) { reject(error); }
    });
    child.once("exit", (code) => { clearTimeout(timeout); reject(new Error(`service exited ${code}: ${stdout}\n${owned.stderr}`)); });
  });
}

async function closeFixture(owned) {
  const child = owned.child;
  if (child && child.exitCode === null && child.signalCode === null) {
    await new Promise((done) => { child.once("exit", done); child.kill("SIGTERM"); });
  }
  owned.child = null;
  psql(`DROP SCHEMA IF EXISTS ${owned.schema} CASCADE`);
}

/** 서비스가 응답한 JSON 본문에서 비교에 쓸 최소 모양만 뽑는다. */
export function outcome(reply) {
  const b = reply.body;
  return b.ok === true ? { ok: true } : { ok: false, code: b.code };
}
