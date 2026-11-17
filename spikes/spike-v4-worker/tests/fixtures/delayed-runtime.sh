#!/bin/sh
# Slower than the former 2s probe limit but inside the 5s startup budget.
/bin/sleep 3
printf "v0.0.0-delayed\n"
