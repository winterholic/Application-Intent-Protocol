import os
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "prototype" / "tools"))
import dedicated_db  # noqa: E402

GOOD = "postgres://tester@127.0.0.1:32801/aip_test_proto"


def clean_env(**extra):
    env = {k: v for k, v in os.environ.items() if not k.startswith("PG") and k != "AIP_TEST_DATABASE_URL"}
    env.update(extra)
    return mock.patch.dict(os.environ, env, clear=True)


class DedicatedDsnTest(unittest.TestCase):
    def test_accepts_a_dedicated_uri_and_keyword_dsn(self):
        self.assertEqual(dedicated_db.validate(GOOD), GOOD)
        keyword = "host=127.0.0.1 port=32801 user=tester dbname=aip_test_proto"
        self.assertEqual(dedicated_db.validate(keyword), keyword)

    def test_refuses_every_default_or_shared_target(self):
        for dsn in [
            "",
            "host=localhost dbname=postgres",  # the old built-in default
            "postgres://tester@127.0.0.1/aip_test_proto",  # port left to libpq (5432)
            "postgres://tester@127.0.0.1:5432/aip_test_proto",
            "postgres://tester@127.0.0.1:1024/aip_test_proto",
            "postgres://tester@127.0.0.1:32801/postgres",  # not an aip_test_ database
            "postgres://127.0.0.1:32801/aip_test_proto",  # user left to the environment
            "postgres://tester@:32801/aip_test_proto",  # host left to libpq
            "postgres://tester@127.0.0.1:32801/aip_test_proto?host=other&port=5432",
            "host=a,b port=32801 user=t dbname=aip_test_x",
            "host=127.0.0.1 port=32801 user=t dbname=aip_test_x service=prod",
            "host=127.0.0.1 port=32801 user=t dbname=aip_test_x hostaddr=10.0.0.5",
            "host=127.0.0.1 host=other port=32801 user=t dbname=aip_test_x",
        ]:
            with self.subTest(dsn=dsn), self.assertRaises(SystemExit) as caught:
                dedicated_db.validate(dsn)
            self.assertIn("refusing PostgreSQL target", str(caught.exception))

    def test_resolve_has_no_built_in_default(self):
        with clean_env():
            with self.assertRaises(SystemExit) as caught:
                dedicated_db.resolve(None)
            self.assertIn("no DSN given", str(caught.exception))
        with clean_env(AIP_TEST_DATABASE_URL=GOOD):
            self.assertEqual(dedicated_db.resolve(None), GOOD)
            self.assertEqual(dedicated_db.resolve("host=127.0.0.1 port=32802 user=t dbname=aip_test_y"), "host=127.0.0.1 port=32802 user=t dbname=aip_test_y")

    def test_an_explicit_argument_is_validated_too_and_pg_variables_are_refused(self):
        with clean_env(AIP_TEST_DATABASE_URL=GOOD):
            with self.assertRaises(SystemExit):
                dedicated_db.resolve("host=localhost dbname=postgres")
        with clean_env(AIP_TEST_DATABASE_URL=GOOD, PGHOST="elsewhere"):
            with self.assertRaises(SystemExit) as caught:
                dedicated_db.resolve(None)
            self.assertIn("PGHOST", str(caught.exception))


class ToolsUseTheGuardTest(unittest.TestCase):
    def test_no_prototype_tool_keeps_a_postgres_default_dsn(self):
        tools = Path(__file__).resolve().parents[2] / "prototype" / "tools"
        for path in sorted(tools.glob("*.py")):
            text = path.read_text()
            self.assertNotIn("dbname=postgres", text, path.name)
            if 'add_argument("--db-url"' in text:
                self.assertIn("dedicated_db.resolve(args.db_url)", text, f"{path.name} must validate its DSN")


if __name__ == "__main__":
    unittest.main()
