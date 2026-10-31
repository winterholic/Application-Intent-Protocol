"""Independent lint failures are collected while unsafe prerequisites stay fatal."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
ENGINES = [f"spike-v{number}-{name}" for number, name in
           [(1, "fixture"), (2, "read"), (3, "write"), (4, "worker"), (5, "sdk"), (6, "transport")]]


class RustLintGateTest(unittest.TestCase):
    def run_gate(self, fail=(), locator_enabled=True):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("tools", "scripts", "bin", "spikes/spike-0-ts/node_modules/.bin"):
                (root / name).mkdir(parents=True)
            for name in ("tools/verify.sh", "scripts/verify-safe-locator.sh"):
                shutil.copyfile(ROOT / name, root / name)
            executable = root / "bin/cargo"
            executable.write_text(f'''#!{sys.executable}
import json, os, sys
args = sys.argv[1:]
with open(os.environ["RECEIPT"], "a") as receipt:
    receipt.write(json.dumps(args) + "\\n")
if args[0] == "test":
    stage = "runtime" if "--workspace" in args else "guard"
elif args[0] == "fmt":
    stage = "format"
elif "--manifest-path" in args:
    stage = args[args.index("--manifest-path") + 1].split("/")[1]
else:
    stage = "mysql" if "mysql,aip-cli/mysql" in args else "workspace"
if stage in json.loads(os.environ["FAIL_STAGES"]):
    print("fixture failure: " + stage, file=sys.stderr)
    sys.exit(19 if stage == "spike-v6-transport" else 7)
if stage == "runtime":
    sys.exit(73)  # Stop after proving that lint success permits runtime tests.
''')
            executable.chmod(0o755)
            for name in ("node", "npm", "psql", "python3", "mypy"):
                program = root / "bin" / name
                program.write_text("#!/bin/sh\nexit 0\n")
                program.chmod(0o755)
            (root / "spikes/spike-0-ts/node_modules/.bin/tsc").touch(mode=0o755)
            env = {**os.environ, "PATH": str(root / "bin") + os.pathsep + os.environ["PATH"],
                   "FAIL_STAGES": json.dumps(fail), "RECEIPT": str(root / "receipt"),
                   "AIP_VERIFY_FAILURE_FILE": str(root / "failure"), "AIP_TEST_DATABASE_URL": "fixture"}
            if not locator_enabled:
                env.pop("AIP_VERIFY_FAILURE_FILE", None)
            result = subprocess.run(["bash", str(root / "tools/verify.sh")], env=env,
                                    capture_output=True, text=True)
            calls = [json.loads(line) for line in (root / "receipt").read_text().splitlines()]
            return result, calls, (root / "failure").read_text() if locator_enabled else ""

    def assert_all_lints_ran(self, calls):
        lints = [args for args in calls if args[0] == "clippy"]
        self.assertEqual(len(lints), 11, lints)
        self.assertEqual([args[args.index("--manifest-path") + 1].split("/")[1]
                          for args in lints if "--manifest-path" in args], ENGINES)
        self.assertTrue(all(args[-3:] == ["--", "-D", "warnings"] for args in lints))

    def test_multiple_failures_collect_all_lints_and_keep_first_locator(self):
        result, calls, locator = self.run_gate(("mysql", "spike-v6-transport"))
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assert_all_lints_ran(calls)
        self.assertFalse(any(args[:2] == ["test", "--workspace"] for args in calls))
        self.assertIn("stage=MySQL migrate service CLI all-target clippy", locator)
        self.assertIn("command=cargo clippy -p aip-migrate", locator)
        self.assertIn("exit_status=7", locator)
        self.assertIn("spike-v6-transport", result.stderr)

    def test_format_failure_also_collects_lints_without_running_runtime(self):
        result, calls, locator = self.run_gate(("format",))
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assert_all_lints_ran(calls)
        self.assertFalse(any(args[:2] == ["test", "--workspace"] for args in calls))
        self.assertIn("stage=Rust formatting", locator)

    def test_late_failure_preserves_its_status_after_following_success(self):
        result, calls, locator = self.run_gate(("spike-v6-transport",))
        self.assertEqual(result.returncode, 19, result.stderr)
        self.assert_all_lints_ran(calls)
        self.assertIn("stage=rust engine clippy: spike-v6-transport", locator)
        self.assertIn("exit_status=19", locator)

    def test_failure_without_ci_locator_is_still_fatal(self):
        result, calls, _ = self.run_gate(("mysql",), locator_enabled=False)
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assert_all_lints_ran(calls)
        self.assertFalse(any(args[:2] == ["test", "--workspace"] for args in calls))

    def test_success_runs_every_lint_before_runtime(self):
        result, calls, _ = self.run_gate()
        self.assertEqual(result.returncode, 73, result.stderr)
        self.assert_all_lints_ran(calls)
        self.assertEqual(calls[-1], ["test", "--workspace", "--locked"])

    def test_database_guard_failure_still_stops_before_lints(self):
        result, calls, locator = self.run_gate(("guard",))
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertEqual(len(calls), 1)
        self.assertIn("stage=explicit database guard tests", locator)


if __name__ == "__main__":
    unittest.main()
