#!/usr/bin/env bash
# 1.0 release gate evidence run (WBS 8.7). It never reads the working tree: everything runs from a fresh clone of a committed ref.
#
#   scripts/release-gate.sh --baseline <git-ref-of-the-previous-release> [--ref <git-ref>] [--out <fresh-directory>]
#
# Needs the dedicated test PostgreSQL (AIP_TEST_DATABASE_URL / AIP_TEST_ADMIN_URL on an explicit non-default port, no PG* variables),
# cargo, node, npm, python3, psql, pg_dump and pg_restore. Each step writes its own log and one line to summary.txt, so a later
# reader can see which command produced which result. The script stops at the first failing step.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ref="HEAD"
baseline=""
out=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --ref) ref="$2"; shift 2 ;;
    --baseline) baseline="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    *) echo "usage: scripts/release-gate.sh --baseline <ref> [--ref <ref>] [--out <fresh-directory>]" >&2; exit 2 ;;
  esac
done
[[ -n "$baseline" ]] || { echo "--baseline is required: the upgrade drill needs the previous release to start from" >&2; exit 2; }
for variable in AIP_TEST_DATABASE_URL AIP_TEST_ADMIN_URL; do
  [[ -n "${!variable:-}" ]] || { echo "$variable must name the dedicated test PostgreSQL" >&2; exit 2; }
done
for program in git cargo node npm python3 psql pg_dump pg_restore shasum; do
  command -v "$program" >/dev/null 2>&1 || { echo "missing tool: $program" >&2; exit 2; }
done

sha="$(git -C "$repo_root" rev-parse --verify "$ref^{commit}")"
baseline_sha="$(git -C "$repo_root" rev-parse --verify "$baseline^{commit}")"
out="${out:-$(mktemp -d "${TMPDIR:-/tmp}/aip-release-gate-XXXXXX")}"
[[ ! -e "$out/summary.txt" ]] || { echo "$out already holds a run; pass a fresh --out" >&2; exit 2; }
mkdir -p "$out/logs"
summary="$out/summary.txt"
src="$out/src"
baseline_src="$out/baseline-src"
artifact="$out/artifact"

note() { printf '%s\n' "$*" | tee -a "$summary"; }
step() {
  local name="$1"; shift
  local log="$out/logs/$name.log"
  local started=$SECONDS
  if "$@" >"$log" 2>&1; then
    note "PASS  $name ($((SECONDS - started))s)  log=logs/$name.log"
  else
    note "FAIL  $name ($((SECONDS - started))s)  log=logs/$name.log"
    tail -n 25 "$log" >&2
    exit 1
  fi
}

note "release gate for $sha (baseline $baseline_sha) at $(date -u +%Y-%m-%dT%H:%M:%SZ)"
note "host: $(uname -srm); rustc: $(rustc --version); node: $(node --version); python: $(python3 --version 2>&1); postgres client: $(psql --version)"

step clone-release git clone --quiet --no-hardlinks "$repo_root" "$src"
step checkout-release git -C "$src" checkout --quiet --detach "$sha"
[[ -z "$(git -C "$src" status --porcelain)" ]] || { note "FAIL  the clean clone is not clean"; exit 1; }
step clone-baseline git clone --quiet --no-hardlinks "$repo_root" "$baseline_src"
step checkout-baseline git -C "$baseline_src" checkout --quiet --detach "$baseline_sha"

# A fresh checkout has no node_modules: the install the validation guide prescribes is part of what is being proven.
step npm-ci npm ci --prefix "$src/spikes/spike-0-ts" --ignore-scripts --no-audit --no-fund

# The full regression exactly as the repository defines it (fmt, clippy, workspace and engine suites, SDK and package tests).
step full-regression bash -c "cd '$src' && bash tools/verify.sh"

# Release build and the packaged artifact, then the artifact's own checksums and a real run of its binary and SDK.
step release-build bash -c "cd '$src' && cargo build --release -p aip-cli --locked"
step package bash -c "cd '$src' && node product/tools/package.mjs --out '$artifact' --profile release"
step artifact-checksums bash -c "cd '$artifact' && shasum -a 256 -c SHA256SUMS"
step artifact-binary bash -c "'$artifact/bin/aip' service --help"
step artifact-smoke bash -c "cd '$src' && node product/tools/smoke.mjs --bin '$artifact/bin/aip' --sdk-package '$artifact/sdk/aip-sdk-0.1.0.tgz' --workers python"

# Upgrade from the previous release and the two rollbacks, with the release binary that was just packaged.
step baseline-build bash -c "cd '$baseline_src' && cargo build -p aip-cli --locked"
step upgrade-drill bash -c "cd '$src' && node product/tools/upgrade-drill.mjs --old-bin '$baseline_src/target/debug/aip' --new-bin '$artifact/bin/aip'"

{
  echo "artifact files:"
  (cd "$artifact" && find . -type f | sort | sed 's/^/  /')
  echo "artifact checksums:"
  sed 's/^/  /' "$artifact/SHA256SUMS"
} >>"$summary"
note "ALL STEPS PASSED for $sha"
echo "evidence: $out"
