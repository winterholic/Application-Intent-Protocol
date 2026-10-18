#!/usr/bin/env bash
# WBS 7.2.8 emotion summary/DNA: oracle differential tests, plus the Mongo capability gap test
# when the caller provides a run-owned loopback Mongo (never a shared server).
set -euo pipefail
cd "$(dirname "$0")/.."

python3 -m unittest -v product/tests/slice728_summary_oracle_test.py

if [[ -n "${AIP_TEST_MONGO_URL:-}" ]]; then
  : "${CARGO_TARGET_DIR:?set a run-owned cargo target dir}"
  unset PGHOST PGPORT PGDATABASE PGUSER PGPASSWORD PGSERVICE PGSERVICEFILE
  # Synthetic DSNs satisfy aip-test-support's build guard only; no PostgreSQL connection is made.
  export AIP_TEST_DATABASE_URL="host=127.0.0.1 port=55473 dbname=aip_test_run user=runner"
  export AIP_TEST_ADMIN_URL="host=127.0.0.1 port=55473 dbname=postgres user=runner"
  (cd spikes/spike-v4-worker && cargo test --offline --test slice728_summary_mongo_gap -- --nocapture)
else
  echo "skipped: slice728_summary_mongo_gap (set AIP_TEST_MONGO_URL=mongodb://127.0.0.1:<dedicated high port>)"
fi
