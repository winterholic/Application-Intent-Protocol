"""Infisical machine-identity worker spike (WBS 7.3.3.7, I02).

Declared in app.aip as implementations under "infisical_jobs.*".
Shape follows the repo worker convention: `async def fn(input, ctx)`.

Scope of this spike: JWT-bearer identity verification (one of the 12+
upstream methods in services/identity-*-auth) against a deployment-provided
JWKS (INFISICAL_JWKS_JSON env, no network fetch). Claim checks: exp/nbf
with skew, issuer and audience allowlists from env. Everything else
(AWS/GCP/Azure/K8s/OIDC/LDAP/TLS/SPIFFE/token auth) is pending here.
"""

import json
import os
import time

from joserfc import jwt as _jwt
from joserfc.jwk import KeySet as _KeySet


def _config():
    raw = os.environ.get("INFISICAL_JWKS_JSON", "")
    issuers = [i for i in os.environ.get("INFISICAL_JWT_ISSUERS", "").split(",") if i]
    audience = os.environ.get("INFISICAL_JWT_AUDIENCE", "")
    skew = int(os.environ.get("INFISICAL_JWT_SKEW_S", "60") or 60)
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
    moment = now if now is not None else time.time()
    exp = claims.get("exp")
    if not isinstance(exp, (int, float)) or exp + skew < moment:
        raise ValueError("identity JWT expired")
    nbf = claims.get("nbf")
    if isinstance(nbf, (int, float)) and nbf - skew > moment:
        raise ValueError("identity JWT not yet valid")
    if claims.get("iss") not in issuers:
        raise ValueError("identity JWT issuer not allowed")
    if audience:
        aud = claims.get("aud")
        auds = aud if isinstance(aud, list) else [aud]
        if audience not in auds:
            raise ValueError("identity JWT audience mismatch")
    subject = claims.get("sub")
    if not subject:
        raise ValueError("identity JWT has no subject")
    return subject, dict(claims)


async def verify_identity_jwt(input, ctx):
    """Worker entry: {"token": str} -> {"valid": True, "subject": str}."""
    subject, _claims = verify_identity_token(input["token"])
    return {"valid": True, "subject": subject}
