// A stand-in identity provider for local development and demos. NOT for production.
//
//   TOKEN=$(node product/tools/dev-jwt.mjs --dir ./dev-idp --subject user-42)
//
// `aip service` accepts only RS256 access tokens whose signature verifies against the JWKS in its configuration; it issues no
// tokens itself and has no development bypass. This script plays the identity provider: it creates an RSA key pair in --dir on
// first use (the private key stays there, mode 0600), writes the public JWKS next to it and prints one access token on stdout.
// Point the service configuration at the JWKS file: "jwks": {"kind": "file", "path": "<dir>/jwks.json"}. Everything the service
// checks (signature, issuer, audience, subject, lifetime of at most 900 seconds, principal binding) still applies to this token.
import { createPrivateKey, createPublicKey, generateKeyPairSync, sign } from "node:crypto";
import { chmod, mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

function parse(argv) {
  const options = { dir: undefined, subject: undefined, issuer: "https://issuer.example", audience: "aip-api", ttl: 900 };
  for (let index = 0; index < argv.length; index++) {
    const name = argv[index];
    const value = argv[index + 1];
    if (name === "--dir" && value) options.dir = resolve(argv[++index]);
    else if (name === "--subject" && value) options.subject = argv[++index];
    else if (name === "--issuer" && value) options.issuer = argv[++index];
    else if (name === "--audience" && value) options.audience = argv[++index];
    else if (name === "--ttl" && /^\d+$/u.test(value ?? "")) options.ttl = Number(argv[++index]);
    else fail("usage: node product/tools/dev-jwt.mjs --dir <directory> --subject <subject> [--issuer <url>] [--audience <name>] [--ttl <1-900 seconds>]");
  }
  if (!options.dir || !options.subject) fail("--dir and --subject are required");
  if (!(options.ttl >= 1 && options.ttl <= 900)) fail("--ttl must be between 1 and 900 seconds (the service refuses longer-lived tokens)");
  return options;
}

function fail(message) {
  console.error(message);
  process.exit(2);
}

async function loadOrCreateKey(dir) {
  const privatePath = resolve(dir, "dev-idp-private.pem");
  const jwksPath = resolve(dir, "jwks.json");
  await mkdir(dir, { recursive: true });
  let privateKey;
  try {
    privateKey = createPrivateKey(await readFile(privatePath));
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    privateKey = generateKeyPairSync("rsa", { modulusLength: 2048 }).privateKey;
    await writeFile(privatePath, privateKey.export({ type: "pkcs8", format: "pem" }), { mode: 0o600, flag: "wx" });
  }
  await chmod(privatePath, 0o600);
  const jwk = createPublicKey(privateKey).export({ format: "jwk" });
  Object.assign(jwk, { kid: "dev-idp", use: "sig", alg: "RS256", key_ops: ["verify"] });
  await writeFile(jwksPath, JSON.stringify({ keys: [jwk] }) + "\n", { mode: 0o644 });
  return { privateKey, kid: jwk.kid, jwksPath };
}

const options = parse(process.argv.slice(2));
const { privateKey, kid, jwksPath } = await loadOrCreateKey(options.dir);
const now = Math.floor(Date.now() / 1000);
const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
const unsigned = `${encode({ alg: "RS256", kid, typ: "at+jwt" })}.${encode({ iss: options.issuer, aud: options.audience, sub: options.subject, iat: now, nbf: now, exp: now + options.ttl })}`;
console.error(`JWKS: ${jwksPath} (issuer ${options.issuer}, audience ${options.audience}, valid ${options.ttl}s)`);
console.log(`${unsigned}.${sign("RSA-SHA256", Buffer.from(unsigned), privateKey).toString("base64url")}`);
