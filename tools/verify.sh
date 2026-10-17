#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

for program in cargo node npm psql python3; do
  if ! command -v "$program" >/dev/null 2>&1; then
    printf 'Missing verification tool: %s\n' "$program" >&2
    exit 1
  fi
done
if [[ ! -x spikes/spike-0-ts/node_modules/.bin/tsc ]]; then
  printf 'Install the SDK toolchain: npm ci --prefix spikes/spike-0-ts\n' >&2
  exit 1
fi

# Compile-time DB guard validates the explicit DSNs and matching non-default PGPORT
# before psql or any database-backed test can connect.
cargo test -p aip-test-support --features strict-db --locked
psql -X -q "$AIP_TEST_DATABASE_URL" -Atc 'SELECT 1'
cargo fmt --all --check
cargo clippy --workspace --lib --bins --locked -- -D warnings
cargo clippy -p aip-auth -p aip-migrate -p aip-service --all-targets --locked -- -D warnings
# The PoC runtime is an optional aip-cli package; its end-to-end tests only build with the feature.
cargo clippy -p aip-cli --features poc-runtime --bins --locked -- -D warnings
cargo test --workspace --locked
cargo test -p aip-cli --features poc-runtime --locked
# These product engines are path dependencies excluded from the root workspace.
# v4 (worker) and v6 (transport) are included because their regressions (S-24) were otherwise caught only by CI.
for engine in spike-v1-fixture spike-v2-read spike-v3-write spike-v4-worker spike-v5-sdk spike-v6-transport; do
  cargo test --manifest-path "spikes/$engine/Cargo.toml" --locked --target-dir target
done
# The update smoke executes the product CLI; rebuild before its installed-SDK test.
cargo build -p aip-cli --locked
node --test \
  spikes/spike-v5-sdk/sdk/cache.test.ts \
  spikes/spike-v5-sdk/sdk/lifetime.test.ts \
  spikes/spike-v6-transport/client/recovery.test.ts \
  spikes/spike-v6-transport/client/session.test.ts \
  spikes/spike-v6-transport/client/v12-contract.test.ts \
  spikes/spike-v6-transport/client/v13-binding.test.ts \
  spikes/spike-v6-transport/client/v14-envelope.test.ts \
  spikes/spike-v6-transport/client/scalar-validation.test.ts \
  spikes/spike-v6-transport/client/operations.test.ts \
  spikes/spike-v6-transport/client/jobs.test.ts \
  spikes/spike-v6-transport/client/infrastructure.test.ts \
  spikes/spike-v6-transport/client/websocket.test.ts \
  spikes/spike-v6-transport/client/rag.test.ts \
  spikes/spike-v6-transport/client/traverse-many.test.ts \
  spikes/spike-v6-transport/client/request-id.test.ts \
  spikes/spike-v6-transport/client/hardening.test.ts \
  product/tests/general-update.test.mjs \
  product/tests/postgres-guard.test.mjs \
  product/tests/uuid-external.test.mjs \
  product/tests/facereview-comments.test.mjs \
  product/tests/sdk-package.test.mjs \
  product/tests/package-boundaries.test.mjs \
  product/tests/google-storage-provider.test.mjs
python3 -m unittest product.tests.google_storage_provider_test product.tests.python_sdk_test

# Test feature combinations can replace target/debug/aip; rebuild the product last.
cargo build -p aip-cli --locked
printf 'AIP core verification passed.\n'
