#!/usr/bin/env node
// Real Keycloak (quay.io/keycloak/keycloak:26.2.5) against `aip service`: the token profile a real provider emits, fetched over TLS
// from the provider's own JWKS endpoint. It starts Keycloak in dev mode behind its own HTTPS listener with a throw-away CA,
// configures a realm, two clients and a user through kcadm, asks Keycloak for real access tokens and sends them to a locally
// started service. Needs docker, openssl, psql, a built aip binary (AIP_BINARY or target/debug/aip) and the dedicated test PostgreSQL.
//
// What it pins: a stock Keycloak token has no `nbf` and carries `aud` as an array; the `at+jwt` header type is a client setting
// (`access.token.header.type.rfc9068`); the JWKS has an encryption key next to the signing key.
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import https from "node:https";
import net from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { requireTestDatabase } from "../../product/test-support/postgres-guard.mjs";

const repoDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const testDatabase = requireTestDatabase().raw;
const binary = process.env.AIP_BINARY || resolve(repoDir, "target/debug/aip");
const IMAGE = "quay.io/keycloak/keycloak:26.2.5";
const container = `aip-keycloak-smoke-${randomUUID().slice(0, 8)}`;
const adminPassword = `admin-${randomUUID()}`;
const userPassword = `user-${randomUUID()}`;

const results = [];
function check(name, ok, detail = "") {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok || !detail ? "" : `\n      ${detail}`}`);
}

function run(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", cwd: repoDir, ...options });
  if (result.status !== 0) throw new Error(`${program} ${args[0]} failed (exit ${result.status}): ${(result.stderr || result.stdout).slice(0, 600)}`);
  return result.stdout;
}

const kcadm = (...args) => run("docker", ["exec", container, "/opt/keycloak/bin/kcadm.sh", ...args]);

function freePort() {
  return new Promise((resolvePort, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolvePort(port));
    });
  });
}

function httpsRequest(port, ca, { method = "GET", path, headers = {}, body }) {
  return new Promise((resolveRequest, reject) => {
    const req = https.request({ host: "localhost", port, method, path, headers, ca, servername: "localhost" }, (res) => {
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => resolveRequest({ status: res.statusCode, body: Buffer.concat(chunks).toString() }));
    });
    req.on("error", reject);
    req.setTimeout(15000, () => req.destroy(new Error(`request to ${path} timed out`)));
    req.end(body);
  });
}

const decode = (part) => JSON.parse(Buffer.from(part, "base64url").toString());

async function generateCertificates(dir) {
  await writeFile(join(dir, "ca.cnf"), "[req]\ndistinguished_name=dn\nx509_extensions=v3_ca\nprompt=no\n[dn]\nCN=AIP Keycloak smoke CA\n[v3_ca]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n");
  await writeFile(join(dir, "server.cnf"), "[req]\ndistinguished_name=dn\nreq_extensions=v3_req\nprompt=no\n[dn]\nCN=localhost\n[v3_req]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=@alt\n[alt]\nDNS.1=localhost\nIP.1=127.0.0.1\n");
  run("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-keyout", join(dir, "ca.key"), "-out", join(dir, "kc-ca.crt"), "-config", join(dir, "ca.cnf")]);
  run("openssl", ["req", "-newkey", "rsa:2048", "-nodes", "-keyout", join(dir, "server.key"), "-out", join(dir, "server.csr"), "-config", join(dir, "server.cnf")]);
  run("openssl", ["x509", "-req", "-in", join(dir, "server.csr"), "-CA", join(dir, "kc-ca.crt"), "-CAkey", join(dir, "ca.key"), "-CAcreateserial", "-out", join(dir, "server.crt"), "-days", "2", "-sha256", "-extensions", "v3_req", "-extfile", join(dir, "server.cnf")]);
  run("chmod", ["644", join(dir, "server.key"), join(dir, "server.crt"), join(dir, "kc-ca.crt")]);
}

async function waitForLine(child, predicate, label) {
  return new Promise((resolveLine, reject) => {
    let output = "";
    let errors = "";
    const timer = setTimeout(() => reject(new Error(`${label} did not start: ${errors.trim().slice(0, 400)}`)), 30000);
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk) => { errors += chunk; });
    child.stdout.on("data", (chunk) => {
      output += chunk;
      for (const line of output.split("\n")) {
        if (predicate(line)) { clearTimeout(timer); resolveLine(line); return; }
      }
    });
    child.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`${label} exited early (${code}): ${(output + errors).trim().slice(0, 400)}`));
    });
  });
}

async function main() {
  const work = await mkdtemp(join(tmpdir(), "aip-keycloak-smoke-"));
  const certs = join(work, "certs");
  const schema = `aip_kc_smoke_${randomUUID().replaceAll("-", "")}`;
  const port = await freePort();
  const servicePort = await freePort();
  const issuer = `https://localhost:${port}/realms/aip`;
  let service = null;
  let started = false;
  const cleanup = async () => {
    if (service) { service.kill("SIGTERM"); service = null; }
    if (started) { spawnSync("docker", ["rm", "-f", container]); started = false; }
    spawnSync("psql", ["-X", "-q", "-d", testDatabase, "-c", `DROP SCHEMA IF EXISTS ${schema} CASCADE`]);
    await rm(work, { recursive: true, force: true }).catch(() => {});
  };
  process.on("SIGINT", async () => { await cleanup(); process.exit(1); });

  try {
    console.log("==> 1. Starting Keycloak 26.2.5 with an HTTPS listener...");
    await mkdir(certs, { recursive: true });
    await generateCertificates(certs);
    const ca = await readFile(join(certs, "kc-ca.crt"));
    run("docker", ["run", "-d", "--name", container, "-e", "KC_BOOTSTRAP_ADMIN_USERNAME=admin", "-e", `KC_BOOTSTRAP_ADMIN_PASSWORD=${adminPassword}`,
      "-e", `KC_HOSTNAME=https://localhost:${port}`, "-e", "KC_HTTP_ENABLED=true", "-e", "KC_HTTPS_CERTIFICATE_FILE=/certs/server.crt",
      "-e", "KC_HTTPS_CERTIFICATE_KEY_FILE=/certs/server.key", "-v", `${certs}:/certs:ro`, "-p", `127.0.0.1:${port}:8443`, IMAGE, "start-dev"]);
    started = true;
    let ready = false;
    for (let attempt = 0; attempt < 80 && !ready; attempt++) {
      ready = await httpsRequest(port, ca, { path: "/realms/master" }).then((r) => r.status === 200, () => false);
      if (!ready) await new Promise((r) => setTimeout(r, 3000));
    }
    if (!ready) throw new Error("Keycloak did not become ready");

    console.log("==> 2. Configuring realm, clients and user (kcadm)...");
    kcadm("config", "credentials", "--server", "http://localhost:8080", "--realm", "master", "--user", "admin", "--password", adminPassword);
    kcadm("create", "realms", "-s", "realm=aip", "-s", "enabled=true", "-s", "accessTokenLifespan=300");
    for (const [clientId, profile] of [["aip-web", true], ["aip-stock", false]]) {
      const attrs = profile ? ["-s", "attributes.\"access.token.header.type.rfc9068\"=true"] : [];
      kcadm("create", "clients", "-r", "aip", "-s", `clientId=${clientId}`, "-s", "publicClient=true", "-s", "directAccessGrantsEnabled=true", "-s", "standardFlowEnabled=false", ...attrs);
      if (profile) {
        const id = kcadm("get", "clients", "-r", "aip", "-q", `clientId=${clientId}`, "--fields", "id", "--format", "csv", "--noquotes").trim();
        kcadm("create", `clients/${id}/protocol-mappers/models`, "-r", "aip", "-s", "name=aip-audience", "-s", "protocol=openid-connect", "-s", "protocolMapper=oidc-audience-mapper",
          "-s", "config.\"included.custom.audience\"=aip-api", "-s", "config.\"access.token.claim\"=true", "-s", "config.\"id.token.claim\"=false");
      }
    }
    kcadm("create", "users", "-r", "aip", "-s", "username=kc-user", "-s", "enabled=true", "-s", "email=kc-user@example.test", "-s", "emailVerified=true", "-s", "firstName=K", "-s", "lastName=U");
    kcadm("set-password", "-r", "aip", "--username", "kc-user", "--new-password", userPassword);

    const accessToken = async (clientId) => {
      const form = new URLSearchParams({ grant_type: "password", client_id: clientId, username: "kc-user", password: userPassword, scope: "openid" }).toString();
      const res = await httpsRequest(port, ca, { method: "POST", path: "/realms/aip/protocol/openid-connect/token", headers: { "content-type": "application/x-www-form-urlencoded" }, body: form });
      const token = JSON.parse(res.body || "{}").access_token;
      if (!token) throw new Error(`no access token for ${clientId}: ${res.body.slice(0, 200)}`);
      return token;
    };
    const profileToken = await accessToken("aip-web");
    const stockToken = await accessToken("aip-stock");
    const [rawHeader, rawClaims] = profileToken.split(".");
    const header = decode(rawHeader);
    const claims = decode(rawClaims);

    console.log("==> 3. What a real Keycloak token looks like...");
    check("with the RFC 9068 client setting the header type is at+jwt and the algorithm RS256", header.typ === "at+jwt" && header.alg === "RS256", JSON.stringify(header));
    check("a real Keycloak access token carries no nbf claim", !("nbf" in claims), JSON.stringify(Object.keys(claims)));
    check("iat and exp are present with a lifetime inside the 900 second profile", Number.isInteger(claims.iat) && claims.exp - claims.iat > 0 && claims.exp - claims.iat <= 900, `${claims.iat} ${claims.exp}`);
    check("aud is an array that contains the API audience", Array.isArray(claims.aud) && claims.aud.includes("aip-api"), JSON.stringify(claims.aud));
    check("the issuer is the realm URL", claims.iss === issuer, claims.iss);
    check("a client without the setting issues a header type the profile refuses", decode(stockToken.split(".")[0]).typ !== "at+jwt", JSON.stringify(decode(stockToken.split(".")[0])));

    console.log("==> 4. Starting aip service against Keycloak's own HTTPS JWKS...");
    const appPath = join(work, "app.aip");
    await writeFile(appPath, await readFile(join(repoDir, "spikes", "spike-v6-transport", "tests", "fixtures", "jobs.aip")));
    const configPath = join(work, "service.json");
    await writeFile(configPath, JSON.stringify({
      schema, database_url_env: "AIP_TEST_DATABASE", listen: `127.0.0.1:${servicePort}`, wire: "safe",
      auth: { issuer, audience: "aip-api", jwks: { kind: "https", url: `${issuer}/protocol/openid-connect/certs` } },
      workers: { directory: join(repoDir, "spikes", "spike-v6-transport", "tests", "fixtures", "jobs"), language: "node" },
    }, null, 2));
    const env = { ...process.env, AIP_TEST_DATABASE: testDatabase, SSL_CERT_FILE: join(certs, "kc-ca.crt") };
    run(binary, ["service", "init", appPath, "--config", configPath], { env });
    run(binary, ["service", "principal", "--config", configPath, "bind", "--subject", claims.sub, "--actor", "42"], { env });
    const bindingPath = join(work, "binding.ts");
    run(binary, ["service", "gen", appPath, "--wire", "safe", "--out", bindingPath, "--sdk-import", "@aip/sdk"], { env });
    const fingerprint = (await readFile(bindingPath, "utf8")).match(/contractFingerprint = "([a-f0-9]{64})"/u)?.[1];
    if (!fingerprint) throw new Error("contract fingerprint missing from generated binding");
    service = spawn(binary, ["service", "serve", appPath, "--config", configPath], { cwd: repoDir, env, stdio: ["ignore", "pipe", "pipe"] });
    await waitForLine(service, (line) => line.includes('"listening"'), "aip service");
    check("the service starts with Keycloak's JWKS (signing and encryption keys) configured", true);

    const post = async (path, token, body) => {
      const headers = { "content-type": "application/json", "x-aip-contract": fingerprint, ...(token ? { authorization: `Bearer ${token}` } : {}) };
      const res = await fetch(`http://127.0.0.1:${servicePort}${path}`, { method: "POST", headers, body: JSON.stringify(body) });
      return { status: res.status, body: await res.json().catch(() => ({})) };
    };

    console.log("==> 5. Real tokens against the service...");
    const session = await post("/session", profileToken, {});
    check("a real Keycloak token (no nbf, aud array) is accepted and maps to the bound actor", session.status === 200 && session.body.ok === true && session.body.principal?.actorId === "42", JSON.stringify(session));
    const operation = await post("/operation", profileToken, { operation: "length", input: { text: "가😀A" } });
    check("an operation call with the real token returns its declared output (body checked)", operation.status === 200 && operation.body.ok === true && operation.body.output?.length === 3, JSON.stringify(operation));
    const stock = await post("/session", stockToken, {});
    check("the stock Keycloak token (no at+jwt, no audience mapper) is refused with WIRE.UNAUTHENTICATED", stock.body.ok === false && stock.body.code === "WIRE.UNAUTHENTICATED", JSON.stringify(stock));
    const anonymous = await post("/operation", null, { operation: "length", input: { text: "x" } });
    check("the same call without a token is refused in the body", anonymous.body.ok === false && anonymous.body.code === "WIRE.UNAUTHENTICATED", JSON.stringify(anonymous));
    run(binary, ["service", "principal", "--config", configPath, "revoke", "--subject", claims.sub], { env });
    const revoked = await post("/session", profileToken, {});
    check("after revoking the principal the same real token is refused", revoked.body.ok === false && revoked.body.code === "WIRE.UNAUTHENTICATED", JSON.stringify(revoked));
  } finally {
    await cleanup();
  }
  const failed = results.filter((r) => !r.ok);
  if (failed.length > 0) {
    console.error(`\n[FAIL] ${failed.length} of ${results.length} checks failed.`);
    process.exit(1);
  }
  console.log(`\n[SUCCESS] All ${results.length} checks passed against Keycloak 26.2.5.`);
}

main().catch(async (error) => {
  console.error("FATAL:", error.message);
  spawnSync("docker", ["rm", "-f", container]);
  process.exit(1);
});
