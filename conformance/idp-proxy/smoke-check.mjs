#!/usr/bin/env node
import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import https from "node:https";
import net from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";

const conformanceDir = resolve(dirname(fileURLToPath(import.meta.url)));
const repoDir = resolve(conformanceDir, "..", "..");
const testDatabase = process.env.AIP_TEST_DATABASE || "host=localhost dbname=postgres";
const binary = process.env.AIP_BINARY || resolve(repoDir, "target/debug/aip");
const composeFile = join(conformanceDir, "docker-compose.yml");
const certsDir = join(conformanceDir, "certs");

const results = [];
function check(name, ok, detail = "") {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok || !detail ? "" : `\n      ${detail}`}`);
}

function run(program, programArgs, options = {}) {
  const result = spawnSync(program, programArgs, { encoding: "utf8", cwd: repoDir, ...options });
  if (result.status !== 0) {
    throw new Error(`${program} ${programArgs[0]} failed (exit ${result.status}): ${result.stderr || result.stdout}`);
  }
  return result.stdout;
}

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

function request(port, ca, { method = "GET", path, headers = {}, body }) {
  return new Promise((resolveRequest, reject) => {
    const req = https.request(
      {
        host: "localhost",
        port,
        method,
        path,
        headers,
        ca,
        servername: "localhost",
        rejectUnauthorized: true,
      },
      (res) => {
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => {
          resolveRequest({
            status: res.statusCode,
            headers: res.headers,
            body: Buffer.concat(chunks).toString(),
          });
        });
      }
    );
    req.on("error", reject);
    req.setTimeout(10000, () => req.destroy(new Error(`request to ${path} timed out`)));
    req.end(body);
  });
}

async function waitForLine(child, predicate, label) {
  return new Promise((resolveLine, reject) => {
    let output = "";
    let errors = "";
    const timer = setTimeout(() => reject(new Error(`${label} did not start: ${errors.trim().slice(0, 400)}`)), 20000);
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
  const work = await mkdtemp(join(tmpdir(), "aip-idp-proxy-smoke-"));
  const schema = `aip_idp_smoke_${randomUUID().replaceAll("-", "")}`;
  const allowedOrigin = "https://app.example.test";
  const disallowedOrigin = "https://unauthorized.attacker.test";

  let serviceChild = null;
  let containersStarted = false;

  const cleanup = async () => {
    console.log("==> Cleaning up idp-proxy smoke environment...");
    if (serviceChild) {
      serviceChild.kill("SIGTERM");
      serviceChild = null;
    }
    if (containersStarted) {
      run("docker", ["compose", "-f", composeFile, "down", "-v"]);
      containersStarted = false;
    }
    run("psql", ["-X", "-q", "-d", testDatabase, "-c", `DROP SCHEMA IF EXISTS ${schema} CASCADE`]);
    await rm(certsDir, { recursive: true, force: true }).catch(() => {});
    await rm(work, { recursive: true, force: true }).catch(() => {});
  };

  process.on("SIGINT", async () => { await cleanup(); process.exit(1); });
  process.on("SIGTERM", async () => { await cleanup(); process.exit(1); });

  try {
    console.log("==> 1. Generating certificates for Caddy TLS Reverse Proxy...");
    await rm(certsDir, { recursive: true, force: true }).catch(() => {});
    const fs = await import("node:fs/promises");
    await fs.mkdir(certsDir, { recursive: true });

    const caCnf = `[req]
distinguished_name=dn
x509_extensions=v3_ca
prompt=no
[dn]
CN=AIP Smoke Root CA
[v3_ca]
basicConstraints=critical,CA:TRUE
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
`;
    const serverCnf = `[req]
distinguished_name=dn
req_extensions=v3_req
prompt=no
[dn]
CN=localhost
[v3_req]
basicConstraints=critical,CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=@alt_names
[alt_names]
DNS.1=localhost
DNS.2=host.docker.internal
IP.1=127.0.0.1
`;
    await writeFile(join(certsDir, "ca.cnf"), caCnf);
    await writeFile(join(certsDir, "server.cnf"), serverCnf);

    run("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
      "-keyout", join(certsDir, "ca.key"), "-out", join(certsDir, "ca.pem"), "-config", join(certsDir, "ca.cnf")]);
    run("openssl", ["req", "-newkey", "rsa:2048", "-nodes",
      "-keyout", join(certsDir, "key.pem"), "-out", join(certsDir, "server.csr"), "-config", join(certsDir, "server.cnf")]);
    run("openssl", ["x509", "-req", "-in", join(certsDir, "server.csr"), "-CA", join(certsDir, "ca.pem"),
      "-CAkey", join(certsDir, "ca.key"), "-CAcreateserial", "-out", join(certsDir, "cert.pem"),
      "-days", "2", "-sha256", "-extensions", "v3_req", "-extfile", join(certsDir, "server.cnf")]);
    run("chmod", ["644", join(certsDir, "cert.pem"), join(certsDir, "key.pem"), join(certsDir, "ca.pem")]);
    const ca = await readFile(join(certsDir, "ca.pem"));

    const backendPort = await freePort();
    console.log(`==> 2. Starting Caddy and OIDC IdP containers (backendPort=${backendPort})...`);
    run("docker", ["compose", "-f", composeFile, "up", "-d"], {
      env: { ...process.env, AIP_BACKEND_PORT: String(backendPort) },
    });
    containersStarted = true;

    console.log("==> 3. Waiting for Caddy and IdP to become healthy...");
    let idpReady = false;
    for (let i = 0; i < 30; i++) {
      try {
        const res = await request(8444, ca, { path: "/health" });
        if (res.status === 200) {
          idpReady = true;
          break;
        }
      } catch (_) {}
      await new Promise((r) => setTimeout(r, 1000));
    }
    if (!idpReady) {
      console.error("Caddy container logs:\n", run("docker", ["compose", "-f", composeFile, "logs", "caddy"]));
      console.error("IdP container logs:\n", run("docker", ["compose", "-f", composeFile, "logs", "idp"]));
      throw new Error("IdP container via Caddy (port 8444) did not become ready");
    }
    check("OIDC IdP container is healthy behind Caddy TLS proxy", true);

    // Verify OIDC Discovery and JWKS
    const discoveryRes = await request(8444, ca, { path: "/.well-known/openid-configuration" });
    const discovery = JSON.parse(discoveryRes.body || "{}");
    check(
      "OIDC Discovery endpoint returns valid configuration",
      discovery.issuer === "https://localhost:8444" && discovery.jwks_uri === "https://localhost:8444/jwks.json",
      `status=${discoveryRes.status} body=${discoveryRes.body}`
    );

    const jwksRes = await request(8444, ca, { path: "/jwks.json" });
    const jwks = JSON.parse(jwksRes.body || "{}");
    check(
      "OIDC IdP serves RS256 JWKS keys",
      Array.isArray(jwks.keys) && jwks.keys.length > 0 && jwks.keys[0].kty === "RSA",
      `status=${jwksRes.status} body=${jwksRes.body}`
    );

    console.log("==> 4. Initializing aip service with remote HTTPS IdP configuration...");
    const appPath = join(work, "app.aip");
    await writeFile(appPath, await readFile(join(repoDir, "spikes", "spike-v6-transport", "tests", "fixtures", "jobs.aip")));

    const configPath = join(work, "service.json");
    await writeFile(
      configPath,
      JSON.stringify({
        schema,
        database_url_env: "AIP_TEST_DATABASE",
        listen: `127.0.0.1:${backendPort}`,
        wire: "safe",
        allowed_origins: [allowedOrigin],
        auth: {
          issuer: "https://localhost:8444",
          audience: "aip-api",
          jwks: {
            kind: "https",
            url: "https://localhost:8444/jwks.json",
          },
        },
        realtime: { heartbeat_ms: 400, poll_ms: 100 },
      }, null, 2)
    );

    const env = {
      ...process.env,
      AIP_TEST_DATABASE: testDatabase,
      SSL_CERT_FILE: join(certsDir, "ca.pem"),
    };

    run(binary, ["service", "init", appPath, "--config", configPath], { env });
    run(binary, ["service", "principal", "--config", configPath, "bind", "--subject", "user-42", "--actor", "42"], { env });

    const bindingPath = join(work, "binding.ts");
    run(binary, ["service", "gen", appPath, "--wire", "safe", "--out", bindingPath, "--sdk-import", "@aip/sdk"], { env });
    const fingerprintMatch = (await readFile(bindingPath, "utf8")).match(/contractFingerprint = "([a-f0-9]{64})"/u);
    if (!fingerprintMatch) throw new Error("contract fingerprint missing from generated binding");
    const fingerprint = fingerprintMatch[1];

    console.log("==> 5. Launching aip service behind TLS Reverse Proxy...");
    serviceChild = spawn(binary, ["service", "serve", appPath, "--config", configPath], {
      cwd: repoDir,
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });

    const listening = JSON.parse(await waitForLine(serviceChild, (line) => line.includes('"listening"'), "aip service"));
    check("aip service started and bound to designated port", Number(listening.address.split(":").pop()) === backendPort);

    console.log("==> 6. Testing Caddy TLS Reverse Proxy forwarding, headers, and CORS...");
    // Direct TLS to the backend loopback port must fail (backend speaks plain HTTP only)
    const plainDirect = await request(backendPort, ca, { path: "/health" }).catch((err) => ({ status: 0, error: err.message }));
    check("Direct TLS to plain HTTP backend loopback is rejected", plainDirect.status === 0);

    // Health check through Caddy (port 8443)
    const health = await request(8443, ca, { path: "/health" });
    check("GET /health through Caddy TLS proxy returns 200 OK", health.status === 200);

    // Request ID tracking through proxy
    const customReqId = "smoke-req-" + randomUUID();
    const forwardedReq = await request(8443, ca, {
      path: "/health",
      headers: { "x-request-id": customReqId },
    });
    check("x-request-id is preserved across TLS proxy", forwardedReq.headers["x-request-id"] === customReqId);

    const generatedReq = await request(8443, ca, { path: "/health" });
    check("x-request-id is generated when omitted", Boolean(generatedReq.headers["x-request-id"]));

    // Origin and CORS forwarding
    const corsAllowed = await request(8443, ca, {
      path: "/health",
      headers: { origin: allowedOrigin },
    });
    check(
      "Allowed Origin header passes through and receives Access-Control-Allow-Origin",
      corsAllowed.headers["access-control-allow-origin"] === allowedOrigin
    );

    const corsDenied = await request(8443, ca, {
      path: "/health",
      headers: { origin: disallowedOrigin },
    });
    check(
      "Disallowed Origin does not receive Access-Control-Allow-Origin",
      corsDenied.headers["access-control-allow-origin"] !== disallowedOrigin
    );

    console.log("==> 7. Testing authentication against remote OIDC IdP container...");
    // 7.1. Valid JWT access token
    const tokenRes = await request(8444, ca, { path: "/token" });
    const { access_token: validToken } = JSON.parse(tokenRes.body || "{}");
    if (!validToken) throw new Error("Failed to obtain valid token from IdP container");

    const sessionHeaders = {
      authorization: `Bearer ${validToken}`,
      "content-type": "application/json",
      "x-aip-contract": fingerprint,
    };
    const sessionRes = await request(8443, ca, {
      method: "POST",
      path: "/session",
      headers: sessionHeaders,
      body: "{}",
    });
    const sessionBody = JSON.parse(sessionRes.body || "{}");
    check(
      "Valid IdP access token verified via remote HTTPS JWKS and maps to actor",
      sessionRes.status === 200 && sessionBody.ok === true && sessionBody.principal?.actorId === "42",
      `status=${sessionRes.status} body=${sessionRes.body}`
    );

    // 7.2. Expired token rejection
    const expiredRes = await request(8444, ca, { path: "/token?expired=true" });
    const { access_token: expiredToken } = JSON.parse(expiredRes.body || "{}");
    const expiredSession = await request(8443, ca, {
      method: "POST",
      path: "/session",
      headers: { ...sessionHeaders, authorization: `Bearer ${expiredToken}` },
      body: "{}",
    });
    const expiredBody = JSON.parse(expiredSession.body || "{}");
    check("Expired JWT token is rejected (WIRE.TOKEN_EXPIRED)", expiredBody.ok === false && expiredBody.code === "WIRE.TOKEN_EXPIRED", expiredSession.body);

    // 7.3. Mismatched audience rejection
    const badAudRes = await request(8444, ca, { path: "/token?bad_aud=true" });
    const { access_token: badAudToken } = JSON.parse(badAudRes.body || "{}");
    const badAudSession = await request(8443, ca, {
      method: "POST",
      path: "/session",
      headers: { ...sessionHeaders, authorization: `Bearer ${badAudToken}` },
      body: "{}",
    });
    const badAudBody = JSON.parse(badAudSession.body || "{}");
    check("JWT with mismatched audience is rejected (WIRE.UNAUTHENTICATED)", badAudBody.ok === false && badAudBody.code === "WIRE.UNAUTHENTICATED", badAudSession.body);

    // 7.4. Mismatched issuer rejection
    const badIssRes = await request(8444, ca, { path: "/token?bad_iss=true" });
    const { access_token: badIssToken } = JSON.parse(badIssRes.body || "{}");
    const badIssSession = await request(8443, ca, {
      method: "POST",
      path: "/session",
      headers: { ...sessionHeaders, authorization: `Bearer ${badIssToken}` },
      body: "{}",
    });
    const badIssBody = JSON.parse(badIssSession.body || "{}");
    check("JWT with mismatched issuer is rejected (WIRE.UNAUTHENTICATED)", badIssBody.ok === false && badIssBody.code === "WIRE.UNAUTHENTICATED", badIssSession.body);

    // 7.5. Keycloak default header type (typ: Bearer) rejection
    const badTypRes = await request(8444, ca, { path: "/token?bad_typ=true" });
    const { access_token: badTypToken } = JSON.parse(badTypRes.body || "{}");
    const badTypSession = await request(8443, ca, {
      method: "POST",
      path: "/session",
      headers: { ...sessionHeaders, authorization: `Bearer ${badTypToken}` },
      body: "{}",
    });
    const badTypBody = JSON.parse(badTypSession.body || "{}");
    check("Token without typ=at+jwt (e.g. standard Keycloak Bearer) is rejected (WIRE.UNAUTHENTICATED)", badTypBody.ok === false && badTypBody.code === "WIRE.UNAUTHENTICATED", badTypSession.body);

    // 7.6. Unknown subject rejection
    const unknownSubRes = await request(8444, ca, { path: "/token?unknown_sub=true" });
    const { access_token: unknownSubToken } = JSON.parse(unknownSubRes.body || "{}");
    const unknownSubSession = await request(8443, ca, {
      method: "POST",
      path: "/session",
      headers: { ...sessionHeaders, authorization: `Bearer ${unknownSubToken}` },
      body: "{}",
    });
    const unknownSubBody = JSON.parse(unknownSubSession.body || "{}");
    check("Token for unbound subject is rejected (WIRE.UNAUTHENTICATED)", unknownSubBody.ok === false && unknownSubBody.code === "WIRE.UNAUTHENTICATED", unknownSubSession.body);

    // 7.7. Perform authenticated read through Caddy
    const readRes = await request(8443, ca, {
      method: "POST",
      path: "/read",
      headers: sessionHeaders,
      body: JSON.stringify({ select: { job: { "*": {} } } }),
    });
    check("Authenticated read through TLS reverse proxy succeeds", readRes.status === 200, `status=${readRes.status} body=${readRes.body}`);

    console.log("==> All IdP and TLS reverse proxy smoke tests passed!");
  } finally {
    await cleanup();
  }

  const failed = results.filter((r) => !r.ok);
  if (failed.length > 0) {
    console.error(`\n[FAIL] ${failed.length} checks failed.`);
    process.exit(1);
  } else {
    console.log(`\n[SUCCESS] All ${results.length} checks passed!`);
  }
}

main().catch((err) => {
  console.error("FATAL:", err);
  process.exit(1);
});
