#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

if [ -n "${AIP_BINARY:-}" ]; then
  if [ ! -f "$AIP_BINARY" ] || [ ! -x "$AIP_BINARY" ]; then
    echo "ERROR: AIP_BINARY is set to '$AIP_BINARY' but the file does not exist or is not executable" >&2
    exit 1
  fi
  echo "==> Using pre-built aip binary: $AIP_BINARY"
else
  echo "==> Building aip binary..."
  cargo build -p aip-cli --locked
fi

echo "==> Running OIDC IdP and Caddy TLS reverse proxy smoke test..."
node "$repo_root/conformance/idp-proxy/smoke-check.mjs"
