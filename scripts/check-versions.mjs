#!/usr/bin/env node
// 버전과 패키지 이름이 한 원천(Cargo.toml workspace 버전, product/release.config.json)과 일치하는지 검사한다.
// 하나라도 어긋나면 종료 코드 1로 실패한다.
// 사용: node scripts/check-versions.mjs [--tag vX.Y.Z[-rc.N]] [--npm-tarball <file>] [--wheel <file>] [--sdist <file>]
import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { resolve } from "node:path";
import {
  npmTarballName, protocolVersion, pythonVersion, releaseConfig, repoRoot, sdkPackageManifest, workspaceVersion,
} from "../product/tools/version.mjs";

const problems = [];
const fail = (message) => problems.push(message);
const read = (path) => readFileSync(resolve(repoRoot, path), "utf8");

function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!["--tag", "--npm-tarball", "--wheel", "--sdist"].includes(key) || !value) {
      console.error("usage: node scripts/check-versions.mjs [--tag vX.Y.Z] [--npm-tarball <file>] [--wheel <file>] [--sdist <file>]");
      process.exit(2);
    }
    options[key.slice(2)] = value;
  }
  return options;
}

function section(text, name) {
  const match = new RegExp(`^\\[${name.replace(/\./gu, "\\.")}\\]\\s*$([\\s\\S]*?)(?=^\\[|(?![\\s\\S]))`, "mu").exec(text);
  return match ? match[1] : "";
}

function tomlString(body, key) {
  return new RegExp(`^${key}\\s*=\\s*"([^"]*)"\\s*$`, "mu").exec(body)?.[1];
}

function sh(program, args, input) {
  const result = spawnSync(program, args, { encoding: "utf8", input, maxBuffer: 16 * 1024 * 1024 });
  if (result.error || result.status !== 0) throw new Error(`${program} 실행 실패: ${result.error?.message ?? result.stderr}`);
  return result.stdout;
}

const options = parseArgs(process.argv.slice(2));
const version = workspaceVersion();
const pyVersion = pythonVersion(version);
const config = releaseConfig();
const protocol = protocolVersion();
const coreIr = /pub const CORE_IR_VERSION: &str = "([^"]+)";/u.exec(read("crates/aip-ir/src/lib.rs"))?.[1];

// 1. 모든 workspace crate가 workspace 버전을 쓰고, 모든 Cargo.lock(루트, prototype, spikes)이 같은 버전이다.
// spikes와 prototype은 workspace 밖이라 자기 Cargo.lock을 가지며 CI가 --locked로 빌드한다.
const workspaceCrates = [];
for (const entry of readdirSync(resolve(repoRoot, "crates"), { withFileTypes: true })) {
  if (!entry.isDirectory() || !existsSync(resolve(repoRoot, "crates", entry.name, "Cargo.toml"))) continue;
  const pkg = section(read(`crates/${entry.name}/Cargo.toml`), "package");
  if (!/^version\.workspace\s*=\s*true\s*$/mu.test(pkg)) fail(`crates/${entry.name}/Cargo.toml이 version.workspace = true를 쓰지 않습니다`);
  workspaceCrates.push(tomlString(pkg, "name"));
}
const lockFiles = ["Cargo.lock", "prototype/Cargo.lock"];
for (const entry of readdirSync(resolve(repoRoot, "spikes"), { withFileTypes: true })) {
  if (entry.isDirectory() && existsSync(resolve(repoRoot, "spikes", entry.name, "Cargo.lock"))) lockFiles.push(`spikes/${entry.name}/Cargo.lock`);
}
for (const file of lockFiles) {
  // 경로 crate는 source 줄이 없다. 같은 이름의 registry crate와 구분하려고 source가 없는 항목만 본다.
  const packages = [...read(file).matchAll(/^\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"\n(?!source)/gmu)];
  for (const [, name, locked] of packages) {
    if (workspaceCrates.includes(name) && locked !== version) fail(`${file}의 ${name}은 ${locked}인데 workspace 버전은 ${version}입니다(cargo update --workspace --manifest-path로 갱신)`);
  }
}

// 2. Python 패키지: 배포 이름, 버전, import 모듈, LICENSE.
const pyproject = read("product/python/pyproject.toml");
const project = section(pyproject, "project");
if (tomlString(project, "name") !== config.pypiDistribution) fail(`pyproject name ${tomlString(project, "name")}은 설정의 ${config.pypiDistribution}과 다릅니다`);
if (tomlString(project, "version") !== pyVersion) fail(`pyproject version ${tomlString(project, "version")}은 workspace 버전에서 파생한 ${pyVersion}과 다릅니다`);
const init = read(`product/python/${config.pythonImport}/__init__.py`);
const initVersion = /^__version__ = "([^"]*)"$/mu.exec(init)?.[1];
if (initVersion !== pyVersion) fail(`${config.pythonImport}.__version__ ${initVersion}은 ${pyVersion}과 다릅니다`);
const initProtocol = /^PROTOCOL = "([^"]*)"$/mu.exec(init)?.[1];
if (initProtocol !== protocol) fail(`${config.pythonImport}.PROTOCOL ${initProtocol}은 PROTOCOL_VERSION ${protocol}과 다릅니다`);
if (read("product/python/LICENSE") !== read("LICENSE")) fail("product/python/LICENSE가 루트 LICENSE와 다릅니다(node scripts/sync-version.mjs로 복사)");
if (!/^readme\s*=\s*"README\.md"/mu.test(project) || !existsSync(resolve(repoRoot, "product/python/README.md"))) fail("Python 패키지에 README.md가 필요합니다");

// 3a. TS SDK가 자기 버전으로 노출하고 x-aip-sdk 헤더로 보내는 상수.
const sdkVersionSource = read("product/sdk/version.ts");
const tsVersion = /^export const SDK_VERSION = "([^"]*)";$/mu.exec(sdkVersionSource)?.[1];
const tsProtocol = /^export const SDK_PROTOCOL = "([^"]*)";$/mu.exec(sdkVersionSource)?.[1];
if (tsVersion !== version) fail(`product/sdk/version.ts SDK_VERSION ${tsVersion}은 workspace 버전 ${version}과 다릅니다(node scripts/sync-version.mjs)`);
if (tsProtocol !== protocol) fail(`product/sdk/version.ts SDK_PROTOCOL ${tsProtocol}은 PROTOCOL_VERSION ${protocol}과 다릅니다`);

// 3. TS 패키지 매니페스트가 같은 값에서 만들어진다.
const manifest = sdkPackageManifest(config, version, protocol);
if (manifest.name !== config.npmPackage || manifest.version !== version) fail("SDK package.json 생성 값이 원천과 다릅니다");
if (manifest.private) fail("SDK package.json이 private입니다");

// 4. 호환 표(COMPATIBILITY.md)에 이 버전의 행이 있고 protocol, Core IR이 맞다.
const compatibility = read("COMPATIBILITY.md");
const row = compatibility.split("\n").find((line) => line.startsWith(`| ${version} |`));
if (!row) fail(`COMPATIBILITY.md에 ${version} 행이 없습니다`);
else {
  if (!row.includes(`\`${protocol}\``)) fail(`COMPATIBILITY.md의 ${version} 행에 ${protocol}이 없습니다`);
  if (coreIr && !row.includes(`\`${coreIr}\``)) fail(`COMPATIBILITY.md의 ${version} 행에 ${coreIr}이 없습니다`);
}

// 5. 태그.
if (options.tag !== undefined) {
  if (options.tag !== `v${version}`) fail(`태그 ${options.tag}는 workspace 버전 v${version}과 다릅니다`);
}

// 6. 산출물(선택).
if (options["npm-tarball"]) {
  const file = options["npm-tarball"];
  try {
    const packed = JSON.parse(sh("tar", ["-xOf", file, "package/package.json"]));
    if (packed.name !== config.npmPackage) fail(`tarball 이름 ${packed.name}은 ${config.npmPackage}과 다릅니다`);
    if (packed.version !== version) fail(`tarball 버전 ${packed.version}은 ${version}과 다릅니다`);
    if (packed.private) fail("tarball package.json이 private입니다");
    if (packed.aip?.protocol !== protocol) fail(`tarball의 aip.protocol ${packed.aip?.protocol}은 ${protocol}과 다릅니다`);
    if (!file.endsWith(npmTarballName(config.npmPackage, version))) fail(`tarball 파일 이름이 ${npmTarballName(config.npmPackage, version)}이 아닙니다`);
    const listing = sh("tar", ["-tzf", file]).split("\n").filter(Boolean);
    if (listing.some((name) => name.includes("spike"))) fail("tarball에 spikes 내부 경로가 있습니다");
    for (const required of ["package/LICENSE", "package/README.md"]) if (!listing.includes(required)) fail(`tarball에 ${required}가 없습니다`);
  } catch (error) {
    fail(`npm tarball 검사 실패: ${error.message}`);
  }
}
if (options.wheel) {
  try {
    const members = sh("unzip", ["-Z1", options.wheel]).split("\n").filter(Boolean);
    const metadataName = members.find((name) => /\.dist-info\/METADATA$/u.test(name));
    const metadata = sh("unzip", ["-p", options.wheel, metadataName]);
    if (!new RegExp(`^Name: ${config.pypiDistribution}$`, "mu").test(metadata)) fail(`wheel METADATA Name이 ${config.pypiDistribution}이 아닙니다`);
    if (!new RegExp(`^Version: ${pyVersion.replace(/\./gu, "\\.")}$`, "mu").test(metadata)) fail(`wheel METADATA Version이 ${pyVersion}이 아닙니다`);
    if (!members.some((name) => /\.dist-info\/licenses\/LICENSE$/u.test(name))) fail("wheel에 LICENSE가 없습니다");
    if (!/^Description-Content-Type:/mu.test(metadata) || !metadata.includes("# AIP Python SDK")) fail("wheel METADATA에 README(long_description)가 없습니다");
    if (!members.includes(`${config.pythonImport}/py.typed`)) fail("wheel에 py.typed가 없습니다");
  } catch (error) {
    fail(`wheel 검사 실패: ${error.message}`);
  }
}
if (options.sdist) {
  try {
    const listing = sh("tar", ["-tzf", options.sdist]).split("\n").filter(Boolean);
    for (const required of ["LICENSE", "README.md", "pyproject.toml"]) {
      if (!listing.some((name) => name.endsWith(`/${required}`) && name.split("/").length === 2)) fail(`sdist에 ${required}가 없습니다`);
    }
  } catch (error) {
    fail(`sdist 검사 실패: ${error.message}`);
  }
}

const summary = { version, pythonVersion: pyVersion, npm: config.npmPackage, pypi: config.pypiDistribution, protocol, coreIr, tag: options.tag ?? null };
if (problems.length > 0) {
  console.error(JSON.stringify({ ok: false, ...summary, problems }, null, 2));
  process.exit(1);
}
console.log(JSON.stringify({ ok: true, ...summary }));
