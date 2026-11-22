#!/usr/bin/env bash
# One entry point that reproduces the required verification of the latest HEAD, locally and in CI, and records which SHA it proved.
#
#   scripts/verify-head.sh [--stage fast|heavy|all] [--dry-run] [--only <step>] [--out <fresh dir>] [--allow-dirty]
#                          [--skip-provenance-unit]
#                          [--pg-port <port>] [--with-release-gate] [--list]
#
#   fast   no PostgreSQL: script syntax, workflow structure and the release-candidate gate tests, tooling unit tests, version
#          consistency, cargo fmt, and the sema quick suites. Minutes; this is what Verify runs in CI as `head-fast`.
#   heavy  needs PostgreSQL 17: tools/verify.sh (fmt, clippy, every engine suite, SDK and package tests), the expressiveness
#          consumer apps, the stable concurrency probes, worker isolation, semantic conformance and the SDK semantic e2e.
#          Reuses AIP_TEST_DATABASE_URL/AIP_TEST_ADMIN_URL when set, otherwise starts a throwaway PostgreSQL 17 on --pg-port.
#   all    fast, then heavy (default).
#
# Only a clean tree counts as proof of HEAD: heavy refuses a dirty tree (`--allow-dirty` records `dirty=true` and the verdict says
# so). The run writes summary.txt and result.json under --out with the SHA, the dirty flag, each step's status and log. The verdict is
# PROVEN only when every requested step passed; a step that did not run is NOT RUN, never a pass.
# Nothing here tags, releases or publishes. `--with-release-gate` adds scripts/release-gate.sh, which builds and smoke-tests an
# artifact in a temporary directory and uploads nothing; it needs docker native-store fixtures and PostgreSQL client tools 17.
# `--dry-run` prints the plan for the SHA and executes nothing.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

stage="all"; dry_run=0; only=""; out=""; allow_dirty=0; pg_port="${AIP_VERIFY_HEAD_PG_PORT:-55432}"; with_release_gate=0; skip_provenance_unit=0; list=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --stage) stage="${2:?--stage needs fast, heavy or all}"; shift 2 ;;
    --dry-run) dry_run=1; shift ;;
    --only) only="${2:?--only needs a step name}"; shift 2 ;;
    --out) out="${2:?--out needs a directory}"; shift 2 ;;
    --allow-dirty) allow_dirty=1; shift ;;
    --pg-port) pg_port="${2:?--pg-port needs a port}"; shift 2 ;;
    --with-release-gate) with_release_gate=1; shift ;;
    --skip-provenance-unit) skip_provenance_unit=1; shift ;;
    --list) list=1; shift ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done
case "$stage" in fast|heavy|all) ;; *) echo "--stage must be fast, heavy or all" >&2; exit 2 ;; esac
if (( skip_provenance_unit )) && { [[ "${GITHUB_ACTIONS:-}" != "true" ]] || [[ "${PROVENANCE_UNIT_RUNS_IN_INDEPENDENT_CONSUMERS:-}" != "true" ]]; }; then
  echo "--skip-provenance-unit is reserved for the Release Candidate Verify call whose required independent-consumer job runs that suite" >&2
  exit 2
fi

is_linux=0; [[ "$(uname -s)" == "Linux" ]] && is_linux=1

# ---- the plan: "<stage>|<step>|<shell command>" -------------------------------------------------------------------------
fast_dsn_default="host=127.0.0.1 port=${pg_port} dbname=aip_test_ci user=runner"
declare -a PLAN=()
add() { PLAN+=("$1|$2|$3"); }

add fast scripts-syntax "for f in tools/verify.sh scripts/*.sh; do bash -n \"\$f\"; done && python3 -m py_compile scripts/release-candidate-required.py"
add fast workflow-structure "python3 -W error::ResourceWarning -m unittest product.tests.test_release_candidate_required product.tests.test_rust_lint_gate"
add fast tooling-tests "python3 -W error::ResourceWarning -m unittest discover -s tools -p 'test_*.py' && python3 -W error::ResourceWarning -m unittest discover -s scripts -p 'test_*.py'"
add fast versions "node scripts/check-versions.mjs"
add fast rust-format "cargo fmt --all --check"
if (( ! skip_provenance_unit )); then
  add fast provenance-unit "python3 verification/1.0-consumers/test_provenance.py"
fi
# The test build needs an explicit non-default test DSN (crates/aip-test-support/build.rs) but these suites never connect.
add fast sema-quick "cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --locked --test policy_hardening --test advisories"

add heavy core-verify "bash tools/verify.sh"
add heavy expr-apps "node product/tests/fixtures/expr-apps/run.mjs"
add heavy stable-probes "bash scripts/run-concurrency-probes.sh stable"
if (( is_linux )); then
  add heavy worker-isolation "AIP_TEST_BWRAP=1 cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --lib isolation::tests --locked && AIP_TEST_BWRAP=1 cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --lib cleanup_tests --locked"
else
  add heavy worker-isolation "cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --lib isolation::tests --locked && cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --test v4_1 --locked"
  add heavy macos-only-v6 "cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --test extension_lifetime --test extension_unix --test write_extension_lifetime --test write_session_outcome && cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --test durable messages_share_atomic_job_effects_and_reject_foreign_acknowledgements && cargo test -p aip-cli --locked --test service_flow operation_only_service_preserves_principal_contract_revocation_and_deployment_fences"
fi
add heavy semantic-conformance "cargo test -p aip-sema --locked --test shared_semantic_fixture && cargo test -p aip-cli --locked --features poc-runtime --test core_semantic_fixture && cargo test -p aip-cli --locked --test service_flow"
add heavy sdk-semantic-e2e "cargo build --locked -p aip-cli && node --test --test-concurrency=1 product/tests/sdk-semantic-e2e.test.mjs"
if (( with_release_gate )); then
  add heavy release-gate "bash scripts/release-gate.sh --baseline 4c27040c6d4a2a2931321e1980842be79512bdb0 --ref \"\$AIP_VERIFY_HEAD_SHA\" --out \"\$AIP_VERIFY_HEAD_OUT/release-gate\""
fi

if (( list )); then
  printf '%s\n' "${PLAN[@]}" | awk -F'|' '{printf "%-6s %-20s %s\n", $1, $2, $3}'
  exit 0
fi

# ---- identity of what is being proven -----------------------------------------------------------------------------------
git rev-parse --git-dir >/dev/null 2>&1 || { echo "not a git checkout: cannot record the SHA" >&2; exit 2; }
sha="$(git rev-parse --verify 'HEAD^{commit}')"
dirty=0; [[ -z "$(git status --porcelain)" ]] || dirty=1
if [[ -n "${GITHUB_SHA:-}" && "$GITHUB_SHA" != "$sha" ]]; then
  echo "checkout HEAD $sha is not the SHA this workflow runs for ($GITHUB_SHA)" >&2
  exit 2
fi
origin_main="$(git rev-parse --verify --quiet refs/remotes/origin/main || true)"
relation="unknown (no origin/main ref)"
if [[ -n "$origin_main" ]]; then
  if [[ "$origin_main" == "$sha" ]]; then relation="equals origin/main"
  elif git merge-base --is-ancestor "$origin_main" "$sha"; then relation="ahead of origin/main"
  elif git merge-base --is-ancestor "$sha" "$origin_main"; then relation="BEHIND origin/main (not the latest HEAD)"
  else relation="diverged from origin/main"; fi
fi
out="${out:-$(mktemp -d "${TMPDIR:-/tmp}/aip-verify-head-${sha:0:12}-XXXXXX")}"
[[ ! -e "$out/summary.txt" ]] || { echo "$out already holds a run; pass a fresh --out" >&2; exit 2; }
mkdir -p "$out/logs"
summary="$out/summary.txt"
note() { printf '%s\n' "$*" | tee -a "$summary"; }

export AIP_VERIFY_HEAD_SHA="$sha" AIP_VERIFY_HEAD_OUT="$out"
selected=()
for entry in "${PLAN[@]}"; do
  IFS='|' read -r entry_stage entry_name _ <<<"$entry"
  if [[ -n "$only" ]]; then
    [[ "$entry_name" == "$only" ]] && selected+=("$entry")
  elif [[ "$stage" == "all" || "$stage" == "$entry_stage" ]]; then
    selected+=("$entry")
  fi
done
(( ${#selected[@]} )) || { echo "no step matches (--stage $stage${only:+ --only $only}); see --list" >&2; exit 2; }
has_heavy=0
for entry in "${selected[@]}"; do [[ "${entry%%|*}" == heavy ]] && has_heavy=1; done
if (( dirty && ! allow_dirty && ! dry_run && has_heavy )); then
  echo "working tree is dirty: heavy verification of HEAD $sha would not prove HEAD. Commit, or pass --allow-dirty to record dirty=true." >&2
  exit 2
fi

note "verify-head: sha=$sha dirty=$dirty relation=\"$relation\" stage=$stage${only:+ only=$only} dry_run=$dry_run"
note "started $(date -u +%Y-%m-%dT%H:%M:%SZ) on $(uname -srm); out=$out"

declare -a RESULTS=()
if (( dry_run )); then
  for entry in "${selected[@]}"; do
    IFS='|' read -r entry_stage entry_name entry_cmd <<<"$entry"
    note "PLAN  [$entry_stage] $entry_name: $entry_cmd"
    RESULTS+=("$entry_name|NOT RUN|0")
  done
  note "VERDICT: NOT RUN (dry run: $sha, ${#selected[@]} steps planned, nothing executed, nothing published)"
else
  pg_started=0; pg_dir=""
  stop_pg() {
    if (( pg_started )); then "$pg_bin/pg_ctl" -D "$pg_dir" -m fast stop >/dev/null 2>&1 || true; fi
  }
  trap stop_pg EXIT
  needs_db=0
  for entry in "${selected[@]}"; do [[ "${entry%%|*}" == heavy ]] && needs_db=1; done
  # Test builds refuse PG* variables (crates/aip-test-support/build.rs) and need explicit non-default test DSNs.
  unset PGHOST PGPORT PGDATABASE PGUSER PGPASSWORD PGSERVICE PGSERVICEFILE
  if (( needs_db )); then
    if [[ -z "${AIP_TEST_DATABASE_URL:-}" || -z "${AIP_TEST_ADMIN_URL:-}" ]]; then
      pg_bin="${AIP_PG_BIN_DIR:-}"
      if [[ -z "$pg_bin" ]]; then
        for candidate in /usr/lib/postgresql/17/bin /opt/homebrew/opt/postgresql@17/bin /usr/local/opt/postgresql@17/bin; do
          [[ -x "$candidate/pg_ctl" ]] && pg_bin="$candidate" && break
        done
      fi
      [[ -n "$pg_bin" && -x "$pg_bin/pg_ctl" ]] || { note "VERDICT: NOT RUN (no PostgreSQL 17: set AIP_TEST_DATABASE_URL/AIP_TEST_ADMIN_URL or AIP_PG_BIN_DIR)"; exit 2; }
      export PATH="$pg_bin:$PATH" LC_ALL="${LC_ALL:-en_US.UTF-8}"
      pg_dir="$out/pgdata"
      "$pg_bin/initdb" -D "$pg_dir" -U runner --auth=trust -E UTF8 --locale=C >"$out/logs/pg-initdb.log" 2>&1
      "$pg_bin/pg_ctl" -D "$pg_dir" -o "-p $pg_port -c listen_addresses=127.0.0.1 -c unix_socket_directories=$out -c deadlock_timeout=1s -c max_connections=300" \
        -l "$out/logs/pg.log" -w start >/dev/null
      pg_started=1
      "$pg_bin/psql" -X -h 127.0.0.1 -p "$pg_port" -U runner -d postgres -Atc 'create database aip_test_ci' >/dev/null
      export AIP_TEST_DATABASE_URL="host=127.0.0.1 port=$pg_port dbname=aip_test_ci user=runner"
      export AIP_TEST_ADMIN_URL="host=127.0.0.1 port=$pg_port dbname=postgres user=runner"
    fi
    version_num="$(psql -XAt --dbname "$AIP_TEST_ADMIN_URL" -c 'SHOW server_version_num')" || { note "VERDICT: NOT RUN (cannot reach the test PostgreSQL)"; exit 2; }
    (( version_num >= 170000 && version_num < 180000 )) || { note "VERDICT: NOT RUN (test PostgreSQL is $version_num, PostgreSQL 17 is required)"; exit 2; }
    note "postgresql: server_version_num=$version_num (dedicated test database, PG* variables unset)"
  else
    export AIP_TEST_DATABASE_URL="${AIP_TEST_DATABASE_URL:-$fast_dsn_default}"
    export AIP_TEST_ADMIN_URL="${AIP_TEST_ADMIN_URL:-host=127.0.0.1 port=${pg_port} dbname=postgres user=runner}"
  fi

  index=0
  for entry in "${selected[@]}"; do
    IFS='|' read -r entry_stage entry_name entry_cmd <<<"$entry"
    index=$((index + 1))
    log="$out/logs/$(printf '%02d' "$index")-$entry_name.log"
    started=$SECONDS
    echo "== [$entry_stage] $entry_name: $entry_cmd" >"$log"
    if bash -c "set -euo pipefail; $entry_cmd" >>"$log" 2>&1; then
      verdict=PASS
    else
      verdict=FAIL
      tail -n 25 "$log" >&2
    fi
    RESULTS+=("$entry_name|$verdict|$((SECONDS - started))")
    note "$verdict  [$entry_stage] $entry_name ($((SECONDS - started))s)  log=logs/$(basename "$log")"
  done
  failed=0
  for result in "${RESULTS[@]}"; do [[ "$result" == *"|PASS|"* ]] || failed=1; done
  if (( failed )); then
    note "VERDICT: NOT PROVEN for $sha (a step failed)"
  elif (( dirty )); then
    note "VERDICT: PASSED ON A DIRTY TREE for $sha; this is not proof of HEAD"
  else
    note "VERDICT: PROVEN for $sha ($stage${only:+, only $only}; ${#selected[@]} steps passed)"
  fi
fi

python3 - "$out/result.json" "$sha" "$dirty" "$relation" "$stage" "$dry_run" "${RESULTS[@]}" <<'PY'
import json, sys
path, sha, dirty, relation, stage, dry_run, *results = sys.argv[1:]
steps = [dict(zip(("name", "status", "seconds"), r.split("|"))) for r in results]
proven = dry_run == "0" and dirty == "0" and all(s["status"] == "PASS" for s in steps)
json.dump({"sha": sha, "dirty": dirty == "1", "relation_to_origin_main": relation, "stage": stage, "dry_run": dry_run == "1",
           "steps": steps, "proven": proven}, open(path, "w"), indent=2)
PY
note "evidence: $out (summary.txt, result.json, logs/)"
for result in "${RESULTS[@]}"; do [[ "$result" == *"|FAIL|"* ]] && exit 1; done
exit 0
