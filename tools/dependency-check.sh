#!/usr/bin/env bash
# Known-vulnerability, license, source and yanked-crate checks for every lock file in the repository, plus npm audit.
# Needs: cargo install cargo-audit cargo-deny --locked. Policy and the one justified exception live in deny.toml.
set -euo pipefail

cd "$(dirname "$0")/.."

for program in cargo cargo-audit cargo-deny npm; do
  if ! command -v "$program" >/dev/null 2>&1; then
    printf 'Missing tool: %s (cargo install cargo-audit cargo-deny --locked)\n' "$program" >&2
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
  # RUSTSEC-2023-0071 is the one accepted exception; its reason is in deny.toml, so audit gets the same id.
  cargo audit -f "$lock" --ignore RUSTSEC-2023-0071 || failed=1
  cargo deny --manifest-path "$manifest" check || failed=1
done

printf '== npm audit (spikes/spike-0-ts)\n'
npm audit --prefix spikes/spike-0-ts --audit-level=low || failed=1

if [[ "$failed" != 0 ]]; then
  printf 'Dependency check failed.\n' >&2
  exit 1
fi
printf 'Dependency check passed.\n'
