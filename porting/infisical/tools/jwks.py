"""Throwaway RS256 signing key, JWKS and access tokens for a loopback-only Infisical port deployment.

The token shape is the product profile in product/IDP-RUNBOOK.md: RS256, typ at+jwt, kid, and
sub/iat/nbf/exp with exp - iat <= 900 seconds. The private key is a JWK file that never leaves the
work directory and is not a stand-in for a real identity provider.

  python3 jwks.py keygen WORKDIR
  python3 jwks.py token  WORKDIR SUBJECT [--issuer URL] [--audience AUD] [--lifetime SECONDS]
"""

import argparse
import json
import pathlib
import sys
import time

from joserfc import jwt
from joserfc.jwk import RSAKey

ISSUER = "https://idp.example.test"
AUDIENCE = "infisical-spike"
MAX_LIFETIME = 900
KEY_FILE = "signing-key.json"
JWKS_FILE = "jwks.json"


def keygen(workdir):
    workdir = pathlib.Path(workdir)
    workdir.mkdir(parents=True, exist_ok=True)
    key = RSAKey.generate_key(2048, parameters={"kid": "infisical-spike-1", "use": "sig", "alg": "RS256"})
    (workdir / KEY_FILE).write_text(json.dumps(key.as_dict(private=True)), encoding="utf-8")
    (workdir / KEY_FILE).chmod(0o600)
    (workdir / JWKS_FILE).write_text(json.dumps({"keys": [key.as_dict(private=False)]}), encoding="utf-8")
    return key


def mint(workdir, subject, issuer=ISSUER, audience=AUDIENCE, lifetime=MAX_LIFETIME):
    if not 1 <= lifetime <= MAX_LIFETIME:
        raise ValueError(f"token lifetime must be 1..{MAX_LIFETIME} seconds")
    spec = json.loads((pathlib.Path(workdir) / KEY_FILE).read_text(encoding="utf-8"))
    key = RSAKey.import_key(spec)
    now = int(time.time())
    claims = {"iss": issuer, "aud": audience, "sub": subject, "iat": now, "nbf": now, "exp": now + lifetime}
    return jwt.encode({"alg": "RS256", "typ": "at+jwt", "kid": spec["kid"]}, claims, key)


def main(argv=None):
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    gen = sub.add_parser("keygen")
    gen.add_argument("workdir")
    tok = sub.add_parser("token")
    tok.add_argument("workdir")
    tok.add_argument("subject")
    tok.add_argument("--issuer", default=ISSUER)
    tok.add_argument("--audience", default=AUDIENCE)
    tok.add_argument("--lifetime", type=int, default=MAX_LIFETIME)
    args = parser.parse_args(argv)
    if args.cmd == "keygen":
        keygen(args.workdir)
        print(pathlib.Path(args.workdir) / JWKS_FILE)
    else:
        print(mint(args.workdir, args.subject, args.issuer, args.audience, args.lifetime))
    return 0


if __name__ == "__main__":
    sys.exit(main())
