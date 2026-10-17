#!/usr/bin/env bash
# AIP OTLP Observability Smoke Test
# WBS 8.3: Durable Job trace_id E2E propagation & OTLP collector integration
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

echo "==> [WBS 8.3] Running actual Rust spike-v6-transport OTLP integration tests..."
cargo test --manifest-path "$repo_root/spikes/spike-v6-transport/Cargo.toml" --test otlp --locked --target-dir "${CARGO_TARGET_DIR:-$repo_root/target}"
