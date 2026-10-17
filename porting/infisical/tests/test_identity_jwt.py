"""I02 JWT identity spike tests (WBS 7.3.3.7): no sockets, no network.

Builds an ephemeral RSA key in-test, serves it to the worker through the
deployment env (INFISICAL_JWKS_JSON), and asserts accept/reject semantics:
valid, expired, not-yet-valid, wrong issuer, wrong audience, tampered
signature, missing config.
"""
import asyncio
import json
import os
import sys
import time
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "workers"))

import infisical_jobs
from joserfc import jwt as _jwt
from joserfc.jwk import RSAKey as _RSAKey


class IdentityJwtTests(unittest.TestCase):
    def setUp(self):
        self.key = _RSAKey.generate_key(2048)
        public_jwks = {"keys": [self.key.as_dict(public_only=True)]}
        self.old = {k: os.environ.get(k) for k in ("INFISICAL_JWKS_JSON", "INFISICAL_JWT_ISSUERS", "INFISICAL_JWT_AUDIENCE", "INFISICAL_JWT_SKEW_S")}
        os.environ.update({
            "INFISICAL_JWKS_JSON": json.dumps(public_jwks),
            "INFISICAL_JWT_ISSUERS": "https://idp.example.test",
            "INFISICAL_JWT_AUDIENCE": "infisical-spike",
            "INFISICAL_JWT_SKEW_S": "60",
        })
        self.now = time.time()

    def tearDown(self):
        for k, v in self.old.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v

    def _token(self, claims, key=None):
        return _jwt.encode({"alg": "RS256"}, claims, key or self.key)

    def _claims(self, **over):
        base = {"iss": "https://idp.example.test", "aud": "infisical-spike",
                "sub": "machine-7", "iat": self.now, "exp": self.now + 300}
        base.update(over)
        return base

    def test_valid_token_accepted(self):
        result = asyncio.run(infisical_jobs.verify_identity_jwt({"token": self._token(self._claims())}, {}))
        self.assertEqual(result, {"valid": True, "subject": "machine-7"})

    def test_expired_token_rejected(self):
        with self.assertRaisesRegex(ValueError, "expired"):
            infisical_jobs.verify_identity_token(self._token(self._claims(exp=self.now - 3600)), now=self.now)

    def test_not_yet_valid_token_rejected(self):
        with self.assertRaisesRegex(ValueError, "not yet valid"):
            infisical_jobs.verify_identity_token(self._token(self._claims(nbf=self.now + 3600)), now=self.now)

    def test_wrong_issuer_rejected(self):
        with self.assertRaisesRegex(ValueError, "issuer"):
            infisical_jobs.verify_identity_token(self._token(self._claims(iss="https://evil.example.test")))

    def test_wrong_audience_rejected(self):
        with self.assertRaisesRegex(ValueError, "audience"):
            infisical_jobs.verify_identity_token(self._token(self._claims(aud="other")))

    def test_tampered_signature_rejected(self):
        good = self._token(self._claims())
        head, body, _sig = good.split(".")
        bad = f"{head}.{body}.{'A' * 342}"
        with self.assertRaisesRegex(ValueError, "signature rejected"):
            infisical_jobs.verify_identity_token(bad)

    def test_missing_config_fails_closed(self):
        os.environ.pop("INFISICAL_JWKS_JSON", None)
        with self.assertRaises(RuntimeError):
            infisical_jobs.verify_identity_token(self._token(self._claims()))


if __name__ == "__main__":
    unittest.main()
