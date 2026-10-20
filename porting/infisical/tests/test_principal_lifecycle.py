"""The actual product principal lifecycle used by the Infisical port, without worker execution.

Keys and tokens are generated for a throwaway deployment; no real IdP or user credentials are used.
"""

import json
import time
import unittest
import uuid

from joserfc import jwt
from joserfc.jwk import RSAKey

import live_harness as lh
from test_live_db import APP, SEED, PRINCIPALS, eq


class PrincipalLifecycleTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.dep = lh.Deployment(APP, SEED, PRINCIPALS)
        except lh.HarnessUnavailable as why:
            raise unittest.SkipTest(str(why))
        try:
            cls.dep.start()
        except BaseException:
            cls.dep.stop()
            raise

    @classmethod
    def tearDownClass(cls):
        cls.dep.stop()

    def subject(self, actor="2", not_before=0):
        subject = "lifecycle-" + uuid.uuid4().hex
        self.bind(subject, actor, not_before)
        return subject

    def bind(self, subject, actor="2", not_before=0):
        return self.dep.cli("principal", "--config", str(self.dep.work / "service.json"), "bind",
                            "--subject", subject, "--actor", actor, "--not-before", str(not_before))

    def revoke(self, subject):
        return self.dep.cli("principal", "--config", str(self.dep.work / "service.json"), "revoke", "--subject", subject)

    def client(self, subject, issued_at=None, lifetime=900, **extra):
        # Read only this fixture's generated signing key; never print or persist a token.
        spec = json.loads((self.dep.work / lh.jwks.KEY_FILE).read_text())
        issued_at = int(time.time()) - 10 if issued_at is None else issued_at
        claims = {"iss": lh.jwks.ISSUER, "aud": lh.jwks.AUDIENCE, "sub": subject,
                  "iat": issued_at, "nbf": issued_at, "exp": issued_at + lifetime, **extra}
        token = jwt.encode({"alg": "RS256", "typ": "at+jwt", "kid": spec["kid"]}, claims, RSAKey.import_key(spec))
        return lh.connect(self.dep.base, token, self.dep.fingerprint)

    def project(self, client):
        return client.read({"read": "Project", "select": ["id"], "filter": eq("id", "1")})

    def watermark(self, subject):
        return int(self.dep.sql(f"SELECT min_iat FROM :schema.aip_principals WHERE subject = '{subject}'"))

    def wait_for_watermark(self, watermark):
        # The product rejects future-issued JWTs, and revoke records server time + 1s.
        # Wait for the observed boundary, so the disabled flag is tested without masking it with a stale iat.
        deadline = time.monotonic() + 3
        while int(time.time()) < watermark:
            self.assertLess(time.monotonic(), deadline, "fixture clock did not reach the revocation watermark")
            time.sleep(0.02)

    def test_revoke_blocks_an_already_used_client_read_write_and_idempotent_replay(self):
        subject = self.subject()
        client = self.client(subject)
        self.assertEqual(client.session(), {"actorId": "2"})
        self.assertEqual(self.project(client), [{"id": "1"}])
        key = "revocation-" + uuid.uuid4().hex
        values = {"folder": "1", "key": key, "owner": "2"}
        first = lh.create(client, "Secret", values, key=key)
        self.assertTrue(first.get("ok"), first)
        revoked = self.revoke(subject)
        self.assertEqual(revoked, {"ok": True, "affected": 1})
        self.assertEqual(lh.error_code(self.project, client), "WIRE.UNAUTHENTICATED")
        replay = lh.create(client, "Secret", values, key=key)
        self.assertFalse(replay.get("ok"), replay)
        self.assertEqual(replay.get("code"), "WIRE.UNAUTHENTICATED")
        new_write = lh.create(client, "Secret", {**values, "key": key + "-new"})
        self.assertFalse(new_write.get("ok"), new_write)
        self.assertEqual(new_write.get("code"), "WIRE.UNAUTHENTICATED")
        self.assertEqual(self.dep.sql(f"SELECT count(*) FROM :schema.secret WHERE key = '{key}'").strip(), "1")
        self.assertEqual(self.dep.sql(f"SELECT count(*) FROM :schema.secret WHERE key = '{key}-new'").strip(), "0")

    def test_disabled_principal_refuses_tokens_issued_after_revocation(self):
        subject = self.subject()
        self.assertEqual(self.project(self.client(subject)), [{"id": "1"}])
        self.revoke(subject)
        watermark = self.watermark(subject)
        self.wait_for_watermark(watermark)
        newer = self.client(subject, issued_at=watermark)
        self.assertEqual(lh.error_code(self.project, newer), "WIRE.UNAUTHENTICATED")
        # The same token must work after enabling the same binding, with its iat already at the unchanged floor.
        # Otherwise a stale or future iat could have hidden a missing enabled check in the first assertion.
        self.bind(subject)
        self.assertEqual(self.watermark(subject), watermark)
        self.assertEqual(self.project(newer), [{"id": "1"}])

    def test_rebind_preserves_revocation_watermark_and_actor(self):
        subject = self.subject()
        old = self.client(subject)
        self.assertEqual(self.project(old), [{"id": "1"}])
        self.revoke(subject)
        watermark = self.watermark(subject)
        self.assertEqual(self.bind(subject), {"ok": True, "affected": 1})
        stored = self.dep.sql(f"SELECT enabled || '|' || min_iat || '|' || actor_id FROM :schema.aip_principals WHERE subject = '{subject}'").strip()
        self.assertEqual(stored, f"true|{watermark}|2")
        self.assertEqual(lh.error_code(self.project, old), "WIRE.UNAUTHENTICATED")
        self.wait_for_watermark(watermark)
        fresh = self.client(subject, issued_at=watermark)
        self.assertEqual(fresh.session(), {"actorId": "2"})
        self.assertEqual(self.project(fresh), [{"id": "1"}])

    def test_not_before_boundary_accepts_equal_and_rejects_older_tokens(self):
        watermark = int(time.time()) - 30
        subject = self.subject(not_before=watermark)
        self.assertEqual(lh.error_code(self.project, self.client(subject, issued_at=watermark - 1)), "WIRE.UNAUTHENTICATED")
        self.assertEqual(self.project(self.client(subject, issued_at=watermark)), [{"id": "1"}])
        self.assertEqual(self.project(self.client(subject, issued_at=watermark + 1)), [{"id": "1"}])
        self.bind(subject, not_before=0)
        self.assertEqual(lh.error_code(self.project, self.client(subject, issued_at=watermark - 1)), "WIRE.UNAUTHENTICATED")

    def test_existing_subject_cannot_be_reassigned_to_another_user(self):
        subject = self.subject()
        with self.assertRaisesRegex(RuntimeError, "PRINCIPAL_CONFLICT"):
            self.bind(subject, actor="1")
        self.assertEqual(self.client(subject).session(), {"actorId": "2"})
        self.revoke(subject)
        with self.assertRaisesRegex(RuntimeError, "PRINCIPAL_CONFLICT"):
            self.bind(subject, actor="1")
        stored = self.dep.sql(f"SELECT actor_id || '|' || enabled FROM :schema.aip_principals WHERE subject = '{subject}'").strip()
        self.assertEqual(stored, "2|false")

    def test_token_claims_cannot_choose_actor_or_grant_project_admin(self):
        subject = self.subject(actor="3")
        client = self.client(subject, actorId="1", actor_id=1, role="ADMIN", roles=["ADMIN"], admin=True)
        self.assertEqual(client.session(), {"actorId": "3"})
        self.assertEqual(self.project(client), [{"id": "1"}])
        refused = lh.create(client, "Secret", {"folder": "1", "key": "FORGED_ADMIN", "owner": "3"})
        self.assertFalse(refused.get("ok"), refused)
        self.assertEqual(self.dep.sql("SELECT count(*) FROM :schema.secret WHERE key = 'FORGED_ADMIN'").strip(), "0")

    def test_unbound_subject_and_overlong_or_expired_tokens_are_denied(self):
        self.assertEqual(lh.error_code(self.project, self.client("unbound-" + uuid.uuid4().hex)), "WIRE.UNAUTHENTICATED")
        subject = self.subject()
        self.assertEqual(self.project(self.client(subject, lifetime=900)), [{"id": "1"}])
        self.assertEqual(lh.error_code(self.project, self.client(subject, lifetime=901)), "WIRE.UNAUTHENTICATED")
        self.assertEqual(lh.error_code(self.project, self.client(subject, issued_at=int(time.time()) - 901)), "WIRE.TOKEN_EXPIRED")

    def test_revoking_an_unbound_subject_does_not_create_or_revoke_other_bindings(self):
        subject = self.subject()
        missing = "missing-" + uuid.uuid4().hex
        with self.assertRaisesRegex(RuntimeError, "PRINCIPAL_NOT_FOUND"):
            self.revoke(missing)
        self.assertEqual(self.dep.sql(f"SELECT count(*) FROM :schema.aip_principals WHERE subject = '{missing}'").strip(), "0")
        self.assertEqual(self.project(self.client(subject)), [{"id": "1"}])
