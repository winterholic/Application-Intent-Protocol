#!/usr/bin/env python3
"""Mark the aggregate storage verification result and fail after all checks ran."""

import json
import sys
from pathlib import Path


# Labels emitted by the workflow's six direct invocations and the three
# independent checks inside test-minio-object-storage.sh.
EXPECTED_LABELS = {
    "fmt-ledger",
    "fmt-transport",
    "diff",
    "pg-ledger",
    "pg-principal-seed",
    "tls-minio-contract",
    "pg-minio-ledger",
    "pg-minio-list-scan",
    "tls-minio-list-scan",
}


def main() -> int:
    path = Path(sys.argv[1])
    result = json.loads(path.read_text())
    checks = result.get("checks")
    validation_errors = []
    failures = []

    if not isinstance(checks, list) or not checks:
        validation_errors.append("checks must be a non-empty list")
        checks = checks if isinstance(checks, list) else []

    labels = []
    source_shas = set()
    missing_sha = False
    for index, check in enumerate(checks):
        if not isinstance(check, dict):
            validation_errors.append(f"check at index {index} must be an object")
            continue

        label = check.get("label")
        if not isinstance(label, str) or not label:
            validation_errors.append(f"check at index {index} has no valid label")
        else:
            labels.append(label)

        exit_code = check.get("exit_code")
        if not isinstance(exit_code, int) or isinstance(exit_code, bool):
            validation_errors.append(f"{label or '<unknown>'} has no valid exit_code")
        elif exit_code != 0:
            failures.append(check)

        git_sha = check.get("git_sha")
        if isinstance(git_sha, str) and git_sha:
            source_shas.add(git_sha)
        else:
            missing_sha = True

    for label in sorted(set(labels)):
        if labels.count(label) > 1:
            validation_errors.append(f"duplicate check label: {label}")

    missing_labels = EXPECTED_LABELS - set(labels)
    unexpected_labels = set(labels) - EXPECTED_LABELS
    if missing_labels:
        validation_errors.append(f"missing expected labels: {', '.join(sorted(missing_labels))}")
    if unexpected_labels:
        validation_errors.append(f"unexpected labels: {', '.join(sorted(unexpected_labels))}")
    if missing_sha:
        validation_errors.append("every check must include a non-empty git_sha")
    if len(source_shas) > 1:
        validation_errors.append("checks contain mixed source SHAs")

    result["failure_count"] = len(failures) + len(validation_errors)
    result["failed_labels"] = [check.get("label", "<unknown>") for check in failures]
    result["validation_errors"] = validation_errors
    result["final_exit_code"] = 1 if failures or validation_errors else 0
    path.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    print(
        json.dumps(
            {
                "failure_count": result["failure_count"],
                "failed_labels": result["failed_labels"],
                "validation_errors": validation_errors,
                "final_exit_code": result["final_exit_code"],
            }
        )
    )
    return result["final_exit_code"]


if __name__ == "__main__":
    raise SystemExit(main())
