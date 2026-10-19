import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const repoDir = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const manifestScript = join(repoDir, "product/tools/app-manifest.mjs");
const packageScript = join(repoDir, "product/tools/package.mjs");

function runCommand(program, args, options = {}) {
  return spawnSync(program, args, {
    cwd: options.cwd ?? repoDir,
    encoding: "utf8",
    timeout: options.timeout ?? 20_000,
    maxBuffer: 4 * 1024 * 1024,
    ...options,
  });
}

function createFixtureApp1(baseDir) {
  const appDir = join(baseDir, "event-app");
  mkdirSync(join(appDir, "extensions"), { recursive: true });
  mkdirSync(join(appDir, "public"), { recursive: true });

  writeFileSync(join(appDir, "app.aip"), `
resource Event {
  read
  transition mark {
    set checked = true
  }
}
`);
  writeFileSync(join(appDir, "extensions/event.mjs"), `
export async function confirm(event) {
  return { ok: true, confirmed: true };
}
`);
  writeFileSync(join(appDir, "public/index.html"), "<h1>Event App</h1>\n");
  writeFileSync(join(appDir, "package.json"), JSON.stringify({ name: "event-app-worker", type: "module" }, null, 2) + "\n");
  writeFileSync(join(appDir, "package-lock.json"), JSON.stringify({ name: "event-app-worker", lockfileVersion: 3 }, null, 2) + "\n");
  writeFileSync(join(appDir, "config.example.json"), JSON.stringify({
    schema: "aip_event",
    database_url_env: "AIP_DATABASE_URL",
    listen: "127.0.0.1:8080",
    wire: "decimal",
    auth: {
      issuer: "https://issuer.example.com",
      audience: "aip-api",
      jwks: { kind: "file", path: "./jwks.example.json" },
    },
  }, null, 2) + "\n");
  writeFileSync(join(appDir, "jwks.example.json"), JSON.stringify({ keys: [{ kty: "RSA", kid: "test-key", n: "abc", e: "AQAB" }] }, null, 2) + "\n");

  const manifest = {
    $schema: "https://aip.dev/schemas/app-manifest-v1.json",
    name: "event-app",
    version: "1.0.0",
    description: "Event Management App on AIP",
    definition: "./app.aip",
    extensions: {
      directory: "./extensions",
      language: "node",
      entrypoints: ["event.mjs"],
      enable_writes: true,
    },
    static_assets: [
      { mount: "/public", path: "./public" },
    ],
    dependencies: {
      node: {
        package_json: "./package.json",
        lockfile: "./package-lock.json",
      },
    },
    targets: ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"],
    features: ["realtime", "metrics"],
    config_template: "./config.example.json",
  };
  writeFileSync(join(appDir, "aip.app.json"), JSON.stringify(manifest, null, 2) + "\n");
  return appDir;
}

function createFixtureApp2(baseDir) {
  const appDir = join(baseDir, "review-app");
  mkdirSync(join(appDir, "extensions"), { recursive: true });
  mkdirSync(join(appDir, "assets"), { recursive: true });

  writeFileSync(join(appDir, "review.aip"), `
resource Review {
  read
  transition submit {
    set status = 'SUBMITTED'
  }
}
`);
  writeFileSync(join(appDir, "extensions/handler.py"), `
def confirm(review):
    return {"ok": True, "score": 95}
`);
  writeFileSync(join(appDir, "assets/logo.svg"), "<svg></svg>\n");
  writeFileSync(join(appDir, "requirements.txt"), "# Frozen Python dependencies\npytest==8.0.0\n");
  writeFileSync(join(appDir, "config.example.json"), JSON.stringify({
    schema: "aip_review",
    database_url_env: "AIP_DATABASE_URL",
    listen: "127.0.0.1:8081",
    wire: "safe",
    auth: {
      issuer: "https://auth.review.example",
      audience: "review-api",
      jwks: { kind: "file", path: "./jwks.json" },
    },
  }, null, 2) + "\n");
  writeFileSync(join(appDir, "jwks.json"), JSON.stringify({ keys: [{ kty: "RSA", kid: "review-key", n: "xyz", e: "AQAB" }] }, null, 2) + "\n");

  const manifest = {
    $schema: "https://aip.dev/schemas/app-manifest-v1.json",
    name: "review-app",
    version: "2.1.0",
    description: "Review Service on AIP with Python worker",
    definition: "./review.aip",
    extensions: {
      directory: "./extensions",
      language: "python",
      entrypoints: ["handler.py"],
      enable_writes: false,
    },
    static_assets: [
      { mount: "/assets", path: "./assets" },
    ],
    dependencies: {
      python: {
        requirements: "./requirements.txt",
      },
    },
    targets: ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"],
    features: ["outbox", "cgroup"],
    config_template: "./config.example.json",
  };
  writeFileSync(join(appDir, "aip.app.json"), JSON.stringify(manifest, null, 2) + "\n");
  return appDir;
}

test("Valid App 1 (Node extension, static mount, node dependencies) validates and produces AppContract", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const result = runCommand(process.execPath, [manifestScript, join(appDir, "aip.app.json")]);
  assert.equal(result.status, 0, result.stderr);

  const contract = JSON.parse(result.stdout);
  assert.equal(contract.ok, true);
  assert.equal(contract.app.name, "event-app");
  assert.equal(contract.app.version, "1.0.0");
  assert.equal(contract.app.definition, "app.aip");
  assert.equal(contract.app.extensions.language, "node");
  assert.deepEqual(contract.app.extensions.entrypoints, ["event.mjs"]);
  assert.equal(contract.app.extensions.enable_writes, true);
  assert.equal(contract.app.static_assets.length, 1);
  assert.equal(contract.app.static_assets[0].mount, "/public");
  assert.ok(contract.contractDigest && contract.contractDigest.length === 64);

  // Verify file roles and sha256 checksums
  const manifestFile = contract.files.find((f) => f.role === "manifest");
  assert.ok(manifestFile);
  assert.equal(manifestFile.path, "aip.app.json");

  const definitionFile = contract.files.find((f) => f.role === "definition");
  assert.ok(definitionFile);
  assert.equal(definitionFile.path, "app.aip");

  const extFile = contract.files.find((f) => f.role === "extension");
  assert.ok(extFile);
  assert.equal(extFile.path, "extensions/event.mjs");
});

test("Valid App 2 (Python extension, requirements.txt, static assets) validates and produces AppContract", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp2(root);
  const result = runCommand(process.execPath, [manifestScript, appDir]);
  assert.equal(result.status, 0, result.stderr);

  const contract = JSON.parse(result.stdout);
  assert.equal(contract.ok, true);
  assert.equal(contract.app.name, "review-app");
  assert.equal(contract.app.version, "2.1.0");
  assert.equal(contract.app.extensions.language, "python");
  assert.deepEqual(contract.app.extensions.entrypoints, ["handler.py"]);
  assert.equal(contract.app.dependencies.python.requirements, "requirements.txt");
  assert.deepEqual(contract.app.targets, ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]);
  assert.deepEqual(contract.app.features, ["outbox", "cgroup"]);
});

test("Missing manifest file is rejected with MANIFEST_MISSING", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const missingPath = join(root, "non-existent", "aip.app.json");
  const result = runCommand(process.execPath, [manifestScript, missingPath]);
  assert.equal(result.status, 1);
  const err = JSON.parse(result.stderr);
  assert.equal(err.code, "MANIFEST_MISSING");
});

test("Malformed JSON manifest is rejected with BAD_MANIFEST", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const badJsonPath = join(root, "aip.app.json");
  writeFileSync(badJsonPath, "{ name: 'unquoted' invalid json }");
  const result = runCommand(process.execPath, [manifestScript, badJsonPath]);
  assert.equal(result.status, 1);
  const err = JSON.parse(result.stderr);
  assert.equal(err.code, "BAD_MANIFEST");
  assert.ok(err.message.includes("JSON"));
});

test("Missing required fields or invalid schema fields are rejected with BAD_MANIFEST", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  // Missing version and definition
  const p1 = join(root, "p1.json");
  writeFileSync(p1, JSON.stringify({ name: "only-name" }));
  let res = runCommand(process.execPath, [manifestScript, p1]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "BAD_MANIFEST");

  // Invalid app name (uppercase and spaces)
  const p2 = join(root, "p2.json");
  writeFileSync(p2, JSON.stringify({ name: "Invalid Name With Spaces", version: "1.0.0", definition: "./app.aip" }));
  res = runCommand(process.execPath, [manifestScript, p2]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "BAD_MANIFEST");

  // Invalid semver
  const p3 = join(root, "p3.json");
  writeFileSync(p3, JSON.stringify({ name: "valid-name", version: "not-a-version", definition: "./app.aip" }));
  res = runCommand(process.execPath, [manifestScript, p3]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "BAD_MANIFEST");

  // Unknown field (deny_unknown_fields)
  const p4 = join(root, "p4.json");
  writeFileSync(p4, JSON.stringify({ name: "valid-name", version: "1.0.0", definition: "./app.aip", unauthorized_field: 123 }));
  res = runCommand(process.execPath, [manifestScript, p4]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "BAD_MANIFEST");
});

test("Path traversal escaping app root is rejected with PATH_ESCAPES_APP", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const manifestPath = join(appDir, "aip.app.json");
  const original = JSON.parse(readFileSync(manifestPath, "utf8"));

  // Traverse via definition
  const escapeDef = { ...original, definition: "../outside.aip" };
  writeFileSync(manifestPath, JSON.stringify(escapeDef, null, 2));
  let res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "PATH_ESCAPES_APP");

  // Traverse via absolute path
  const absoluteDef = { ...original, definition: "/tmp/escape.aip" };
  writeFileSync(manifestPath, JSON.stringify(absoluteDef, null, 2));
  res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "PATH_ESCAPES_APP");

  // Traverse via config_template
  const escapeConfig = { ...original, config_template: "../../secret.json" };
  writeFileSync(manifestPath, JSON.stringify(escapeConfig, null, 2));
  res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "PATH_ESCAPES_APP");

  // Traverse via extensions entrypoint
  const escapeExt = { ...original, extensions: { ...original.extensions, entrypoints: ["../escape.mjs"] } };
  writeFileSync(manifestPath, JSON.stringify(escapeExt, null, 2));
  res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "PATH_ESCAPES_APP");
});

test("Symlink pointing outside app root is rejected with SYMLINK_ESCAPES_APP", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const externalFile = join(root, "external.aip");
  writeFileSync(externalFile, "resource Escaped { read }\n");

  const symlinkedDef = join(appDir, "symlink.aip");
  symlinkSync(externalFile, symlinkedDef);

  const manifestPath = join(appDir, "aip.app.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  manifest.definition = "./symlink.aip";
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

  const res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  const err = JSON.parse(res.stderr);
  assert.equal(err.code, "SYMLINK_ESCAPES_APP");
});

test("Referencing disallowed / secret files (.env, *.pem, .git) is rejected", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const manifestPath = join(appDir, "aip.app.json");
  const original = JSON.parse(readFileSync(manifestPath, "utf8"));

  // Referencing .env in static assets
  writeFileSync(join(appDir, ".env"), "DATABASE_PASSWORD=supersecret\n");
  const withEnv = {
    ...original,
    static_assets: [{ mount: "/secret", path: "./.env" }],
  };
  writeFileSync(manifestPath, JSON.stringify(withEnv, null, 2));
  let res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "DISALLOWED_FILE");

  // Referencing private key .pem
  writeFileSync(join(appDir, "private.pem"), "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgk...\n");
  const withPem = {
    ...original,
    static_assets: [{ mount: "/key", path: "./private.pem" }],
  };
  writeFileSync(manifestPath, JSON.stringify(withPem, null, 2));
  res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "DISALLOWED_FILE");
});

test("Credential embedded in config_template is rejected with CREDENTIAL_LEAK", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const badConfigPath = join(appDir, "leaky.config.json");
  writeFileSync(badConfigPath, JSON.stringify({
    schema: "aip_demo",
    database_url: "postgres://admin:super_secret_password_123@127.0.0.1:5432/db",
  }, null, 2));

  const manifestPath = join(appDir, "aip.app.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  manifest.config_template = "./leaky.config.json";
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

  const res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  const err = JSON.parse(res.stderr);
  assert.equal(err.code, "CREDENTIAL_LEAK");
});

test("Fixed dependency scope: arbitrary files in dependencies are rejected with INVALID_DEPENDENCY", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const manifestPath = join(appDir, "aip.app.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));

  // Attempting to specify an unauthorized dependency file
  writeFileSync(join(appDir, "random.bin"), "binary content\n");
  manifest.dependencies.node.arbitrary_file = "./random.bin";
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

  const res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "BAD_MANIFEST");
});

test("Missing referenced files fail with respective *_MISSING codes", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const manifestPath = join(appDir, "aip.app.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));

  // Missing definition
  manifest.definition = "./absent.aip";
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
  let res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "DEFINITION_MISSING");

  // Non-.aip definition extension
  writeFileSync(join(appDir, "app.txt"), "text definition\n");
  manifest.definition = "./app.txt";
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
  res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "INVALID_DEFINITION_EXTENSION");

  // Missing extension entrypoint
  manifest.definition = "./app.aip";
  manifest.extensions.entrypoints = ["non_existent_event.mjs"];
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
  res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "EXTENSION_MISSING");

  // Language mismatch: Python file given to Node worker
  manifest.extensions.entrypoints = ["event.py"];
  writeFileSync(join(appDir, "extensions/event.py"), "def run(): pass\n");
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
  res = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(res.status, 1);
  assert.equal(JSON.parse(res.stderr).code, "INVALID_EXTENSION_LANGUAGE");
});

test("Negative / contrast checks: field changes alter contract digest and validated output", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const manifestPath = join(appDir, "aip.app.json");

  // Base contract
  const baseRes = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(baseRes.status, 0);
  const baseContract = JSON.parse(baseRes.stdout);

  // Mutate version
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  manifest.version = "1.0.1";
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
  const mutRes = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(mutRes.status, 0);
  const mutContract = JSON.parse(mutRes.stdout);

  assert.notEqual(baseContract.contractDigest, mutContract.contractDigest);
  assert.equal(mutContract.app.version, "1.0.1");

  // Mutate definition content
  writeFileSync(join(appDir, "app.aip"), "resource Event { read transition second { set checked = false } }\n");
  const defMutRes = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(defMutRes.status, 0);
  const defMutContract = JSON.parse(defMutRes.stdout);
  assert.notEqual(mutContract.contractDigest, defMutContract.contractDigest);
});

test("package.mjs CLI integration: accepts --manifest and --validate-only", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const manifestPath = join(appDir, "aip.app.json");

  // Validate only with --manifest
  const valRes = runCommand(process.execPath, [packageScript, "--manifest", manifestPath, "--validate-only"]);
  assert.equal(valRes.status, 0, valRes.stderr);
  const parsed = JSON.parse(valRes.stdout);
  assert.equal(parsed.ok, true);
  assert.equal(parsed.app.name, "event-app");

  // Invalid manifest with --manifest
  const badManifestPath = join(root, "bad.aip.app.json");
  writeFileSync(badManifestPath, JSON.stringify({ name: "missing-version-and-def" }));
  const badRes = runCommand(process.execPath, [packageScript, "--manifest", badManifestPath, "--validate-only"]);
  assert.equal(badRes.status, 1);
  const err = JSON.parse(badRes.stderr);
  assert.equal(err.code, "BAD_MANIFEST");
});

test("package.mjs CLI: --manifest without --validate-only is rejected with BAD_ARGS in WBS 9.1", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const manifestPath = join(appDir, "aip.app.json");

  // --manifest without --validate-only
  const res1 = runCommand(process.execPath, [packageScript, "--manifest", manifestPath]);
  assert.equal(res1.status, 1);
  const err1 = JSON.parse(res1.stderr);
  assert.equal(err1.code, "BAD_ARGS");

  // --validate-only without --manifest
  const res2 = runCommand(process.execPath, [packageScript, "--validate-only"]);
  assert.equal(res2.status, 1);
  const err2 = JSON.parse(res2.stderr);
  assert.equal(err2.code, "BAD_ARGS");
});

test("static_assets tracks files, changes contractDigest on asset modification, and rejects disallowed/symlink files", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const manifestPath = join(appDir, "aip.app.json");

  // Initial validation
  const baseRes = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(baseRes.status, 0, baseRes.stderr);
  const baseContract = JSON.parse(baseRes.stdout);
  const staticFile = baseContract.files.find((f) => f.path === "public/index.html");
  assert.ok(staticFile, "public/index.html should be tracked in contract files");
  assert.equal(staticFile.role, "static_asset");

  // Asset content modification must change contractDigest
  writeFileSync(join(appDir, "public/index.html"), "<h1>Modified Event App</h1>\n");
  const modRes = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(modRes.status, 0, modRes.stderr);
  const modContract = JSON.parse(modRes.stdout);
  assert.notEqual(baseContract.contractDigest, modContract.contractDigest);

  // Asset addition must change contractDigest
  writeFileSync(join(appDir, "public/style.css"), "body { color: red; }\n");
  const addRes = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(addRes.status, 0, addRes.stderr);
  const addContract = JSON.parse(addRes.stdout);
  assert.notEqual(modContract.contractDigest, addContract.contractDigest);
  assert.ok(addContract.files.some((f) => f.path === "public/style.css"));

  // Disallowed file (.env) nested inside static directory is rejected with DISALLOWED_FILE
  writeFileSync(join(appDir, "public/.env"), "SECRET=leak\n");
  const disallowRes = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(disallowRes.status, 1);
  assert.equal(JSON.parse(disallowRes.stderr).code, "DISALLOWED_FILE");
  rmSync(join(appDir, "public/.env"));

  // Symlink pointing outside app root nested inside static directory is rejected with SYMLINK_ESCAPES_APP
  const outsideFile = join(root, "outside.txt");
  writeFileSync(outsideFile, "outside content\n");
  symlinkSync(outsideFile, join(appDir, "public/outside-link.txt"));
  const symlinkRes = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(symlinkRes.status, 1);
  assert.equal(JSON.parse(symlinkRes.stderr).code, "SYMLINK_ESCAPES_APP");
});

test("Manifest file itself cannot be a symlink pointing outside app root", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const outsideManifest = join(root, "outside-manifest.json");
  writeFileSync(outsideManifest, readFileSync(join(appDir, "aip.app.json")));
  rmSync(join(appDir, "aip.app.json"));
  symlinkSync(outsideManifest, join(appDir, "aip.app.json"));

  // Directory input
  const dirRes = runCommand(process.execPath, [manifestScript, appDir]);
  assert.equal(dirRes.status, 1);
  assert.equal(JSON.parse(dirRes.stderr).code, "SYMLINK_ESCAPES_APP");

  // File input
  const fileRes = runCommand(process.execPath, [manifestScript, join(appDir, "aip.app.json")]);
  assert.equal(fileRes.status, 1);
  assert.equal(JSON.parse(fileRes.stderr).code, "SYMLINK_ESCAPES_APP");
});

test("Dependencies reject explicit falsy / invalid values with BAD_MANIFEST", (t) => {
  const root = mkdtempSync(join(tmpdir(), "aip-manifest-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const appDir = createFixtureApp1(root);
  const manifestPath = join(appDir, "aip.app.json");

  const originalText = readFileSync(manifestPath, "utf8");

  for (const badValue of ["", false, 0, null]) {
    // node.package_json
    const m1 = JSON.parse(originalText);
    m1.dependencies.node.package_json = badValue;
    writeFileSync(manifestPath, JSON.stringify(m1, null, 2));
    let res = runCommand(process.execPath, [manifestScript, manifestPath]);
    assert.equal(res.status, 1, `Expected failure for node.package_json = ${JSON.stringify(badValue)}`);
    assert.equal(JSON.parse(res.stderr).code, "BAD_MANIFEST");

    // node.lockfile
    const m2 = JSON.parse(originalText);
    m2.dependencies.node.lockfile = badValue;
    writeFileSync(manifestPath, JSON.stringify(m2, null, 2));
    res = runCommand(process.execPath, [manifestScript, manifestPath]);
    assert.equal(res.status, 1, `Expected failure for node.lockfile = ${JSON.stringify(badValue)}`);
    assert.equal(JSON.parse(res.stderr).code, "BAD_MANIFEST");

    // python.requirements
    const m3 = JSON.parse(originalText);
    delete m3.dependencies.node;
    m3.dependencies.python = { requirements: badValue };
    writeFileSync(manifestPath, JSON.stringify(m3, null, 2));
    res = runCommand(process.execPath, [manifestScript, manifestPath]);
    assert.equal(res.status, 1, `Expected failure for python.requirements = ${JSON.stringify(badValue)}`);
    assert.equal(JSON.parse(res.stderr).code, "BAD_MANIFEST");

    // python.lockfile
    const m4 = JSON.parse(originalText);
    delete m4.dependencies.node;
    m4.dependencies.python = { lockfile: badValue };
    writeFileSync(manifestPath, JSON.stringify(m4, null, 2));
    res = runCommand(process.execPath, [manifestScript, manifestPath]);
    assert.equal(res.status, 1, `Expected failure for python.lockfile = ${JSON.stringify(badValue)}`);
    assert.equal(JSON.parse(res.stderr).code, "BAD_MANIFEST");
  }

  // Valid empty objects are accepted
  const mEmpty = JSON.parse(readFileSync(manifestPath, "utf8"));
  mEmpty.dependencies = {};
  writeFileSync(manifestPath, JSON.stringify(mEmpty, null, 2));
  let resEmpty = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(resEmpty.status, 0);

  mEmpty.dependencies = { node: {} };
  writeFileSync(manifestPath, JSON.stringify(mEmpty, null, 2));
  resEmpty = runCommand(process.execPath, [manifestScript, manifestPath]);
  assert.equal(resEmpty.status, 0);
});
