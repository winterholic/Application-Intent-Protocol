#!/usr/bin/env bash
# Runs the concurrency probes of spikes/spike-v6-transport/tests/concurrency_probe_http.rs, one process per probe
# (sqlgen::set_schema is process-wide, so two probes in one process would share a schema).
#
#   scripts/run-concurrency-probes.sh stable             # product contracts: required on every main push and release candidate
#   scripts/run-concurrency-probes.sh exploratory        # a1..a5: costly or known-open; manual and scheduled only
#   scripts/run-concurrency-probes.sh <probe name> ...   # exact test names
#
# Needs AIP_TEST_DATABASE_URL / AIP_TEST_ADMIN_URL on the dedicated PostgreSQL 17 and no PG* variables. PROBE_ROUNDS and PROBE_N
# tune the exploratory runs. Every probe is attempted even after a failure, and the exit status is non-zero if any failed.
set -euo pipefail
cd "$(dirname "$0")/.."

# Stable: results confirmed in plan-docs/hardening-1.0/concurrency.md (3.2-3.4) and cheap (about 20 s for all twelve).
STABLE=(
  f1_failing_bundles_leave_nothing_behind_under_concurrency
  i1_same_key_concurrent_duplicates_read_committed
  i2_same_key_concurrent_duplicates_serializable
  i3_same_key_different_requests_race
  i4_response_loss_then_replay_and_fence
  i5_serializable_apply_distinct_keys_never_internal
  n1_concurrent_increment_bounds_and_overflow
  e1_extension_same_key_concurrent_duplicates
  e2_extension_contention_distinct_keys
  e3_extension_unique_create_race_on_distinct_rows
  e4_serializable_extension_race_and_same_key_duplicates
  e5_saturated_slots_replay_status_and_inflight_duplicates
)
# Exploratory: a1 deliberately reproduces the open opposite-order deadlock (LIMITS E9) and takes seconds per round at scale;
# a2-a4 are guarded by permanent regression tests (bundle_lock_order_*.rs), a5 by write rollback tests, and e6 measures
# replay latency against an artificial database round trip while all worker slots are occupied.
EXPLORATORY=(
  a1_opposite_order_two_product_bundles
  a2_line_first_then_compose_on_same_product
  a3_line_first_then_where_target_apply
  a4_line_first_then_ids_apply_control
  a5_failure_after_creates_leaves_no_rows_and_no_idempotency_record
  e6_saturated_replay_latency_by_database_round_trip
)

if [[ "${1:-}" == "--list" ]]; then
  printf 'stable %s\n' "${STABLE[@]}"
  printf 'exploratory %s\n' "${EXPLORATORY[@]}"
  exit 0
fi
case "${1:-}" in
  stable) probes=("${STABLE[@]}") ;;
  exploratory) probes=("${EXPLORATORY[@]}") ;;
  "") echo "usage: $0 stable | exploratory | <probe name>... | --list" >&2; exit 2 ;;
  *) probes=("$@") ;;
esac
for variable in AIP_TEST_DATABASE_URL AIP_TEST_ADMIN_URL; do
  [[ -n "${!variable:-}" ]] || { echo "$variable must name the dedicated test PostgreSQL" >&2; exit 2; }
done
[[ -z "${PGHOST:-}${PGPORT:-}" ]] || { echo "unset PGHOST and PGPORT: aip-test-support refuses to build with them" >&2; exit 2; }

cargo_args=(--manifest-path spikes/spike-v6-transport/Cargo.toml --locked --features test-db ${AIP_PROBE_TARGET_DIR:+--target-dir "$AIP_PROBE_TARGET_DIR"})
cargo test "${cargo_args[@]}" --test concurrency_probe_http --no-run
failed=()
for probe in "${probes[@]}"; do
  echo "::group::probe $probe"
  if cargo test "${cargo_args[@]}" --test concurrency_probe_http -- --ignored --nocapture --exact "$probe"; then
    echo "PASS $probe"
  else
    echo "FAIL $probe" >&2
    failed+=("$probe")
  fi
  echo "::endgroup::"
done
if ((${#failed[@]})); then
  printf 'failed probes: %s\n' "${failed[*]}" >&2
  exit 1
fi
echo "all ${#probes[@]} probes passed"
