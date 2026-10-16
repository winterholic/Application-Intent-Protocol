import http from "node:http";
import crypto from "node:crypto";

const port = Number(process.env.PORT || 8080);
const issuer = process.env.IDP_ISSUER || "https://localhost:8444";
const audience = process.env.IDP_AUDIENCE || "aip-api";

// Generate two RSA 2048 key pairs: primary and secondary (for rotation tests)
const key1 = crypto.generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwk1 = key1.publicKey.export({ format: "jwk" });
Object.assign(jwk1, { kid: "idp-smoke-key-1", use: "sig", alg: "RS256" });

const key2 = crypto.generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwk2 = key2.publicKey.export({ format: "jwk" });
Object.assign(jwk2, { kid: "idp-smoke-key-2", use: "sig", alg: "RS256" });

let rotationActive = false;

function b64url(input) {
  const buf = typeof input === "string" ? Buffer.from(input) : Buffer.from(JSON.stringify(input));
  return buf.toString("base64url");
}

function signToken({
  sub = "user-42",
  aud = audience,
  iss = issuer,
  expired = false,
  badTyp = false,
  rotated = false,
} = {}) {
  const now = Math.floor(Date.now() / 1000);
  const selectedKey = rotated ? key2 : key1;
  const kid = rotated ? "idp-smoke-key-2" : "idp-smoke-key-1";

  const header = {
    alg: "RS256",
    typ: badTyp ? "Bearer" : "at+jwt",
    kid,
  };

  const payload = {
    iss,
    aud,
    sub,
    iat: expired ? now - 600 : now,
    nbf: expired ? now - 600 : now,
    exp: expired ? now - 30 : now + 300,
  };

  const unsigned = `${b64url(header)}.${b64url(payload)}`;
  const signature = crypto.sign("RSA-SHA256", Buffer.from(unsigned), selectedKey.privateKey).toString("base64url");
  return `${unsigned}.${signature}`;
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, `http://${req.headers.host || "localhost"}`);

  if (url.pathname === "/health") {
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify({ ok: true }));
  }

  if (url.pathname === "/.well-known/openid-configuration") {
    const config = {
      issuer,
      jwks_uri: `${issuer}/jwks.json`,
      token_endpoint: `${issuer}/token`,
      id_token_signing_alg_values_supported: ["RS256"],
    };
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify(config, null, 2));
  }

  if (url.pathname === "/jwks.json" || url.pathname === "/jwks") {
    const isRotated = rotationActive || url.searchParams.get("rotated") === "true";
    const keys = isRotated ? [jwk1, jwk2] : [jwk1];
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify({ keys }, null, 2));
  }

  if (url.pathname === "/rotate") {
    rotationActive = true;
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify({ rotated: true }));
  }

  if (url.pathname === "/token") {
    const expired = url.searchParams.get("expired") === "true";
    const badAud = url.searchParams.get("bad_aud") === "true";
    const badIss = url.searchParams.get("bad_iss") === "true";
    const badTyp = url.searchParams.get("bad_typ") === "true";
    const rotated = url.searchParams.get("rotated") === "true";
    const unknownSub = url.searchParams.get("unknown_sub") === "true";

    const token = signToken({
      sub: unknownSub ? "unregistered-user-999" : "user-42",
      aud: badAud ? "wrong-api-audience" : audience,
      iss: badIss ? "https://impostor.example.test" : issuer,
      expired,
      badTyp,
      rotated,
    });

    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify({
      access_token: token,
      token_type: "Bearer",
      expires_in: expired ? 0 : 300,
    }));
  }

  res.writeHead(404, { "Content-Type": "application/json" });
  res.end(JSON.stringify({ error: "not_found" }));
});

server.listen(port, "0.0.0.0", () => {
  console.log(`Mock OIDC IdP listening on port ${port}, issuer=${issuer}`);
});
