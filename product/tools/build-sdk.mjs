import { execFile } from "node:child_process";
import { constants } from "node:fs";
import { access, copyFile, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, posix, resolve } from "node:path";
import { promisify } from "node:util";
import { releaseConfig, repoRoot as root, sdkPackageManifest } from "./version.mjs";

const execute = promisify(execFile);

// tsc는 저장소 구조(rootDir=저장소 루트) 그대로 내보낸다. 공개 패키지에 spikes 이름이 남지 않도록
// 빌드 단계에서 product/sdk/ 단일 트리로 재배치하고, 상대 import를 새 위치에 맞게 다시 쓴다.
// 진입점 경로(product/sdk/index.js)는 이미 설치된 소비자와 기존 시험이 쓰므로 바꾸지 않는다.
const RELOCATIONS = [
  ["product/sdk/", "product/sdk/"],
  ["spikes/spike-v5-sdk/sdk/", "product/sdk/engine/"],
  ["spikes/spike-v6-transport/client/", "product/sdk/transport/"],
];
const SPECIFIER = /((?:\bfrom\s*|\bimport\s*\(?\s*)["'])(\.{1,2}\/[^"']+)(["'])/gu;

function relocated(path) {
  for (const [from, to] of RELOCATIONS) {
    if (path.startsWith(from)) return to + path.slice(from.length);
  }
  return undefined;
}

async function walk(directory, prefix = "") {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const name = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) files.push(...await walk(join(directory, entry.name), name));
    else files.push(name);
  }
  return files;
}

async function relocate(staging, out) {
  for (const file of await walk(staging)) {
    const target = relocated(file);
    if (!target) throw new Error(`재배치 규칙이 없는 산출물: ${file}`);
    const text = (await readFile(join(staging, file), "utf8")).replace(SPECIFIER, (whole, head, specifier, tail) => {
      const dependency = relocated(posix.join(posix.dirname(file), specifier));
      if (!dependency) throw new Error(`${file}의 import가 패키지 밖을 가리킵니다: ${specifier}`);
      let next = posix.relative(posix.dirname(target), dependency);
      if (!next.startsWith(".")) next = `./${next}`;
      return `${head}${next}${tail}`;
    });
    await mkdir(dirname(join(out, target)), { recursive: true });
    await writeFile(join(out, target), text);
  }
}

async function build(args) {
  if (args.length !== 2 || args[0] !== "--out" || !args[1].trim()) {
    throw { code: "BAD_ARGS", msg: "usage: node product/tools/build-sdk.mjs --out <fresh-directory>" };
  }

  const out = resolve(args[1]);
  const compiler = resolve(root, "spikes/spike-0-ts/node_modules/.bin/tsc");
  try {
    await access(compiler, constants.X_OK);
  } catch {
    throw { code: "SDK_TOOLCHAIN_MISSING", msg: "저장소 루트에서 npm ci --prefix spikes/spike-0-ts 실행 후 SDK를 빌드하십시오" };
  }
  let manifest;
  try {
    manifest = sdkPackageManifest(releaseConfig());
  } catch (error) {
    throw { code: "BAD_RELEASE_CONFIG", msg: error.message };
  }
  await mkdir(dirname(out), { recursive: true });
  try {
    await mkdir(out);
  } catch (error) {
    throw { code: error.code === "EEXIST" ? "OUTPUT_EXISTS" : "OUTPUT_IO", msg: "새 출력 디렉터리가 필요함" };
  }

  let staging;
  let compilerStderr;
  try {
    staging = await mkdtemp(join(tmpdir(), "aip-sdk-tsc-"));
    try {
      await execute(compiler, [
        "--strict", "--declaration", "--rootDir", root, "--outDir", staging,
        "--module", "esnext", "--moduleResolution", "bundler", "--rewriteRelativeImportExtensions",
        "--target", "es2022", "--noEmitOnError", resolve(root, "product/sdk/index.ts"),
      ], { cwd: root, timeout: 30_000, maxBuffer: 1024 * 1024 });
    } catch (error) {
      const output = [error.stderr, error.stdout].filter(Boolean).join("\n");
      const lines = output.trim().split(/\r?\n/u).filter(Boolean).slice(0, 8);
      compilerStderr = lines.join(" | ").slice(0, 1200);
      throw error;
    }
    await relocate(staging, out);

    await writeFile(resolve(out, "package.json"), JSON.stringify(manifest, null, 2) + "\n");
    await copyFile(resolve(root, "LICENSE"), resolve(out, "LICENSE"));
    await copyFile(resolve(root, "product/sdk/README.md"), resolve(out, "README.md"));
  } catch {
    await rm(out, { recursive: true, force: true });
    const detail = compilerStderr ? `: ${compilerStderr}` : "";
    throw { code: "BUILD_FAILED", msg: `SDK 컴파일 실패. 소유한 출력 디렉터리를 정리함${detail}` };
  } finally {
    if (staging) await rm(staging, { recursive: true, force: true });
  }
  return { ok: true, out };
}

try {
  console.log(JSON.stringify(await build(process.argv.slice(2))));
} catch (error) {
  console.error(JSON.stringify({ ok: false, code: error.code ?? "OUTPUT_IO", msg: error.msg ?? "SDK 출력 실패" }));
  process.exitCode = 1;
}
