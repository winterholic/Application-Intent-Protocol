import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, copyFile, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const productDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = resolve(productDir, "..");
const artifactNames = [
  "bin/aip", "config.example.json", "example.aip", "extensions/event.mjs", "extensions/event.py",
  "README.md", "sdk/aip-sdk-0.1.0.tgz", "SOURCE_SHA256SUMS",
];
const manifestFiles = [
  "Cargo.toml", "Cargo.lock",
  "crates/aip-cli", "crates/aip-service", "crates/aip-auth", "crates/aip-migrate", "crates/aip-contract",
  "crates/aip-syntax", "crates/aip-sema", "crates/aip-ir", "crates/aip-pg", "crates/aip-plan", "crates/aip-runtime",
  "spikes/spike-v1-fixture", "spikes/spike-v2-read", "spikes/spike-v3-write", "spikes/spike-v4-worker",
  "spikes/spike-v5-sdk", "spikes/spike-v6-transport", "spikes/spike-v11-dev-checks",
  "prototype/src/schema_catalog.sql",
  "product/sdk/index.ts", "product/tools/build-sdk.mjs", "product/tools/package.mjs", "product/tools/smoke.mjs",
  "prototype/example/app.aip", "prototype/example/extensions/event.mjs", "prototype/example/extensions/event.py",
];

class PackageFailure extends Error {
  constructor(code, message, stage) {
    super(message);
    this.code = code;
    this.stage = stage;
  }
}

function parseArgs(args) {
  if (args.length === 0) {
    const stamp = new Date().toISOString().replaceAll(":", "-").replaceAll(".", "-");
    return resolve(productDir, "dist", stamp);
  }
  if (args.length !== 2 || args[0] !== "--out" || !args[1].trim()) {
    throw new PackageFailure("BAD_ARGS", "usage: node product/tools/package.mjs [--out <fresh-directory>]", "arguments");
  }
  return resolve(args[1]);
}

function command(program, args, stage, cwd = repoDir) {
  const result = spawnSync(program, args, {
    cwd,
    encoding: "utf8",
    timeout: 90_000,
    maxBuffer: 4 * 1024 * 1024,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: process.env.TMPDIR, LANG: process.env.LANG },
  });
  if (result.error) throw new PackageFailure("COMMAND_START", "로컬 빌드 명령을 실행할 수 없습니다", stage);
  if (result.signal) throw new PackageFailure("COMMAND_SIGNAL", `로컬 빌드 명령이 ${result.signal} 신호로 종료됐습니다`, stage);
  if (result.status !== 0) throw new PackageFailure("COMMAND_FAILED", "로컬 빌드 명령이 실패했습니다", stage);
  return result.stdout;
}

function packageReadme() {
  return `# AIP product artifact

이 디렉터리는 로컬 제품 통합을 위한 실행 파일과 SDK 패키지입니다.

## 파일

- \`bin/aip\`: 패키징 시점의 저장소 \`target/debug/aip\`에서 복사한 로컬 빌드 실행 파일
- \`sdk/aip-sdk-0.1.0.tgz\`: 로컬 TypeScript compiler로 만든 설치 가능한 \`@aip/sdk@0.1.0\` tarball
- \`example.aip\`: 제품 service에서 검증할 수 있는 호출자 정의 예시
- \`extensions/event.mjs\`, \`extensions/event.py\`: 정본 예제의 Node와 Python Event.confirm 구현
- \`config.example.json\`: issuer, schema, origin, JWKS 경로가 예시값인 비밀 없는 설정
- \`SHA256SUMS\`: 배포 파일과 source manifest의 SHA-256 해시
- \`SOURCE_SHA256SUMS\`: 현재 workspace의 CLI·SDK·예제 source 입력 해시

## 확인과 사용

\`shasum -a 256 -c SHA256SUMS\`로 파일을 확인하고, 앱 프로젝트에서 \`npm install <이 디렉터리>/sdk/aip-sdk-0.1.0.tgz\`로 SDK를 설치합니다.

\`./bin/aip service check example.aip\`로 정의를 확인하고, \`./bin/aip service gen example.aip --wire decimal --out bindings.ts --sdk-import @aip/sdk\`로 TypeScript binding을 생성할 수 있습니다. 배포 전에는 예시 설정의 schema, DB 환경 변수, issuer, audience, JWKS와 허용 origin을 운영 값으로 채우십시오.

기본 설정은 worker를 사용하지 않습니다. \`Event.confirm\`을 활성화하려면 \`workers.directory\`를 \`./extensions\`로, \`workers.language\`를 \`node\` 또는 \`python\`으로, \`workers.enable_writes\`를 \`true\`로 설정합니다. Node에는 Node.js, Python에는 Python 3가 필요합니다. extension 실행은 현재 macOS의 \`sandbox-exec\` 기반 \`MacNetDeny\`에 한정되며 네트워크 접근만 차단합니다. 파일 격리는 제공하지 않으므로 신뢰한 로컬 extension source를 사용하십시오.

## 산출 범위

이 번들은 현재 workspace의 로컬 빌드에서 가져온 실행 파일과 SDK tarball, 예시 입력을 포함합니다. 소스 전체, 공개 registry 배포, 빌드 서명 또는 외부 provenance는 포함하지 않습니다. 해시는 파일 무결성 확인용입니다.
\`SOURCE_SHA256SUMS\`의 경로는 생성 시점의 source workspace에 상대적입니다. 이 artifact는 Git commit provenance를 주장하지 않습니다.
원본 workspace가 있으면 workspace root에서 \`shasum -a 256 -c <artifact>/SOURCE_SHA256SUMS\`로 source 입력을 확인할 수 있습니다.
`;
}

function exampleConfig() {
  return {
    schema: "aip_example",
    database_url_env: "AIP_DATABASE_URL",
    listen: "127.0.0.1:8080",
    wire: "decimal",
    allowed_origins: ["https://app.example.com"],
    auth: {
      issuer: "https://issuer.example",
      audience: "aip-api",
      jwks: { kind: "file", path: "./jwks.json" },
    },
  };
}

async function packageArtifacts(out) {
  const binary = resolve(repoDir, "target", "debug", "aip");
  const binaryInfo = await stat(binary).catch(() => undefined);
  if (!binaryInfo?.isFile() || (binaryInfo.mode & 0o111) === 0) {
    throw new PackageFailure("BINARY_MISSING", "root target/debug/aip 실행 파일이 필요합니다", "binary");
  }
  const definition = join(repoDir, "prototype", "example", "app.aip");
  const nodeExtension = join(repoDir, "prototype", "example", "extensions", "event.mjs");
  const pythonExtension = join(repoDir, "prototype", "example", "extensions", "event.py");
  const builder = join(productDir, "tools", "build-sdk.mjs");
  await readFile(definition).catch(() => { throw new PackageFailure("EXAMPLE_MISSING", "기존 caller 예시를 읽을 수 없습니다", "example"); });

  await mkdir(dirname(out), { recursive: true });
  try {
    await mkdir(out);
  } catch (error) {
    throw new PackageFailure(error.code === "EEXIST" ? "OUTPUT_EXISTS" : "OUTPUT_IO", "fresh 출력 디렉터리가 필요합니다", "output");
  }

  let scratch;
  try {
    scratch = await mkdtemp(join(tmpdir(), "aip-product-package-"));
    const sdkDir = join(scratch, "sdk");
    const packDir = join(scratch, "pack");
    await mkdir(packDir);
    command(process.execPath, [builder, "--out", sdkDir], "sdk-build");
    const packedText = command("npm", ["pack", "--offline", "--ignore-scripts", "--json", "--pack-destination", packDir, sdkDir], "sdk-pack");
    let packed;
    try {
      packed = JSON.parse(packedText);
    } catch {
      throw new PackageFailure("BAD_PACK_OUTPUT", "npm pack 결과 JSON을 읽지 못했습니다", "sdk-pack");
    }
    if (packed.length !== 1 || packed[0].name !== "@aip/sdk" || packed[0].version !== "0.1.0") {
      throw new PackageFailure("PACKAGE_IDENTITY", "SDK tarball이 @aip/sdk@0.1.0이 아닙니다", "sdk-pack");
    }

    await mkdir(join(out, "bin"), { recursive: true });
    await mkdir(join(out, "sdk"), { recursive: true });
    await mkdir(join(out, "extensions"), { recursive: true });
    await copyFile(binary, join(out, "bin", "aip"));
    await chmod(join(out, "bin", "aip"), 0o755);
    await copyFile(join(packDir, packed[0].filename), join(out, "sdk", "aip-sdk-0.1.0.tgz"));
    await copyFile(definition, join(out, "example.aip"));
    await copyFile(nodeExtension, join(out, "extensions", "event.mjs"));
    await copyFile(pythonExtension, join(out, "extensions", "event.py"));
    await writeFile(join(out, "config.example.json"), JSON.stringify(exampleConfig(), null, 2) + "\n", { flag: "wx" });
    await writeFile(join(out, "README.md"), packageReadme(), { flag: "wx" });

    const sourceFiles = await collectManifestSources();
    const sourceSums = [];
    for (const name of sourceFiles) {
      const digest = createHash("sha256").update(await readFile(join(repoDir, name))).digest("hex");
      sourceSums.push(`${digest}  ${name}`);
    }
    await writeFile(join(out, "SOURCE_SHA256SUMS"), `${sourceSums.join("\n")}\n`, { flag: "wx" });

    const sums = [];
    for (const name of artifactNames) {
      const bytes = await readFile(join(out, name));
      const digest = createHash("sha256").update(bytes).digest("hex");
      sums.push(`${digest}  ${name}`);
    }
    await writeFile(join(out, "SHA256SUMS"), `${sums.join("\n")}\n`, { flag: "wx" });
    return { ok: true, out, files: artifactNames.length, manifest: "SHA256SUMS" };
  } catch (error) {
    await rm(out, { recursive: true, force: true });
    throw error;
  } finally {
    if (scratch) await rm(scratch, { recursive: true, force: true });
  }
}

async function collectManifestSources() {
  const files = new Set(["Cargo.toml", "Cargo.lock", "prototype/src/schema_catalog.sql"]);
  const directories = manifestFiles.filter((name) => !name.endsWith(".toml") && !name.endsWith(".lock") && !name.endsWith(".sql"));
  for (const entry of directories) {
    const path = join(repoDir, entry);
    const info = await stat(path).catch(() => undefined);
    if (!info) throw new PackageFailure("SOURCE_MISSING", `source 입력이 없습니다: ${entry}`, "source-manifest");
    if (info.isFile()) {
      files.add(entry);
      continue;
    }
    for (const subdir of ["src", "sdk", "client", "workers"]) {
      await collectSourceTree(join(path, subdir), files);
    }
    const cargoFile = join(path, "Cargo.toml");
    if (await stat(cargoFile).then((value) => value.isFile()).catch(() => false)) files.add(relative(repoDir, cargoFile).split(sep).join("/"));
  }
  for (const entry of manifestFiles.filter((name) => name.endsWith(".aip") || name.endsWith(".mjs") || name.endsWith(".py") || name.endsWith(".ts"))) {
    files.add(entry);
  }
  return [...files].sort();
}

async function collectSourceTree(directory, files) {
  const entries = await readdir(directory, { withFileTypes: true }).catch((error) => {
    if (error.code === "ENOENT") return [];
    throw error;
  });
  for (const entry of entries) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "target" || entry.name === "node_modules" || entry.name === "__pycache__") continue;
      await collectSourceTree(path, files);
    } else if (entry.isFile() && /\.(?:rs|ts|tsx|js|mjs|py|sql)$/u.test(entry.name)) {
      files.add(relative(repoDir, path).split(sep).join("/"));
    }
  }
}

try {
  const out = parseArgs(process.argv.slice(2));
  console.log(JSON.stringify(await packageArtifacts(out)));
} catch (error) {
  const failure = error instanceof PackageFailure ? error : new PackageFailure("OUTPUT_IO", "제품 artifact 생성 실패", "package");
  console.error(JSON.stringify({ ok: false, code: failure.code, stage: failure.stage, message: failure.message }));
  process.exitCode = 1;
}
