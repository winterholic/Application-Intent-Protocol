# Safe CI failure locator: stage, checked-in command text, and exit status only.
# Callers must set -E and aip_verify_stage before each meaningful command.
aip_verify_locator_write() {
  [[ -n "${AIP_VERIFY_FAILURE_FILE:-}" ]] || return 0
  local stage="${1:-${AIP_VERIFY_STAGE:-not_started}}"
  local command="${2:-not_started}"
  local status="${3:-pending}"
  command="${command//$'\n'/ }"
  umask 077
  {
    printf 'stage=%s\n' "${stage:0:160}"
    printf 'command=%s\n' "${command:0:500}"
    printf 'exit_status=%s\n' "$status"
  } > "$AIP_VERIFY_FAILURE_FILE"
}

aip_verify_fail() {
  local status="$1"
  local command="$2"
  aip_verify_locator_write "${AIP_VERIFY_STAGE:-not_started}" "$command" "$status"
  AIP_VERIFY_RECORDED=1
  exit "$status"
}

aip_verify_failure_locator() {
  local status="$1"
  local command="$2"
  aip_verify_fail "$status" "$command"
}

aip_verify_stage() {
  AIP_VERIFY_STAGE="$1"
  aip_verify_locator_write "$AIP_VERIFY_STAGE" "pending" "pending"
}

if [[ -n "${AIP_VERIFY_FAILURE_FILE:-}" ]]; then
  AIP_VERIFY_RECORDED=0
  aip_verify_locator_write "${AIP_VERIFY_STAGE:-workflow_started}" "not_started" "pending"
  trap 'aip_verify_failure_locator "$?" "$BASH_COMMAND"' ERR
  trap 'status=$?; if (( status != 0 && AIP_VERIFY_RECORDED != 1 )); then aip_verify_locator_write "${AIP_VERIFY_STAGE:-not_started}" "$BASH_COMMAND" "$status"; fi' EXIT
fi
