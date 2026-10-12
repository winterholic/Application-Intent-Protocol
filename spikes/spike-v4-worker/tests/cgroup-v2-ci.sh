#!/usr/bin/env bash
set -euo pipefail

if ! command -v systemd-run >/dev/null 2>&1; then
  echo "systemd-run is required to create a delegated cgroup v2 test scope" >&2
  exit 1
fi

systemd_version="$(systemd-run --version | awk 'NR == 1 { print $2 }')"
if [[ "${systemd_version}" -lt 254 ]]; then
  echo "systemd 254+ is required for DelegateSubgroup= (found ${systemd_version})" >&2
  exit 1
fi

unit="aip-worker-cgroup-test-$$"
systemd-run --quiet --scope --collect --unit="${unit}" \
  --property=Delegate=cpu,memory,pids \
  --property=DelegateSubgroup=init \
  bash -euo pipefail -c '
    cgroup_path="$(awk -F: '\''$1 == "0" { print $3 }'\'' /proc/self/cgroup)"
    case "${cgroup_path}" in
      */init) cgroup_path="${cgroup_path%/init}" ;;
      *)
        echo "DelegateSubgroup=init did not place this process in an init subgroup: ${cgroup_path}" >&2
        exit 1
        ;;
    esac
    cgroup_root="/sys/fs/cgroup${cgroup_path}"
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
