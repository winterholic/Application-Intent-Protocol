import assert from "node:assert/strict";
import { createPublicKey, verify } from "node:crypto";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const script = resolve(dirname(fileURLToPath(import.meta.url)), "../tools/dev-jwt.mjs");

function run(args) {
  return spawnSync(process.execPath, [script, ...args], { encoding: "utf8", timeout: 20_000 });
}

function decode(token) {
  const [header, claims, signature] = token.split(".");
  return { header: JSON.parse(Buffer.from(header, "base64url")), claims: JSON.parse(Buffer.from(claims, "base64url")), signature, signed: `${header}.${claims}` };
}

test("the stand-in IdP mints a token the service profile accepts and reuses its key", (t) => {
  const dir = join(mkdtempSync(join(tmpdir(), "aip-dev-jwt-")), "idp");
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const first = run(["--dir", dir, "--subject", "user-42"]);
  assert.equal(first.status, 0, first.stderr);
  const token = first.stdout.trim();
  const { header, claims, signature, signed } = decode(token);
  // Profile of the service: RS256, typ at+jwt, exact issuer and audience, sub/iat/nbf/exp, at most 900 seconds.
  assert.deepEqual([header.alg, header.typ], ["RS256", "at+jwt"]);
  assert.deepEqual([claims.iss, claims.aud, claims.sub], ["https://issuer.example", "aip-api", "user-42"]);
  assert.equal(claims.exp - claims.iat, 900);
  assert.equal(claims.nbf, claims.iat);

  const jwks = JSON.parse(readFileSync(join(dir, "jwks.json"), "utf8"));
  assert.equal(jwks.keys.length, 1);
  assert.equal(jwks.keys[0].kid, header.kid);
  assert.ok(!("d" in jwks.keys[0]), "the published JWKS must not contain private key material");
  const publicKey = createPublicKey({ key: jwks.keys[0], format: "jwk" });
  assert.equal(verify("RSA-SHA256", Buffer.from(signed), publicKey, Buffer.from(signature, "base64url")), true);
  assert.equal(statSync(join(dir, "dev-idp-private.pem")).mode & 0o077, 0, "the private key must be readable by its owner only");

  const second = run(["--dir", dir, "--subject", "user-7", "--ttl", "60", "--audience", "other-api"]);
  assert.equal(second.status, 0, second.stderr);
  const again = decode(second.stdout.trim());
  assert.equal(again.claims.exp - again.claims.iat, 60);
  assert.equal(again.claims.aud, "other-api");
  assert.equal(verify("RSA-SHA256", Buffer.from(again.signed), publicKey, Buffer.from(again.signature, "base64url")), true, "the same key signs later tokens");
});

test("lifetimes beyond what the service accepts and missing arguments are refused", (t) => {
  const dir = join(mkdtempSync(join(tmpdir(), "aip-dev-jwt-")), "idp");
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const args of [["--dir", dir, "--subject", "u", "--ttl", "901"], ["--dir", dir, "--subject", "u", "--ttl", "0"], ["--dir", dir], ["--subject", "u"], ["--dir", dir, "--subject", "u", "--bogus", "1"]]) {
    const result = run(args);
    assert.equal(result.status, 2, `${args.join(" ")}: ${result.stdout}${result.stderr}`);
    assert.equal(result.stdout, "", "no token is printed on failure");
  }
});
