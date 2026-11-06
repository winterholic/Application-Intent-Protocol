#!/usr/bin/env python3
"""Mark the aggregate storage verification result and fail after all checks ran."""

import json
import sys
from pathlib import Path


def main() -> int:
    path = Path(sys.argv[1])
    result = json.loads(path.read_text())
    failures = [check for check in result.get("checks", []) if check["exit_code"] != 0]
    result["failure_count"] = len(failures)
    result["failed_labels"] = [check["label"] for check in failures]
    result["final_exit_code"] = 1 if failures else 0
    path.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps({"failure_count": len(failures), "failed_labels": result["failed_labels"], "final_exit_code": result["final_exit_code"]}))
    return result["final_exit_code"]


if __name__ == "__main__":
    raise SystemExit(main())
