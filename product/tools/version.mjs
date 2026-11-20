// 버전과 패키지 이름의 단일 원천. 버전은 루트 Cargo.toml의 [workspace.package] version 하나에서만 읽는다.
// 이름은 product/release.config.json 한 곳에서 읽고, 환경 변수 AIP_NPM_PACKAGE, AIP_PYPI_PACKAGE로 덮어쓸 수 있다.
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-rc\.([1-9]\d*))?$/u;
const NPM_NAME = /^(?:@[a-z0-9][a-z0-9._~-]*\/)?[a-z0-9][a-z0-9._~-]*$/u;
const PYPI_NAME = /^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$/u;
const PY_IMPORT = /^[A-Za-z_][A-Za-z0-9_]*$/u;

export function workspaceVersion(root = repoRoot) {
  const text = readFileSync(resolve(root, "Cargo.toml"), "utf8");
  const section = /^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/mu.exec(text);
  const match = section && /^version\s*=\s*"([^"]+)"\s*$/mu.exec(section[1]);
  if (!match) throw new Error("Cargo.toml [workspace.package]에서 version을 읽지 못했습니다");
  if (!SEMVER.test(match[1])) throw new Error(`지원하지 않는 버전 형식입니다: ${match[1]} (X.Y.Z 또는 X.Y.Z-rc.N)`);
  return match[1];
}

/** SemVer `1.0.0-rc.1` → PEP 440 `1.0.0rc1`. 정식 버전은 그대로다. */
export function pythonVersion(version) {
  const match = SEMVER.exec(version);
  if (!match) throw new Error(`지원하지 않는 버전 형식입니다: ${version}`);
  return match[4] ? `${match[1]}.${match[2]}.${match[3]}rc${match[4]}` : `${match[1]}.${match[2]}.${match[3]}`;
}

/** rc는 npm `next` 태그로, 정식은 `latest`로 올린다. */
export function npmDistTag(version) {
  return version.includes("-rc.") ? "next" : "latest";
}

export function releaseConfig(env = process.env, root = repoRoot) {
  const file = JSON.parse(readFileSync(resolve(root, "product/release.config.json"), "utf8"));
  const config = {
    npmPackage: env.AIP_NPM_PACKAGE || file.npmPackage,
    pypiDistribution: env.AIP_PYPI_PACKAGE || file.pypiDistribution,
    pythonImport: file.pythonImport,
    repository: file.repository,
    namesApprovedByOwner: file.namesApprovedByOwner === true && !env.AIP_NPM_PACKAGE && !env.AIP_PYPI_PACKAGE,
  };
  if (!NPM_NAME.test(config.npmPackage ?? "")) throw new Error(`npm 패키지 이름이 올바르지 않습니다: ${config.npmPackage}`);
  if (!PYPI_NAME.test(config.pypiDistribution ?? "")) throw new Error(`PyPI 배포 이름이 올바르지 않습니다: ${config.pypiDistribution}`);
  if (!PY_IMPORT.test(config.pythonImport ?? "")) throw new Error(`Python import 이름이 올바르지 않습니다: ${config.pythonImport}`);
  if (!/^https:\/\/github\.com\/[^/]+\/[^/]+$/u.test(config.repository ?? "")) throw new Error("repository는 https://github.com/<owner>/<repo> 형식이어야 합니다");
  return config;
}

/** `npm pack`이 만드는 파일 이름. `@scope/name` → `scope-name-<version>.tgz`. */
export function npmTarballName(name = releaseConfig().npmPackage, version = workspaceVersion()) {
  return `${name.replace(/^@/u, "").replace("/", "-")}-${version}.tgz`;
}

/** PEP 427 wheel 파일 이름 규칙: 배포 이름의 구분자 `-._` 연속을 `_` 하나로 바꾼다. */
export function wheelFileName(distribution = releaseConfig().pypiDistribution, version = workspaceVersion()) {
  return `${distribution.replace(/[-_.]+/gu, "_").toLowerCase()}-${pythonVersion(version)}-py3-none-any.whl`;
}

/** crates/aip-contract의 PROTOCOL_VERSION. 호환 경계는 이 문자열의 major다. */
export function protocolVersion(root = repoRoot) {
  const text = readFileSync(resolve(root, "crates/aip-contract/src/lib.rs"), "utf8");
  const match = /pub const PROTOCOL_VERSION: &str = "(aip-protocol\/\d+\.\d+)";/u.exec(text);
  if (!match) throw new Error("PROTOCOL_VERSION을 읽지 못했습니다");
  return match[1];
}

/** `aip-protocol/1.0` → major 1. */
export function protocolMajor(protocol) {
  const match = /^aip-protocol\/(\d+)\.\d+$/u.exec(protocol);
  if (!match) throw new Error(`protocol 표지 형식이 올바르지 않습니다: ${protocol}`);
  return Number(match[1]);
}

/** build-sdk.mjs가 dist 디렉터리에 쓰는 package.json. publish 가능한 형태다. */
export function sdkPackageManifest(config = releaseConfig(), version = workspaceVersion(), protocol = protocolVersion()) {
  return {
    name: config.npmPackage,
    version,
    description: "TypeScript caller SDK for Application Intent Protocol services",
    license: "MIT",
    type: "module",
    types: "./product/sdk/index.d.ts",
    exports: { ".": { types: "./product/sdk/index.d.ts", import: "./product/sdk/index.js" }, "./package.json": "./package.json" },
    files: ["product/sdk", "README.md", "LICENSE"],
    sideEffects: false,
    repository: { type: "git", url: `git+${config.repository}.git` },
    homepage: `${config.repository}#readme`,
    bugs: { url: `${config.repository}/issues` },
    keywords: ["aip", "application-intent-protocol", "sdk"],
    publishConfig: { access: "public" },
    aip: { protocol },
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const queries = {
    version: () => workspaceVersion(),
    "python-version": () => pythonVersion(workspaceVersion()),
    "npm-dist-tag": () => npmDistTag(workspaceVersion()),
    "npm-package": () => releaseConfig().npmPackage,
    "pypi-distribution": () => releaseConfig().pypiDistribution,
    "npm-tarball": () => npmTarballName(),
    "wheel-file": () => wheelFileName(),
    protocol: () => protocolVersion(),
  };
  const query = queries[process.argv[2]];
  if (!query) {
    console.error(`usage: node product/tools/version.mjs <${Object.keys(queries).join("|")}>`);
    process.exitCode = 2;
  } else {
    try {
      console.log(query());
    } catch (error) {
      console.error(error.message);
      process.exitCode = 1;
    }
  }
}
