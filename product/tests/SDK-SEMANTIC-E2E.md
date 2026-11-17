# SDK semantic end-to-end coverage

`sdk-semantic-e2e.test.mjs` runs the declared AIP contract through the real `aip service`, the generated TypeScript binding and installed `@aip/sdk`, and the official Python caller SDK. Both SDK clients use JWT authentication and a disposable PostgreSQL 17 service. The test checks each client result and then queries the fixture rows directly.

## Existing smoke coverage at the P1-2 baseline

| Existing asset | What it already proves | What it does not prove |
|---|---|---|
| `product/tests/service-smoke.test.mjs:15-187` → `product/tools/smoke.mjs:330-374` | Builds and installs `@aip/sdk`; generated binding and Python client call the live service; auth/session, read/cache, write/cache invalidation, idempotent replay, revoked token rejection; optional Node/Python worker operation and successful job result. Uses both decimal and safe Id wires. | It uses the example schema and does not assert nullable/omitted update behavior, exact SQL Decimal values, policy field redaction, visible forbidden versus missing targets, or the DB rows for those SDK writes. |
| `product/tests/sdk-package.test.mjs` (strict consumer and runtime mock cases) | SDK package closure, offline installation, strict TypeScript type consumer, and negative type-check control. | No real service or PostgreSQL call. |
| `.github/workflows/app-packages.yml:56-57` → `product/tests/app-package-e2e.test.mjs:14-53` | Release packaging, relocation, deployment check, contract regeneration equality, installed package import, and binding typecheck. | No SDK request to HTTP or PostgreSQL. |
| `product/tests/python_sdk_test.py:48-72, 187-205` | Python protocol behavior with a scripted in-process fake transport, including contract drift and managed routes. | No live service or database. |
| `crates/aip-cli/tests/service_flow.rs:445-530, 533-640, 642-800` | Service-level HTTP/PostgreSQL value semantics, redaction, policy-denial distinctions, and aggregate behavior. | Requests are made by Rust test helpers, not by the generated TypeScript/Python caller SDKs. |

## Ten semantic requirements

| Requirement | At baseline | P1-2 coverage |
|---|---|---|
| Nullable versus optional/omitted values | Missing for generated SDKs | Both SDKs read `null`, explicitly clear a nullable field, and send a partial update whose omitted fields remain unchanged. |
| Decimal precision | Only Decimal Id wire is exercised by the smoke; field Decimal precision is missing | Both SDKs round-trip a high precision Decimal as a string and PostgreSQL retains the exact numeric text. |
| Relational response | Service-level relation traversal exists in `service_flow.rs`; no caller SDK assertion | Both SDKs request `Record.owner { id, org }` through the declared traversal and compare the returned object to the fixture's related Member row. |
| Policy field hiding | Service-level redaction exists; no caller SDK assertion | Both clients see `null` for a policy-hidden field on a visible row, and retain the owner value where authorized. |
| Permission denial versus absent resource | Service-level `FORBIDDEN`/`MISSING_TARGET` exists; no caller SDK result assertion | Both SDKs return `ok: false` with `WIRE.FORBIDDEN` for a visible but unauthorized write and `WIRE.MISSING_TARGET` for a row-hidden or nonexistent target; PostgreSQL confirms denied data is unchanged. These apply outcomes are returned values, not thrown SDK exceptions. |
| Partial update value preservation | Service-level partial update exists; no caller SDK + DB assertion | Both SDKs omit nullable and Decimal fields in one patch, assert they remain unchanged, then explicitly send null and a new exact Decimal. |
| Idempotent request/result replay | Existing smoke checks replay; only summary result is asserted | Both SDKs assert `replayed` from the live HTTP response; DB confirms the final state reflects one value and the replayed request did not alter it. |
| Job state and result lookup | Existing optional worker smoke asserts `SUCCEEDED` and result payload for both SDK pathways | No new job test; existing package smoke already checks enqueue, replay, terminal state, and result for the generated TypeScript SDK and Python SDK. |
| Error code and exception mapping | Python fake transport checks selected codes; no live SDK policy errors | Both SDKs assert the live service code through the thrown SDK error. |
| Deployment/contract change compatibility | Python package smoke rejects a pinned fingerprint drift; package test checks regeneration equality | Both SDK clients reject a stale fingerprint with `WIRE.CONTRACT_MISMATCH` in the new live service fixture. Migration compatibility across an actual old/new schema revision remains outside this test. |

Run locally only with a disposable PostgreSQL 17 target using the guarded `AIP_TEST_DATABASE_URL`; do not initialize a local cluster. CI runs this suite on Linux PostgreSQL 17 in `.github/workflows/sdk-semantic-e2e.yml`.

## CI execution evidence

- PR #5 run `44003092137` on `d168ad3` used Ubuntu 24.04 and PostgreSQL 17.12. The build, formatting/Clippy, and isolated database guard passed. The SDK journey command ran **2 tests, 0 passed, 2 failed, 0 skipped**; it did not skip or run zero tests.
- Both generated SDK journeys reached the live denial cases. The TypeScript script reported `AssertionError [ERR_ASSERTION]: Missing expected rejection` at `run.mjs:33`; Python reported `AssertionError: expected WIRE.FORBIDDEN`. Cause: test expectation error. The official SDKs return application-level apply denials as `{ok:false, code:...}`; they do not throw for those outcomes. The contracts and denial assertions remain in place and now inspect the returned result. No SDK, generator, or service behavior was changed for this failure.
- Final-head PR run `44003935452` on `83ebedb` used Ubuntu 24.04, Node 26.13.1, and PostgreSQL 17.12. `TypeScript SDK tests: 1 passed, 0 failed`; `Python SDK tests: 1 passed, 0 failed`; Node runner: **2 tests, 2 passed, 0 failed, 0 skipped**. Build, formatting/Clippy, and the isolated database guard also passed. This confirms both real SDK journeys ran against the CI database.
- Verified here: live TypeScript/Python reads and writes, null/omission behavior, Decimal round trip and stored precision, relation traversal, field redaction, idempotent replay, error codes, and direct PostgreSQL state. Existing package smoke coverage remains the evidence for job state/result lookup. Still unverified here: compatibility between actual old/new schema revisions during migration. Migration compatibility remains outside this test.
