#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

echo "==> Building aip binary..."
cargo build -p aip-cli --locked

echo "==> Running OIDC IdP and Caddy TLS reverse proxy smoke test..."
node "$repo_root/conformance/idp-proxy/smoke-check.mjs"
