import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { loadAndValidateAppManifest, ManifestFailure } from "./app-manifest.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const hash = bytes => createHash("sha256").update(bytes).digest("hex");
const fail = (code, message, stage = "app-package") => { throw new ManifestFailure(code, message, stage); };
const portable = path => path.split(sep).join("/");

function command(program, args, stage, cwd) {
  const result = spawnSync(program, args, {
    cwd, encoding: "utf8", timeout: stage === "engine-build" ? 1_200_000 : 90_000,
    maxBuffer: 8 * 1024 * 1024,
    env: Object.fromEntries(["PATH", "HOME", "TMPDIR", "LANG", "RUSTUP_HOME", "CARGO_HOME", "CARGO_NET_OFFLINE", "CARGO_BUILD_JOBS"].filter(key => process.env[key] !== undefined).map(key => [key, process.env[key]])),
  });
  if (result.error || result.signal || result.status !== 0) {
    // Do not echo subprocess output: compiler diagnostics can include app data.
    fail("COMMAND_FAILED", `${stage} 명령이 실패했습니다 (${result.error?.code ?? result.signal ?? result.status})`, stage);
  }
  return result.stdout;
}

async function sourceSnapshot(root) {
  const files = new Map();
  const collect = async path => {
    const info = await lstat(path).catch(() => undefined);
    if (!info) return;
    if (info.isSymbolicLink()) fail("SOURCE_SYMLINK", "엔진 source 입력의 symlink는 지원하지 않습니다", "source-manifest");
    if (info.isDirectory()) {
      for (const name of (await readdir(path)).sort()) {
        if (!["target", "node_modules", "__pycache__", ".git", ".cache", "dist"].includes(name)) await collect(join(path, name));
      }
    } else if (info.isFile() && (/\.(?:rs|toml|lock|sql|ts|tsx|js|mjs|json|py|aip|yaml|service)$/u.test(path) || ["Dockerfile", "dockerignore"].includes(basename(path)))) {
      files.set(portable(relative(root, path)), hash(await readFile(path)));
    }
  };
  for (const path of ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo", "crates", "spikes", "prototype/src", "prototype/example", "product/sdk", "product/tools", "product/deploy"]) await collect(join(root, path));
  if (!files.has("Cargo.toml") || !files.has("Cargo.lock")) fail("SOURCE_MISSING", "Cargo.toml과 Cargo.lock이 필요합니다", "source-manifest");
  return files;
}

async function canonicalFuturePath(path) {
  try { return await realpath(path); } catch (error) {
    if (error.code !== "ENOENT") throw error;
    return join(await canonicalFuturePath(dirname(path)), basename(path));
  }
}

async function validateDependencies(contract, appRoot) {
  const node = contract.app.dependencies?.node;
  if (node) {
    if (!node.package_json || !node.lockfile) fail("DEPENDENCY_UNLOCKED", "Node 의존성에는 package.json과 package-lock.json이 함께 필요합니다", "dependencies");
    if (basename(node.package_json) !== "package.json" || basename(node.lockfile) !== "package-lock.json" || dirname(node.package_json) !== dirname(node.lockfile)) {
      fail("DEPENDENCY_FORMAT", "동일 디렉터리의 npm package.json/package-lock.json만 지원합니다", "dependencies");
    }
    let pkg, lock;
    try { pkg = JSON.parse(await readFile(join(appRoot, node.package_json))); lock = JSON.parse(await readFile(join(appRoot, node.lockfile))); }
    catch { fail("DEPENDENCY_FORMAT", "npm 의존성 JSON을 읽을 수 없습니다", "dependencies"); }
    if (![2, 3].includes(lock.lockfileVersion) || !lock.packages || typeof lock.packages !== "object" || Array.isArray(lock.packages)) fail("DEPENDENCY_FORMAT", "npm lockfileVersion 2 또는 3이 필요합니다", "dependencies");
    const lockedRoot = lock.packages[""] ?? {};
    for (const field of ["dependencies", "devDependencies", "optionalDependencies"]) {
      if (JSON.stringify(Object.entries(pkg[field] ?? {}).sort()) !== JSON.stringify(Object.entries(lockedRoot[field] ?? {}).sort())) fail("DEPENDENCY_UNLOCKED", "package.json과 lockfile의 직접 의존성이 다릅니다", "dependencies");
    }
    const text = JSON.stringify([pkg, lock]);
    if (/"(?:file:|link:|workspace:|\/|[A-Za-z]:\\)/u.test(text)) fail("DEPENDENCY_LOCAL_PATH", "로컬 경로 의존성은 재배치 패키지에 포함할 수 없습니다", "dependencies");
  }
  const python = contract.app.dependencies?.python;
  if (python) {
    for (const path of [python.requirements, python.lockfile].filter(Boolean)) {
      const lines = (await readFile(join(appRoot, path), "utf8")).split(/\r?\n/u).map(line => line.trim()).filter(line => line && !line.startsWith("#"));
      if (lines.some(line => !/^[A-Za-z0-9_.-]+(?:\[[A-Za-z0-9_,.-]+\])?==[A-Za-z0-9_.+!-]+(?:\s*;[^\r\n]+)?$/u.test(line))) {
        fail("DEPENDENCY_UNLOCKED", "Python은 로컬 경로 없는 name==version 형식의 고정 requirements만 지원합니다", "dependencies");
      }
    }
    if (!python.requirements && !python.lockfile) fail("DEPENDENCY_UNLOCKED", "고정 Python requirements가 필요합니다", "dependencies");
  }
}

function safeConfig(name) {
  return {
    schema: `aip_${name.replaceAll("-", "_").slice(0, 59)}`, database_url_env: "AIP_DATABASE_URL", listen: "127.0.0.1:8080", wire: "decimal",
    allowed_origins: ["https://app.example.com"],
    auth: { issuer: "https://issuer.example", audience: "aip-api", jwks: { kind: "file", path: "./jwks.json" } },
  };
}

export async function packageApplication({ manifest, out, profile = "release" }, { root = repoRoot, run = command } = {}) {
  out = resolve(out);
  const input = resolve(manifest);
  const contract = await loadAndValidateAppManifest(input);
  const appRoot = (await stat(input)).isDirectory() ? input : dirname(input);
  if (await lstat(out).catch(error => { if (error.code !== "ENOENT") throw error; return undefined; })) fail("OUTPUT_EXISTS", "새 출력 디렉터리가 필요합니다", "output");
  const appReal = await realpath(appRoot);
  const outReal = await canonicalFuturePath(out);
  const fromApp = relative(appReal, outReal);
  if (fromApp === "" || (!fromApp.startsWith(`..${sep}`) && fromApp !== ".." && !isAbsolute(fromApp))) fail("OUTPUT_IN_APP", "출력은 앱 입력 디렉터리 밖에 지정하십시오", "output");
  if (!["debug", "release"].includes(profile)) fail("BAD_ARGS", "지원하지 않는 빌드 profile", "arguments");
  const rustc = run("rustc", ["-vV"], "toolchain", root);
  const target = rustc.match(/^host: (\S+)$/mu)?.[1];
  if (!target) fail("TOOLCHAIN_TARGET", "Rust host target을 확인하지 못했습니다", "toolchain");
  if (contract.app.targets?.length && !contract.app.targets.includes(target)) fail("TARGET_MISMATCH", "현재 Rust host가 앱 targets에 없습니다. 교차 컴파일은 지원하지 않습니다", "target");
  const features = [...new Set(contract.app.features ?? [])].sort();
  const cargoManifest = await readFile(join(root, "crates/aip-cli/Cargo.toml"), "utf8");
  const featureSection = cargoManifest.match(/^\[features\]\s*\n([\s\S]*?)(?=^\[|(?![\s\S]))/mu)?.[1] ?? "";
  const allowedFeatures = [...featureSection.matchAll(/^([A-Za-z0-9_-]+)\s*=/gmu)].map(match => match[1]).filter(name => name !== "default");
  if (features.some(feature => !allowedFeatures.includes(feature))) fail("UNSUPPORTED_FEATURE", `허용 feature: ${allowedFeatures.join(", ")}`, "features");
  await validateDependencies(contract, appRoot);
  // Checksum file names must be unambiguous on both macOS and Linux.
  if (contract.files.some(file => /[\\\r\n\0]/u.test(file.path))) fail("UNSAFE_FILENAME", "이스케이프가 필요한 파일 이름은 패키징할 수 없습니다", "security");
  const sources = await sourceSnapshot(root);
  let scratch;
  let ownsOutput = false;
  try {
    await mkdir(dirname(out), { recursive: true });
    try { await mkdir(out); ownsOutput = true; }
    catch (error) { fail(error.code === "EEXIST" ? "OUTPUT_EXISTS" : "OUTPUT_IO", "출력 디렉터리를 만들 수 없습니다", "output"); }
    const cargoArgs = ["build", "--locked", "-p", "aip-cli", "--bin", "aip", "--target-dir", join(root, "target")];
    if (profile === "release") cargoArgs.push("--release");
    if (features.length) cargoArgs.push("--features", features.join(","));
    run("cargo", cargoArgs, "engine-build", root);
    const binary = join(root, "target", profile, "aip");
    const binaryDigest = hash(await readFile(binary));
    const engineVersion = run(binary, ["--version"], "engine-version", root).trim();
    run(binary, ["service", "check", join(appRoot, contract.app.definition)], "definition-check", appRoot);
    const packagedFiles = new Set();
    const write = async (name, bytes) => {
      await mkdir(dirname(join(out, name)), { recursive: true });
      await writeFile(join(out, name), bytes, { flag: "wx" });
      packagedFiles.add(name);
    };
    for (const file of contract.files) {
      const bytes = await readFile(join(appRoot, file.path));
      if (hash(bytes) !== file.sha256) fail("SOURCE_CHANGED", `앱 입력이 변경됐습니다: ${file.path}`, "source-manifest");
      if (/-----BEGIN (?:[A-Z ]*PRIVATE KEY)-----/u.test(bytes.toString("utf8")) || /[a-zA-Z0-9+.-]+:\/\/[^\s/:]+:[^\s@]+@/u.test(bytes.toString("utf8"))) fail("CREDENTIAL_LEAK", `앱 입력에 자격 증명이 있습니다: ${file.path}`, "security");
      await write(`app/${file.path}`, bytes);
    }
    // Preserve empty directories used by static mounts and worker discovery.
    for (const asset of contract.app.static_assets ?? []) {
      if ((await stat(join(appRoot, asset.path))).isDirectory()) await mkdir(join(out, "app", asset.path), { recursive: true });
    }
    scratch = await mkdtemp(join(tmpdir(), "aip-app-package-"));
    const sdk = join(scratch, "sdk");
    const tarDir = join(scratch, "pack");
    await mkdir(tarDir);
    run(process.execPath, [join(root, "product/tools/build-sdk.mjs"), "--out", sdk], "sdk-build", root);
    const packs = JSON.parse(run("npm", ["pack", "--offline", "--ignore-scripts", "--json", "--cache", join(scratch, "npm-cache"), "--pack-destination", tarDir, sdk], "sdk-pack", root));
    if (packs.length !== 1 || packs[0].name !== "@aip/sdk" || packs[0].version !== "0.1.0" || basename(packs[0].filename) !== packs[0].filename) fail("PACKAGE_IDENTITY", "SDK tarball 식별자가 다릅니다", "sdk-pack");
    await write("sdk/aip-sdk-0.1.0.tgz", await readFile(join(tarDir, packs[0].filename)));
    await write("bin/aip", await readFile(binary));
    await chmod(join(out, "bin/aip"), 0o755);
    await write("tools/app-deploy.mjs", await readFile(join(root, "product/tools/app-deploy.mjs")));
    for (const name of ["Dockerfile", "compose.yaml", "aip-app.service"]) await write(`deploy/${name}`, await readFile(join(root, "product/deploy", name)));
    await write(".dockerignore", await readFile(join(root, "product/deploy/dockerignore")));
    run(join(out, "bin/aip"), ["service", "gen", join(out, "app", contract.app.definition), "--wire", "decimal", "--out", join(out, "bindings.ts"), "--sdk-import", "@aip/sdk"], "bindings", out);
    if (!(await stat(join(out, "bindings.ts"))).isFile()) fail("BINDINGS_MISSING", "binding 산출물이 없습니다", "bindings");
    packagedFiles.add("bindings.ts");
    const finalContract = await loadAndValidateAppManifest(input);
    if (finalContract.contractDigest !== contract.contractDigest) fail("SOURCE_CHANGED", "패키징 중 앱 입력이 변경됐습니다", "source-manifest");
    const currentSources = await sourceSnapshot(root);
    if (JSON.stringify([...currentSources]) !== JSON.stringify([...sources]) || hash(await readFile(binary)) !== binaryDigest || hash(await readFile(join(out, "bin/aip"))) !== binaryDigest) fail("SOURCE_CHANGED", "패키징 중 엔진 입력이 변경됐습니다", "source-manifest");
    const manifestFile = contract.files.find(file => file.role === "manifest");
    contract.packageLayout = {
      bin: "bin/aip", definition: `app/${contract.app.definition}`, manifest: `app/${manifestFile.path}`,
      config: contract.app.config_template ? `app/${contract.app.config_template}` : "config.example.json",
      extensions: contract.app.extensions ? `app/${contract.app.extensions.directory}` : undefined,
      static: contract.app.static_assets ? "app/" : undefined,
      sdk: "sdk/aip-sdk-0.1.0.tgz", bindings: "bindings.ts", deploy: "tools/app-deploy.mjs", checksums: "SHA256SUMS", appChecksums: "APP_SHA256SUMS",
    };
    if (!contract.app.config_template) await write("config.example.json", JSON.stringify(safeConfig(contract.app.name), null, 2) + "\n");
    const json = value => JSON.stringify(value, null, 2) + "\n";
    await write("APP_CONTRACT.json", json(contract));
    let revision = null, dirty = null;
    try { revision = run("git", ["rev-parse", "HEAD"], "source-revision", root).trim(); dirty = run("git", ["status", "--porcelain"], "source-status", root).trim().length > 0; } catch { /* Git metadata is optional; source hashes remain authoritative. */ }
    await write("BUILD.json", json({ format: 1, app: { name: contract.app.name, version: contract.app.version }, target, profile, features, engine_version: engineVersion, rustc: rustc.trim(), source_revision: revision, source_dirty: dirty, contract_digest: contract.contractDigest, binary_sha256: binaryDigest, sdk: "@aip/sdk@0.1.0", dependency_mode: "locked-manifests" }));
    await write("SOURCE_SHA256SUMS", [...sources].map(([name, digest]) => `${digest}  ${name}`).join("\n") + "\n");
    await write("APP_SHA256SUMS", contract.files.map(file => `${file.sha256}  app/${file.path}`).join("\n") + "\n");
    await write("README.md", `# ${contract.app.name} ${contract.app.version}\n\n대상: ${target}, ${profile}, ${engineVersion}.\n\n원본 checkout 없이 이 디렉터리 전체를 이동하십시오.\n\n\`shasum -a 256 -c SHA256SUMS\`\n\`./bin/aip service check ${contract.packageLayout.definition}\`\n\`npm install /absolute/path/to/bundle/sdk/aip-sdk-0.1.0.tgz\`\n\nSDK는 별도 client 프로젝트 디렉터리에서 설치하십시오. 이 패키지는 불변으로 유지하며 내부에 node_modules·운영 설정·로그를 추가하지 않습니다.\n\n설정 예제: ${contract.packageLayout.config}. 비밀·JWKS는 배포 환경에서 주입합니다.\n확장 의존성은 고정 명세만 포함합니다. Node는 해당 package.json 디렉터리에서 npm ci --ignore-scripts, Python은 고정 requirements를 별도 환경에 설치하십시오. Node/Python runtime과 OS 격리 도구는 대상 서버에 필요합니다. 정적 파일은 포함만 하며 HTTP 서빙 설정은 별도입니다.\n\nSOURCE_SHA256SUMS는 source 입력 해시이며 원본 checkout에서만 검사할 수 있습니다. BUILD.json의 commit은 참고이며 서명된 provenance나 바이트 단위 재현성 보증이 아닙니다. 설치·기동·변경배포 도구: node tools/app-deploy.mjs install --out <새경로>. check/preflight/init/plan/migrate/serve/health/ready를 제공합니다. 운영 설정은 패키지 밖에 두고 --config로 지정하십시오. serve는 foreground이며 SIGTERM으로 종료합니다. migrate는 검토한 --digest를 요구합니다. 사용법: https://github.com/winterholic/Application-Intent-Protocol/blob/main/product/APP-DEPLOY.md\n`);
    const sums = [];
    for (const name of [...packagedFiles].sort()) sums.push(`${hash(await readFile(join(out, name)))}  ${name}`);
    await writeFile(join(out, "SHA256SUMS"), sums.join("\n") + "\n", { flag: "wx" });
    return { ok: true, out, app: contract.app.name, files: packagedFiles.size + 1, target, profile, manifest: "SHA256SUMS" };
  } catch (error) {
    if (ownsOutput) await rm(out, { recursive: true, force: true });
    throw error;
  } finally {
    if (scratch) await rm(scratch, { recursive: true, force: true });
  }
}
