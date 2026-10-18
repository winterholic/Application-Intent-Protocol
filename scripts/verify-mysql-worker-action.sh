#!/usr/bin/env bash
set -euo pipefail

: "${AIP_FIXTURE_TAG:?set a unique run-owned fixture tag}"
: "${AIP_FIXTURE_MYSQL_PORT:?set a dedicated loopback port above 1024}"
: "${AIP_FIXTURE_MARIADB_PORT:?set a second dedicated loopback port above 1024}"
export AIP_FIXTURE_DIR="${AIP_FIXTURE_DIR:-/private/tmp/aip-mysql-${AIP_FIXTURE_TAG}}"

unset PGHOST PGPORT PGDATABASE PGUSER PGPASSWORD PGSERVICE PGSERVICEFILE
unset AIP_TEST_DATABASE_URL AIP_TEST_ADMIN_URL

# These synthetic PostgreSQL DSNs satisfy aip-test-support's build guard only; no PostgreSQL connection is made.
export AIP_TEST_DATABASE_URL="host=127.0.0.1 port=55473 dbname=aip_test_run user=runner"
export AIP_TEST_ADMIN_URL="host=127.0.0.1 port=55473 dbname=postgres user=runner"

# Verify all intended test targets exist in the binary test list (protects against 0-match typos)
TARGET_NODB_TEST="worker_action_validator_decides_commit_without_database"
TARGET_LIVE_TESTS=(
  "mysql_write_action_coupling_commits_or_rolls_back_same_transaction"
  "mysql_worker_action_uses_one_fenced_idempotent_transaction"
  "mysql_apply_is_atomic_idempotent_audited_and_fenced"
  "mysql_write_without_business_grants_names_the_missing_privilege"
)

echo "=== Verifying test targets exist in mysql_write_http test binary ==="
AVAILABLE_TESTS=$(cargo test -p aip-service --features mysql --test mysql_write_http -- --list)
for t in "$TARGET_NODB_TEST" "${TARGET_LIVE_TESTS[@]}"; do
  if ! echo "$AVAILABLE_TESTS" | grep -Fxq "$t: test"; then
    echo "ERROR: Target test '$t' not found in mysql_write_http test list!" >&2
    exit 1
  fi
done
echo "All 5 target tests found in test suite."

run_single_test() {
  local test_name="$1"
  shift
  echo "=== Running test: $test_name ($*) ==="
  local status=0
  local out
  out=$(cargo test -p aip-service --features mysql --test mysql_write_http "$test_name" -- "$@") || status=$?
  printf '%s\n' "$out"
  if (( status != 0 )); then
    return "$status"
  fi
  if ! echo "$out" | grep -Fq "test result: ok. 1 passed; 0 failed;"; then
    echo "ERROR: Test '$test_name' did not pass exactly 1 test (got 0 passed or filtered out)" >&2
    return 1
  fi
}

# 1. Run the no-DB validator test (does not require live MySQL)
run_single_test "$TARGET_NODB_TEST" --exact

# 2. Spin up run-owned MySQL 8.4 live fixture and run live coupling & transaction tests
scripts/mysql-live-fixture.sh up
cleanup() {
  scripts/mysql-live-fixture.sh down
}
trap cleanup EXIT

eval "$(scripts/mysql-live-fixture.sh env mysql)"
test -n "${AIP_TEST_MYSQL_URL:-}"

for t in "${TARGET_LIVE_TESTS[@]}"; do
  run_single_test "$t" --ignored --exact
done
