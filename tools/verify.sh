#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

for program in cargo node npm psql python3; do
  if ! command -v "$program" >/dev/null 2>&1; then
    printf 'Missing verification tool: %s\n' "$program" >&2
    exit 1
  fi
done
if ! command -v mypy >/dev/null 2>&1 && ! command -v uv >/dev/null 2>&1; then
  printf 'Install mypy 2.4.0 or uv for Python binding verification.\n' >&2
  exit 1
fi
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
cargo clippy -p aip-cli --features poc-runtime --all-targets --locked -- -D warnings
# The MySQL adapter is an optional package too: its code and live tests (ignored without a container) must stay lint-clean and compile.
cargo clippy -p aip-migrate -p aip-service -p aip-cli --features mysql,aip-cli/mysql --all-targets --locked -- -D warnings
# The excluded engine crates were not linted before; a clippy regression in one (spike-v6 server.rs) reached main unnoticed.
for engine in spike-v1-fixture spike-v2-read spike-v3-write spike-v4-worker spike-v5-sdk spike-v6-transport; do
  cargo clippy --manifest-path "spikes/$engine/Cargo.toml" --all-targets --locked --target-dir target -- -D warnings
done
cargo test --workspace --locked
# Legacy import is isolated from runtime builds but remains part of the offline migration gate.
cargo clippy -p aip-crypto --all-targets --all-features --locked -- -D warnings
cargo test -p aip-crypto --all-features --locked
cargo test -p aip-cli --features poc-runtime --locked
# These product engines are path dependencies excluded from the root workspace.
# v4 (worker) and v6 (transport) are included because their regressions (S-24) were otherwise caught only by CI.
for engine in spike-v1-fixture spike-v2-read spike-v3-write spike-v4-worker spike-v5-sdk spike-v6-transport; do
  cargo test --manifest-path "spikes/$engine/Cargo.toml" --locked --target-dir target
done
# The Socket.IO-compatible transport (WBS 7.2.6) is also run with the real socket.io-client the FaceReview player locks
# (4.8.3, pinned by package-lock.json). Lifecycle scripts stay off: only the library files are installed.
npm ci --ignore-scripts --no-audit --no-fund --prefix spikes/spike-v6-transport/tests/socketio-client
cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test socketio_transport -- --ignored
# Pure-Python checks of the FaceReview extensions (the first-frame session setup of WBS 7.2.6 has no server in this step).
python3 -m unittest discover -s product/tests/fixtures/facereview-mysql/extensions -p 'test_*.py'
# The legacy prototype imports the shared generator and must export every binding type it emits.
# Keep its CLI and committed lock file in the product gate so an SDK change cannot silently break it.
cargo test --manifest-path prototype/Cargo.toml --test cli --locked --target-dir target
# The update smoke executes the product CLI; rebuild before its installed-SDK test.
cargo build -p aip-cli --locked
node --test \
  porting/facereview/ui-overlay/tests/adminAdapter.test.ts \
  porting/facereview/ui-overlay/tests/adminDashboardStatus.test.ts \
  porting/facereview/ui-overlay/tests/adminReadAdapter.test.ts \
  porting/facereview/ui-overlay/tests/authAdapter.test.ts \
  porting/facereview/ui-overlay/tests/mypageAdapter.test.ts \
  porting/facereview/ui-overlay/tests/readAdapter.test.ts \
  porting/facereview/ui-overlay/tests/writeAdapter.test.ts \
  spikes/spike-v6-transport/client/aggregate.test.ts \
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
  product/tests/memoir-folder-rename.test.mjs \
  product/tests/sdk-package.test.mjs \
  product/tests/package-boundaries.test.mjs \
  product/tests/app-manifest.test.mjs \
  product/tests/app-package.test.mjs \
  product/tests/app-deploy.test.mjs \
  product/tests/smoke-options.test.mjs \
  product/tests/dev-jwt.test.mjs \
  product/tests/google-storage-provider.test.mjs
python3 -W error::ResourceWarning -m unittest product.tests.google_storage_provider_test product.tests.password_auth_test product.tests.python_sdk_test product.tests.dedicated_db_test product.tests.test_facereview_ui_browser_evidence
python3 spikes/spike-v4-worker/tests/python_frame_limit.py -v

# Test feature combinations can replace target/debug/aip; rebuild the product last.
cargo build -p aip-cli --locked
printf 'AIP core verification passed.\n'
