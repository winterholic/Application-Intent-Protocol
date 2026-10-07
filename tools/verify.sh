#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

for program in cargo node npm psql; do
  if ! command -v "$program" >/dev/null 2>&1; then
    printf 'Missing verification tool: %s\n' "$program" >&2
    exit 1
  fi
done
if [[ ! -x spikes/spike-0-ts/node_modules/.bin/tsc ]]; then
  printf 'Install the SDK toolchain: npm ci --prefix spikes/spike-0-ts\n' >&2
  exit 1
fi

psql -X -q -d 'host=localhost dbname=postgres' -Atc 'SELECT 1'
cargo fmt --all --check
cargo clippy --workspace --lib --bins --locked -- -D warnings
cargo clippy -p aip-auth -p aip-migrate -p aip-service --all-targets --locked -- -D warnings
cargo test --workspace --locked
# These product engines are path dependencies excluded from the root workspace.
for engine in spike-v1-fixture spike-v2-read spike-v3-write spike-v5-sdk; do
  cargo test --manifest-path "spikes/$engine/Cargo.toml" --locked --target-dir target
done
node --test \
  spikes/spike-v5-sdk/sdk/cache.test.ts \
  spikes/spike-v5-sdk/sdk/lifetime.test.ts \
  spikes/spike-v6-transport/client/recovery.test.ts \
  spikes/spike-v6-transport/client/session.test.ts \
  spikes/spike-v6-transport/client/v12-contract.test.ts \
  spikes/spike-v6-transport/client/v13-binding.test.ts \
  spikes/spike-v6-transport/client/v14-envelope.test.ts \
  spikes/spike-v6-transport/client/scalar-validation.test.ts \
  spikes/spike-v6-transport/client/traverse-many.test.ts \
  product/tests/sdk-package.test.mjs \
  product/tests/package-boundaries.test.mjs

# Test feature combinations can replace target/debug/aip; rebuild the product last.
cargo build -p aip-cli --locked
printf 'AIP core verification passed. Worker and relocated-service checks are separate.\n'
