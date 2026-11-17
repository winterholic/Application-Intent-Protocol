#!/bin/sh
set -eu
setsid sh -c 'printf "%s" "$$" > "$AIP_PROBE_DESCENDANT_PID_FILE"; exec sleep 300' &
printf 'leader exits while detached descendant keeps probe pipes open\n'
exit 0
