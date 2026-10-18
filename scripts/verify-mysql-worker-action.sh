#!/usr/bin/env bash
set -euo pipefail

: "${AIP_FIXTURE_TAG:?set a unique run-owned fixture tag}"
: "${AIP_FIXTURE_MYSQL_PORT:?set a dedicated loopback port above 1024}"
: "${AIP_FIXTURE_MARIADB_PORT:?set a second dedicated loopback port above 1024}"
export AIP_FIXTURE_DIR="${AIP_FIXTURE_DIR:-/private/tmp/aip-mysql-${AIP_FIXTURE_TAG}}"

unset PGHOST PGPORT PGDATABASE PGUSER PGPASSWORD PGSERVICE PGSERVICEFILE
unset AIP_TEST_DATABASE_URL AIP_TEST_ADMIN_URL

scripts/mysql-live-fixture.sh up
cleanup() {
  scripts/mysql-live-fixture.sh down
}
trap cleanup EXIT

eval "$(scripts/mysql-live-fixture.sh env mysql)"
test -n "${AIP_TEST_MYSQL_URL:-}"

# These synthetic PostgreSQL DSNs satisfy aip-test-support's build guard only; no PostgreSQL connection is made.
export AIP_TEST_DATABASE_URL="host=127.0.0.1 port=55473 dbname=aip_test_run user=runner"
export AIP_TEST_ADMIN_URL="host=127.0.0.1 port=55473 dbname=postgres user=runner"

cargo test -p aip-service --features mysql --test mysql_write_http \
  mysql_worker_action_uses_one_fenced_idempotent_transaction -- --ignored --exact
cargo test -p aip-service --features mysql --test mysql_write_http \
  mysql_apply_is_atomic_idempotent_audited_and_fenced -- --ignored --exact
cargo test -p aip-service --features mysql --test mysql_write_http \
  mysql_write_without_business_grants_names_the_missing_privilege -- --ignored --exact
