import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, copyFile, lstat, mkdir, readFile, readdir, realpath, rm } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// This module is shipped verbatim in app packages. No source checkout or npm dependency is required.
class DeployFailure extends Error {
  constructor(code, message) { super(message); this.code = code; }
}
const fail = (code, message) => { throw new DeployFailure(code, message); };
const digest = bytes => createHash("sha256").update(bytes).digest("hex");
const within = (root, path) => { const rel = relative(root, path); return rel === "" || (!rel.startsWith("../") && rel !== ".." && !isAbsolute(rel)); };
function safePath(root, name) {
  if (typeof name !== "string" || !name || /[\\\r\n\0]/u.test(name) || isAbsolute(name) || name.split("/").some(part => !part || part === "." || part === "..")) fail("PACKAGE_PATH", "패키지 상대 경로가 올바르지 않습니다");
  return join(root, name);
}
async function json(path) {
  try { return JSON.parse(await readFile(path, "utf8")); }
  catch { fail("BAD_JSON", "필수 JSON 파일을 읽을 수 없습니다"); }
}
async function inventory(root, prefix = "") {
  const result = [];
  for (const name of (await readdir(join(root, prefix))).sort()) {
    const rel = prefix ? `${prefix}/${name}` : name;
    const path = safePath(root, rel);
    const info = await lstat(path);
    if (info.isSymbolicLink() || (!info.isDirectory() && !info.isFile())) fail("PACKAGE_FILE_TYPE", "패키지에 symlink 또는 특수 파일이 있습니다");
    if (info.isDirectory()) result.push(...await inventory(root, rel));
    else result.push(rel);
  }
  return result;
}

export async function verifyPackage(input) {
  const root = await realpath(resolve(input));
  const files = await inventory(root);
  const text = await readFile(join(root, "SHA256SUMS"), "utf8").catch(() => fail("CHECKSUM_MISSING", "SHA256SUMS가 필요합니다"));
  const sums = new Map();
  for (const line of text.trimEnd().split("\n")) {
    const match = /^([a-f0-9]{64})  (.+)$/u.exec(line);
    if (!match || match[2] === "SHA256SUMS" || sums.has(match[2])) fail("CHECKSUM_FORMAT", "체크섬 목록이 올바르지 않습니다");
    const path = safePath(root, match[2]);
    if (!files.includes(match[2]) || digest(await readFile(path)) !== match[1]) fail("CHECKSUM_MISMATCH", "패키지 파일이 누락되거나 변경됐습니다");
    sums.set(match[2], match[1]);
  }
  if (files.some(name => name !== "SHA256SUMS" && !sums.has(name))) fail("UNLISTED_FILE", "체크섬에 없는 파일이 있습니다");
  for (const name of ["BUILD.json", "APP_CONTRACT.json"]) if (!sums.has(name)) fail("PACKAGE_METADATA", "패키지 메타데이터가 필요합니다");
  const build = await json(join(root, "BUILD.json"));
  const contract = await json(join(root, "APP_CONTRACT.json"));
  const targets = { "darwin-arm64": "aarch64-apple-darwin", "darwin-x64": "x86_64-apple-darwin", "linux-x64": "x86_64-unknown-linux-gnu", "linux-arm64": "aarch64-unknown-linux-gnu" };
  if (!targets[`${process.platform}-${process.arch}`] || build.target !== targets[`${process.platform}-${process.arch}`]) fail("TARGET_MISMATCH", "패키지 OS/CPU가 현재 서버와 다릅니다");
  if (build.format !== 1 || !contract.app || build.app?.name !== contract.app.name || build.app?.version !== contract.app.version || build.contract_digest !== contract.contractDigest) fail("PACKAGE_METADATA", "빌드와 앱 계약이 일치하지 않습니다");
  for (const key of ["bin", "definition", "manifest"]) {
    const name = contract.packageLayout?.[key]; safePath(root, name);
    if (!sums.has(name)) fail("PACKAGE_METADATA", "필수 산출물이 체크섬에 없습니다");
  }
  if (contract.packageLayout.bin !== "bin/aip" || contract.packageLayout.definition !== `app/${contract.app.definition}`) fail("PACKAGE_METADATA", "실행 파일 또는 정의 layout이 올바르지 않습니다");
  return { root, contract, build, files, sums };
}

export async function installPackage(input, output) {
  const requested = resolve(output);
  if (await lstat(requested).catch(e => { if (e.code !== "ENOENT") throw e; return undefined; })) fail("OUTPUT_EXISTS", "새 설치 디렉터리가 필요합니다");
  const canonicalFuture = async path => {
    try { return await realpath(path); } catch (error) {
      if (error.code !== "ENOENT") throw error;
      return join(await canonicalFuture(dirname(path)), basename(path));
    }
  };
  const out = await canonicalFuture(requested);
  const source = await verifyPackage(input);
  // mkdir without recursive preserves ownership: never remove a pre-existing destination.
  if (within(source.root, out)) fail("OUTPUT_IN_PACKAGE", "설치는 원본 패키지 밖에 지정하십시오");
  await mkdir(dirname(out), { recursive: true });
  await mkdir(out);
  try {
    // Recreate empty asset directories as well as regular files; never follow links.
    const copy = async (prefix = "") => {
      for (const name of await readdir(join(source.root, prefix))) {
        const rel = prefix ? `${prefix}/${name}` : name;
        const src = safePath(source.root, rel); const dst = safePath(out, rel); const info = await lstat(src);
        if (info.isDirectory()) { await mkdir(dst); await copy(rel); }
        else if (info.isFile() && source.files.includes(rel)) { await copyFile(src, dst); await chmod(dst, rel === "bin/aip" ? 0o755 : 0o644); }
        else fail("PACKAGE_CHANGED", "복사 중 패키지 입력이 변경됐습니다");
      }
    };
    await copy();
    const installed = await verifyPackage(out);
    if (JSON.stringify([...installed.sums]) !== JSON.stringify([...source.sums])) fail("PACKAGE_CHANGED", "설치 중 패키지가 변경됐습니다");
    return { ok: true, out, app: installed.contract.app.name, version: installed.contract.app.version };
  } catch (error) { await rm(out, { recursive: true, force: true }); throw error; }
}

function noSecrets(value) {
  if (typeof value === "string" && (/-----BEGIN [A-Z ]*PRIVATE KEY-----/u.test(value) || /[a-zA-Z0-9+.-]+:\/\/[^\s/:]+:[^\s@]+@/u.test(value))) fail("CONFIG_SECRET", "설정의 비밀은 환경 변수로 주입하십시오");
  if (value && typeof value === "object") for (const [key, item] of Object.entries(value)) {
    if (/^(password|secret|token|credential|api_key)$/iu.test(key) && item) fail("CONFIG_SECRET", "설정의 비밀은 환경 변수로 주입하십시오");
    noSecrets(item);
  }
}
function listenAddress(listen) {
  const match = /^(127\.0\.0\.1|\[::1\]):([0-9]+)$/u.exec(listen ?? "");
  if (!match || Number(match[2]) < 1 || Number(match[2]) > 65535) fail("BAD_LISTEN", "명시적인 loopback 주소와 1~65535 port가 필요합니다");
  return `http://${listen}`;
}
function requireEnv(config, migration, environment) {
  const name = migration ? config.migrate_database_url_env ?? config.database_url_env : config.database_url_env;
  if (typeof name !== "string" || !/^[A-Za-z_][A-Za-z0-9_]*$/u.test(name)) fail("BAD_CONFIG", "DB 환경 변수 이름을 명시하십시오");
  if (!environment[name]?.trim()) fail("ENV_MISSING", "필수 DB 환경 변수가 없습니다 (값은 출력하지 않음)");
}
function executable(program, args) {
  const result = spawnSync(program, args, { stdio: "ignore", timeout: 10_000 });
  if (result.error || result.signal || result.status !== 0) fail("RUNTIME_MISSING", "필수 runtime 또는 격리 도구의 사전 검사 실패");
}

export async function prepareCommand({ root: input, action, config: configInput, digest: acknowledged, migration }, environment = process.env) {
  const pkg = await verifyPackage(input);
  if (!["check", "preflight", "init", "plan", "migrate", "serve", "health", "ready"].includes(action)) fail("BAD_ARGS", "지원하지 않는 배포 동작입니다");
  const binary = safePath(pkg.root, pkg.contract.packageLayout.bin);
  const definition = safePath(pkg.root, pkg.contract.packageLayout.definition);
  if (action === "check") return { ...pkg, binary, args: ["service", "check", definition] };
  if (!configInput) fail("CONFIG_REQUIRED", "외부 운영 설정 파일을 명시하십시오");
  const configPath = await realpath(resolve(configInput));
  if (within(pkg.root, configPath)) fail("CONFIG_IN_PACKAGE", "운영 설정은 불변 패키지 밖에 두십시오");
  const config = await json(configPath); noSecrets(config);
  if (typeof config.schema !== "string" || !/^aip_[a-z0-9_]{1,59}$/u.test(config.schema)) fail("BAD_SCHEMA", "명시적인 aip_ schema가 필요합니다");
  const endpoint = listenAddress(config.listen);
  if (["health", "ready"].includes(action)) return { ...pkg, endpoint: `${endpoint}/${action}` };
  requireEnv(config, ["init", "plan", "migrate"].includes(action), environment);
  const extensions = pkg.contract.app.extensions;
  if (extensions) {
    const worker = config.workers;
    if (!worker || worker.language !== extensions.language || Boolean(worker.enable_writes) !== Boolean(extensions.enable_writes)) fail("WORKER_CONFIG", "worker 언어와 쓰기 권한은 앱 manifest와 일치해야 합니다");
    const expected = safePath(pkg.root, `app/${extensions.directory}`);
    if (await realpath(resolve(dirname(configPath), worker.directory)) !== await realpath(expected)) fail("WORKER_CONFIG", "worker 디렉터리가 이 패키지의 확장 경로와 다릅니다");
    const isolation = worker.isolation ?? "auto";
    if (!(process.platform === "linux" ? ["auto", "linux_bubblewrap"] : ["auto", "mac_sandbox"]).includes(isolation)) fail("WORKER_ISOLATION", "이 배포 도구는 완전한 OS worker 격리를 요구합니다");
    if (["preflight", "serve"].includes(action)) {
      executable(worker.runtime_executable ?? (extensions.language === "node" ? "node" : "python3"), ["--version"]);
      // Availability only; the engine performs the actual isolation policy validation when serving.
      executable(process.platform === "linux" ? "bwrap" : "/bin/sh", process.platform === "linux" ? ["--version"] : ["-c", "test -x /usr/bin/sandbox-exec"]);
    }
  } else if (config.workers) fail("WORKER_CONFIG", "앱 manifest에 없는 worker는 활성화할 수 없습니다");
  if (action === "migrate" && !/^[a-f0-9]{64}$/u.test(acknowledged ?? "")) fail("DIGEST_REQUIRED", "plan에서 확인한 64자리 digest를 --digest로 명시하십시오");
  const args = ["service", ["plan", "migrate"].includes(action) ? "migrate" : action === "preflight" ? "check" : action, definition];
  if (action !== "preflight") args.push("--config", configPath);
  if (migration) {
    if (!["plan", "migrate"].includes(action)) fail("BAD_ARGS", "--migration은 plan/migrate에만 사용할 수 있습니다");
    args.push("--migration", await realpath(resolve(migration)));
  }
  if (action === "migrate") args.push("--apply", acknowledged);
  return { ...pkg, binary, args, endpoint };
}

export async function probe(endpoint) {
  const url = new URL(endpoint);
  if (url.protocol !== "http:" || !["127.0.0.1", "[::1]"].includes(url.hostname) || !["/health", "/ready"].includes(url.pathname) || url.username || url.password) fail("BAD_ENDPOINT", "probe는 loopback health/ready만 허용합니다");
  try {
    const response = await fetch(url, { redirect: "manual", signal: AbortSignal.timeout(3000) });
    await response.body?.cancel();
    if (response.status !== 200) fail("NOT_READY", "서비스 준비상태 검사가 실패했습니다");
    return { ok: true, check: url.pathname.slice(1) };
  } catch { fail("NOT_READY", "서비스 준비상태 검사가 실패했습니다"); }
}

export async function runDeployment(options) {
  const prepared = await prepareCommand(options);
  if (["health", "ready"].includes(options.action)) return probe(prepared.endpoint);
  if (options.action !== "serve") {
    const result = spawnSync(prepared.binary, prepared.args, { cwd: prepared.root, encoding: "utf8", timeout: 120_000, maxBuffer: 8 * 1024 * 1024 });
    if (result.error || result.signal || result.status !== 0) fail("SERVICE_FAILED", "제품 명령 실패: DB 권한·정의·migration fence를 확인하십시오 (원문 오류는 비밀 보호를 위해 생략)");
    // Successful plan output includes the engine's own digest; never synthesize or auto-acknowledge it.
    return { ok: true, action: options.action, output: result.stdout };
  }
  // Foreground supervision: service managers own stop/restart. Forward signals and wait for engine shutdown.
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(prepared.binary, prepared.args, { cwd: prepared.root, stdio: "inherit" });
    const forwardTerm = () => child.kill("SIGTERM"); const forwardInt = () => child.kill("SIGINT");
    process.on("SIGTERM", forwardTerm); process.on("SIGINT", forwardInt);
    const clean = () => { process.off("SIGTERM", forwardTerm); process.off("SIGINT", forwardInt); };
    child.once("error", () => { clean(); rejectRun(new DeployFailure("SERVICE_START", "제품 프로세스를 시작할 수 없습니다")); });
    child.once("exit", (code, signal) => { clean(); if (code === 0 || signal === "SIGTERM" || signal === "SIGINT") resolveRun({ ok: true, action: "serve", stopped: true }); else rejectRun(new DeployFailure("SERVICE_EXIT", "제품 프로세스가 실패했습니다")); });
  });
}

function argumentsFor(args) {
  const action = args.shift(); const options = { action };
  const keys = { "--package": "root", "--out": "out", "--config": "config", "--digest": "digest", "--migration": "migration" };
  while (args.length) { const key = keys[args.shift()]; if (!key || options[key] !== undefined || !args[0] || args[0].startsWith("--")) fail("BAD_ARGS", "잘못된 배포 인자입니다"); options[key] = args.shift(); }
  if (!options.root) options.root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  if (action === "install" && !options.out) fail("BAD_ARGS", "install --package <bundle> --out <fresh-directory>");
  if (!action || (action !== "install" && options.out)) fail("BAD_ARGS", "install|check|preflight|init|plan|migrate|serve|health|ready --package <bundle> [--config <external.json>] [--digest <plan-digest>]");
  return options;
}
if (process.argv[1] && await realpath(resolve(process.argv[1])).catch(() => "") === fileURLToPath(import.meta.url)) {
  try {
    const options = argumentsFor(process.argv.slice(2));
    console.log(JSON.stringify(options.action === "install" ? await installPackage(options.root, options.out) : await runDeployment(options)));
  } catch (error) { console.error(JSON.stringify({ ok: false, code: error instanceof DeployFailure ? error.code : "DEPLOY_IO", message: error instanceof DeployFailure ? error.message : "배포 입출력 실패 (민감한 값은 출력하지 않음)" })); process.exitCode = 1; }
}
