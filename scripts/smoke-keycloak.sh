#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

if [ -n "${AIP_BINARY:-}" ]; then
  [ -x "$AIP_BINARY" ] || { echo "ERROR: AIP_BINARY='$AIP_BINARY' is not an executable file" >&2; exit 1; }
else
  echo "==> Building aip binary..."
  cargo build -p aip-cli --locked
fi

echo "==> Running the real Keycloak 26.2.5 smoke (needs docker and the dedicated test PostgreSQL)..."
node "$repo_root/conformance/keycloak/smoke-check.mjs"
