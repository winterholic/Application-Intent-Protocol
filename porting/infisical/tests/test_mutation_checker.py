"""A broken fixture or skipped suite must never count as mutation evidence."""
import contextlib
import io
import pathlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import mutation_check as checker


class MutationCheckerTests(unittest.TestCase):
    def run_checker(self, stderr, returncode=1, equivalent=False):
        with tempfile.TemporaryDirectory() as directory:
            source = pathlib.Path(directory) / "app.aip"
            source.write_text("original rule")
            result = subprocess.CompletedProcess([], returncode, stdout="", stderr=stderr)
            responses = [result] if equivalent else [subprocess.CompletedProcess([], 0, stdout='{"ok":true}', stderr=""), result]
            output = io.StringIO()
            with patch.object(checker, "APP", source), \
                    patch.object(checker, "MUTATIONS", [] if equivalent else [("probe", "original rule", "weaker rule", "test_policy")]), \
                    patch.object(checker, "EQUIVALENT", [("probe", "original rule", "weaker rule")] if equivalent else []), \
                    patch.object(checker.subprocess, "run", side_effect=responses), contextlib.redirect_stdout(output):
                code = checker.main()
            return code, output.getvalue()

    def test_named_assertion_failure_counts_as_caught(self):
        code, output = self.run_checker("FAIL: test_policy (test_live_db.LiveDbTests.test_policy)\nAssertionError\nRan 1 test in 0.1s\nFAILED (failures=1)\n")
        self.assertEqual(code, 0, output)
        self.assertIn("caught", output)

    def test_fixture_setup_failure_does_not_count_as_caught(self):
        code, output = self.run_checker("ERROR: setUpClass (test_live_db.LiveDbTests)\nRuntimeError: database unavailable\nRan 0 tests in 0.1s\nFAILED (errors=1)\n")
        self.assertNotEqual(code, 0, output)
        self.assertIn("BROKEN", output)

    def test_fixture_teardown_failure_does_not_count_as_caught(self):
        code, output = self.run_checker("ERROR: tearDownClass (test_live_db.LiveDbTests)\nRuntimeError: cleanup failed\nRan 1 test in 0.1s\nFAILED (errors=1)\n")
        self.assertNotEqual(code, 0, output)
        self.assertIn("BROKEN", output)

    def test_all_skipped_is_not_equivalence_evidence(self):
        code, output = self.run_checker("Ran 0 tests in 0.1s\nOK (skipped=1)\n", returncode=0, equivalent=True)
        self.assertNotEqual(code, 0, output)

    def test_executed_green_suite_is_observational_equivalence_evidence(self):
        code, output = self.run_checker("Ran 66 tests in 0.1s\nOK\n", returncode=0, equivalent=True)
        self.assertEqual(code, 0, output)
        self.assertIn("EQUIV", output)
