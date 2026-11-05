"""Independent Rust failures are collected while unsafe prerequisites stay fatal."""
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
    def run_gate(self, fail=(), locator_enabled=True, runtime_stop=True):
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
    if "aip-test-support" in args:
        stage = "guard"
    elif "--manifest-path" in args:
        stage = "runtime-" + args[args.index("--manifest-path") + 1].split("/")[1]
    elif "aip-crypto" in args:
        stage = "runtime-crypto"
    elif "poc-runtime" in args:
        stage = "runtime-poc"
    else:
        stage = "runtime"
elif args[0] == "fmt":
    stage = "format"
elif "--manifest-path" in args:
    stage = args[args.index("--manifest-path") + 1].split("/")[1]
else:
    stage = "mysql" if "mysql,aip-cli/mysql" in args else "workspace"
if stage in json.loads(os.environ["FAIL_STAGES"]):
    print("fixture failure: " + stage, file=sys.stderr)
    sys.exit(19 if stage in ("spike-v6-transport", "runtime-spike-v6-transport") else 7)
if stage == "runtime" and os.environ["RUNTIME_STOP"] == "1":
    sys.exit(73)  # Prove the first runtime status survives later successful suites.
''')
            executable.chmod(0o755)
            for name in ("node", "npm", "psql", "python3", "mypy"):
                program = root / "bin" / name
                program.write_text("#!/bin/sh\nexit 0\n")
                program.chmod(0o755)
            (root / "bin/npm").write_text('#!/bin/sh\ntouch "$NONCARGO_RECEIPT"\nexit 67\n')
            (root / "spikes/spike-0-ts/node_modules/.bin/tsc").touch(mode=0o755)
            env = {**os.environ, "PATH": str(root / "bin") + os.pathsep + os.environ["PATH"],
                   "FAIL_STAGES": json.dumps(fail), "RECEIPT": str(root / "receipt"),
                   "RUNTIME_STOP": "1" if runtime_stop else "0", "NONCARGO_RECEIPT": str(root / "noncargo"),
                   "AIP_VERIFY_FAILURE_FILE": str(root / "failure"), "AIP_TEST_DATABASE_URL": "fixture"}
            if not locator_enabled:
                env.pop("AIP_VERIFY_FAILURE_FILE", None)
            result = subprocess.run(["bash", str(root / "tools/verify.sh")], env=env,
                                    capture_output=True, text=True)
            calls = [json.loads(line) for line in (root / "receipt").read_text().splitlines()]
            if (root / "noncargo").exists():
                calls.append(["npm-boundary"])
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
        self.assert_all_runtime_suites_ran(calls)
        runtime_index = next(index for index, args in enumerate(calls) if args[:2] == ["test", "--workspace"])
        self.assertTrue(all(index < runtime_index for index, args in enumerate(calls) if args[0] == "clippy"))

    def assert_all_runtime_suites_ran(self, calls):
        runtime = [args for args in calls if args[0] == "test" and "aip-test-support" not in args]
        self.assertEqual(len(runtime), 9, runtime)
        self.assertTrue(all("--no-fail-fast" in args for args in runtime), runtime)
        self.assertTrue(all("--locked" in args for args in runtime), runtime)
        self.assertEqual([args[args.index("--manifest-path") + 1].split("/")[1]
                          for args in runtime if "--manifest-path" in args], ENGINES)

    def test_runtime_failures_collect_all_engines_and_preserve_first_locator(self):
        result, calls, locator = self.run_gate(("runtime-spike-v2-read", "runtime-spike-v6-transport"), runtime_stop=False)
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assert_all_runtime_suites_ran(calls)
        self.assertNotIn(["npm-boundary"], calls)
        self.assertIn("stage=rust engine tests: spike-v2-read", locator)
        self.assertIn("command=cargo test --manifest-path spikes/spike-v2-read/Cargo.toml", locator)
        self.assertIn("exit_status=7", locator)
        self.assertIn("spike-v6-transport", result.stderr)

    def test_late_runtime_failure_is_fatal_without_locator(self):
        result, calls, _ = self.run_gate(("runtime-spike-v6-transport",), locator_enabled=False, runtime_stop=False)
        self.assertEqual(result.returncode, 19, result.stderr)
        self.assert_all_runtime_suites_ran(calls)
        self.assertNotIn(["npm-boundary"], calls)

    def test_crypto_and_poc_runtime_failures_do_not_block_other_engines(self):
        result, calls, locator = self.run_gate(("runtime-crypto", "runtime-poc"), runtime_stop=False)
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assert_all_runtime_suites_ran(calls)
        self.assertIn("stage=crypto all-feature tests", locator)
        self.assertNotIn(["npm-boundary"], calls)

    def test_all_runtime_success_continues_to_the_next_gate(self):
        result, calls, _ = self.run_gate(runtime_stop=False)
        self.assertEqual(result.returncode, 67, result.stderr)
        self.assert_all_runtime_suites_ran(calls)
        self.assertEqual(calls[-1], ["npm-boundary"])

    def test_database_guard_failure_still_stops_before_lints(self):
        result, calls, locator = self.run_gate(("guard",))
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertEqual(len(calls), 1)
        self.assertIn("stage=explicit database guard tests", locator)


if __name__ == "__main__":
    unittest.main()
