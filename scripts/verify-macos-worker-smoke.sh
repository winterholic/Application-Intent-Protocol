#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "$(uname -s)" != Darwin ]]; then
  echo "This fallback is for macOS Terminal; Linux worker isolation is tested by .github/workflows/worker-jobs-linux.yml." >&2
  exit 2
fi
for program in cargo node npm python3 psql; do
  if ! command -v "$program" >/dev/null 2>&1; then
    echo "Missing prerequisite: $program" >&2
    exit 2
  fi
done
if [[ ! -x "$repo_dir/spikes/spike-0-ts/node_modules/.bin/tsc" ]]; then
  echo "Install SDK test dependencies first: npm ci --prefix spikes/spike-0-ts --ignore-scripts --no-audit --no-fund" >&2
  exit 2
fi
if [[ ! -x /usr/bin/sandbox-exec ]]; then
  echo "Missing macOS sandbox-exec at /usr/bin/sandbox-exec" >&2
  exit 2
fi
cargo test --manifest-path "$repo_dir/Cargo.toml" -p aip-test-support --features strict-db --locked
psql -X "$AIP_TEST_DATABASE_URL" -Atc 'SELECT 1' >/dev/null

scratch="$(mktemp -d "${TMPDIR:-/tmp}/aip-worker-smoke.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/home"
cd "$repo_dir"
cargo build -p aip-cli --locked
HOME="$scratch/home" TMPDIR="$scratch" npm_config_cache="$scratch/npm-cache" \
  node --test product/tests/service-smoke.test.mjs
