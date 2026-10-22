#!/usr/bin/env bash
# Sprint 13-A end to end: real HTTP /apply -> real Node/Python worker subprocess -> one MySQL 8.4 transaction.
#
#   AIP_E2E_TAG=<unique> AIP_E2E_MYSQL_PORT=<port> scripts/verify-mysql-worker-e2e.sh run-ci   # GitHub Ubuntu runner
#   AIP_E2E_TAG=<unique> AIP_E2E_MYSQL_PORT=<port> scripts/verify-mysql-worker-e2e.sh run      # Linux builder container
#   AIP_E2E_TAG=<unique> AIP_E2E_MYSQL_PORT=<port> scripts/verify-mysql-worker-e2e.sh down     # remove this tag's objects
#
# What runs: crates/aip-cli/tests/mysql_worker_write_e2e.rs drives the real `aip service serve` binary (JWKS-verified
# bearer token bound to a principal, workers.enable_writes, contract fingerprint from the listening event). The worker
# keeps the product default isolation: Bubblewrap on Linux. No mode selects Isolation::None, edits the service config to
# weaken isolation, changes a container security profile or touches a MySQL/PostgreSQL service it did not start.
#
# run-ci: cargo and Bubblewrap run directly on the host. Only the MySQL 8.4 fixture is a container, published on
# 127.0.0.1:$AIP_E2E_MYSQL_PORT. It relies on the runner preparation that .github/workflows/verify.yml already does
# (bubblewrap install, the "Configure CI Bubblewrap user namespace policy" step, Node 26). If Bubblewrap cannot create
# namespaces the run stops before any test; the script never adds a profile or relaxes a policy itself.
#
# run: the same test inside a Linux builder container that shares the fixture's network namespace. Docker's default
# seccomp profile denies the namespaces Bubblewrap needs, so on such a host this mode stops at its preflight; use run-ci.
#
# Ownership: every container and volume this script creates carries labels aip.e2e.tag=<tag> and aip.e2e.run=<run id>.
# Cleanup (exit trap and `down`) removes only objects whose label matches; a same-named object created by anything else
# is never removed. Name collisions are checked before the trap is installed and before anything is created.
# The MySQL fixture also proves itself to the test: a random run marker is written into the container's admin database
# (table aip_e2e_owner) and passed as AIP_E2E_FIXTURE_MARKER; the test fails closed before any DDL without a match.
# Other options: AIP_E2E_PLATFORM (run only, default linux/amd64), AIP_E2E_NODE_VERSION (run only, default 26.9.0),
# AIP_E2E_KEEP=1 to keep this run's objects.
set -euo pipefail

mode="${1:-run}"
case "$mode" in
  run | run-ci | down) ;;
  *) echo "usage: $0 [run|run-ci|down]" >&2; exit 2 ;;
esac

if [[ -z "${AIP_E2E_TAG:-}" || -z "${AIP_E2E_MYSQL_PORT:-}" ]]; then
  echo "ERROR: set AIP_E2E_TAG (unique run-owned tag) and AIP_E2E_MYSQL_PORT (dedicated loopback port above 1024)" >&2
  exit 2
fi
if [[ ! "$AIP_E2E_TAG" =~ ^[a-z0-9][a-z0-9-]{2,39}$ ]]; then
  echo "ERROR: AIP_E2E_TAG must match ^[a-z0-9][a-z0-9-]{2,39}\$ (lowercase letters, digits, '-'; 3-40 characters)" >&2
  exit 2
fi
if [[ ! "$AIP_E2E_MYSQL_PORT" =~ ^[1-9][0-9]{0,4}$ ]] || (( AIP_E2E_MYSQL_PORT <= 1024 || AIP_E2E_MYSQL_PORT > 65535 )) \
  || (( AIP_E2E_MYSQL_PORT == 3306 || AIP_E2E_MYSQL_PORT == 5432 || AIP_E2E_MYSQL_PORT == 5433 )); then
  echo "ERROR: AIP_E2E_MYSQL_PORT must be a number in 1025-65535 other than 3306, 5432 and 5433" >&2
  exit 2
fi

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tag="$AIP_E2E_TAG"
run_id="$tag.$$.$(date +%s)"
mysql_name="aip-e2e-mysql-$tag"
builder_name="aip-e2e-builder-$tag"
target_volume="aip-e2e-target-$tag"
cargo_volume="aip-e2e-cargo-$tag"
node_version="${AIP_E2E_NODE_VERSION:-26.9.0}"
# CI and earlier local Linux runs use x86_64.
platform="${AIP_E2E_PLATFORM:-linux/amd64}"
password="aip_e2e_pw_$tag"
database="aip_test_admin"
test_name="http_apply_runs_real_node_and_python_workers_in_one_mysql_transaction"
cargo_args=(--locked -p aip-cli --features mysql --test mysql_worker_write_e2e)

exists() { docker "$1" inspect "$2" >/dev/null 2>&1; }

label_of() {
  local format='{{index .Config.Labels "'"$3"'"}}'
  [[ "$1" == volume ]] && format='{{index .Labels "'"$3"'"}}'
  docker "$1" inspect -f "$format" "$2" 2>/dev/null || true
}

# remove_if_labelled <container|volume> <name> <label key> <label value>
remove_if_labelled() {
  exists "$1" "$2" || return 0
  if [[ "$(label_of "$1" "$2" "$3")" == "$4" ]]; then
    docker "$1" rm -f "$2" >/dev/null 2>&1 || echo "WARNING: could not remove $1 $2" >&2
  else
    echo "skip: $1 $2 is not labelled $3=$4; leaving it alone" >&2
  fi
}

cleanup_by() { # <label key> <label value>
  remove_if_labelled container "$builder_name" "$1" "$2"
  remove_if_labelled container "$mysql_name" "$1" "$2"
  remove_if_labelled volume "$target_volume" "$1" "$2"
  remove_if_labelled volume "$cargo_volume" "$1" "$2"
}

if [[ "$mode" == down ]]; then
  cleanup_by aip.e2e.tag "$tag"
  exit 0
fi

names=("$mysql_name")
kinds=(container)
if [[ "$mode" == run ]]; then
  names+=("$builder_name" "$target_volume" "$cargo_volume")
  kinds+=(container volume volume)
fi
# Collision check first: nothing exists yet that this run owns, so nothing may be removed on this path.
for i in "${!names[@]}"; do
  if exists "${kinds[$i]}" "${names[$i]}"; then
    echo "ERROR: ${kinds[$i]} ${names[$i]} already exists and was not created by this run; pick another AIP_E2E_TAG (nothing was removed)" >&2
    exit 2
  fi
done
if [[ "$mode" == run-ci ]]; then
  [[ "$(uname -s)" == Linux ]] || { echo "ERROR: run-ci needs the Linux runner that already provides Bubblewrap" >&2; exit 2; }
  for tool in cargo bwrap node python3 docker; do
    command -v "$tool" >/dev/null || { echo "ERROR: run-ci needs '$tool' on PATH (see the verify.yml runner preparation)" >&2; exit 2; }
  done
fi

if [[ -z "${AIP_E2E_KEEP:-}" ]]; then
  trap 'status=$?; cleanup_by aip.e2e.run "$run_id"; exit "$status"' EXIT
fi
labels=(--label "aip.e2e.tag=$tag" --label "aip.e2e.run=$run_id")

wait_for_mysql() { # <container port as seen by mysqladmin inside the container>
  local ready=""
  for _ in $(seq 1 60); do
    if docker exec "$mysql_name" mysqladmin -h127.0.0.1 -P"$1" -uroot -p"$password" ping 2>/dev/null | grep -q alive; then
      ready=1
      break
    fi
    sleep 2
  done
  [[ -n "$ready" ]] || { echo "ERROR: $mysql_name did not become ready" >&2; exit 1; }
}

# Proves to the test that this MySQL was started by this run: a random marker, written into the container this script
# just created and handed to the test as AIP_E2E_FIXTURE_MARKER. The test refuses destructive DDL without a match.
fixture_marker="aip-e2e-$run_id-$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
write_owner_marker() { # <container port>
  docker exec -e MYSQL_PWD="$password" "$mysql_name" mysql -h127.0.0.1 -P"$1" -uroot "$database" \
    -e "CREATE TABLE aip_e2e_owner (marker VARCHAR(128) NOT NULL PRIMARY KEY); INSERT INTO aip_e2e_owner (marker) VALUES ('$fixture_marker')"
}

if [[ "$mode" == run-ci ]]; then
  echo "=== Starting run-owned MySQL 8.4 ($mysql_name, 127.0.0.1:$AIP_E2E_MYSQL_PORT) ==="
  docker run -d --name "$mysql_name" "${labels[@]}" -e MYSQL_ROOT_PASSWORD="$password" -e MYSQL_DATABASE="$database" \
    -p "127.0.0.1:$AIP_E2E_MYSQL_PORT:3306" --tmpfs /var/lib/mysql mysql:8.4 >/dev/null
  wait_for_mysql 3306
  write_owner_marker 3306

  echo "=== Preflight: does the runner's Bubblewrap ($(command -v bwrap)) create namespaces? ==="
  if ! bwrap --unshare-all --ro-bind / / --proc /proc --dev /dev true; then
    echo "BLOCKED: Bubblewrap cannot create namespaces on this host. No worker ran and nothing was changed. The runner needs the" >&2
    echo "'Configure CI Bubblewrap user namespace policy' step of .github/workflows/verify.yml before this mode." >&2
    exit 3
  fi

  # The test never connects to PostgreSQL; the build guard of aip-test-support only needs syntactically safe DSNs.
  export AIP_TEST_DATABASE_URL="${AIP_TEST_DATABASE_URL:-host=127.0.0.1 port=55473 dbname=aip_test_run user=runner}"
  export AIP_TEST_ADMIN_URL="${AIP_TEST_ADMIN_URL:-host=127.0.0.1 port=55473 dbname=postgres user=runner}"
  export AIP_TEST_MYSQL_URL="mysql://root:$password@127.0.0.1:$AIP_E2E_MYSQL_PORT/$database"
  export AIP_E2E_FIXTURE_MARKER="$fixture_marker"
  cd "$repo"
  echo "=== Verifying the target test exists ==="
  cargo test "${cargo_args[@]}" -- --list --ignored 2>&1 | tee /tmp/aip-e2e-list-"$tag".txt | tail -3
  grep -Fxq "$test_name: test" /tmp/aip-e2e-list-"$tag".txt
  echo "=== Running the end-to-end test (real aip binary, real workers, real MySQL) ==="
  cargo test "${cargo_args[@]}" -- --ignored --exact "$test_name" --nocapture --test-threads=1 2>&1 | tee /tmp/aip-e2e-run-"$tag".txt
  grep -Fq "test result: ok. 1 passed; 0 failed;" /tmp/aip-e2e-run-"$tag".txt
  # The FaceReview password login (WBS 7.2.2) checks passwords in the same sandboxed Python worker, on the same fixture.
  echo "=== Running the password login end-to-end test (real aip binary, Python extension in the sandbox, real MySQL) ==="
  auth_args=(--locked -p aip-cli --features mysql --test mysql_facereview_auth)
  auth_test="facereview_accounts_log_in_refresh_and_log_out_without_the_originals_replay_defect"
  cargo test "${auth_args[@]}" -- --list --ignored 2>&1 | tee /tmp/aip-e2e-auth-list-"$tag".txt | tail -3
  grep -Fxq "$auth_test: test" /tmp/aip-e2e-auth-list-"$tag".txt
  cargo test "${auth_args[@]}" -- --ignored --exact "$auth_test" --nocapture --test-threads=1 2>&1 | tee /tmp/aip-e2e-auth-run-"$tag".txt
  grep -Fq "test result: ok. 1 passed; 0 failed;" /tmp/aip-e2e-auth-run-"$tag".txt
  grep -Fq "member SDK journey: login, one refresh, UI adapter video read, bookmark on/off, comment create/edit/delete, logout" /tmp/aip-e2e-auth-run-"$tag".txt
  # The FaceReview recommendation lists (WBS 7.2.3) read MongoDB through workers.mongo and compare with the original services' answers.
  # verify.yml starts the native-stores MongoDB fixture and exports AIP_TEST_MONGO_URL for it; the test refuses a non-loopback URL.
  if [[ -z "${AIP_TEST_MONGO_URL:-}" ]]; then
    echo "ERROR: run-ci needs AIP_TEST_MONGO_URL for the recommendation end-to-end test (see the native fixtures step of verify.yml)" >&2
    exit 2
  fi
  echo "=== Running the recommendation end-to-end test (real aip binary, Python extension in the sandbox, real MySQL and MongoDB) ==="
  recommend_args=(--locked -p aip-cli --features mysql --test mysql_facereview_recommend)
  recommend_test="facereview_recommendations_match_the_original_services_through_the_real_server"
  cargo test "${recommend_args[@]}" -- --list --ignored 2>&1 | tee /tmp/aip-e2e-recommend-list-"$tag".txt | tail -3
  grep -Fxq "$recommend_test: test" /tmp/aip-e2e-recommend-list-"$tag".txt
  cargo test "${recommend_args[@]}" -- --ignored --exact "$recommend_test" --nocapture --test-threads=1 2>&1 | tee /tmp/aip-e2e-recommend-run-"$tag".txt
  grep -Fq "test result: ok. 1 passed; 0 failed;" /tmp/aip-e2e-recommend-run-"$tag".txt
  # The watch detail (WBS 7.2.5) adds the server-side all-viewer timeline aggregate and the `view` increment on the same fixtures.
  echo "=== Running the watch detail end-to-end test (real aip binary, Python extension in the sandbox, real MySQL and MongoDB) ==="
  watch_args=(--locked -p aip-cli --features mysql --test mysql_facereview_watch)
  watch_test="facereview_watch_detail_matches_the_original_service_through_the_real_server"
  cargo test "${watch_args[@]}" -- --list --ignored 2>&1 | tee /tmp/aip-e2e-watch-list-"$tag".txt | tail -3
  grep -Fxq "$watch_test: test" /tmp/aip-e2e-watch-list-"$tag".txt
  cargo test "${watch_args[@]}" -- --ignored --exact "$watch_test" --nocapture --test-threads=1 2>&1 | tee /tmp/aip-e2e-watch-run-"$tag".txt
  grep -Fq "test result: ok. 1 passed; 0 failed;" /tmp/aip-e2e-watch-run-"$tag".txt
  # The saga of a write extension (WBS 7.2.5): MongoDB and Redis effects next to the MySQL commit, compensation and unknown commit.
  # verify.yml exports AIP_TEST_REDIS_URL for the native-stores Redis fixture next to AIP_TEST_MONGO_URL.
  if [[ -z "${AIP_TEST_REDIS_URL:-}" ]]; then
    echo "ERROR: run-ci needs AIP_TEST_REDIS_URL for the external effects end-to-end test (see the native fixtures step of verify.yml)" >&2
    exit 2
  fi
  echo "=== Running the external effects end-to-end test (real aip binary, Python extension, MySQL, MongoDB, Redis) ==="
  effects_args=(--locked -p aip-cli --features mysql --test mysql_external_effects)
  effects_test="write_extension_effects_commit_or_compensate_with_the_relational_transaction"
  cargo test "${effects_args[@]}" -- --list --ignored 2>&1 | tee /tmp/aip-e2e-effects-list-"$tag".txt | tail -3
  grep -Fxq "$effects_test: test" /tmp/aip-e2e-effects-list-"$tag".txt
  cargo test "${effects_args[@]}" -- --ignored --exact "$effects_test" --nocapture --test-threads=1 2>&1 | tee /tmp/aip-e2e-effects-run-"$tag".txt
  grep -Fq "test result: ok. 1 passed; 0 failed;" /tmp/aip-e2e-effects-run-"$tag".txt
  # The scheduled recommendation pool refresh (WBS 7.2.7): a durable job writes the Redis cache the recommendation lists read.
  echo "=== Running the recommendation pool job end-to-end test (real aip binary, Python extension, MySQL, MongoDB, Redis, schedule) ==="
  pool_args=(--locked -p aip-cli --features mysql --test mysql_facereview_pool_job)
  pool_test="the_scheduled_pool_job_fills_the_cache_the_recommendation_lists_read"
  cargo test "${pool_args[@]}" -- --list --ignored 2>&1 | tee /tmp/aip-e2e-pool-list-"$tag".txt | tail -3
  grep -Fxq "$pool_test: test" /tmp/aip-e2e-pool-list-"$tag".txt
  cargo test "${pool_args[@]}" -- --ignored --exact "$pool_test" --nocapture --test-threads=1 2>&1 | tee /tmp/aip-e2e-pool-run-"$tag".txt
  grep -Fq "test result: ok. 1 passed; 0 failed;" /tmp/aip-e2e-pool-run-"$tag".txt
  # The trending video collection (WBS 7.2.7): a query-string integration, job-created rows and initial MongoDB documents.
  echo "=== Running the trending video job end-to-end test (real aip binary, Python extension, MySQL, MongoDB, stand-in YouTube) ==="
  trend_args=(--locked -p aip-cli --features mysql --test mysql_facereview_trending_job)
  trend_test="the_trending_job_saves_new_videos_with_their_initial_documents"
  cargo test "${trend_args[@]}" -- --list --ignored 2>&1 | tee /tmp/aip-e2e-trend-list-"$tag".txt | tail -3
  grep -Fxq "$trend_test: test" /tmp/aip-e2e-trend-list-"$tag".txt
  cargo test "${trend_args[@]}" -- --ignored --exact "$trend_test" --nocapture --test-threads=1 2>&1 | tee /tmp/aip-e2e-trend-run-"$tag".txt
  grep -Fq "test result: ok. 1 passed; 0 failed;" /tmp/aip-e2e-trend-run-"$tag".txt
  # The category fill (WBS 7.2.7): search queries per thinnest genre within the original's quota, saved under the searched genre.
  echo "=== Running the category fill job end-to-end test (real aip binary, Python extension, MySQL, MongoDB, stand-in YouTube) ==="
  fill_args=(--locked -p aip-cli --features mysql --test mysql_facereview_category_fill_job)
  fill_test="the_category_fill_job_searches_the_thinnest_genres_within_the_original_budget"
  cargo test "${fill_args[@]}" -- --list --ignored 2>&1 | tee /tmp/aip-e2e-fill-list-"$tag".txt | tail -3
  grep -Fxq "$fill_test: test" /tmp/aip-e2e-fill-list-"$tag".txt
  cargo test "${fill_args[@]}" -- --ignored --exact "$fill_test" --nocapture --test-threads=1 2>&1 | tee /tmp/aip-e2e-fill-run-"$tag".txt
  grep -Fq "test result: ok. 1 passed; 0 failed;" /tmp/aip-e2e-fill-run-"$tag".txt
  # The administrator lists (WBS 7.2.9) compare the real server's reads and totals with the original AdminService's recorded answers.
  echo "=== Running the administrator list end-to-end test (real aip binary, real MySQL) ==="
  admin_args=(--locked -p aip-cli --features mysql --test mysql_facereview_admin)
  admin_test="facereview_administrator_lists_match_the_original_admin_service_through_the_real_server"
  cargo test "${admin_args[@]}" -- --list --ignored 2>&1 | tee /tmp/aip-e2e-admin-list-"$tag".txt | tail -3
  grep -Fxq "$admin_test: test" /tmp/aip-e2e-admin-list-"$tag".txt
  cargo test "${admin_args[@]}" -- --ignored --exact "$admin_test" --nocapture --test-threads=1 2>&1 | tee /tmp/aip-e2e-admin-run-"$tag".txt
  grep -Fq "test result: ok. 1 passed; 0 failed;" /tmp/aip-e2e-admin-run-"$tag".txt
  exit 0
fi

echo "=== Starting run-owned MySQL 8.4 ($mysql_name, 127.0.0.1:$AIP_E2E_MYSQL_PORT in its own network namespace) ==="
docker run -d --name "$mysql_name" "${labels[@]}" -e MYSQL_ROOT_PASSWORD="$password" -e MYSQL_DATABASE="$database" \
  --tmpfs /var/lib/mysql mysql:8.4 --port="$AIP_E2E_MYSQL_PORT" --bind-address=127.0.0.1 >/dev/null
wait_for_mysql "$AIP_E2E_MYSQL_PORT"
write_owner_marker "$AIP_E2E_MYSQL_PORT"

echo "=== Starting Linux builder ($builder_name) ==="
docker volume create "${labels[@]}" "$target_volume" >/dev/null
docker volume create "${labels[@]}" "$cargo_volume" >/dev/null
docker run -d --name "$builder_name" "${labels[@]}" --platform "$platform" --network "container:$mysql_name" \
  -v "$repo":/src:ro -v "$target_volume":/target -v "$cargo_volume":/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR=/target \
  -e AIP_TEST_MYSQL_URL="mysql://root:$password@127.0.0.1:$AIP_E2E_MYSQL_PORT/$database" \
  -e AIP_E2E_FIXTURE_MARKER="$fixture_marker" \
  -e AIP_TEST_DATABASE_URL="host=127.0.0.1 port=55473 dbname=aip_test_run user=runner" \
  -e AIP_TEST_ADMIN_URL="host=127.0.0.1 port=55473 dbname=postgres user=runner" \
  -w /src rust:1 sleep infinity >/dev/null

in_builder() { docker exec "$builder_name" bash -c "$1"; }

echo "=== Installing Bubblewrap, Python and Node $node_version in the builder ==="
in_builder "set -euo pipefail
apt-get update -qq >/dev/null
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq bubblewrap python3 xz-utils curl >/dev/null
arch=\$(uname -m); case \"\$arch\" in aarch64) arch=arm64 ;; x86_64) arch=x64 ;; esac
base=https://nodejs.org/dist/v$node_version
file=node-v$node_version-linux-\$arch.tar.xz
cd /tmp
curl -fsSLO \"\$base/\$file\"
curl -fsSL \"\$base/SHASUMS256.txt\" | grep \" \$file\\\$\" | sha256sum -c -
tar -xJf \"\$file\" -C /usr/local --strip-components=1
node --version; python3 --version; bwrap --version"

echo "=== Building the test and the aip binary (mysql feature) on Linux ==="
in_builder 'set -o pipefail; cargo test '"${cargo_args[*]}"' --no-run 2>&1 | tail -5'

echo "=== Preflight: can this container create the namespaces Bubblewrap needs? ==="
if ! in_builder 'bwrap --unshare-all --ro-bind / / --proc /proc --dev /dev true'; then
  echo "BLOCKED: Bubblewrap cannot create namespaces in this container. No worker ran and nothing was weakened." >&2
  echo "Use run-ci on a runner prepared as in .github/workflows/verify.yml." >&2
  exit 3
fi

echo "=== Verifying the target test exists ==="
in_builder 'set -o pipefail; cargo test '"${cargo_args[*]}"' -- --list --ignored 2>&1 | tee /tmp/list.txt | tail -3
grep -Fxq "'"$test_name"': test" /tmp/list.txt'

echo "=== Running the end-to-end test (real aip binary, real workers, real MySQL) ==="
in_builder 'set -o pipefail; cargo test '"${cargo_args[*]}"' -- --ignored --exact '"$test_name"' --nocapture --test-threads=1 2>&1 | tee /tmp/run.txt
grep -Fq "test result: ok. 1 passed; 0 failed;" /tmp/run.txt'
