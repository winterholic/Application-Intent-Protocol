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
from joserfc import jws as _jws
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

    def test_expiration_rejects_non_finite_and_non_numeric_dates(self):
        for exp in (float("nan"), float("inf"), float("-inf"), True, False,
                    "300", None, [], {}):
            with self.subTest(exp=exp), self.assertRaises(ValueError):
                infisical_jobs.verify_identity_token(self._token(self._claims(exp=exp)), now=0)

    def test_not_before_rejects_malformed_dates_instead_of_ignoring_them(self):
        for nbf in (float("nan"), float("inf"), float("-inf"), True, False,
                    "300", None, [], {}):
            with self.subTest(nbf=nbf), self.assertRaises(ValueError):
                infisical_jobs.verify_identity_token(self._token(self._claims(nbf=nbf)), now=self.now)

    def test_issued_at_rejects_malformed_dates_when_present(self):
        for iat in (float("nan"), float("inf"), float("-inf"), True, False,
                    "300", None, [], {}):
            with self.subTest(iat=iat), self.assertRaises(ValueError):
                infisical_jobs.verify_identity_token(self._token(self._claims(iat=iat)), now=self.now)

    def test_subject_requires_a_nonempty_string(self):
        for subject in (None, "", True, 7, ["machine-7"], {"id": "machine-7"}):
            with self.subTest(subject=subject), self.assertRaisesRegex(ValueError, "subject"):
                infisical_jobs.verify_identity_token(self._token(self._claims(sub=subject)), now=self.now)

    def test_audience_requires_strings_even_if_one_entry_matches(self):
        for audience in (["infisical-spike", 7], ["infisical-spike", None],
                         ["infisical-spike", []]):
            with self.subTest(audience=audience), self.assertRaisesRegex(ValueError, "audience"):
                infisical_jobs.verify_identity_token(self._token(self._claims(aud=audience)), now=self.now)

    def test_expiration_boundary_includes_configured_skew(self):
        # RFC 7519: time MUST be before exp; equality after leeway has elapsed is expired.
        for skew in (0, 60):
            os.environ["INFISICAL_JWT_SKEW_S"] = str(skew)
            token = self._token(self._claims(exp=self.now - skew))
            with self.subTest(skew=skew), self.assertRaisesRegex(ValueError, "expired"):
                infisical_jobs.verify_identity_token(token, now=self.now)

    def test_fractional_dates_and_not_before_boundary_are_accepted(self):
        os.environ["INFISICAL_JWT_SKEW_S"] = "0"
        token = self._token(self._claims(exp=self.now + 0.5, nbf=self.now,
                                        aud=["other", "infisical-spike"]))
        subject, claims = infisical_jobs.verify_identity_token(token, now=self.now)
        self.assertEqual(subject, "machine-7")
        self.assertEqual(claims["nbf"], self.now)

    def test_negative_or_invalid_skew_is_configuration_failure(self):
        for skew in ("-1", "nan", "1.5"):
            os.environ["INFISICAL_JWT_SKEW_S"] = skew
            with self.subTest(skew=skew), self.assertRaisesRegex(RuntimeError, "skew"):
                infisical_jobs.verify_identity_token(self._token(self._claims()), now=self.now)

    def test_signed_payload_must_be_a_claims_object(self):
        for payload in ([], [self._claims()], "identity", None, 7):
            token = _jws.serialize_compact({"alg": "RS256"}, json.dumps(payload).encode(), self.key)
            with self.subTest(payload=payload), self.assertRaisesRegex(ValueError, "claims"):
                infisical_jobs.verify_identity_token(token, now=self.now)

    def test_optional_dates_may_be_absent_and_large_integer_dates_stay_exact(self):
        claims = self._claims(exp=10 ** 400)
        claims.pop("iat")
        subject, verified = infisical_jobs.verify_identity_token(self._token(claims), now=self.now)
        self.assertEqual(subject, "machine-7")
        self.assertEqual(verified["exp"], 10 ** 400)


if __name__ == "__main__":
    unittest.main()
