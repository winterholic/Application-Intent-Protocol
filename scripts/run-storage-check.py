#!/usr/bin/env python3
"""Run one storage verification command and persist exact command/output/exit evidence."""

import json
import os
import shlex
import subprocess
import sys
from pathlib import Path


def main() -> int:
    args = sys.argv[1:]
    if "--" not in args:
        raise SystemExit("usage: run-storage-check.py --dir DIR --label LABEL -- COMMAND [ARGS...]")
    split = args.index("--")
    options, command = args[:split], args[split + 1 :]
    if not command:
        raise SystemExit("missing command")
    try:
        evidence_dir = Path(options[options.index("--dir") + 1])
        label = options[options.index("--label") + 1]
    except (ValueError, IndexError):
        raise SystemExit("--dir and --label are required") from None
    if not label.replace("-", "").replace("_", "").isalnum():
        raise SystemExit("label must be alphanumeric, hyphen or underscore")
    evidence_dir.mkdir(parents=True, exist_ok=True)
    stdout_path = evidence_dir / f"{label}.stdout.txt"
    stderr_path = evidence_dir / f"{label}.stderr.txt"
    completed = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    stdout_path.write_bytes(completed.stdout)
    stderr_path.write_bytes(completed.stderr)
    sys.stdout.buffer.write(completed.stdout)
    sys.stderr.buffer.write(completed.stderr)
    results_path = evidence_dir / "results.json"
    results = json.loads(results_path.read_text()) if results_path.exists() else {"checks": []}
    results["checks"].append(
        {
            "label": label,
            "command": shlex.join(command),
            "argv": command,
            "exit_code": completed.returncode,
            "stdout_file": stdout_path.name,
            "stderr_file": stderr_path.name,
            "git_sha": os.environ.get("GITHUB_SHA") or subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip(),
        }
    )
    temporary = results_path.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(results, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(results_path)
    return completed.returncode


if __name__ == "__main__":
    raise SystemExit(main())
