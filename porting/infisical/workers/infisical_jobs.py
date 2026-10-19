"""Infisical port workers under "infisical_jobs.*".

`secret_reminder` is the only function a served app.aip operation points at. `verify_identity_jwt`
(WBS 7.3.3.7, I02) is a unit-tested spike that no operation declares yet, so it is not reachable
through the service. Shape follows the repo worker convention: `async def fn(input, ctx)`.

Scope of this spike: JWT-bearer identity verification (one of the 12+
upstream methods in services/identity-*-auth) against a deployment-provided
JWKS (INFISICAL_JWKS_JSON env, no network fetch). Claim checks: exp/nbf
with skew, issuer and audience allowlists from env. Everything else
(AWS/GCP/Azure/K8s/OIDC/LDAP/TLS/SPIFFE/token auth) is pending here.
"""

import json
import math
import os
import time

from joserfc import jwt as _jwt
from joserfc.jwk import KeySet as _KeySet


def _config():
    raw = os.environ.get("INFISICAL_JWKS_JSON", "")
    issuers = [i for i in os.environ.get("INFISICAL_JWT_ISSUERS", "").split(",") if i]
    audience = os.environ.get("INFISICAL_JWT_AUDIENCE", "")
    try:
        skew = int(os.environ.get("INFISICAL_JWT_SKEW_S", "60") or 60)
    except ValueError as exc:
        raise RuntimeError("identity JWT misconfigured, skew must be nonnegative integer seconds") from exc
    if skew < 0:
        raise RuntimeError("identity JWT misconfigured, skew must be nonnegative integer seconds")
    if not raw:
        raise RuntimeError("identity JWT not configured, missing INFISICAL_JWKS_JSON")
    if not issuers:
        raise RuntimeError("identity JWT not configured, missing INFISICAL_JWT_ISSUERS")
    try:
        jwks = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise RuntimeError("identity JWT misconfigured, JWKS is not JSON") from exc
    try:
        keys = _KeySet.import_key_set(jwks)
    except Exception as exc:
        raise RuntimeError(f"identity JWT misconfigured, unusable JWKS: {exc}") from exc
    if not keys:
        raise RuntimeError("identity JWT misconfigured, no usable JWKS keys")
    return keys, issuers, audience, skew


def _is_numeric_date(value):
    # bool is an int subclass in Python, but a JSON boolean is not a NumericDate.
    # Keep integer dates exact; converting a large JSON integer to float can overflow.
    return type(value) is int or (type(value) is float and math.isfinite(value))


def verify_identity_token(token, now=None):
    """Pure verification core (no I/O): returns (subject, claims dict).

    Raises ValueError on signature/expiry/issuer/audience/subject failures and
    RuntimeError on missing/misconfigured key material.
    """
    keys, issuers, audience, skew = _config()
    try:
        decoded = _jwt.decode(token, keys)
    except Exception as exc:
        raise ValueError(f"identity JWT signature rejected: {exc}") from exc

    claims = decoded.claims
    if not isinstance(claims, dict):
        raise ValueError("identity JWT claims must be an object")
    moment = now if now is not None else time.time()
    exp = claims.get("exp")
    if not _is_numeric_date(exp):
        raise ValueError("identity JWT expiration must be a finite NumericDate")
    if exp <= moment - skew:
        raise ValueError("identity JWT expired")
    if "nbf" in claims:
        nbf = claims["nbf"]
        if not _is_numeric_date(nbf):
            raise ValueError("identity JWT not-before must be a finite NumericDate")
        if nbf > moment + skew:
            raise ValueError("identity JWT not yet valid")
    if "iat" in claims and not _is_numeric_date(claims["iat"]):
        raise ValueError("identity JWT issued-at must be a finite NumericDate")
    if claims.get("iss") not in issuers:
        raise ValueError("identity JWT issuer not allowed")
    aud = claims.get("aud")
    if "aud" in claims and not (isinstance(aud, str) or
                                (isinstance(aud, list) and all(isinstance(a, str) for a in aud))):
        raise ValueError("identity JWT audience must be a string or array of strings")
    if audience:
        auds = aud if isinstance(aud, list) else [aud]
        if audience not in auds:
            raise ValueError("identity JWT audience mismatch")
    subject = claims.get("sub")
    if not isinstance(subject, str) or not subject:
        raise ValueError("identity JWT subject must be a nonempty string")
    return subject, dict(claims)


async def verify_identity_jwt(input, ctx):
    """Worker entry: {"token": str} -> {"valid": True, "subject": str}."""
    subject, _claims = verify_identity_token(input["token"])
    return {"valid": True, "subject": subject}


async def secret_reminder(input, ctx):
    """Record a reminder for a secret as an outbox notification (WBS 7.3.3.5).

    The read and the transition both run as the job principal, so row policies and the
    Secret.remind allow rule decide; a secret the principal cannot see fails with
    WIRE.MISSING_TARGET like the synchronous path. Delivery (mail, Slack) is not done here.
    """
    secret = input["secret"]
    page = await ctx.data.read({"read": "Secret", "select": ["id"],
                                "filter": [{"field": "id", "op": "eq", "value": secret}]})
    if not page["rows"]:
        raise ValueError("secret not visible to the job principal")
    result = await ctx.data.apply("Secret", "remind", [secret], f"remind-{secret}")
    return {"reminded": len(result["changed"]) + len(result.get("unchanged", []))}
