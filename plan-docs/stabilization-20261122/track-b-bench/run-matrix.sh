#!/usr/bin/env bash
# E9 (opposite-order bundle locks) measurement matrix. One probe process per cell because sqlgen::set_schema is process-wide.
#
#   source <env with AIP_TEST_DATABASE_URL, AIP_TEST_ADMIN_URL, PG* unset>   # see track-b.md "재현"
#   bash plan-docs/stabilization-20261122/track-b-bench/run-matrix.sh <label> [rounds]
#
# Output: plan-docs/stabilization-20261122/track-b-bench/<label>.txt (one BENCH line per cell, from a1_opposite_order_two_product_bundles).
# CELLS="n:rows:order ..." overrides the default grid. PG deadlock_timeout is the server's (1s in the baseline environment).
set -euo pipefail
label=${1:?label}
rounds=${2:-10}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
out="$here/$label.txt"
cells=${CELLS:-"2:2:opposite 4:2:opposite 8:2:opposite 12:2:opposite 2:4:opposite 4:4:opposite 8:4:opposite 12:4:opposite 4:8:opposite 8:8:opposite 12:8:opposite 8:4:shuffle 8:8:shuffle 8:4:sorted 12:8:sorted"}
: >"$out"
for cell in $cells; do
  IFS=: read -r n rows order <<<"$cell"
  PROBE_N=$n PROBE_ROWS=$rows PROBE_ORDER=$order PROBE_ROUNDS=$rounds \
    cargo test --manifest-path "$root/spikes/spike-v6-transport/Cargo.toml" --locked --features test-db --test concurrency_probe_http \
    -- --ignored --nocapture --exact a1_opposite_order_two_product_bundles 2>&1 | grep '^BENCH' | tee -a "$out"
done
