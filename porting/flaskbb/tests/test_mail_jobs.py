"""F02/F14 mail job tests: message content without sockets.

The declared worker functions build their SMTP messages purely, so the
activation/reset content (recipient, sender, token link) is asserted here
with no listener. Actual SMTP delivery and serve-side job triggering need
a live deployment (see STATUS.md) and are not executed here.
"""
import asyncio
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "mail"))

import flaskbb_mail


class MailJobTests(unittest.TestCase):
    def setUp(self):
        self.old = {k: os.environ.get(k) for k in ("FLASKBB_SMTP_HOST", "FLASKBB_SMTP_PORT", "FLASKBB_SMTP_SENDER", "FLASKBB_ACTIVATION_BASE_URL")}
        os.environ.update({
            "FLASKBB_SMTP_HOST": "127.0.0.1",
            "FLASKBB_SMTP_PORT": "25025",
            "FLASKBB_SMTP_SENDER": "noreply@flaskbb.test",
            "FLASKBB_ACTIVATION_BASE_URL": "http://127.0.0.1:58080",
        })

    def tearDown(self):
        for k, v in self.old.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v

    def test_activation_email_carries_token_link(self):
        msg = flaskbb_mail.build_activation_email("alice@example.test", "member-alice", "tok-activation-123")
        self.assertEqual(msg["To"], "alice@example.test")
        self.assertEqual(msg["From"], "noreply@flaskbb.test")
        self.assertEqual(msg["Subject"], "Activate your FlaskBB account")
        body = msg.get_content()
        self.assertIn("member-alice", body)
        self.assertIn("http://127.0.0.1:58080/auth/activate/tok-activation-123", body)

    def test_password_reset_email_carries_token_link(self):
        msg = flaskbb_mail.build_password_reset_email("bob@example.test", "admin-bob", "tok-reset-456")
        self.assertEqual(msg["To"], "bob@example.test")
        self.assertEqual(msg["Subject"], "Reset your FlaskBB password")
        body = msg.get_content()
        self.assertIn("admin-bob", body)
        self.assertIn("http://127.0.0.1:58080/auth/reset/tok-reset-456", body)

    def test_missing_smtp_config_fails_closed(self):
        for k in ("FLASKBB_SMTP_HOST", "FLASKBB_SMTP_PORT", "FLASKBB_SMTP_SENDER"):
            os.environ.pop(k, None)
        with self.assertRaises(RuntimeError):
            flaskbb_mail.build_activation_email("x@example.test", "u", "t")
        with self.assertRaises(RuntimeError):
            asyncio.run(
                flaskbb_mail.send_activation_email(
                    {"email": "x@example.test", "username": "u", "token": "t"}, {}
                )
            )


if __name__ == "__main__":
    unittest.main()
