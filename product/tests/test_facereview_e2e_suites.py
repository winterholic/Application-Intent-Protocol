"""The whole-app runner collects independent failures without weakening gates."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "scripts/facereview-e2e-suites.py"


class SuiteRunnerTest(unittest.TestCase):
    def run_runner(self, fail="", mode="run-ci", missing_test="", env_missing=(), fake_discovery="", zero_tests="", omit_markers=""):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cargo = root / "cargo"
            cargo.write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
suite = args[args.index("--test") + 1]
with open(os.environ["RECEIPT"], "a") as receipt:
    receipt.write(json.dumps(args) + "\\n")
names = json.loads(os.environ["SUITE_NAMES"])
if "--list" in args:
    if suite != os.environ.get("MISSING_TEST"):
        print(("prefix_" if suite == os.environ.get("FAKE_DISCOVERY") else "") + names[suite] + ": test")
elif suite == os.environ.get("FAIL_SUITE"):
    print("test result: FAILED. 0 passed; 1 failed;")
    sys.exit(7)
else:
    print("test result: ok. 0 passed; 0 failed;" if suite == os.environ.get("ZERO_TESTS") else "test result: ok. 1 passed; 0 failed;")
    if suite != os.environ.get("OMIT_MARKERS"):
        print("member SDK journey: login, one refresh, UI adapter video read, bookmark on/off, comment create/edit/delete, logout")
        print("policy: member refused 6 administrator aggregates, 3 administrator extensions and the approval job")
        print("React admin console journey: SUPER_ADMIN login")
''')
            cargo.chmod(0o755)
            spec = importlib.util.spec_from_file_location("suite_runner", RUNNER)
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            env = {**os.environ, "PATH": str(root) + os.pathsep + os.environ["PATH"],
                   "RECEIPT": str(root / "receipt"), "FAIL_SUITE": fail,
                   "MISSING_TEST": missing_test, "AIP_TEST_MONGO_URL": "fixture",
                   "FAKE_DISCOVERY": fake_discovery, "ZERO_TESTS": zero_tests, "OMIT_MARKERS": omit_markers,
                   "AIP_TEST_REDIS_URL": "fixture", "AIP_FACEREVIEW_ADMIN_UI_DIR": "fixture",
                   "SUITE_NAMES": json.dumps({s.binary: s.test for s in module.SUITES if s.name != "scope"})}
            # Auth and scope share a Rust binary, so discovery lists both names.
            cargo.write_text(cargo.read_text().replace(' + names[suite] + ": test")',
                ' + names[suite] + ": test"); print("snapshot_scope_rejects_an_unintended_summary_insert: test")'))
            for key in env_missing:
                env.pop(key, None)
            result = subprocess.run([sys.executable, str(RUNNER), mode, "--output", str(root / "results")],
                                    env=env, text=True, capture_output=True)
            summary = json.loads((root / "results/suites.json").read_text())
            receipt = [json.loads(line) for line in (root / "receipt").read_text().splitlines()]
            return result, summary, receipt

    def test_first_failure_does_not_block_independent_admin_suite(self):
        result, summary, receipt = self.run_runner(fail="mysql_facereview_auth")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(summary["suites"]["auth"]["status"], "failed")
        self.assertEqual(summary["suites"]["adminui"]["status"], "passed")
        self.assertTrue(any("mysql_facereview_admin_ui" in args and "--exact" in args for args in receipt))

    def test_missing_test_is_failure_and_other_suites_still_run(self):
        result, summary, receipt = self.run_runner(missing_test="mysql_facereview_recommend")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(summary["suites"]["recommend"]["stage"], "discovery")
        self.assertFalse(any("mysql_facereview_recommend" in args and "--exact" in args for args in receipt))
        self.assertEqual(summary["suites"]["adminui"]["status"], "passed")

    def test_missing_store_only_blocks_suites_that_need_it(self):
        result, summary, _ = self.run_runner(env_missing=("AIP_TEST_MONGO_URL",))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(summary["suites"]["recommend"]["status"], "blocked")
        self.assertEqual(summary["suites"]["admin"]["status"], "passed")

    def test_member_diagnostic_does_not_expand_into_full_gate(self):
        result, summary, _ = self.run_runner(mode="run-ci-member")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(set(summary["suites"]), {"write", "auth"})

    def test_full_gate_requires_all_suites(self):
        result, summary, _ = self.run_runner()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(summary["suites"]), 11)
        self.assertTrue(all(s["status"] == "passed" for s in summary["suites"].values()))

    def test_similar_test_name_cannot_satisfy_exact_discovery(self):
        result, summary, _ = self.run_runner(fake_discovery="mysql_facereview_watch")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(summary["suites"]["watch"]["stage"], "discovery")

    def test_zero_executed_tests_cannot_pass(self):
        result, summary, _ = self.run_runner(zero_tests="mysql_facereview_watch")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(summary["suites"]["watch"]["status"], "failed")

    def test_missing_policy_marker_cannot_pass(self):
        result, summary, _ = self.run_runner(omit_markers="mysql_facereview_admin_ui")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(summary["suites"]["adminui"]["required_markers_present"])


if __name__ == "__main__":
    unittest.main()
