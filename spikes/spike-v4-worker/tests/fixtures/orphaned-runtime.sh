#!/bin/sh
# The runtime itself exits at once, but its background child keeps the output pipes open.
/bin/sleep 300 &
printf "%s" "$!" > "$AIP_PROBE_DESCENDANT_PID_FILE"
