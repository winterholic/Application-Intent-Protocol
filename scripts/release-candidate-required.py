#!/usr/bin/env python3
"""Decide whether a release-candidate run proved every required workflow on one SHA.

Only `success` passes. A skipped, cancelled, failed, missing or unfinished workflow or job fails the gate, and so does any job
that ran for a different commit. The decision lives here, not in YAML, so product/tests/test_release_candidate_required.py can
run it against recorded results.

  release-candidate-required.py --sha <sha> --needs needs.json --workflow .github/workflows/release-candidate.yml \
      [--jobs jobs.jsonl] [--allow-skipped 'prefix' ...]

needs.json is `toJSON(needs)` of the aggregating job. jobs.jsonl is one JSON object per line from
`GET /repos/{repo}/actions/runs/{run}/attempts/{n}/jobs` (`--jq '.jobs[]'`). Without --jobs only the `needs` results are checked.
"""
import argparse
import json
import re
import sys
from pathlib import Path

AGGREGATOR_JOB = "required"


def workflow_job_ids(path):
    """Job ids under the top-level `jobs:` key, read without a YAML dependency (GitHub-hosted python3 has none guaranteed)."""
    ids, in_jobs = [], False
    for line in Path(path).read_text().splitlines():
        if re.match(r"^jobs:\s*$", line):
            in_jobs = True
            continue
        if in_jobs and re.match(r"^\S", line) and not line.startswith("#"):
            break
        match = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line) if in_jobs else None
        if match:
            ids.append(match.group(1))
    return ids


def evaluate(sha, needs, expected, jobs=None, allow_skipped=()):
    """Return (problems, rows). An empty problem list means the candidate is proven for `sha`."""
    problems, rows = [], []
    if not sha:
        problems.append("no candidate SHA was given")
    if not needs:
        problems.append("the aggregating job has no `needs` results")
    for name in sorted(set(expected) - set(needs)):
        problems.append(f"workflow job `{name}` is declared but not aggregated in `needs`")
    for name in sorted(set(needs) - set(expected)):
        problems.append(f"`needs` lists `{name}`, which the workflow does not declare")
    for name, value in sorted(needs.items()):
        result = value.get("result") if isinstance(value, dict) else None
        rows.append(f"{name}\t{result}")
        if result != "success":
            problems.append(f"workflow `{name}` finished as `{result}`, not `success`")
    if jobs is not None:
        seen = set()
        for job in jobs:
            name = job.get("name", "")
            if name.split(" / ")[0] == AGGREGATOR_JOB or name.startswith("required"):
                continue
            head, conclusion = job.get("head_sha"), job.get("conclusion")
            rows.append(f"  job {name}\t{conclusion}\t{(head or '')[:12]}")
            seen.add(name.split(" / ")[0])
            if head != sha:
                problems.append(f"job `{name}` ran for {head}, not {sha}")
            if conclusion == "skipped" and any(name.startswith(prefix) for prefix in allow_skipped):
                continue
            if conclusion != "success":
                problems.append(f"job `{name}` concluded `{conclusion}`, not `success`")
        for name in sorted(set(expected) - seen):
            problems.append(f"no job was observed for workflow `{name}`")
    return problems, rows


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--needs", required=True, help="file holding toJSON(needs), or - for stdin")
    parser.add_argument("--workflow", required=True, help="the release-candidate workflow file that declares the jobs")
    parser.add_argument("--jobs", help="jsonl of the run's jobs from the Actions API")
    parser.add_argument("--allow-skipped", action="append", default=[],
                        help="job-name prefix that may legitimately be skipped (a job-level `if:` that cannot hold in this run)")
    args = parser.parse_args(argv)
    raw = sys.stdin.read() if args.needs == "-" else Path(args.needs).read_text()
    needs = json.loads(raw)
    jobs = None
    if args.jobs:
        jobs = [json.loads(line) for line in Path(args.jobs).read_text().splitlines() if line.strip()]
    expected = [name for name in workflow_job_ids(args.workflow) if name != AGGREGATOR_JOB]
    problems, rows = evaluate(args.sha, needs, expected, jobs, args.allow_skipped)
    print(f"release candidate SHA: {args.sha}")
    print("\n".join(rows))
    if problems:
        for problem in problems:
            print(f"NOT PROVEN: {problem}", file=sys.stderr)
        return 1
    print(f"ALL REQUIRED WORKFLOWS PASSED for {args.sha}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
