# FlaskBB 2019 AIP porting track

This track keeps the original FlaskBB source snapshot under `upstream/` and does not edit it. Read [the source decision](../../plan-docs/porting/FLASKBB-SELECTION.md), then [status](STATUS.md) and the forthcoming [full inventory](INVENTORY.md).

To build and run the upstream baseline, see the commands recorded in `STATUS.md` after the first actual run. All users, data, credentials, service ports, and databases are test-only and isolated. This application is not the FaceReview port and does not close its open WBS.

`runtime/aip-introspect.json` uses only test configuration and a no-key JWKS file. `tests/aip-read.test.mjs` creates its own ephemeral RSA key/JWKS, database schema and principal mapping per run; the empty fixture JWKS is not an identity provider and cannot authenticate a user.

## Isolated port tests

Tests require the same explicit `AIP_TEST_DATABASE_URL` used by the main test harness: a database named `aip_test_*`, an explicit user and a non-default port. No DB URL is inferred. A legacy `AIP_FLASKBB_TEST_DATABASE_URL`, if set, must be identical. `PG*` environment fallbacks are rejected before any fixture is created.

For a new, run-owned Compose fixture, choose an unused `FLASKBB_FIX_PG_PORT` and a unique Compose project name, start only `aip-postgres`, then source `tests/env.sh`. The fixture database is `aip_test_flaskbb`; this change does not rename or modify an existing database. The upstream `flaskbb_baseline` database stays separate. CI uses its own disposable PostgreSQL database rather than a local fallback.

The two live UI suites (`aip-read.test.mjs`, `features.test.mjs`) also require `AIP_FLASKBB_TEST_PYTHON` to point to a Python environment owned by that run. Install `ui/requirements.txt` there. A historical Mac temporary venv is never selected implicitly. CI creates its environment under `RUNNER_TEMP`. `mail-jobs.test.mjs` requires an explicit loopback `AIP_FLASKBB_TEST_MAILPIT_URL`; CI owns its Mailpit service and removes it after the job. Tests stop only their child processes, drop their application and derived infrastructure schemas, and remove their own temporary directories. A cleanup failure fails the suite.

## Existing duplicate mailbox copies (WBS 7.5.5)

`unique user, shared_id` prevents two copies for the same mailbox; the sender and recipient may each retain one copy. An existing source database needs the corresponding physical unique constraint before external adoption. AIP must refuse the missing constraint and must not silently create it or delete data.

Before adding the constraint, keep a restorable snapshot and inspect duplicates in a read-only transaction against the source table from the external mapping:

```sql
BEGIN READ ONLY;
SELECT user_id, shared_id, count(*) AS copies, array_agg(id ORDER BY id) AS conversation_ids
FROM legacy_app.conversation
GROUP BY user_id, shared_id
HAVING count(*) > 1;
ROLLBACK;
```

Replace `legacy_app` with the actual mapped source schema. This inspection does not read message bodies. Any duplicate group blocks adoption. Keep every conversation id and its message links while the application owner decides the correct correspondence between mailbox copies. Blindly deleting one copy, merging messages, or generating a new `shared_id` for one side can destroy the sender/recipient pairing and is not an automatic migration.

Apply an approved repair to an isolated restored copy first, check both mailbox histories and message authors, then verify zero duplicate groups and create the physical unique index with the database owner. Failure to create the index leaves source rows intact. Re-run external adoption planning after the source constraint exists. The inspection never repairs an existing source; automated regressions use only synthetic schemas owned by their test run.
