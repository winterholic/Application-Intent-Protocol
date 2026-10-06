import { execFile } from "node:child_process";
import { constants } from "node:fs";
import { access, mkdir, rm, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const execute = promisify(execFile);

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
  await mkdir(dirname(out), { recursive: true });
  try {
    await mkdir(out);
  } catch (error) {
    throw { code: error.code === "EEXIST" ? "OUTPUT_EXISTS" : "OUTPUT_IO", msg: "새 출력 디렉터리가 필요함" };
  }

  try {
    await execute(compiler, [
      "--strict", "--declaration", "--rootDir", root, "--outDir", out,
      "--module", "esnext", "--moduleResolution", "bundler", "--rewriteRelativeImportExtensions",
      "--target", "es2022", "--noEmitOnError", resolve(root, "product/sdk/index.ts"),
    ], { cwd: root, timeout: 30_000, maxBuffer: 1024 * 1024 });

    await writeFile(resolve(out, "package.json"), JSON.stringify({
      name: "@aip/sdk",
      version: "0.1.0",
      private: true,
      type: "module",
      types: "./product/sdk/index.d.ts",
      exports: { ".": { types: "./product/sdk/index.d.ts", import: "./product/sdk/index.js" } },
      files: ["product/sdk", "spikes/spike-v5-sdk/sdk", "spikes/spike-v6-transport/client"],
    }, null, 2) + "\n");
  } catch {
    await rm(out, { recursive: true, force: true });
    throw { code: "BUILD_FAILED", msg: "SDK 컴파일 실패. 소유한 출력 디렉터리를 정리함" };
  }
  return { ok: true, out };
}

try {
  console.log(JSON.stringify(await build(process.argv.slice(2))));
} catch (error) {
  console.error(JSON.stringify({ ok: false, code: error.code ?? "OUTPUT_IO", msg: error.msg ?? "SDK 출력 실패" }));
  process.exitCode = 1;
}
