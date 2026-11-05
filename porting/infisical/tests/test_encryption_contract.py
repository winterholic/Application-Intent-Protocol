"""Static and boundary encryption contract tests for porting/infisical.

Proves:
1. The experimental encrypted fixture compiles and produces valid execution facts.
2. `service check --dev-checks` issues `DEF.ADVISORY_ENCRYPTION_RUNTIME_DISABLED`.
3. Prohibited encrypted modifiers (expect, literal assignment, sort, unindexed filter, repeat) fail closed.
4. Runtime entrypoints reject the experimental encrypted fixture.
5. The active structure/authorization port does not require the disabled encryption runtime.
"""

import json
import os
import pathlib
import subprocess
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
PORT = HERE.parent
ROOT = PORT.parent.parent
APP = HERE / "fixtures" / "encrypted-app.aip"
ACTIVE_APP = PORT / "app.aip"
BINARY = os.environ.get("AIP_BIN", str(ROOT / "target" / "debug" / "aip"))


class InfisicalEncryptionContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not pathlib.Path(BINARY).is_file():
            raise unittest.SkipTest(f"aip binary not found at {BINARY}")

    def run_check(self, source_path, dev_checks=False):
        cmd = [BINARY, "service", "check"]
        if dev_checks:
            cmd.append("--dev-checks")
        cmd.append(str(source_path))
        proc = subprocess.run(cmd, capture_output=True, text=True)
        try:
            parsed = json.loads(proc.stdout)
        except json.JSONDecodeError:
            parsed = None
        return proc.returncode, parsed, proc.stderr

    def test_app_declares_encrypted_secret_values_and_produces_advisory(self):
        code, parsed, stderr = self.run_check(APP, dev_checks=True)
        self.assertEqual(code, 0, f"check failed: {stderr}\n{parsed}")
        self.assertTrue(parsed.get("ok"), parsed)
        advisories = [a.get("code") for a in parsed.get("advisories", [])]
        self.assertIn(
            "DEF.ADVISORY_ENCRYPTION_RUNTIME_DISABLED",
            advisories,
            f"expected encryption disabled advisory in {advisories}",
        )

    def test_active_port_does_not_require_the_disabled_encryption_runtime(self):
        code, parsed, stderr = self.run_check(ACTIVE_APP, dev_checks=True)
        self.assertEqual(code, 0, f"check failed: {stderr}\n{parsed}")
        self.assertTrue(parsed.get("ok"), parsed)
        advisories = [a.get("code") for a in parsed.get("advisories", [])]
        self.assertNotIn(
            "DEF.ADVISORY_ENCRYPTION_RUNTIME_DISABLED",
            advisories,
            "the active structure/authorization port must remain usable while encryption is unavailable",
        )

    def test_secret_value_cannot_have_expect_clause(self):
        source = APP.read_text()
        # Adding an expect clause to an encrypted field must fail closed with DEF.ENCRYPTED_TRANSITION_VALUE_FORBIDDEN
        mutated = source.replace(
            "transition updateValue {",
            "transition updateValue {\n    expect value\n",
        )
        with tempfile.NamedTemporaryFile("w", suffix=".aip", delete=False) as f:
            f.write(mutated)
            f.flush()
            temp_path = f.name
        try:
            code, parsed, stderr = self.run_check(temp_path)
            self.assertNotEqual(code, 0, "expected check to fail with expect clause on encrypted field")
            combined = f"{parsed} {stderr}"
            self.assertIn("DEF.ENCRYPTED_TRANSITION_VALUE_FORBIDDEN", combined)
        finally:
            pathlib.Path(temp_path).unlink(missing_ok=True)

    def test_secret_value_cannot_have_literal_assignment(self):
        source = APP.read_text()
        # Assigning literal to an encrypted field must fail closed
        mutated = source.replace(
            "patch value",
            'to value = "fixed-secret"',
        )
        with tempfile.NamedTemporaryFile("w", suffix=".aip", delete=False) as f:
            f.write(mutated)
            f.flush()
            temp_path = f.name
        try:
            code, parsed, stderr = self.run_check(temp_path)
            self.assertNotEqual(code, 0, "expected check to fail with literal assignment on encrypted field")
            combined = f"{parsed} {stderr}"
            self.assertIn("DEF.ENCRYPTED_TRANSITION_VALUE_FORBIDDEN", combined)
        finally:
            pathlib.Path(temp_path).unlink(missing_ok=True)

    def test_secret_value_cannot_be_sorted(self):
        source = APP.read_text()
        # Sorting on encrypted field must fail closed with DEF.ENCRYPTED_SORT_FORBIDDEN
        mutated = source.replace(
            "sort key, id",
            "sort value, id",
        )
        with tempfile.NamedTemporaryFile("w", suffix=".aip", delete=False) as f:
            f.write(mutated)
            f.flush()
            temp_path = f.name
        try:
            code, parsed, stderr = self.run_check(temp_path)
            self.assertNotEqual(code, 0, "expected check to fail sorting on encrypted field")
            combined = f"{parsed} {stderr}"
            self.assertIn("DEF.ENCRYPTED_SORT_FORBIDDEN", combined)
        finally:
            pathlib.Path(temp_path).unlink(missing_ok=True)

    def test_secret_value_cannot_be_filtered_without_lookup(self):
        source = APP.read_text()
        # Filtering on encrypted field without lookup must fail closed with DEF.ENCRYPTED_FILTER_FORBIDDEN
        mutated = source.replace(
            "filter id.eq, folder.eq, key.eq, key.prefix, owner.eq",
            "filter id.eq, folder.eq, key.eq, key.prefix, owner.eq, value.eq",
        )
        with tempfile.NamedTemporaryFile("w", suffix=".aip", delete=False) as f:
            f.write(mutated)
            f.flush()
            temp_path = f.name
        try:
            code, parsed, stderr = self.run_check(temp_path)
            self.assertNotEqual(code, 0, "expected check to fail filtering on encrypted field without lookup")
            combined = f"{parsed} {stderr}"
            self.assertIn("DEF.ENCRYPTED_FILTER_FORBIDDEN", combined)
        finally:
            pathlib.Path(temp_path).unlink(missing_ok=True)

    def test_secret_value_cannot_be_patched_with_repeat_unchanged(self):
        source = APP.read_text()
        # Patching an encrypted field in a transition that specifies 'repeat unchanged' must fail closed with DEF.UNSUPPORTED
        mutated = source.replace(
            "transition updateValue {\n    allow canWriteProject(actor, folder.env.project)\n    from true\n    patch value\n  }",
            "transition updateValue {\n    allow canWriteProject(actor, folder.env.project)\n    from true\n    patch value\n    repeat unchanged\n  }",
        )
        with tempfile.NamedTemporaryFile("w", suffix=".aip", delete=False) as f:
            f.write(mutated)
            f.flush()
            temp_path = f.name
        try:
            code, parsed, stderr = self.run_check(temp_path)
            self.assertNotEqual(code, 0, "expected check to fail patching encrypted field with repeat unchanged")
            combined = f"{parsed} {stderr}"
            self.assertIn("DEF.UNSUPPORTED", combined)
        finally:
            pathlib.Path(temp_path).unlink(missing_ok=True)

    def test_runtime_init_refuses_encrypted_facts_with_bad_facts_error(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            jwks_path = pathlib.Path(tmpdir) / "jwks.json"
            jwks_path.write_text(json.dumps({"keys": []}))
            config_path = pathlib.Path(tmpdir) / "service.json"
            config_path.write_text(json.dumps({
                "schema": "aip_enc_test",
                "database_url_env": "AIP_TEST_DATABASE_URL",
                "listen": "127.0.0.1:0",
                "wire": "decimal",
                "auth": {
                    "issuer": "https://idp.example.test",
                    "audience": "infisical-test",
                    "jwks": {"kind": "file", "path": str(jwks_path)}
                }
            }))
            proc = subprocess.run(
                [BINARY, "service", "init", str(APP), "--config", str(config_path)],
                capture_output=True,
                text=True,
                env=dict(os.environ, AIP_TEST_DATABASE_URL="postgres://localhost:5433/test"),
            )
            self.assertNotEqual(proc.returncode, 0)
            combined = f"{proc.stdout} {proc.stderr}"
            self.assertIn("BAD_FACTS", combined)

    def test_runtime_serve_refuses_encrypted_facts_with_unavailable_error(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            jwks_path = pathlib.Path(tmpdir) / "jwks.json"
            jwks_path.write_text(json.dumps({"keys": []}))
            config_path = pathlib.Path(tmpdir) / "service.json"
            config_path.write_text(json.dumps({
                "schema": "aip_enc_test",
                "database_url_env": "AIP_TEST_DATABASE_URL",
                "listen": "127.0.0.1:0",
                "wire": "decimal",
                "auth": {
                    "issuer": "https://idp.example.test",
                    "audience": "infisical-test",
                    "jwks": {"kind": "file", "path": str(jwks_path)}
                }
            }))
            proc = subprocess.run(
                [BINARY, "service", "serve", str(APP), "--config", str(config_path)],
                capture_output=True,
                text=True,
                env=dict(os.environ, AIP_TEST_DATABASE_URL="postgres://localhost:5433/test"),
            )
            self.assertNotEqual(proc.returncode, 0)
            combined = f"{proc.stdout} {proc.stderr}"
            self.assertIn("DEF.ENCRYPTION_RUNTIME_UNAVAILABLE", combined)


if __name__ == "__main__":
    unittest.main()
