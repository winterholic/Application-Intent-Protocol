#!/usr/bin/env bash
# Sprint 13-A end to end: real HTTP /apply -> real Node/Python worker subprocess -> one MySQL 8.4 transaction.
#
#   AIP_E2E_TAG=<unique> AIP_E2E_MYSQL_PORT=<port above 1024, not 3306> scripts/verify-mysql-worker-e2e.sh [run|down]
#
# What runs: crates/aip-cli/tests/mysql_worker_write_e2e.rs drives the real `aip service serve` binary (JWKS-verified
# bearer token bound to a principal, workers.enable_writes, contract fingerprint from the listening event). The worker
# keeps the product default isolation: Bubblewrap on Linux. This script never selects Isolation::None, never edits the
# service config to weaken isolation and never touches a MySQL/PostgreSQL service it did not start.
#
# Environment: a run-owned MySQL 8.4 container and a Linux builder container share one network namespace, so the
# fixture is reachable at 127.0.0.1:$AIP_E2E_MYSQL_PORT without publishing any host port. Bubblewrap needs the builder
# to be allowed to create user namespaces. Docker's default seccomp profile denies that, so on such a host the run stops
# at the preflight below with the exact error. Choosing a more permissive container profile is a security decision for
# the host owner: pass it explicitly as AIP_E2E_DOCKER_SECURITY_OPT (for example "seccomp=unconfined") once approved.
# Other options: AIP_E2E_PLATFORM (default linux/amd64), AIP_E2E_NODE_VERSION (default 26.9.0, matching CI), AIP_E2E_KEEP=1 to keep the containers and volumes.
set -euo pipefail

: "${AIP_E2E_TAG:?set a unique run-owned fixture tag}"
: "${AIP_E2E_MYSQL_PORT:?set a dedicated loopback port above 1024 (not 3306)}"
if (( AIP_E2E_MYSQL_PORT <= 1024 || AIP_E2E_MYSQL_PORT == 3306 )); then
  echo "ERROR: AIP_E2E_MYSQL_PORT must be above 1024 and not 3306" >&2
  exit 2
fi

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tag="$AIP_E2E_TAG"
mysql_name="aip-e2e-mysql-$tag"
builder_name="aip-e2e-builder-$tag"
target_volume="aip-e2e-target-$tag"
cargo_volume="aip-e2e-cargo-$tag"
node_version="${AIP_E2E_NODE_VERSION:-26.9.0}"
# CI and earlier local Linux runs use x86_64; the arm64 build of spike-v6-transport is a separate known compile issue.
platform="${AIP_E2E_PLATFORM:-linux/amd64}"
password="aip_e2e_pw_$tag"
database="aip_test_admin"
security_args=()
if [[ -n "${AIP_E2E_DOCKER_SECURITY_OPT:-}" ]]; then
  security_args=(--security-opt "$AIP_E2E_DOCKER_SECURITY_OPT")
fi

# Removes only the objects this tag created.
down() {
  docker rm -f "$builder_name" "$mysql_name" >/dev/null 2>&1 || true
  docker volume rm "$target_volume" "$cargo_volume" >/dev/null 2>&1 || true
}

case "${1:-run}" in
  down) down; exit 0 ;;
  run) ;;
  *) echo "usage: $0 [run|down]" >&2; exit 2 ;;
esac

if [[ -z "${AIP_E2E_KEEP:-}" ]]; then trap 'status=$?; down; exit "$status"' EXIT; fi
for object in "$mysql_name" "$builder_name"; do
  if docker inspect "$object" >/dev/null 2>&1; then
    echo "ERROR: container $object already exists; pick another AIP_E2E_TAG or run '$0 down'" >&2
    exit 2
  fi
done

echo "=== Starting run-owned MySQL 8.4 ($mysql_name, 127.0.0.1:$AIP_E2E_MYSQL_PORT in its own network namespace) ==="
docker run -d --name "$mysql_name" -e MYSQL_ROOT_PASSWORD="$password" -e MYSQL_DATABASE="$database" \
  --tmpfs /var/lib/mysql mysql:8.4 --port="$AIP_E2E_MYSQL_PORT" --bind-address=127.0.0.1 >/dev/null
for _ in $(seq 1 60); do
  if docker exec "$mysql_name" mysqladmin -h127.0.0.1 -P"$AIP_E2E_MYSQL_PORT" -uroot -p"$password" ping 2>/dev/null | grep -q alive; then
    ready=1
    break
  fi
  sleep 2
done
[[ -n "${ready:-}" ]] || { echo "ERROR: $mysql_name did not become ready" >&2; exit 1; }

echo "=== Starting Linux builder ($builder_name) ==="
docker volume create "$target_volume" >/dev/null
docker volume create "$cargo_volume" >/dev/null
docker run -d --name "$builder_name" --platform "$platform" --network "container:$mysql_name" ${security_args[@]+"${security_args[@]}"} \
  -v "$repo":/src:ro -v "$target_volume":/target -v "$cargo_volume":/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR=/target \
  -e AIP_TEST_MYSQL_URL="mysql://root:$password@127.0.0.1:$AIP_E2E_MYSQL_PORT/$database" \
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
in_builder 'set -o pipefail; cargo test --locked -p aip-cli --features mysql --test mysql_worker_write_e2e --no-run 2>&1 | tail -5'

echo "=== Preflight: can this container create the namespaces Bubblewrap needs? ==="
if ! in_builder 'bwrap --unshare-all --ro-bind / / --proc /proc --dev /dev true'; then
  cat >&2 <<EOF
BLOCKED: Bubblewrap cannot create namespaces in this container (default Docker seccomp profile).
Nothing was weakened and no worker ran. The test needs an approved Linux environment for the product default isolation;
rerun with AIP_E2E_DOCKER_SECURITY_OPT set to the approved container security option.
EOF
  exit 3
fi

echo "=== Verifying the target test exists ==="
in_builder 'set -o pipefail; cargo test --locked -p aip-cli --features mysql --test mysql_worker_write_e2e -- --list --ignored 2>&1 | tee /tmp/list.txt | tail -3
grep -Fxq "http_apply_runs_real_node_and_python_workers_in_one_mysql_transaction: test" /tmp/list.txt'

echo "=== Running the end-to-end test (real aip binary, real workers, real MySQL) ==="
in_builder 'set -o pipefail; cargo test --locked -p aip-cli --features mysql --test mysql_worker_write_e2e -- --ignored --exact http_apply_runs_real_node_and_python_workers_in_one_mysql_transaction --nocapture --test-threads=1 2>&1 | tee /tmp/run.txt
grep -Fq "test result: ok. 1 passed; 0 failed;" /tmp/run.txt'
