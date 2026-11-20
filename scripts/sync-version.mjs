#!/usr/bin/env node
// Cargo.toml의 workspace 버전과 product/release.config.json의 이름을 Python 패키지 파일과 TypeScript SDK의 version.ts에 쓴다.
// 버전의 원천은 Cargo.toml 하나이므로 이 스크립트는 그 값을 복사만 한다. 일치 검사는 check-versions.mjs가 한다.
import { copyFileSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { protocolVersion, pythonVersion, releaseConfig, repoRoot, workspaceVersion } from "../product/tools/version.mjs";

const version = workspaceVersion();
const config = releaseConfig();
const protocol = protocolVersion();

function replaceOnce(text, pattern, replacement, label) {
  if (!pattern.test(text)) throw new Error(`${label}를 찾지 못했습니다`);
  return text.replace(pattern, replacement);
}

const pyproject = resolve(repoRoot, "product/python/pyproject.toml");
let toml = readFileSync(pyproject, "utf8");
toml = replaceOnce(toml, /^name = "[^"]*"$/mu, `name = "${config.pypiDistribution}"`, "pyproject name");
toml = replaceOnce(toml, /^version = "[^"]*"$/mu, `version = "${pythonVersion(version)}"`, "pyproject version");
writeFileSync(pyproject, toml);

const init = resolve(repoRoot, `product/python/${config.pythonImport}/__init__.py`);
let source = readFileSync(init, "utf8");
source = replaceOnce(source, /^__version__ = "[^"]*"$/mu, `__version__ = "${pythonVersion(version)}"`, "__version__");
source = replaceOnce(source, /^PROTOCOL = "[^"]*"$/mu, `PROTOCOL = "${protocol}"`, "PROTOCOL");
writeFileSync(init, source);

const sdkVersionFile = resolve(repoRoot, "product/sdk/version.ts");
writeFileSync(sdkVersionFile, `// Written by scripts/sync-version.mjs from Cargo.toml and crates/aip-contract. Do not edit by hand;
// scripts/check-versions.mjs fails when these values drift from their sources.
export const SDK_VERSION = "${version}";
export const SDK_PROTOCOL = "${protocol}";
export const SDK_IDENTITY = \`typescript/\${SDK_VERSION}\`;
`);

copyFileSync(resolve(repoRoot, "LICENSE"), resolve(repoRoot, "product/python/LICENSE"));
console.log(JSON.stringify({ ok: true, version, pythonVersion: pythonVersion(version), protocol, npm: config.npmPackage, pypi: config.pypiDistribution }));
