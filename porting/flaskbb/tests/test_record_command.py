import json
import io
import os
import sys
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest.mock import patch

import record_command


class RecordCommandTests(unittest.TestCase):
    def run_command(self, name, command, expected_marker=None, startup_error=None):
        writes = {}
        stdout = io.StringIO()
        stderr = io.StringIO()
        argv = ["record_command.py", "--name", name]
        if expected_marker:
            argv += ["--expect-failure-marker", expected_marker]
        argv += ["--", *command]

        def capture_write(path, data, **kwargs):
            writes[path.name] = data
            return len(data)

        run = record_command.subprocess.run
        if startup_error is not None:
            def run(*args, **kwargs):
                raise startup_error

        with patch.dict(os.environ, {"RUNNER_TEMP": "/runner-temp", "GITHUB_SHA": "test-sha"}), \
                patch.object(sys, "argv", argv), \
                patch.object(record_command.Path, "mkdir"), \
                patch.object(record_command.Path, "write_text", autospec=True, side_effect=capture_write), \
                patch.object(record_command.subprocess, "run", side_effect=run), \
                redirect_stdout(stdout), redirect_stderr(stderr):
            result = record_command.main()

        return result, json.loads(writes[name + ".json"]), writes, stdout.getvalue(), stderr.getvalue()

    def test_success_records_pass(self):
        result, receipt, writes, stdout, stderr = self.run_command(
            "success", [sys.executable, "-B", "-c", "print('child ok')"])
        self.assertEqual(result, 0)
        self.assertEqual(receipt["verdict"], "passed")
        self.assertEqual(receipt["exit_code"], 0)
        self.assertIn("child ok", writes["success.stdout.txt"])
        self.assertIn("child ok", stdout)
        self.assertEqual(stderr, "")

    def test_child_failure_records_failure_and_returns_child_code(self):
        result, receipt, writes, stdout, stderr = self.run_command(
            "child-failure", [sys.executable, "-B", "-c", "import sys; print('child failed'); sys.exit(23)"])
        self.assertEqual(result, 23)
        self.assertEqual(receipt["verdict"], "failed")
        self.assertEqual(receipt["exit_code"], 23)
        self.assertIn("child failed", writes["child-failure.stdout.txt"])
        self.assertIn("child failed", stdout)

    def test_expected_assertion_marker_is_successful_expected_failure(self):
        result, receipt, writes, stdout, stderr = self.run_command(
            "assertion-marker",
            [sys.executable, "-B", "-c", "raise AssertionError('EXPECTED ASSERTION')"],
            expected_marker="EXPECTED ASSERTION")
        self.assertEqual(result, 0)
        self.assertEqual(receipt["verdict"], "expected_failure_detected")
        self.assertTrue(receipt["expected_failure_detected"])
        self.assertIn("EXPECTED ASSERTION", writes["assertion-marker.stderr.txt"])

    def test_import_failure_is_recorded_as_failure(self):
        result, receipt, writes, stdout, stderr = self.run_command(
            "import-failure", [sys.executable, "-B", "-c", "import definitely_missing_flaskbb_import"])
        self.assertNotEqual(result, 0)
        self.assertEqual(receipt["verdict"], "failed")
        self.assertIn("ModuleNotFoundError", writes["import-failure.stderr.txt"])

    def test_missing_test_module_is_recorded_as_failure(self):
        result, receipt, writes, stdout, stderr = self.run_command(
            "missing-test", [sys.executable, "-B", "-m", "unittest", "definitely_missing_flaskbb_test_module"])
        self.assertNotEqual(result, 0)
        self.assertEqual(receipt["verdict"], "failed")
        self.assertIn("ModuleNotFoundError", writes["missing-test.stderr.txt"])

    def test_startup_oserror_writes_failed_receipt_and_returns_127(self):
        result, receipt, writes, stdout, stderr = self.run_command(
            "startup", [sys.executable, "-B", "-c", "pass"],
            expected_marker="executable unavailable",
            startup_error=OSError("executable unavailable"))
        self.assertEqual(result, 127)
        self.assertEqual(receipt["exit_code"], 127)
        self.assertEqual(receipt["verdict"], "failed")
        self.assertFalse(receipt["expected_failure_detected"])
        self.assertIn("OSError: executable unavailable", writes["startup.stderr.txt"])
        self.assertEqual(writes["startup.stdout.txt"], "")
        self.assertIn("OSError: executable unavailable", stderr)


if __name__ == "__main__":
    unittest.main()
