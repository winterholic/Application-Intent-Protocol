#!/usr/bin/env bash
set -euo pipefail

if ! command -v systemd-run >/dev/null 2>&1; then
  echo "systemd-run is required to create a delegated cgroup v2 test scope" >&2
  exit 1
fi

systemd_version="$(systemd-run --version | awk 'NR == 1 { print $2 }')"
if [[ "${systemd_version}" -lt 252 ]]; then
  echo "systemd 252+ is required for delegated scopes (found ${systemd_version})" >&2
  exit 1
fi

unit="aip-worker-cgroup-test-$$"
systemd-run --quiet --scope --collect --unit="${unit}" \
  --property="Delegate=cpu memory pids" \
  bash -euo pipefail -c '
    # DelegateSubgroup= is not applied to --scope units, so the shell starts in the scope cgroup itself.
    # cgroup v2 forbids enabling controllers for children while the cgroup holds processes, so move into a child first.
    cgroup_path="$(awk -F: '\''$1 == "0" { print $3 }'\'' /proc/self/cgroup)"
    cgroup_path="${cgroup_path%/init}"
    cgroup_root="/sys/fs/cgroup${cgroup_path}"
    mkdir -p "${cgroup_root}/init"
    echo "$$" > "${cgroup_root}/init/cgroup.procs"
    if [[ ! -f "${cgroup_root}/cgroup.subtree_control" ]]; then
      echo "unified cgroup v2 subtree is unavailable at ${cgroup_root}" >&2
      exit 1
    fi
    printf "%s\n" "+cpu +memory +pids" > "${cgroup_root}/cgroup.subtree_control"
    export AIP_TEST_CGROUP_V2_ROOT="${cgroup_root}"
    "${AIP_CARGO_BIN:-cargo}" test --manifest-path spikes/spike-v4-worker/Cargo.toml --lib \
      delegated_cgroup_v2_ \
      --locked --target-dir target -- --test-threads=1
  '
