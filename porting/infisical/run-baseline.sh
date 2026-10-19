#!/usr/bin/env bash
# WBS 7.3.3.9 live baseline for the Infisical port: check, gen, JWKS, init, migrate plan, seed, principals, serve,
# SDK read/write/job, jobs and audit inspection. Each step's raw output goes to $EVIDENCE.
# Steps and prerequisites are explained in DB-RUNBOOK.md; this file is the single executable source.
set -euo pipefail

: "${AIP_TEST_DATABASE_URL:?set AIP_TEST_DATABASE_URL to an isolated loopback PostgreSQL}"
if ! [[ "$AIP_TEST_DATABASE_URL" =~ ^postgres(ql)?://[^@]*@(127\.0\.0\.1|localhost):([0-9]+)/ ]]; then
  echo "refusing: database must be loopback with an explicit port" >&2; exit 2
fi
if [[ "${BASH_REMATCH[3]}" == "5432" ]]; then
  echo "refusing port 5432: use a dedicated isolated container" >&2; exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT_DIR="$ROOT/porting/infisical"
AIP="${AIP_BIN:-$ROOT/target/debug/aip}"
PY="${INFISICAL_PYTHON:-python3}"
WORK="${INFISICAL_WORK:-$(mktemp -d "${TMPDIR:-/tmp}/infisical-baseline.XXXXXX")}"
EVIDENCE="${INFISICAL_EVIDENCE:-$PORT_DIR/evidence}"
SCHEMA="${INFISICAL_SCHEMA:-aip_infisical_baseline}"
CONFIG="$WORK/service.json"
mkdir -p "$WORK" "$EVIDENCE"
[[ -x "$AIP" ]] || { echo "build first: cargo build -p aip-cli" >&2; exit 2; }

step() { printf '\n== %s\n' "$1"; }
log() { tee "$EVIDENCE/baseline-$1.txt"; }
SERVER_PID=""
cleanup() { [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true; }
trap cleanup EXIT

step "1 check and gen (7.3.3.3/7.3.3.4)"
"$AIP" service check "$PORT_DIR/app.aip" | log 1-check
"$AIP" service gen "$PORT_DIR/app.aip" --wire decimal --out "$WORK/binding.ts" | log 1-gen

step "2 JWKS (RS256 throwaway key; access tokens live 900 s at most)"
"$PY" "$PORT_DIR/tools/jwks.py" keygen "$WORK" | log 2-jwks

step "3 service config with workers block"
cat > "$CONFIG" <<JSON
{"schema": "$SCHEMA", "database_url_env": "AIP_TEST_DATABASE_URL", "listen": "127.0.0.1:0", "wire": "decimal",
 "auth": {"issuer": "https://idp.example.test", "audience": "infisical-spike",
          "jwks": {"kind": "file", "path": "jwks.json"}},
 "workers": {"directory": "$PORT_DIR/workers", "language": "python", "runtime_executable": "$PY",
             "enable_writes": false}}
JSON
echo "config written to $CONFIG" | log 3-config

step "4 init (fresh owned schema $SCHEMA)"
"$AIP" service init "$PORT_DIR/app.aip" --config "$CONFIG" | log 4-init

step "5 migrate plan (read-only; an unchanged definition must plan no change)"
"$AIP" service migrate "$PORT_DIR/app.aip" --config "$CONFIG" | log 5-migrate-plan

step "6 seed"
sed "s/:schema/$SCHEMA/g" "$PORT_DIR/seed.sql" | psql "$AIP_TEST_DATABASE_URL" -v ON_ERROR_STOP=1 -X -q -1 -f - | log 6-seed
psql "$AIP_TEST_DATABASE_URL" -At -c "SELECT 'users', count(*) FROM $SCHEMA.\"user\" UNION ALL SELECT 'environments', count(*) FROM $SCHEMA.environment UNION ALL SELECT 'folders', count(*) FROM $SCHEMA.folder" | log 6-seed-counts

step "7 principals (token subject -> actor id)"
for pair in alice:1 bob:2 carol:3 mallory:4 eve:5 dan:6; do
  "$AIP" service principal --config "$CONFIG" bind --subject "${pair%%:*}" --actor "${pair##*:}"
done | log 7-principals

step "8 serve"
"$AIP" service serve "$PORT_DIR/app.aip" --config "$CONFIG" > "$WORK/serve.out" 2> "$WORK/serve.err" &
SERVER_PID=$!
for _ in $(seq 1 60); do grep -q '"event":"listening"' "$WORK/serve.out" 2>/dev/null && break; sleep 0.5; done
grep '"event":"listening"' "$WORK/serve.out" | log 8-serve
BASE="http://$("$PY" -c 'import json,sys; print(json.loads(open(sys.argv[1]).readline())["address"])' "$WORK/serve.out")"
FINGERPRINT="$("$PY" -c 'import json,sys; print(json.loads(open(sys.argv[1]).readline())["fingerprint"])' "$WORK/serve.out")"
curl -fsS "$BASE/ready" -o /dev/null -w "ready http %{http_code}\n" | log 8-ready

step "9 SDK read, write, denial, job, soft delete"
"$PY" "$PORT_DIR/tools/baseline_client.py" "$BASE" "$FINGERPRINT" "$WORK" | log 9-sdk

step "10 jobs and audit (operator CLI)"
"$AIP" service jobs --config "$CONFIG" list | log 10-jobs
"$AIP" service audit --config "$CONFIG" list --limit 20 | log 10-audit

step "done"
echo "evidence: $EVIDENCE/baseline-*.txt"
if [[ -z "${INFISICAL_KEEP_SCHEMA:-}" ]]; then
  kill "$SERVER_PID" 2>/dev/null || true; SERVER_PID=""
  infra="aip_infra_$(printf %s "$SCHEMA" | shasum -a 256 | cut -c1-52)"
  psql "$AIP_TEST_DATABASE_URL" -X -q -c "DROP SCHEMA IF EXISTS $SCHEMA CASCADE" -c "DROP SCHEMA IF EXISTS $infra CASCADE"
  echo "dropped $SCHEMA and $infra"
fi
