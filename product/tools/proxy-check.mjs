#!/usr/bin/env node
// Verifies that a real TLS reverse proxy (Caddy) in front of `aip service` keeps Origin, headers,
// request ids, SSE streaming and WebSocket upgrades intact. It starts everything locally:
// a temporary schema in the local PostgreSQL, the service on loopback, and Caddy with a
// self-signed certificate. Requires `caddy`, `openssl`, `psql` and a built `target/debug/aip`.
//
// Usage: node product/tools/proxy-check.mjs [--bin target/debug/aip] [--caddy caddy]
import { spawn, spawnSync } from "node:child_process";
import { generateKeyPairSync, randomBytes, randomUUID, sign } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import https from "node:https";
import net from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import tls from "node:tls";
import { fileURLToPath } from "node:url";

const repoDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const testDatabase = "host=localhost dbname=postgres";
const issuer = "https://issuer.example";
const audience = "aip-api";
const allowedOrigin = "https://app.example.test";

const args = process.argv.slice(2);
const option = (name, fallback) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
const binary = resolve(repoDir, option("--bin", "target/debug/aip"));
const caddyBinary = option("--caddy", "caddy");

const results = [];
function check(name, ok, detail = "") {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok || !detail ? "" : `\n      ${detail}`}`);
}

function run(program, programArgs, options = {}) {
  const result = spawnSync(program, programArgs, { encoding: "utf8", cwd: repoDir, ...options });
  if (result.status !== 0) throw new Error(`${program} ${programArgs[0]} failed: ${result.stderr || result.stdout}`);
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

function jwt(jwk, privateKey) {
  const now = Math.floor(Date.now() / 1000);
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const unsigned = `${encode({ alg: "RS256", kid: jwk.kid, typ: "at+jwt" })}.${encode({ iss: issuer, aud: audience, sub: "user-42", iat: now, nbf: now, exp: now + 600 })}`;
  return `${unsigned}.${sign("RSA-SHA256", Buffer.from(unsigned), privateKey).toString("base64url")}`;
}

function request(base, ca, { method = "GET", path, headers = {}, body, expectStream = false }) {
  return new Promise((resolveRequest, reject) => {
    const req = https.request({ host: "localhost", port: base.port, method, path, headers, ca, servername: "localhost" }, (res) => {
      if (expectStream) {
        // Resolve on the first body chunk: a buffering proxy would hold it back until the stream ends.
        const started = Date.now();
        res.once("data", (chunk) => {
          resolveRequest({ status: res.statusCode, headers: res.headers, firstChunk: chunk.toString(), firstChunkMs: Date.now() - started });
          req.destroy();
        });
        res.once("error", () => {});
        return;
      }
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => resolveRequest({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks).toString() }));
    });
    req.on("error", (error) => {
      if (!expectStream) reject(error);
    });
    req.setTimeout(8000, () => req.destroy(new Error("request timed out")));
    req.end(body);
  });
}

// Minimal RFC 6455 client: enough to upgrade through the proxy, send one masked text frame and read one frame.
function webSocket(base, ca, { origin }) {
  return new Promise((resolveSocket, reject) => {
    const socket = tls.connect({ host: "localhost", port: base.port, ca, servername: "localhost" }, () => {
      const key = randomBytes(16).toString("base64");
      const headers = [
        "GET /events/socket HTTP/1.1", "Host: localhost", "Upgrade: websocket", "Connection: Upgrade",
        `Sec-WebSocket-Key: ${key}`, "Sec-WebSocket-Version: 13", ...(origin ? [`Origin: ${origin}`] : []), "", "",
      ];
      socket.write(headers.join("\r\n"));
    });
    let buffer = Buffer.alloc(0);
    let upgraded = false;
    const frames = [];
    let waiter;
    const parseFrames = () => {
      for (;;) {
        if (buffer.length < 2) return;
        let length = buffer[1] & 0x7f;
        let offset = 2;
        if (length === 126) { if (buffer.length < 4) return; length = buffer.readUInt16BE(2); offset = 4; }
        if (buffer.length < offset + length) return;
        frames.push({ opcode: buffer[0] & 0x0f, payload: buffer.subarray(offset, offset + length).toString() });
        buffer = buffer.subarray(offset + length);
      }
    };
    const api = {
      send(text) {
        const payload = Buffer.from(text);
        const mask = randomBytes(4);
        const masked = Buffer.from(payload.map((byte, index) => byte ^ mask[index % 4]));
        const header = payload.length < 126 ? Buffer.from([0x81, 0x80 | payload.length]) : Buffer.from([0x81, 0x80 | 126, payload.length >> 8, payload.length & 255]);
        socket.write(Buffer.concat([header, mask, masked]));
      },
      next(timeoutMs = 4000) {
        return new Promise((resolveFrame, rejectFrame) => {
          const timer = setTimeout(() => rejectFrame(new Error("no WebSocket frame")), timeoutMs);
          const poll = () => {
            const frame = frames.find((candidate) => candidate.opcode === 1);
            if (frame) {
              frames.splice(frames.indexOf(frame), 1);
              clearTimeout(timer);
              resolveFrame(frame.payload);
            } else waiter = poll;
          };
          poll();
        });
      },
      close: () => socket.destroy(),
    };
    socket.on("data", (chunk) => {
      buffer = Buffer.concat([buffer, chunk]);
      if (!upgraded) {
        const end = buffer.indexOf("\r\n\r\n");
        if (end < 0) return;
        const head = buffer.subarray(0, end).toString();
        buffer = buffer.subarray(end + 4);
        const status = Number(head.split(" ")[1]);
        if (status !== 101) {
          socket.destroy();
          resolveSocket({ status, api: null });
          return;
        }
        upgraded = true;
        resolveSocket({ status, api });
      }
      parseFrames();
      waiter?.();
    });
    socket.on("error", reject);
    socket.setTimeout(8000, () => socket.destroy(new Error("WebSocket timed out")));
  });
}

async function waitForLine(child, predicate, label) {
  return new Promise((resolveLine, reject) => {
    let output = "";
    const timer = setTimeout(() => reject(new Error(`${label} did not start`)), 20000);
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      output += chunk;
      for (const line of output.split("\n")) {
        if (predicate(line)) { clearTimeout(timer); resolveLine(line); return; }
      }
    });
    child.once("exit", (code) => { clearTimeout(timer); reject(new Error(`${label} exited early (${code}): ${output}`)); });
  });
}

async function main() {
  const work = await mkdtemp(join(tmpdir(), "aip-proxy-check-"));
  const schema = `aip_proxy_${randomUUID().replaceAll("-", "")}`;
  let service;
  let proxy;
  let schemaCreated = false;
  try {
    run("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", join(work, "key.pem"), "-out", join(work, "cert.pem"), "-days", "1",
      "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1"], { stdio: ["ignore", "pipe", "pipe"] });
    const ca = await readFile(join(work, "cert.pem"));

    const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
    const jwk = publicKey.export({ format: "jwk" });
    Object.assign(jwk, { kid: "proxy-check", use: "sig", alg: "RS256", key_ops: ["verify"] });
    const jwksPath = join(work, "jwks.json");
    await writeFile(jwksPath, JSON.stringify({ keys: [jwk] }));
    const token = jwt(jwk, privateKey);

    const appPath = join(work, "app.aip");
    await writeFile(appPath, await readFile(join(repoDir, "spikes", "spike-v6-transport", "tests", "fixtures", "jobs.aip")));
    const configPath = join(work, "service.json");
    await writeFile(configPath, JSON.stringify({
      schema, database_url_env: "AIP_TEST_DATABASE", listen: "127.0.0.1:0", wire: "safe", allowed_origins: [allowedOrigin],
      auth: { issuer, audience, jwks: { kind: "file", path: jwksPath } },
      realtime: { heartbeat_ms: 400, poll_ms: 100 },
      workers: { directory: join(repoDir, "spikes", "spike-v6-transport", "tests", "fixtures", "jobs"), language: "node" },
    }));
    const env = { ...process.env, AIP_TEST_DATABASE: testDatabase };
    run(binary, ["service", "init", appPath, "--config", configPath], { env });
    schemaCreated = true;
    run(binary, ["service", "principal", "--config", configPath, "bind", "--subject", "user-42", "--actor", "42"], { env });
    const bindingPath = join(work, "binding.ts");
    run(binary, ["service", "gen", appPath, "--wire", "safe", "--out", bindingPath, "--sdk-import", "@aip/sdk"], { env });
    const fingerprint = (await readFile(bindingPath, "utf8")).match(/contractFingerprint = "([a-f0-9]{64})"/u)?.[1];
    if (!fingerprint) throw new Error("contract fingerprint missing from generated binding");

    service = spawn(binary, ["service", "serve", appPath, "--config", configPath], { cwd: repoDir, env, stdio: ["ignore", "pipe", "pipe"] });
    const listening = JSON.parse(await waitForLine(service, (line) => line.includes('"listening"'), "service"));
    const servicePort = Number(listening.address.split(":").pop());

    const proxyPort = await freePort();
    const caddyfile = `{
  admin off
  auto_https off
  skip_install_trust
}
https://localhost:${proxyPort} {
  bind 127.0.0.1
  tls ${join(work, "cert.pem")} ${join(work, "key.pem")}
  reverse_proxy 127.0.0.1:${servicePort}
}
`;
    await writeFile(join(work, "Caddyfile"), caddyfile);
    proxy = spawn(caddyBinary, ["run", "--config", join(work, "Caddyfile"), "--adapter", "caddyfile"], {
      cwd: work, env: { ...process.env, XDG_DATA_HOME: work, XDG_CONFIG_HOME: work, HOME: work }, stdio: ["ignore", "pipe", "pipe"],
    });
    await new Promise((resolveReady, reject) => {
      const timer = setTimeout(() => reject(new Error("caddy did not start")), 15000);
      proxy.stderr.setEncoding("utf8");
      proxy.stderr.on("data", (chunk) => { if (chunk.includes("serving initial configuration")) { clearTimeout(timer); resolveReady(); } });
      proxy.once("exit", (code) => { clearTimeout(timer); reject(new Error(`caddy exited early (${code})`)); });
    });
    const base = { port: proxyPort };
    const bearer = { authorization: `Bearer ${token}`, "content-type": "application/json", "x-aip-contract": fingerprint };

    // TLS terminates at the proxy; the service itself speaks plain HTTP on loopback.
    const plain = await request({ port: servicePort }, ca, { path: "/health" }).catch((error) => ({ status: 0, body: String(error) }));
    check("direct TLS to the loopback listener is refused (plain HTTP only)", plain.status === 0);

    const health = await request(base, ca, { path: "/health" });
    check("GET /health through the proxy", health.status === 200, `status ${health.status} ${health.body}`);

    const session = await request(base, ca, { method: "POST", path: "/session", headers: bearer, body: "{}" });
    const sessionBody = JSON.parse(session.body || "{}");
    check("POST /session: Authorization header and JSON body pass through", session.status === 200 && sessionBody.ok === true && sessionBody.principal?.actorId === "42", session.body);

    const preflight = await request(base, ca, {
      method: "OPTIONS", path: "/session",
      headers: { origin: allowedOrigin, "access-control-request-method": "POST", "access-control-request-headers": "authorization, content-type, x-aip-contract" },
    });
    check("CORS preflight echoes the allowed Origin through the proxy", preflight.headers["access-control-allow-origin"] === allowedOrigin, JSON.stringify(preflight.headers));
    const denied = await request(base, ca, { method: "OPTIONS", path: "/session", headers: { origin: "https://evil.example", "access-control-request-method": "POST" } });
    check("CORS preflight from a foreign Origin gets no allow header", !denied.headers["access-control-allow-origin"], JSON.stringify(denied.headers));
    const withOrigin = await request(base, ca, { method: "POST", path: "/session", headers: { ...bearer, origin: allowedOrigin }, body: "{}" });
    check("simple request carries the Origin back as access-control-allow-origin", withOrigin.headers["access-control-allow-origin"] === allowedOrigin);

    const inbound = "proxy-check-0123456789";
    const echoed = await request(base, ca, { method: "POST", path: "/session", headers: { ...bearer, "x-request-id": inbound }, body: "{}" });
    check("inbound x-request-id is kept and returned", echoed.headers["x-request-id"] === inbound, JSON.stringify(echoed.headers["x-request-id"]));
    const generated = await request(base, ca, { method: "POST", path: "/session", headers: bearer, body: "{}" });
    check("a request id is generated when the client sends none", /^req_[0-9a-f]{24}$/u.test(generated.headers["x-request-id"] ?? ""), String(generated.headers["x-request-id"]));

    // The proxy adds X-Forwarded-*; those must not change authentication or routing.
    const forwarded = await request(base, ca, { method: "POST", path: "/session", headers: { ...bearer, "x-forwarded-for": "203.0.113.9", "x-forwarded-proto": "https" }, body: "{}" });
    check("X-Forwarded-* headers do not break the request", forwarded.status === 200);

    const oversized = await request(base, ca, { method: "POST", path: "/session", headers: bearer, body: JSON.stringify({ pad: "x".repeat(1024 * 1024 + 16) }) });
    check("oversized body is rejected by the service, not swallowed by the proxy", /PAYLOAD_TOO_LARGE|BAD_REQUEST/u.test(oversized.body ?? ""), `${oversized.status} ${oversized.body?.slice(0, 120)}`);

    const sse = await request(base, ca, {
      method: "POST", path: "/events/subscribe", expectStream: true,
      headers: { ...bearer, accept: "text/event-stream", origin: allowedOrigin },
      body: JSON.stringify({ stream: "notifications", after: "0", contractFingerprint: fingerprint }),
    });
    check("SSE: content-type text/event-stream reaches the client", String(sse.headers["content-type"]).startsWith("text/event-stream"), JSON.stringify(sse.headers));
    check("SSE: a keepalive arrives promptly (not buffered until the stream ends)", String(sse.firstChunk).includes("keepalive") && sse.firstChunkMs < 3000, `${sse.firstChunkMs}ms ${JSON.stringify(sse.firstChunk)}`);

    const noOrigin = await webSocket(base, ca, { origin: undefined });
    check("WebSocket without Origin is refused through the proxy", noOrigin.status !== 101, `status ${noOrigin.status}`);
    const foreign = await webSocket(base, ca, { origin: "https://evil.example" });
    check("WebSocket from a foreign Origin is refused through the proxy", foreign.status !== 101, `status ${foreign.status}`);
    const socket = await webSocket(base, ca, { origin: allowedOrigin });
    check("WebSocket upgrade with an allowed Origin returns 101", socket.status === 101, `status ${socket.status}`);
    if (socket.api) {
      socket.api.send(JSON.stringify({ type: "auth", token }));
      const ack = await socket.api.next().catch((error) => String(error));
      check("WebSocket first-frame auth works through the proxy", ack.includes('"authenticated"'), ack);
      socket.api.close();
    }
  } finally {
    proxy?.kill("SIGTERM");
    service?.kill("SIGTERM");
    if (schemaCreated) {
      spawnSync("psql", ["-X", "--dbname", testDatabase, "-c", `DROP SCHEMA IF EXISTS ${schema} CASCADE`], { encoding: "utf8" });
    }
    await rm(work, { recursive: true, force: true });
  }
  const failed = results.filter((result) => !result.ok);
  console.log(`\n${results.length - failed.length}/${results.length} checks passed`);
  process.exitCode = failed.length === 0 ? 0 : 1;
}

main().catch((error) => {
  console.error(`proxy-check failed: ${error.message}`);
  process.exitCode = 2;
});
