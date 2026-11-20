#!/usr/bin/env bash
# Known-vulnerability, license, source and yanked-crate checks for every lock file in the repository, plus npm audit and pip-audit.
# Needs: cargo install cargo-audit cargo-deny --locked, and pip-audit (python3 -m pip install pip-audit). Advisory and package license exceptions live in deny.toml.
set -euo pipefail

cd "$(dirname "$0")/.."

for program in cargo cargo-audit cargo-deny npm pip-audit; do
  if ! command -v "$program" >/dev/null 2>&1; then
    printf 'Missing tool: %s (cargo install cargo-audit cargo-deny --locked; python3 -m pip install pip-audit)\n' "$program" >&2
    exit 1
  fi
done

# The root workspace and every independent crate with its own Cargo.lock.
manifests=(
  Cargo.toml
  prototype/Cargo.toml
  spikes/spike-v1-fixture/Cargo.toml
  spikes/spike-v2-read/Cargo.toml
  spikes/spike-v3-write/Cargo.toml
  spikes/spike-v4-worker/Cargo.toml
  spikes/spike-v5-sdk/Cargo.toml
  spikes/spike-v6-transport/Cargo.toml
  spikes/spike-v7-migrate/Cargo.toml
  spikes/spike-v11-dev-checks/Cargo.toml
)

failed=0
for manifest in "${manifests[@]}"; do
  lock="$(dirname "$manifest")/Cargo.lock"
  printf '== %s\n' "$manifest"
  # Accepted exceptions live in deny.toml with their reasons; audit gets the same ids (RUSTSEC-2023-0071, chrono and chrono-tz unmaintained).
  cargo audit -f "$lock" --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0543 --ignore RUSTSEC-2026-0555 || failed=1
  cargo deny --manifest-path "$manifest" --locked check || failed=1
done

printf '== npm audit (spikes/spike-0-ts)\n'
npm audit --prefix spikes/spike-0-ts --audit-level=low || failed=1

printf '== npm audit (spikes/spike-v6-transport/tests/socketio-client)\n'
npm audit --prefix spikes/spike-v6-transport/tests/socketio-client --audit-level=low || failed=1

# The FlaskBB UI host pins its own dependencies, so a fix needs a pin bump here.
# Not audited on purpose: porting/flaskbb/upstream/requirements*.txt (the unmodified upstream FlaskBB oracle, pinned to the
# old versions the parity baseline was recorded against, installed only in CI test venvs, never shipped) and
# product/python (the SDK, `dependencies = []`). Both are recorded in docs/DEPENDENCY-REVIEW.md.
printf '== pip-audit (porting/flaskbb/ui/requirements.txt)\n'
pip-audit -r porting/flaskbb/ui/requirements.txt --no-deps --disable-pip || failed=1

if [[ "$failed" != 0 ]]; then
  printf 'Dependency check failed.\n' >&2
  exit 1
fi
printf 'Dependency check passed.\n'
