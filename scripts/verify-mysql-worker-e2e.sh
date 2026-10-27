#!/usr/bin/env bash
# Sprint 13-A end to end: real HTTP /apply -> real Node/Python worker subprocess -> one MySQL 8.4 transaction.
#
#   AIP_E2E_TAG=<unique> AIP_E2E_MYSQL_PORT=<port> scripts/verify-mysql-worker-e2e.sh run-ci   # GitHub Ubuntu full suite
#   AIP_E2E_TAG=<unique> AIP_E2E_MYSQL_PORT=<port> scripts/verify-mysql-worker-e2e.sh run-ci-member # member UI diagnostic only
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

if [[ -n "${AIP_E2E_FAILURE_FILE:-}" ]]; then
  AIP_VERIFY_FAILURE_FILE="$AIP_E2E_FAILURE_FILE"
fi
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/verify-safe-locator.sh"
if [[ -n "${AIP_E2E_FAILURE_FILE:-}" ]]; then
  aip_verify_locator_write "run-ci input validation" "not_started" "pending"
  aip_verify_stage "run-ci input validation"
fi

mode="${1:-run}"
case "$mode" in
  run | run-ci | run-ci-member | down) ;;
  *) echo "usage: $0 [run|run-ci|run-ci-member|down]" >&2; aip_verify_stage "mode validation"; aip_verify_fail 2 'case mode validation' ;;
esac

if [[ -z "${AIP_E2E_TAG:-}" || -z "${AIP_E2E_MYSQL_PORT:-}" ]]; then
  echo "ERROR: set AIP_E2E_TAG (unique run-owned tag) and AIP_E2E_MYSQL_PORT (dedicated loopback port above 1024)" >&2
  aip_verify_stage "required E2E inputs"
  aip_verify_fail 2 'AIP_E2E_TAG and AIP_E2E_MYSQL_PORT required'
fi
if [[ ! "$AIP_E2E_TAG" =~ ^[a-z0-9][a-z0-9-]{2,39}$ ]]; then
  echo "ERROR: AIP_E2E_TAG must match ^[a-z0-9][a-z0-9-]{2,39}\$ (lowercase letters, digits, '-'; 3-40 characters)" >&2
  aip_verify_stage "E2E tag validation"
  aip_verify_fail 2 'AIP_E2E_TAG format validation'
fi
if [[ ! "$AIP_E2E_MYSQL_PORT" =~ ^[1-9][0-9]{0,4}$ ]] || (( AIP_E2E_MYSQL_PORT <= 1024 || AIP_E2E_MYSQL_PORT > 65535 )) \
  || (( AIP_E2E_MYSQL_PORT == 3306 || AIP_E2E_MYSQL_PORT == 5432 || AIP_E2E_MYSQL_PORT == 5433 )); then
  echo "ERROR: AIP_E2E_MYSQL_PORT must be a number in 1025-65535 other than 3306, 5432 and 5433" >&2
  aip_verify_stage "MySQL port validation"
  aip_verify_fail 2 'AIP_E2E_MYSQL_PORT range and reserved-port validation'
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
    aip_verify_stage "run-owned fixture name collision check"
    aip_verify_fail 2 'Docker fixture name collision validation'
  fi
done
if [[ "$mode" == run-ci || "$mode" == run-ci-member ]]; then
  [[ "$(uname -s)" == Linux ]] || { echo "ERROR: run-ci needs the Linux runner that already provides Bubblewrap" >&2; aip_verify_stage "run-ci operating-system preflight"; aip_verify_fail 2 'uname -s == Linux'; }
  for tool in cargo bwrap node python3 docker; do
    command -v "$tool" >/dev/null || { echo "ERROR: run-ci needs '$tool' on PATH (see the verify.yml runner preparation)" >&2; aip_verify_stage "run-ci tool preflight: $tool"; aip_verify_fail 2 'command -v "$tool"'; }
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
  [[ -n "$ready" ]] || { echo "ERROR: $mysql_name did not become ready" >&2; aip_verify_stage "MySQL fixture readiness"; aip_verify_fail 1 'MySQL readiness poll'; }
}

# Proves to the test that this MySQL was started by this run: a random marker, written into the container this script
# just created and handed to the test as AIP_E2E_FIXTURE_MARKER. The test refuses destructive DDL without a match.
fixture_marker="aip-e2e-$run_id-$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
write_owner_marker() { # <container port>
  docker exec -e MYSQL_PWD="$password" "$mysql_name" mysql -h127.0.0.1 -P"$1" -uroot "$database" \
    -e "CREATE TABLE aip_e2e_owner (marker VARCHAR(128) NOT NULL PRIMARY KEY); INSERT INTO aip_e2e_owner (marker) VALUES ('$fixture_marker')"
}

if [[ "$mode" == run-ci || "$mode" == run-ci-member ]]; then
  aip_verify_stage "start run-owned MySQL fixture"
  echo "=== Starting run-owned MySQL 8.4 ($mysql_name, 127.0.0.1:$AIP_E2E_MYSQL_PORT) ==="
  docker run -d --name "$mysql_name" "${labels[@]}" -e MYSQL_ROOT_PASSWORD="$password" -e MYSQL_DATABASE="$database" \
    -p "127.0.0.1:$AIP_E2E_MYSQL_PORT:3306" --tmpfs /var/lib/mysql mysql:8.4 >/dev/null
  wait_for_mysql 3306
  write_owner_marker 3306

  aip_verify_stage "Bubblewrap namespace preflight"
  echo "=== Preflight: does the runner's Bubblewrap ($(command -v bwrap)) create namespaces? ==="
  if ! bwrap --unshare-all --ro-bind / / --proc /proc --dev /dev true; then
    echo "BLOCKED: Bubblewrap cannot create namespaces on this host. No worker ran and nothing was changed. The runner needs the" >&2
    echo "'Configure CI Bubblewrap user namespace policy' step of .github/workflows/verify.yml before this mode." >&2
    aip_verify_fail 3 'bwrap --unshare-all --ro-bind / / --proc /proc --dev /dev true'
  fi

  # The test never connects to PostgreSQL; the build guard of aip-test-support only needs syntactically safe DSNs.
  export AIP_TEST_DATABASE_URL="${AIP_TEST_DATABASE_URL:-host=127.0.0.1 port=55473 dbname=aip_test_run user=runner}"
  export AIP_TEST_ADMIN_URL="${AIP_TEST_ADMIN_URL:-host=127.0.0.1 port=55473 dbname=postgres user=runner}"
  export AIP_TEST_MYSQL_URL="mysql://root:$password@127.0.0.1:$AIP_E2E_MYSQL_PORT/$database"
  export AIP_E2E_FIXTURE_MARKER="$fixture_marker"
  cd "$repo"
  # Each Rust suite owns its own schema/processes. Collect independent failures
  # before returning nonzero, so member UI failures cannot hide admin/job results.
  aip_verify_stage "FaceReview independent E2E suites"
  python3 "$repo/scripts/facereview-e2e-suites.py" "$mode" \
    --output "${AIP_FACEREVIEW_BROWSER_OUTPUT:-/tmp/aip-e2e-results-$run_id}"
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
