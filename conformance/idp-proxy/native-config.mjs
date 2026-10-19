import { join } from "node:path";

export function nativeProxyConfig(template, certificates, backendPort, idpPort) {
  for (const port of [backendPort, idpPort]) {
    if (!Number.isInteger(port) || port < 1025 || port > 65535) throw new Error("native proxy requires dedicated high ports");
  }
  const substitutions = [
    ["host.docker.internal:{$AIP_BACKEND_PORT:8099}", `127.0.0.1:${backendPort}`, 1],
    ["idp:8080", `127.0.0.1:${idpPort}`, 1],
    ["/certs/cert.pem", JSON.stringify(join(certificates, "cert.pem")), 2],
    ["/certs/key.pem", JSON.stringify(join(certificates, "key.pem")), 2],
  ];
  let config = template;
  for (const [token, replacement, expected] of substitutions) {
    if (config.split(token).length - 1 !== expected) throw new Error("native proxy template changed; verify its upstream and TLS bindings");
    config = config.replaceAll(token, replacement);
  }
  return config;
}
