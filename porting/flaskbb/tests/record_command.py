"""Run one CI command and preserve its raw output plus a JSON receipt."""
import argparse
import json
import os
import subprocess
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--name", required=True)
    parser.add_argument("--expect-failure-marker")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("a command is required after --")

    evidence_dir = Path(os.environ["FLASKBB_EVIDENCE_DIR"])
    evidence_dir.mkdir(parents=True, exist_ok=True)
    try:
        completed = subprocess.run(command, text=True, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, errors="replace")
        exit_code = completed.returncode
        stdout = completed.stdout
        stderr = completed.stderr
    except OSError as error:
        exit_code = 127
        stdout = ""
        stderr = "{}: {}".format(type(error).__name__, error)
    stdout_path = evidence_dir / (args.name + ".stdout.txt")
    stderr_path = evidence_dir / (args.name + ".stderr.txt")
    stdout_path.write_text(stdout, encoding="utf-8")
    stderr_path.write_text(stderr, encoding="utf-8")

    matched = (exit_code != 0 and args.expect_failure_marker and
               args.expect_failure_marker in stdout + stderr)
    receipt = {
        "name": args.name,
        "source_sha": os.environ.get("GITHUB_SHA"),
        "command": command,
        "cwd": os.getcwd(),
        "exit_code": exit_code,
        "stdout_file": stdout_path.name,
        "stderr_file": stderr_path.name,
        "stdout": stdout,
        "stderr": stderr,
        "expected_failure_marker": args.expect_failure_marker,
        "expected_failure_detected": bool(matched),
        "verdict": ("expected_failure_detected" if matched else
                    "passed" if completed.returncode == 0 else "failed"),
    }
    (evidence_dir / (args.name + ".json")).write_text(
        json.dumps(receipt, ensure_ascii=False, indent=2) + "\n")
    sys.stdout.write(stdout)
    sys.stderr.write(stderr)
    return 0 if matched else exit_code


if __name__ == "__main__":
    raise SystemExit(main())
