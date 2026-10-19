import { createHash } from "node:crypto";
import { readdir, readFile, realpath, stat } from "node:fs/promises";
import { isAbsolute, join, normalize, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

export class ManifestFailure extends Error {
  constructor(code, message, stage = "manifest") {
    super(message);
    this.code = code;
    this.stage = stage;
  }
}

const ALLOWED_ROOT_KEYS = new Set([
  "$schema",
  "name",
  "version",
  "description",
  "definition",
  "extensions",
  "static_assets",
  "dependencies",
  "targets",
  "features",
  "config_template",
]);

const ALLOWED_EXTENSION_KEYS = new Set([
  "directory",
  "language",
  "entrypoints",
  "enable_writes",
]);

const ALLOWED_DEPENDENCY_KEYS = new Set(["node", "python"]);
const ALLOWED_NODE_DEP_KEYS = new Set(["package_json", "lockfile"]);
const ALLOWED_PYTHON_DEP_KEYS = new Set(["requirements", "lockfile"]);
const ALLOWED_STATIC_ASSET_KEYS = new Set(["mount", "path"]);

const DISALLOWED_FILE_PATTERNS = [
  /\.env/u,
  /\.pem$/u,
  /\.key$/u,
  /id_rsa/u,
  /^\.git/u,
  /node_modules/u,
  /^target$/u,
];

const CREDENTIAL_URL_PATTERN = /[a-zA-Z0-9+.-]+:\/\/[^:]+:[^@]+@/u;

function fail(code, message, stage = "manifest") {
  throw new ManifestFailure(code, message, stage);
}

function checkDisallowedName(pathStr, label) {
  const normalized = pathStr.split(sep).join("/");
  for (const part of normalized.split("/")) {
    if (!part || part === ".") continue;
    for (const pattern of DISALLOWED_FILE_PATTERNS) {
      if (pattern.test(part)) {
        fail("DISALLOWED_FILE", `${label}에 허용되지 않는 보안 또는 빌드 부산물 파일이 포함되어 있습니다: ${pathStr}`, "security");
      }
    }
  }
}

async function resolveSafePath(appDir, relPath, label) {
  if (typeof relPath !== "string" || !relPath.trim()) {
    fail("BAD_MANIFEST", `${label} 경로가 올바르지 않습니다: ${relPath}`);
  }
  if (isAbsolute(relPath)) {
    fail("PATH_ESCAPES_APP", `${label} 경로는 절대 경로일 수 없습니다: ${relPath}`, "security");
  }
  checkDisallowedName(relPath, label);

  const normalizedRel = normalize(relPath);
  const resolved = resolve(appDir, relPath);
  const relFromApp = relative(appDir, resolved);

  if (relFromApp.startsWith("..") || isAbsolute(relFromApp)) {
    fail("PATH_ESCAPES_APP", `${label} 경로가 앱 루트를 벗어납니다: ${relPath}`, "security");
  }

  // Check file / symlink boundary
  const fileStat = await stat(resolved).catch(() => undefined);
  if (!fileStat) {
    return { resolved, rel: relFromApp, exists: false };
  }

  const realAppDir = await realpath(appDir).catch(() => appDir);
  const realFile = await realpath(resolved).catch(() => resolved);
  const relReal = relative(realAppDir, realFile);

  if (relReal.startsWith("..") || isAbsolute(relReal)) {
    fail("SYMLINK_ESCAPES_APP", `${label} 심볼릭 링크가 앱 루트 외부를 가리킵니다: ${relPath}`, "security");
  }

  checkDisallowedName(relReal, label);

  return { resolved, rel: relFromApp, exists: true, isFile: fileStat.isFile(), isDirectory: fileStat.isDirectory() };
}

async function fileDigest(filePath) {
  const bytes = await readFile(filePath);
  return createHash("sha256").update(bytes).digest("hex");
}

function validateConfigTemplate(configObj, configText) {
  if (CREDENTIAL_URL_PATTERN.test(configText)) {
    fail("CREDENTIAL_LEAK", "config_template에 자격 증명(URL 내 비밀번호)이 포함되어 있습니다. 환경 변수 이름을 사용하십시오.", "security");
  }

  const inspect = (value, path = "") => {
    if (value && typeof value === "object") {
      for (const [k, v] of Object.entries(value)) {
        const lowerKey = k.toLowerCase();
        if (
          ["password", "secret", "token", "credential", "api_key"].includes(lowerKey) &&
          typeof v === "string" &&
          v.trim() &&
          !lowerKey.endsWith("_env") &&
          !v.startsWith("file:")
        ) {
          fail("CREDENTIAL_LEAK", `config_template의 ${path ? `${path}.` : ""}${k} 필드에 민감한 값이 하드코딩되어 있습니다`, "security");
        }
        inspect(v, path ? `${path}.${k}` : k);
      }
    }
  };
  inspect(configObj);
}

export async function loadAndValidateAppManifest(targetPath) {
  let resolvedTarget = resolve(process.cwd(), targetPath);
  let targetStat = await stat(resolvedTarget).catch(() => undefined);

  if (!targetStat) {
    fail("MANIFEST_MISSING", `매니페스트 또는 앱 디렉터리를 찾을 수 없습니다: ${targetPath}`, "input");
  }

  let appDir;
  let manifestPath;

  if (targetStat.isDirectory()) {
    appDir = resolvedTarget;
    manifestPath = join(appDir, "aip.app.json");
    const mStat = await stat(manifestPath).catch(() => undefined);
    if (!mStat || !mStat.isFile()) {
      fail("MANIFEST_MISSING", `앱 디렉터리 내에 aip.app.json 파일이 없습니다: ${appDir}`, "input");
    }
  } else if (targetStat.isFile()) {
    manifestPath = resolvedTarget;
    appDir = resolve(manifestPath, "..");
  } else {
    fail("BAD_MANIFEST", `올바른 매니페스트 파일이나 디렉터리가 아닙니다: ${targetPath}`, "input");
  }

  const realAppDir = await realpath(appDir).catch(() => appDir);
  const realManifestPath = await realpath(manifestPath).catch(() => manifestPath);
  const relManifestFromRealApp = relative(realAppDir, realManifestPath);
  if (relManifestFromRealApp.startsWith("..") || isAbsolute(relManifestFromRealApp)) {
    fail("SYMLINK_ESCAPES_APP", `매니페스트 심볼릭 링크가 앱 루트 외부를 가리킵니다: ${manifestPath}`, "security");
  }
  checkDisallowedName(relative(appDir, manifestPath), "매니페스트 파일");
  checkDisallowedName(relManifestFromRealApp, "매니페스트 파일");

  const rawManifestText = await readFile(manifestPath, "utf8").catch(() => {
    fail("MANIFEST_MISSING", `매니페스트 파일을 읽을 수 없습니다: ${manifestPath}`, "input");
  });

  let manifest;
  try {
    manifest = JSON.parse(rawManifestText);
  } catch (error) {
    fail("BAD_MANIFEST", `매니페스트 파일의 JSON 형식이 올바르지 않습니다: ${error.message}`, "syntax");
  }

  if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) {
    fail("BAD_MANIFEST", "매니페스트는 JSON 객체여야 합니다", "schema");
  }

  // Deny unknown fields at root
  for (const key of Object.keys(manifest)) {
    if (!ALLOWED_ROOT_KEYS.has(key)) {
      fail("BAD_MANIFEST", `알 수 없는 매니페스트 필드입니다: ${key}`, "schema");
    }
  }

  // Name validation
  if (typeof manifest.name !== "string" || !manifest.name.trim()) {
    fail("BAD_MANIFEST", "name 필드가 필요합니다", "schema");
  }
  if (!/^[a-z0-9][a-z0-9_-]{0,62}$/u.test(manifest.name)) {
    fail("BAD_MANIFEST", "name은 1~63자의 소문자, 숫자, '-', '_'여야 합니다", "schema");
  }

  // Version validation
  if (typeof manifest.version !== "string" || !manifest.version.trim()) {
    fail("BAD_MANIFEST", "version 필드가 필요합니다", "schema");
  }
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/u.test(manifest.version)) {
    fail("BAD_MANIFEST", `version은 유효한 시맨틱 버전이어야 합니다: ${manifest.version}`, "schema");
  }

  // Description validation
  if (manifest.description !== undefined && (typeof manifest.description !== "string" || manifest.description.length > 500)) {
    fail("BAD_MANIFEST", "description은 500자 이하의 문자열이어야 합니다", "schema");
  }

  const filesToTrack = [];
  const manifestDigest = createHash("sha256").update(rawManifestText).digest("hex");
  filesToTrack.push({
    role: "manifest",
    path: relative(appDir, manifestPath).split(sep).join("/"),
    sha256: manifestDigest,
  });

  // Definition validation
  if (typeof manifest.definition !== "string" || !manifest.definition.trim()) {
    fail("BAD_MANIFEST", "definition 필드가 필요합니다", "schema");
  }
  if (!manifest.definition.endsWith(".aip")) {
    fail("INVALID_DEFINITION_EXTENSION", `definition 파일은 .aip 확장자여야 합니다: ${manifest.definition}`, "definition");
  }
  const defPathInfo = await resolveSafePath(appDir, manifest.definition, "definition");
  if (!defPathInfo.exists || !defPathInfo.isFile) {
    fail("DEFINITION_MISSING", `AIP 정의 파일을 찾을 수 없습니다: ${manifest.definition}`, "definition");
  }
  const defDigest = await fileDigest(defPathInfo.resolved);
  filesToTrack.push({
    role: "definition",
    path: defPathInfo.rel.split(sep).join("/"),
    sha256: defDigest,
  });

  // Extensions validation
  let normalizedExtensions;
  if (manifest.extensions !== undefined) {
    if (typeof manifest.extensions !== "object" || manifest.extensions === null || Array.isArray(manifest.extensions)) {
      fail("BAD_MANIFEST", "extensions는 객체여야 합니다", "schema");
    }
    for (const key of Object.keys(manifest.extensions)) {
      if (!ALLOWED_EXTENSION_KEYS.has(key)) {
        fail("BAD_MANIFEST", `extensions의 알 수 없는 필드입니다: ${key}`, "schema");
      }
    }
    const { directory, language, entrypoints, enable_writes = false } = manifest.extensions;
    if (!["node", "python"].includes(language)) {
      fail("BAD_MANIFEST", `extensions.language는 "node" 또는 "python"이어야 합니다: ${language}`, "schema");
    }
    if (typeof directory !== "string" || !directory.trim()) {
      fail("BAD_MANIFEST", "extensions.directory 필드가 필요합니다", "schema");
    }
    const dirInfo = await resolveSafePath(appDir, directory, "extensions.directory");
    if (!dirInfo.exists || !dirInfo.isDirectory) {
      fail("EXTENSION_MISSING", `extensions.directory 디렉터리를 찾을 수 없습니다: ${directory}`, "extensions");
    }
    if (!Array.isArray(entrypoints) || entrypoints.length === 0) {
      fail("BAD_MANIFEST", "extensions.entrypoints는 1개 이상의 진입점을 포함하는 배열이어야 합니다", "schema");
    }
    if (typeof enable_writes !== "boolean") {
      fail("BAD_MANIFEST", "extensions.enable_writes는 불리언이어야 합니다", "schema");
    }

    const normalizedEntrypoints = [];
    for (const ep of entrypoints) {
      if (typeof ep !== "string" || !ep.trim()) {
        fail("BAD_MANIFEST", `유효하지 않은 entrypoint 형식입니다: ${ep}`, "schema");
      }
      if (isAbsolute(ep) || relative(dirInfo.resolved, resolve(dirInfo.resolved, ep)).startsWith("..")) {
        fail("PATH_ESCAPES_APP", `extensions.entrypoint 경로가 extensions 디렉터리를 벗어납니다: ${ep}`, "security");
      }
      if (language === "node" && !/\.(?:mjs|js|ts)$/u.test(ep)) {
        fail("INVALID_EXTENSION_LANGUAGE", `Node.js 확장 진입점은 .mjs, .js 또는 .ts여야 합니다: ${ep}`, "extensions");
      }
      if (language === "python" && !/\.py$/u.test(ep)) {
        fail("INVALID_EXTENSION_LANGUAGE", `Python 확장 진입점은 .py여야 합니다: ${ep}`, "extensions");
      }

      const epRelToApp = join(directory, ep);
      const epInfo = await resolveSafePath(appDir, epRelToApp, `extensions.entrypoint (${ep})`);
      if (!epInfo.exists || !epInfo.isFile) {
        fail("EXTENSION_MISSING", `확장 진입점 파일을 찾을 수 없습니다: ${epRelToApp}`, "extensions");
      }
      const epDigest = await fileDigest(epInfo.resolved);
      filesToTrack.push({
        role: "extension",
        path: epInfo.rel.split(sep).join("/"),
        sha256: epDigest,
      });
      normalizedEntrypoints.push(ep.split(sep).join("/"));
    }

    // Helpers imported by entrypoints are runtime inputs too. Track the declared
    // directory, with the same lexical/canonical boundary rules as entrypoints.
    const collectExtensions = async (directoryPath, ancestry = new Set()) => {
      const info = await resolveSafePath(appDir, relative(appDir, directoryPath), "extensions");
      if (!info.exists) fail("EXTENSION_MISSING", "확장 입력이 사라졌습니다", "extensions");
      if (info.isFile) {
        filesToTrack.push({ role: "extension", path: info.rel.split(sep).join("/"), sha256: await fileDigest(info.resolved) });
      } else if (info.isDirectory) {
        const canonical = await realpath(directoryPath);
        if (ancestry.has(canonical)) return;
        const next = new Set([...ancestry, canonical]);
        for (const entry of (await readdir(directoryPath)).sort()) await collectExtensions(join(directoryPath, entry), next);
      } else {
        fail("DISALLOWED_FILE", "확장에 특수 파일을 포함할 수 없습니다", "security");
      }
    };
    await collectExtensions(dirInfo.resolved);

    normalizedExtensions = {
      directory: dirInfo.rel.split(sep).join("/"),
      language,
      entrypoints: normalizedEntrypoints,
      enable_writes,
    };
  }

  // Static assets validation
  let normalizedStaticAssets;
  if (manifest.static_assets !== undefined) {
    if (!Array.isArray(manifest.static_assets)) {
      fail("BAD_MANIFEST", "static_assets는 배열이어야 합니다", "schema");
    }
    normalizedStaticAssets = [];
    for (const item of manifest.static_assets) {
      if (!item || typeof item !== "object" || Array.isArray(item)) {
        fail("BAD_MANIFEST", "static_assets의 항목은 객체여야 합니다", "schema");
      }
      for (const k of Object.keys(item)) {
        if (!ALLOWED_STATIC_ASSET_KEYS.has(k)) {
          fail("BAD_MANIFEST", `static_assets의 알 수 없는 필드입니다: ${k}`, "schema");
        }
      }
      if (typeof item.mount !== "string" || !item.mount.startsWith("/")) {
        fail("BAD_MANIFEST", `static_assets mount 경로는 '/'로 시작해야 합니다: ${item.mount}`, "schema");
      }
      const assetInfo = await resolveSafePath(appDir, item.path, `static_assets (${item.mount})`);
      if (!assetInfo.exists) {
        fail("STATIC_ASSET_MISSING", `static_assets 경로를 찾을 수 없습니다: ${item.path}`, "static_assets");
      }
      normalizedStaticAssets.push({
        mount: item.mount,
        path: assetInfo.rel.split(sep).join("/"),
      });

      if (assetInfo.isFile) {
        const digest = await fileDigest(assetInfo.resolved);
        filesToTrack.push({
          role: "static_asset",
          path: assetInfo.rel.split(sep).join("/"),
          sha256: digest,
        });
      } else if (assetInfo.isDirectory) {
        const walkDir = async (currentDir, ancestry = new Set()) => {
          const realCurrent = await realpath(currentDir).catch(() => currentDir);
          if (ancestry.has(realCurrent)) {
            return;
          }
          const nextAncestry = new Set(ancestry);
          nextAncestry.add(realCurrent);

          const relFromApp = relative(appDir, currentDir);
          if (relFromApp.startsWith("..") || isAbsolute(relFromApp)) {
            fail("PATH_ESCAPES_APP", `static_assets 디렉터리가 앱 루트를 벗어납니다: ${relFromApp}`, "security");
          }
          const relReal = relative(realAppDir, realCurrent);
          if (relReal.startsWith("..") || isAbsolute(relReal)) {
            fail("SYMLINK_ESCAPES_APP", `static_assets 심볼릭 링크가 앱 루트 외부를 가리킵니다: ${relFromApp}`, "security");
          }
          checkDisallowedName(relReal, `static_assets 디렉터리 (${relFromApp.split(sep).join("/")})`);

          const entries = await readdir(currentDir, { withFileTypes: true });
          entries.sort((a, b) => a.name.localeCompare(b.name));

          for (const entry of entries) {
            const entryPath = join(currentDir, entry.name);
            const entryRelToApp = relative(appDir, entryPath);
            const normalizedRel = entryRelToApp.split(sep).join("/");

            checkDisallowedName(entry.name, `static_assets 하위 항목 (${normalizedRel})`);

            if (entry.isSymbolicLink()) {
              const realTarget = await realpath(entryPath).catch(() => undefined);
              if (!realTarget) {
                fail("STATIC_ASSET_MISSING", `static_assets 심볼릭 링크 대상을 찾을 수 없습니다: ${normalizedRel}`, "static_assets");
              }
              const relRealTarget = relative(realAppDir, realTarget);
              if (relRealTarget.startsWith("..") || isAbsolute(relRealTarget)) {
                fail("SYMLINK_ESCAPES_APP", `static_assets 심볼릭 링크가 앱 루트 외부를 가리킵니다: ${normalizedRel}`, "security");
              }
              checkDisallowedName(relRealTarget, `static_assets 하위 항목 (${normalizedRel})`);

              const targetStat = await stat(realTarget);
              if (targetStat.isDirectory()) {
                await walkDir(entryPath, nextAncestry);
              } else if (targetStat.isFile()) {
                const digest = await fileDigest(entryPath);
                filesToTrack.push({
                  role: "static_asset",
                  path: normalizedRel,
                  sha256: digest,
                });
              } else {
                fail("DISALLOWED_FILE", `static_assets에 특수 파일이 포함되어 있습니다: ${normalizedRel}`, "security");
              }
            } else if (entry.isDirectory()) {
              await walkDir(entryPath, nextAncestry);
            } else if (entry.isFile()) {
              const digest = await fileDigest(entryPath);
              filesToTrack.push({
                role: "static_asset",
                path: normalizedRel,
                sha256: digest,
              });
            } else {
              fail("DISALLOWED_FILE", `static_assets에 특수 파일이 포함되어 있습니다: ${normalizedRel}`, "security");
            }
          }
        };

        await walkDir(assetInfo.resolved);
      } else {
        fail("STATIC_ASSET_MISSING", `static_assets 경로가 올바른 파일이나 디렉터리가 아닙니다: ${item.path}`, "static_assets");
      }
    }
  }

  // Dependencies validation
  let normalizedDependencies;
  if (manifest.dependencies !== undefined) {
    if (typeof manifest.dependencies !== "object" || manifest.dependencies === null || Array.isArray(manifest.dependencies)) {
      fail("BAD_MANIFEST", "dependencies는 객체여야 합니다", "schema");
    }
    for (const k of Object.keys(manifest.dependencies)) {
      if (!ALLOWED_DEPENDENCY_KEYS.has(k)) {
        fail("BAD_MANIFEST", `dependencies의 알 수 없는 런타임입니다: ${k}`, "schema");
      }
    }
    normalizedDependencies = {};

    if (manifest.dependencies.node !== undefined) {
      const nodeDep = manifest.dependencies.node;
      if (typeof nodeDep !== "object" || nodeDep === null || Array.isArray(nodeDep)) {
        fail("BAD_MANIFEST", "dependencies.node는 객체여야 합니다", "schema");
      }
      for (const k of Object.keys(nodeDep)) {
        if (!ALLOWED_NODE_DEP_KEYS.has(k)) {
          fail("BAD_MANIFEST", `dependencies.node의 알 수 없는 필드입니다: ${k}`, "schema");
        }
      }
      normalizedDependencies.node = {};
      if (nodeDep.package_json !== undefined) {
        const info = await resolveSafePath(appDir, nodeDep.package_json, "dependencies.node.package_json");
        if (!info.exists || !info.isFile) {
          fail("DEPENDENCY_MISSING", `package.json 파일을 찾을 수 없습니다: ${nodeDep.package_json}`, "dependencies");
        }
        const digest = await fileDigest(info.resolved);
        filesToTrack.push({ role: "dependency", path: info.rel.split(sep).join("/"), sha256: digest });
        normalizedDependencies.node.package_json = info.rel.split(sep).join("/");
      }
      if (nodeDep.lockfile !== undefined) {
        const info = await resolveSafePath(appDir, nodeDep.lockfile, "dependencies.node.lockfile");
        if (!info.exists || !info.isFile) {
          fail("DEPENDENCY_MISSING", `lockfile 파일을 찾을 수 없습니다: ${nodeDep.lockfile}`, "dependencies");
        }
        const digest = await fileDigest(info.resolved);
        filesToTrack.push({ role: "lockfile", path: info.rel.split(sep).join("/"), sha256: digest });
        normalizedDependencies.node.lockfile = info.rel.split(sep).join("/");
      }
    }

    if (manifest.dependencies.python !== undefined) {
      const pyDep = manifest.dependencies.python;
      if (typeof pyDep !== "object" || pyDep === null || Array.isArray(pyDep)) {
        fail("BAD_MANIFEST", "dependencies.python은 객체여야 합니다", "schema");
      }
      for (const k of Object.keys(pyDep)) {
        if (!ALLOWED_PYTHON_DEP_KEYS.has(k)) {
          fail("BAD_MANIFEST", `dependencies.python의 알 수 없는 필드입니다: ${k}`, "schema");
        }
      }
      normalizedDependencies.python = {};
      if (pyDep.requirements !== undefined) {
        const info = await resolveSafePath(appDir, pyDep.requirements, "dependencies.python.requirements");
        if (!info.exists || !info.isFile) {
          fail("DEPENDENCY_MISSING", `requirements 파일을 찾을 수 없습니다: ${pyDep.requirements}`, "dependencies");
        }
        const digest = await fileDigest(info.resolved);
        filesToTrack.push({ role: "dependency", path: info.rel.split(sep).join("/"), sha256: digest });
        normalizedDependencies.python.requirements = info.rel.split(sep).join("/");
      }
      if (pyDep.lockfile !== undefined) {
        const info = await resolveSafePath(appDir, pyDep.lockfile, "dependencies.python.lockfile");
        if (!info.exists || !info.isFile) {
          fail("DEPENDENCY_MISSING", `python lockfile 파일을 찾을 수 없습니다: ${pyDep.lockfile}`, "dependencies");
        }
        const digest = await fileDigest(info.resolved);
        filesToTrack.push({ role: "lockfile", path: info.rel.split(sep).join("/"), sha256: digest });
        normalizedDependencies.python.lockfile = info.rel.split(sep).join("/");
      }
    }
  }

  // Targets validation
  let normalizedTargets;
  if (manifest.targets !== undefined) {
    if (!Array.isArray(manifest.targets) || manifest.targets.some((t) => typeof t !== "string" || !t.trim())) {
      fail("BAD_MANIFEST", "targets는 문자열 배열이어야 합니다", "schema");
    }
    normalizedTargets = [...manifest.targets];
  }

  // Features validation
  let normalizedFeatures;
  if (manifest.features !== undefined) {
    if (!Array.isArray(manifest.features) || manifest.features.some((f) => typeof f !== "string" || !f.trim())) {
      fail("BAD_MANIFEST", "features는 문자열 배열이어야 합니다", "schema");
    }
    normalizedFeatures = [...manifest.features];
  }

  // Config template validation
  let normalizedConfigTemplate;
  if (manifest.config_template !== undefined) {
    if (typeof manifest.config_template !== "string" || !manifest.config_template.endsWith(".json")) {
      fail("BAD_MANIFEST", "config_template은 .json 파일 경로여야 합니다", "schema");
    }
    const configInfo = await resolveSafePath(appDir, manifest.config_template, "config_template");
    if (!configInfo.exists || !configInfo.isFile) {
      fail("CONFIG_TEMPLATE_MISSING", `config_template 파일을 찾을 수 없습니다: ${manifest.config_template}`, "config");
    }
    const configRaw = await readFile(configInfo.resolved, "utf8");
    let configObj;
    try {
      configObj = JSON.parse(configRaw);
    } catch (e) {
      fail("BAD_MANIFEST", `config_template 파일의 JSON 형식이 올바르지 않습니다: ${e.message}`, "config");
    }
    validateConfigTemplate(configObj, configRaw);

    const configDigest = createHash("sha256").update(configRaw).digest("hex");
    filesToTrack.push({
      role: "config_template",
      path: configInfo.rel.split(sep).join("/"),
      sha256: configDigest,
    });
    normalizedConfigTemplate = configInfo.rel.split(sep).join("/");
  }

  // Deduplicate and sort files
  const seenPaths = new Set();
  const uniqueFiles = [];
  for (const f of filesToTrack) {
    if (!seenPaths.has(f.path)) {
      seenPaths.add(f.path);
      uniqueFiles.push(f);
    }
  }
  uniqueFiles.sort((a, b) => a.path.localeCompare(b.path));

  // Compute contract digest
  const digestPayload = JSON.stringify({
    name: manifest.name,
    version: manifest.version,
    definition: defPathInfo.rel.split(sep).join("/"),
    extensions: normalizedExtensions,
    static_assets: normalizedStaticAssets,
    dependencies: normalizedDependencies,
    targets: normalizedTargets,
    features: normalizedFeatures,
    config_template: normalizedConfigTemplate,
    files: uniqueFiles,
  });
  const contractDigest = createHash("sha256").update(digestPayload).digest("hex");

  return {
    ok: true,
    app: {
      name: manifest.name,
      version: manifest.version,
      description: manifest.description,
      definition: defPathInfo.rel.split(sep).join("/"),
      extensions: normalizedExtensions,
      static_assets: normalizedStaticAssets,
      dependencies: normalizedDependencies,
      targets: normalizedTargets,
      features: normalizedFeatures,
      config_template: normalizedConfigTemplate,
    },
    files: uniqueFiles,
    contractDigest,
    packageLayout: {
      bin: "bin/aip",
      definition: defPathInfo.rel.split(sep).join("/"),
      manifest: "aip.app.json",
      config: normalizedConfigTemplate || "config.example.json",
      extensions: normalizedExtensions ? "extensions/" : undefined,
      static: normalizedStaticAssets ? "static/" : undefined,
      sdk: "sdk/aip-sdk-0.1.0.tgz",
      checksums: "SHA256SUMS",
      appChecksums: "APP_SHA256SUMS",
    },
  };
}

// CLI entry point
if (process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))) {
  const args = process.argv.slice(2).filter((arg) => arg !== "--check");
  if (args.length === 0) {
    console.error(JSON.stringify({ ok: false, code: "BAD_ARGS", stage: "arguments", message: "사용법: node product/tools/app-manifest.mjs <app-dir-or-manifest-path>" }));
    process.exitCode = 1;
  } else {
    try {
      const contract = await loadAndValidateAppManifest(args[0]);
      console.log(JSON.stringify(contract, null, 2));
    } catch (error) {
      const failure = error instanceof ManifestFailure ? error : new ManifestFailure("INTERNAL_ERROR", error.message, "manifest");
      console.error(JSON.stringify({ ok: false, code: failure.code, stage: failure.stage, message: failure.message }));
      process.exitCode = 1;
    }
  }
}
