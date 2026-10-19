import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { nativeProxyConfig } from "./native-config.mjs";

const template = readFileSync(new URL("./Caddyfile", import.meta.url), "utf8");

test("native proxy reaches only host loopback and uses the owned certificate directory", () => {
  const config = nativeProxyConfig(template, "/tmp/owned certificates", 23456, 23457);
  assert.ok(config.includes("reverse_proxy 127.0.0.1:23456"));
  assert.ok(config.includes("reverse_proxy 127.0.0.1:23457"));
  assert.ok(config.includes('tls "/tmp/owned certificates/cert.pem" "/tmp/owned certificates/key.pem"'));
  assert.ok(!config.includes("host.docker.internal") && !config.includes("idp:8080"));
  assert.ok(config.includes("https://localhost:8443") && config.includes("https://localhost:8444"));
});

test("a stale template or an invalid listener port fails instead of testing another upstream", () => {
  assert.throws(() => nativeProxyConfig("unrelated configuration", "/tmp/owned", 23456, 23457));
  for (const port of [0, -1, 65536, "23456", 1.5]) {
    assert.throws(() => nativeProxyConfig(template, "/tmp/owned", port, 23457));
  }
});
