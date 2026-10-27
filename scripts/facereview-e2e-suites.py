#!/usr/bin/env python3
"""Run independent suites on the already validated, run-owned CI fixtures.

Only status metadata goes into browser artifacts. Cargo output stays in the CI
console. Each Rust suite still owns its database and child-process cleanup.
"""
import argparse
from collections import namedtuple
import json
import os
from pathlib import Path
import subprocess

Suite = namedtuple("Suite", "name binary test requires markers", defaults=((), ()))
SUITES = (
    Suite("write", "mysql_worker_write_e2e", "http_apply_runs_real_node_and_python_workers_in_one_mysql_transaction"),
    Suite("auth", "mysql_facereview_auth", "facereview_accounts_log_in_refresh_and_log_out_without_the_originals_replay_defect", ("AIP_TEST_MONGO_URL",), markers=(
        "member SDK journey: login, one refresh, UI adapter video read, bookmark on/off, comment create/edit/delete, logout",)),
    Suite("scope", "mysql_facereview_auth", "snapshot_scope_rejects_an_unintended_summary_insert"),
    Suite("recommend", "mysql_facereview_recommend", "facereview_recommendations_match_the_original_services_through_the_real_server", ("AIP_TEST_MONGO_URL",)),
    Suite("watch", "mysql_facereview_watch", "facereview_watch_detail_matches_the_original_service_through_the_real_server", ("AIP_TEST_MONGO_URL", "AIP_TEST_REDIS_URL")),
    Suite("effects", "mysql_external_effects", "write_extension_effects_commit_or_compensate_with_the_relational_transaction", ("AIP_TEST_MONGO_URL", "AIP_TEST_REDIS_URL")),
    Suite("pool", "mysql_facereview_pool_job", "the_scheduled_pool_job_fills_the_cache_the_recommendation_lists_read", ("AIP_TEST_MONGO_URL", "AIP_TEST_REDIS_URL")),
    Suite("trend", "mysql_facereview_trending_job", "the_trending_job_saves_new_videos_with_their_initial_documents", ("AIP_TEST_MONGO_URL",)),
    Suite("fill", "mysql_facereview_category_fill_job", "the_category_fill_job_searches_the_thinnest_genres_within_the_original_budget", ("AIP_TEST_MONGO_URL",)),
    Suite("admin", "mysql_facereview_admin", "facereview_administrator_lists_match_the_original_admin_service_through_the_real_server"),
    Suite("adminui", "mysql_facereview_admin_ui", "facereview_admin_console_runs_through_the_generated_sdk_and_the_servers_own_policy", ("AIP_TEST_MONGO_URL",), (
        "policy: member refused 6 administrator aggregates, 3 administrator extensions and the approval job",)),
)


def run_command(args, markers, *, exact=False):
    """Stream output, retaining only whether the mandatory markers appeared."""
    seen = set()
    try:
        with subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors="replace") as process:
            for line in process.stdout:
                print(line, end="", flush=True)
                for marker in markers:
                    matches = line.rstrip("\r\n") == marker if exact else marker in line
                    if matches:
                        seen.add(marker)
            status = process.wait()
    except OSError:
        # Keep environment errors (which can contain paths) out of shared artifacts.
        return 127, False
    return status, len(seen) == len(markers)


def run_suite(suite):
    missing = [key for key in suite.requires if not os.environ.get(key)]
    if missing:
        return {"status": "blocked", "stage": "prerequisite", "missing": missing}
    args = ["cargo", "test", "--locked", "-p", "aip-cli", "--features", "mysql", "--test", suite.binary]
    status, found = run_command(args + ["--", "--list", "--ignored"], [suite.test + ": test"], exact=True)
    if status or not found:
        return {"status": "failed", "stage": "discovery", "exit_status": status, "test_found": found}
    markers = ["test result: ok. 1 passed; 0 failed;", *suite.markers]
    if suite.name == "adminui" and os.environ.get("AIP_FACEREVIEW_ADMIN_UI_DIR"):
        markers.append("React admin console journey: SUPER_ADMIN login")
    status, found = run_command(args + ["--", "--ignored", "--exact", suite.test, "--nocapture", "--test-threads=1"], markers)
    return {"status": "passed" if status == 0 and found else "failed", "stage": "execution",
            "exit_status": status, "required_markers_present": found}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("run-ci", "run-ci-member"))
    parser.add_argument("--output", type=Path, required=True)
    options = parser.parse_args()
    options.output.mkdir(parents=True, exist_ok=True)
    suites = SUITES if options.mode == "run-ci" else SUITES[:2]
    summary = {"mode": options.mode, "suites": {}}
    for suite in suites:
        print(f"=== FaceReview E2E suite: {suite.name} ===", flush=True)
        summary["suites"][suite.name] = run_suite(suite)
        # Preserve partial results on runner cancellation/timeout without implying completion.
        (options.output / "suites.json").write_text(json.dumps(summary, indent=2) + "\n")
    failures = [(name, result) for name, result in summary["suites"].items() if result["status"] != "passed"]
    if failures:
        locator = os.environ.get("AIP_E2E_FAILURE_FILE") or os.environ.get("AIP_VERIFY_FAILURE_FILE")
        if locator:
            name, result = failures[0]
            Path(locator).write_text(f"stage=FaceReview suite {name}: {result['stage']}\ncommand=facereview-e2e-suites.py\nexit_status=1\n")
    print(f"FaceReview suites: {len(suites) - len(failures)} passed; {len(failures)} failed/blocked", flush=True)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
