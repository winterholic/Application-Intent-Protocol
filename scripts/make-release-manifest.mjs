#!/usr/bin/env node
// dist 디렉터리의 산출물로 SHA256SUMS와 aip-<version>-manifest.json을 만든다.
// 사용: node scripts/make-release-manifest.mjs <dist-dir> [--tag vX.Y.Z] [--git-sha <sha>]
import { createHash } from "node:crypto";
import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { protocolVersion, repoRoot, workspaceVersion } from "../product/tools/version.mjs";

const TARGETS = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-apple-darwin"];

export function describeFile(name) {
  const archive = /^aip-.+-(x86_64|aarch64)-(unknown-linux-gnu|apple-darwin)\.tar\.gz$/u.exec(name);
  if (archive) return { kind: "cli-archive", target: `${archive[1]}-${archive[2]}` };
  if (name.endsWith(".tgz")) return { kind: "npm-tarball", target: null };
  if (name.endsWith(".whl")) return { kind: "python-wheel", target: null };
  if (name.endsWith(".tar.gz")) return { kind: "python-sdist", target: null };
  return { kind: "other", target: null };
}

export function buildManifest(dist, { tag = null, gitSha = null } = {}) {
  const version = workspaceVersion();
  const coreIr = /pub const CORE_IR_VERSION: &str = "([^"]+)";/u.exec(readFileSync(resolve(repoRoot, "crates/aip-ir/src/lib.rs"), "utf8"))?.[1];
  const names = readdirSync(dist).filter((name) => !["SHA256SUMS"].includes(name) && !/-manifest\.json$/u.test(name) && statSync(join(dist, name)).isFile()).sort();
  const files = names.map((name) => {
    const bytes = readFileSync(join(dist, name));
    return { name, sha256: createHash("sha256").update(bytes).digest("hex"), size: bytes.length, ...describeFile(name) };
  });
  const missing = TARGETS.filter((target) => !files.some((file) => file.target === target));
  if (missing.length > 0) throw new Error(`CLI 아카이브가 없는 target: ${missing.join(", ")}`);
  for (const kind of ["npm-tarball", "python-wheel", "python-sdist"]) {
    if (files.filter((file) => file.kind === kind).length !== 1) throw new Error(`${kind} 산출물이 정확히 하나여야 합니다`);
  }
  return { version, git_sha: gitSha, tag, protocol: protocolVersion(), core_ir: coreIr, files };
}

export function sha256sums(manifest) {
  return manifest.files.map((file) => `${file.sha256}  ${file.name}`).join("\n") + "\n";
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(new URL(import.meta.url).pathname)) {
  const args = process.argv.slice(2);
  const dist = args[0] && !args[0].startsWith("--") ? resolve(args[0]) : undefined;
  const option = (key) => { const index = args.indexOf(key); return index >= 0 ? args[index + 1] : undefined; };
  if (!dist) {
    console.error("usage: node scripts/make-release-manifest.mjs <dist-dir> [--tag vX.Y.Z] [--git-sha <sha>]");
    process.exit(2);
  }
  try {
    const manifest = buildManifest(dist, { tag: option("--tag") ?? null, gitSha: option("--git-sha") ?? null });
    writeFileSync(join(dist, "SHA256SUMS"), sha256sums(manifest));
    writeFileSync(join(dist, `aip-${manifest.version}-manifest.json`), JSON.stringify(manifest, null, 2) + "\n");
    console.log(JSON.stringify({ ok: true, version: manifest.version, files: manifest.files.length }));
  } catch (error) {
    console.error(JSON.stringify({ ok: false, message: error.message }));
    process.exit(1);
  }
}
