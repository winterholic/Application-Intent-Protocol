#!/bin/sh
# Leaves a background child that must not outlive a timed-out probe.
/bin/sleep 300 </dev/null >/dev/null 2>&1 &
printf "%s" "$!" > "$AIP_PROBE_DESCENDANT_PID_FILE"
wait
