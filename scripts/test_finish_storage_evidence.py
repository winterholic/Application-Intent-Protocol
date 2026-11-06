"""Regression tests for the storage evidence finalizer's workflow contract."""

import contextlib
import io
import json
import runpy
import sys
import unittest
from pathlib import Path
from unittest.mock import patch


EXPECTED_LABELS = [
    "fmt-ledger",
    "fmt-transport",
    "diff",
    "pg-ledger",
    "pg-principal-seed",
    "tls-minio-contract",
    "pg-minio-ledger",
    "pg-minio-list-scan",
    "tls-minio-list-scan",
]
RESULTS_SHA = "0904c3b66aa12c77e0f6559ea10302bef80b7d3e"


def complete_result_rows():
    return {
        "checks": [
            {"label": label, "exit_code": 0, "git_sha": RESULTS_SHA}
            for label in EXPECTED_LABELS
        ]
    }


def run_finalizer(result):
    result_file = {"results.json": json.dumps(result)}
    finalizer = runpy.run_path(str(Path(__file__).with_name("finish-storage-evidence.py")))
    stdout = io.StringIO()
    with (
        patch.object(Path, "read_text", lambda self, *args, **kwargs: result_file[self.name]),
        patch.object(
            Path,
            "write_text",
            lambda self, value, *args, **kwargs: result_file.__setitem__(self.name, value),
        ),
        patch.object(sys, "argv", ["finish-storage-evidence.py", "results.json"]),
        contextlib.redirect_stdout(stdout),
    ):
        exit_code = finalizer["main"]()
    return exit_code, json.loads(result_file["results.json"]), stdout.getvalue()


class FinishStorageEvidenceTests(unittest.TestCase):
    def test_complete_nine_rows_from_one_source_pass(self):
        exit_code, result, output = run_finalizer(complete_result_rows())
        self.assertEqual(exit_code, 0)
        self.assertEqual(result["failure_count"], 0)
        self.assertEqual(result["failed_labels"], [])
        self.assertEqual(json.loads(output)["final_exit_code"], 0)

    def test_empty_checks_fail_with_reason(self):
        exit_code, result, output = run_finalizer({"checks": []})
        self.assertNotEqual(exit_code, 0)
        self.assertTrue(any("checks" in error for error in result["validation_errors"]))
        self.assertIn("checks", output)

    def test_missing_expected_label_fails_with_label(self):
        result = complete_result_rows()
        result["checks"] = result["checks"][:-1]
        exit_code, finalized, output = run_finalizer(result)
        self.assertNotEqual(exit_code, 0)
        self.assertTrue(any("tls-minio-list-scan" in error for error in finalized["validation_errors"]))
        self.assertIn("tls-minio-list-scan", output)

    def test_duplicate_label_fails_with_label(self):
        result = complete_result_rows()
        result["checks"].append(dict(result["checks"][0]))
        exit_code, finalized, output = run_finalizer(result)
        self.assertNotEqual(exit_code, 0)
        self.assertTrue(any("fmt-ledger" in error and "duplicate" in error for error in finalized["validation_errors"]))
        self.assertIn("fmt-ledger", output)

    def test_unexpected_label_fails_with_label(self):
        result = complete_result_rows()
        result["checks"].append({"label": "extra-probe", "exit_code": 0, "git_sha": RESULTS_SHA})
        exit_code, finalized, output = run_finalizer(result)
        self.assertNotEqual(exit_code, 0)
        self.assertTrue(any("extra-probe" in error for error in finalized["validation_errors"]))
        self.assertIn("extra-probe", output)

    def test_nonzero_command_fails_with_label(self):
        result = complete_result_rows()
        result["checks"][3]["exit_code"] = 17
        exit_code, finalized, output = run_finalizer(result)
        self.assertNotEqual(exit_code, 0)
        self.assertIn("pg-ledger", finalized["failed_labels"])
        self.assertIn("pg-ledger", output)

    def test_mixed_source_shas_fail(self):
        result = complete_result_rows()
        result["checks"][0]["git_sha"] = "a1143cd7e7222207e906c383e3538f447e0572de"
        exit_code, finalized, output = run_finalizer(result)
        self.assertNotEqual(exit_code, 0)
        self.assertTrue(any("SHA" in error for error in finalized["validation_errors"]))
        self.assertIn("SHA", output)


if __name__ == "__main__":
    unittest.main()
