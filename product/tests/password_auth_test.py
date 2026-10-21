import asyncio
import json
import pathlib
import unittest

from product.providers.password_auth import (
    check_password_hash,
    generate_password_hash,
    needs_rehash,
    verify_password,
)

FIXTURE = pathlib.Path(__file__).parent / "fixtures" / "password-auth" / "werkzeug-3.0.2.json"


class PasswordAuthTest(unittest.TestCase):
    def test_hashes_written_by_werkzeug_3_0_2_verify(self):
        cases = json.loads(FIXTURE.read_text(encoding="utf-8"))["cases"]
        self.assertGreaterEqual(len(cases), 8)
        for case in cases:
            with self.subTest(hash=case["hash"].split("$")[0]):
                self.assertTrue(check_password_hash(case["hash"], case["password"]))
                self.assertFalse(check_password_hash(case["hash"], case["password"] + "x"))
                self.assertFalse(check_password_hash(case["hash"], ""))

    def test_generated_hash_round_trips_with_the_werkzeug_layout(self):
        stored = generate_password_hash("correct horse")
        method, salt, digest = stored.split("$")
        self.assertEqual(method, "scrypt:32768:8:1")
        self.assertEqual(len(salt), 16)
        self.assertEqual(len(digest), 128)
        self.assertTrue(check_password_hash(stored, "correct horse"))
        self.assertNotEqual(stored, generate_password_hash("correct horse"), "each hash carries its own salt")

    def test_malformed_unsupported_and_oversized_hashes_never_verify(self):
        for stored in [
            "",
            "plain$salt$secret",
            "pbkdf2:hash-a",  # the seed rows of the MySQL fixture
            "pbkdf2:sha256:600000$salt",
            "pbkdf2:md5:10$salt$00",
            "pbkdf2:sha256:abc$salt$00",
            "pbkdf2:sha256:999999999$salt$00",
            "scrypt:3:8:1$salt$00",
            "scrypt:1073741824:8:1$salt$00",
            "scrypt:32768:8$salt$00",
            "scrypt:32768:8:99$salt$00",
            "scrypt:32768:8:1$salt$not-hex",
        ]:
            with self.subTest(stored=stored):
                self.assertFalse(check_password_hash(stored, "secret"))

    def test_non_text_inputs_are_rejected_not_raised(self):
        stored = generate_password_hash("pw-pw-pw-pw", "pbkdf2:sha256:1000")
        self.assertFalse(check_password_hash(None, "pw-pw-pw-pw"))
        self.assertFalse(check_password_hash(stored, None))
        self.assertFalse(check_password_hash(stored.encode(), "pw-pw-pw-pw"))

    def test_rehash_is_requested_only_for_weaker_or_foreign_parameters(self):
        self.assertFalse(needs_rehash(generate_password_hash("pw-pw-pw-pw")))
        self.assertTrue(needs_rehash(generate_password_hash("pw-pw-pw-pw", "pbkdf2:sha256:1000")))
        self.assertTrue(needs_rehash(generate_password_hash("pw-pw-pw-pw", "scrypt:1024:8:1")))
        self.assertTrue(needs_rehash("garbage"))

    def test_hashes_written_here_verify_in_the_original_library_when_installed(self):
        try:
            from werkzeug.security import check_password_hash as original
        except ImportError:
            self.skipTest("Werkzeug is not installed; run with a venv holding Werkzeug==3.0.2 to cross-check")
        for method in ("scrypt:32768:8:1", "scrypt:1024:8:1", "pbkdf2:sha256:1000", "pbkdf2:sha512:1000"):
            stored = generate_password_hash("한글 pw 🙂", method)
            self.assertTrue(original(stored, "한글 pw 🙂"), method)
            self.assertFalse(original(stored, "한글 pw"), method)

    def test_worker_entry_reports_only_a_verdict(self):
        stored = generate_password_hash("pw-pw-pw-pw", "pbkdf2:sha256:1000")
        ok = asyncio.run(verify_password({"password": "pw-pw-pw-pw", "hash": stored}, None))
        self.assertEqual(ok, {"ok": True, "rehash": True})
        bad = asyncio.run(verify_password({"password": "nope-nope", "hash": stored}, None))
        self.assertEqual(bad, {"ok": False, "rehash": False})
        self.assertNotIn(stored, json.dumps([ok, bad]))


if __name__ == "__main__":
    unittest.main()
