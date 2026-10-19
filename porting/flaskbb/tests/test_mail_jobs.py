"""F02/F14 mail job unit tests: payload content and broker call without sockets.

The worker builds a provider request body purely and hands it to the declared
`mail` HTTP integration through `ctx.http.call`; a fake ctx stands in for the
broker here. The real serve path (worker network isolation, broker delivery to
Mailpit) is covered by mail-jobs.test.mjs.
"""
import asyncio
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "mail"))

import flaskbb_mail


class FakeHttp:
    def __init__(self, status=200):
        self.status = status
        self.calls = []

    async def call(self, name, payload, key):
        self.calls.append((name, payload, key))
        return {"status": self.status, "body": {}}


class FakeCtx:
    def __init__(self, status=200):
        self.http = FakeHttp(status)


class MailJobTests(unittest.TestCase):
    def setUp(self):
        keys = ("FLASKBB_MAIL_SENDER", "FLASKBB_ACTIVATION_BASE_URL")
        self.old = {k: os.environ.get(k) for k in keys}
        os.environ.update({
            "FLASKBB_MAIL_SENDER": "noreply@flaskbb.test",
            "FLASKBB_ACTIVATION_BASE_URL": "http://127.0.0.1:58080",
        })

    def tearDown(self):
        for k, v in self.old.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v

    def test_activation_email_carries_token_link(self):
        payload = flaskbb_mail.build_activation_email("alice@example.test", "member-alice", "tok-activation-123")
        self.assertEqual(payload["To"][0]["Email"], "alice@example.test")
        self.assertEqual(payload["From"]["Email"], "noreply@flaskbb.test")
        self.assertEqual(payload["Subject"], "Activate your FlaskBB account")
        self.assertIn("member-alice", payload["Text"])
        self.assertIn("http://127.0.0.1:58080/auth/activate/tok-activation-123", payload["Text"])

    def test_password_reset_email_carries_token_link(self):
        payload = flaskbb_mail.build_password_reset_email("bob@example.test", "admin-bob", "tok-reset-456")
        self.assertEqual(payload["To"][0]["Email"], "bob@example.test")
        self.assertEqual(payload["Subject"], "Reset your FlaskBB password")
        self.assertIn("http://127.0.0.1:58080/auth/reset/tok-reset-456", payload["Text"])

    def test_send_goes_through_the_named_integration_with_a_stable_key(self):
        ctx = FakeCtx()
        data = {"email": "alice@example.test", "username": "member-alice", "token": "tok-1"}
        self.assertEqual(asyncio.run(flaskbb_mail.send_activation_email(data, ctx)), {"accepted": True})
        asyncio.run(flaskbb_mail.send_activation_email(data, ctx))
        self.assertEqual([c[0] for c in ctx.http.calls], ["mail", "mail"])
        self.assertEqual(ctx.http.calls[0][2], ctx.http.calls[1][2])
        self.assertLessEqual(len(ctx.http.calls[0][2]), 100)

    def test_activation_and_reset_use_distinct_keys(self):
        a = flaskbb_mail.effect_key("activation", "x@example.test", "t")
        b = flaskbb_mail.effect_key("reset", "x@example.test", "t")
        self.assertNotEqual(a, b)

    def test_provider_rejection_fails_the_job(self):
        ctx = FakeCtx(status=503)
        with self.assertRaises(RuntimeError):
            asyncio.run(flaskbb_mail.send_password_reset_email(
                {"email": "x@example.test", "username": "u", "token": "t"}, ctx))

    def test_missing_sender_config_fails_closed(self):
        os.environ.pop("FLASKBB_MAIL_SENDER", None)
        with self.assertRaises(RuntimeError):
            flaskbb_mail.build_activation_email("x@example.test", "u", "t")
        ctx = FakeCtx()
        with self.assertRaises(RuntimeError):
            asyncio.run(flaskbb_mail.send_activation_email(
                {"email": "x@example.test", "username": "u", "token": "t"}, ctx))
        self.assertEqual(ctx.http.calls, [])


if __name__ == "__main__":
    unittest.main()
