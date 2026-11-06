#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: check-postgres-toolchain.sh <server_version_num> <psql --version> <pg_dump --version> <pg_restore --version>" >&2
  exit 2
fi

server_version_num="$1"
if [[ ! "$server_version_num" =~ ^[0-9]+$ ]] || (( server_version_num < 100000 )); then
  echo "invalid PostgreSQL server_version_num: $server_version_num" >&2
  exit 2
fi
server_major=$((server_version_num / 10000))

for pair in "psql:$2" "pg_dump:$3" "pg_restore:$4"; do
  name="${pair%%:*}"
  version="${pair#*:}"
  if [[ ! "$version" =~ PostgreSQL\)[[:space:]]([0-9]+)\. ]]; then
    echo "cannot parse $name version output: $version" >&2
    exit 2
  fi
  client_major="${BASH_REMATCH[1]}"
  if [[ "$client_major" != "$server_major" ]]; then
    echo "$name client major $client_major does not match PostgreSQL server major $server_major" >&2
    exit 1
  fi
done

echo "PostgreSQL server major $server_major matches psql, pg_dump and pg_restore major $server_major"
