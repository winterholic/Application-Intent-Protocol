from pathlib import Path
import unittest


SCRIPT = Path(__file__).with_name("writes.py").read_text()


class WritesRunnerConfigTests(unittest.TestCase):
    def test_runner_requires_explicit_paths_and_database_urls(self):
        for name in (
            "AIP_FR_BIN",
            "AIP_FR_RUN_DIR",
            "AIP_FR_DEFINITION",
            "AIP_FR_ADOPT_DATABASE_URL",
            "AIP_FR_RUNTIME_DATABASE_URL",
            "AIP_FR_TEST_DATABASE_URL",
        ):
            with self.subTest(name=name):
                self.assertIn(name, SCRIPT)

    def test_runner_has_no_embedded_database_endpoint_or_password(self):
        self.assertNotIn("/private/tmp/", SCRIPT)
        self.assertNotIn("aip_fixture_pw_721", SCRIPT)
        self.assertNotIn("runtime_test_only", SCRIPT)
        self.assertNotIn("34391", SCRIPT)

    def test_runner_rejects_shared_endpoints_and_roles(self):
        self.assertIn("parsed.hostname not in", SCRIPT)
        self.assertIn("port <= 1024 or port == 3306", SCRIPT)
        self.assertIn(".lower() == \"root\"", SCRIPT)
        self.assertIn("must use separate roles", SCRIPT)
        self.assertIn("must target the same explicit loopback fixture endpoint", SCRIPT)


if __name__ == "__main__":
    unittest.main()
