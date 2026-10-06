# AIP operational JWT authenticator

This crate accepts an OAuth JWT access token intended for the AIP API and maps
its verified `(issuer, subject)` to an existing actor. It does not perform login,
issue tokens, create actors, or decide application permissions.

## Design gates

1. It implements server authority and one reusable authentication boundary
   (Principles 1, 2, 6); app-specific issuer and actor mapping remain explicit.
2. Caller intent syntax and available reads/writes are unchanged. A valid token
   only supplies the actor passed to existing authorization rules.
3. Applications configure one issuer and API audience, rather than implement
   per-intent authentication middleware.
4. Token header and claims, signing key, current identity mapping, and actor
   existence are verified on the server. No JWT role or actor ID is trusted as
   an AIP grant.
5. JS/TS and Python callers may obtain/refresh a provider access token through
   their normal ecosystem. The generated AIP client remains provider agnostic.
6. The existing Bearer header and `/session` principal shape are reused; no
   alternate caller identity header is added to the product path.
7. JWT verification is an adapter into the existing V6 `Session`/`Caller` path,
   not a new intent compiler or planner phase.
8. The configured provider and API audience are deployment choices. This crate
   does not settle the project's final Id wire, public deployment, or provider
   onboarding policy.

The accepted token profile is an asymmetric RS256 JWT access token with
`typ=at+jwt`, exact issuer and API audience, `sub`, `exp`, `nbf`, and `iat`.
Its lifetime is at most 900 seconds. Public JWKs come from an explicitly
configured local file or HTTPS URL. A missing/disabled mapping fails closed.
The mapping table is `aip_principals(issuer, subject, actor_id, enabled,
min_iat)` in the selected application schema; schema creation and FK are owned
by the product migration path.

## Verification scope

`cargo test -p aip-auth` exercises eight JWT and mapping integration cases,
including an actual local PostgreSQL mapping and revocation, plus five loader
unit cases. The loader cases cover redirects and oversized bodies with a local
HTTP fixture, and accepted or rejected certificate chains and hostnames with a
controlled local HTTPS server. The private HTTPS test client adds only its test
CA; normal TLS and hostname verification remain active. The public
configuration accepts only HTTPS URLs. A live external provider and its key
rotation have not been exercised here. The file source reloads on each request
under one mutex, with a bounded read and fail-closed errors.
`cargo fmt --package aip-auth --check` and
`cargo clippy -p aip-auth --all-targets -- -D warnings` are the crate checks.
