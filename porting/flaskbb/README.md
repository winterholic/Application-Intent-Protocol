# FlaskBB 2019 AIP porting track

This track keeps the original FlaskBB source snapshot under `upstream/` and does not edit it. Read [the source decision](../../plan-docs/porting/FLASKBB-SELECTION.md), then [status](STATUS.md) and the forthcoming [full inventory](INVENTORY.md).

To build and run the upstream baseline, see the commands recorded in `STATUS.md` after the first actual run. All users, data, credentials, service ports, and databases are test-only and isolated. This application is not the FaceReview port and does not close its open WBS.

`runtime/aip-introspect.json` uses only test configuration and a no-key JWKS file. `tests/aip-read.test.mjs` creates its own ephemeral RSA key/JWKS, database schema and principal mapping per run; the empty fixture JWKS is not an identity provider and cannot authenticate a user.

## Isolated port tests

Tests require the same explicit `AIP_TEST_DATABASE_URL` used by the main test harness: a database named `aip_test_*`, an explicit user and a non-default port. No DB URL is inferred. A legacy `AIP_FLASKBB_TEST_DATABASE_URL`, if set, must be identical. `PG*` environment fallbacks are rejected before any fixture is created.

For a new, run-owned Compose fixture, choose an unused `FLASKBB_FIX_PG_PORT` and a unique Compose project name, start only `aip-postgres`, then source `tests/env.sh`. The fixture database is `aip_test_flaskbb`; this change does not rename or modify an existing database. The upstream `flaskbb_baseline` database stays separate. CI uses its own disposable PostgreSQL database rather than a local fallback.

The two live UI suites (`aip-read.test.mjs`, `features.test.mjs`) also require `AIP_FLASKBB_TEST_PYTHON` to point to a Python environment owned by that run. Install `ui/requirements.txt` there. A historical Mac temporary venv is never selected implicitly. CI creates its environment under `RUNNER_TEMP`. `mail-jobs.test.mjs` requires an explicit loopback `AIP_FLASKBB_TEST_MAILPIT_URL`; CI owns its Mailpit service and removes it after the job. Tests stop only their child processes, drop their application and derived infrastructure schemas, and remove their own temporary directories. A cleanup failure fails the suite.

## Source-order import preflight (G-6)

`seed/post_order_manifest.py` checks the explicit source `Post.id` ↔ public UUID mapping before a future PostOrder import. It emits an offline plan only. The application schema, import executor, writer fence, sequence update and activation remain unfinished; its digest is not migration approval. This initial-import check also supports exact replay before native allocation is enabled.

Pass a JSON object with `manifest` and `snapshot` on stdin:

```json
{
  "manifest": {
    "version": 1,
    "source": {"namespace": "source-export-name", "revision": "0885238df7c54826adda8cfbe8a831e846e2cb64", "high_water": 80},
    "posts": [{"source_id": 42, "post_uuid": "00000000-0000-4000-8000-000000000001"}]
  },
  "snapshot": {
    "version": 1,
    "source": {"namespace": "source-export-name", "revision": "0885238df7c54826adda8cfbe8a831e846e2cb64", "high_water": 80},
    "posts": [{"post_uuid": "00000000-0000-4000-8000-000000000001", "published": true, "source_order": null}],
    "mappings": [],
    "generator": {"last_value": 1, "is_called": false}
  }
}
```

```sh
python3 porting/flaskbb/seed/post_order_manifest.py < verified-export.json
PYTHONDONTWRITEBYTECODE=1 python3 -W error::ResourceWarning -m unittest porting.flaskbb.tests.test_post_order_manifest -v
```

The source namespace identifies the original database/export, not merely the upstream code revision. `high_water` is the original allocator high-water, including IDs of deleted posts; the current maximum live row ID is insufficient. A verified exporter must capture this metadata and every target post (including hidden/deleted tombstones and drafts) plus both directions of any existing mapping. The example's numbers are synthetic. Matching metadata proves consistency of the supplied exports, not their authenticity, completeness or freshness relative to a live DB.

The checker rejects duplicate source IDs/UUIDs/JSON keys, source metadata mismatch, missing published posts, unknown targets, reassigned or dangling mappings, inconsistent reverse links, unknown fields and exhausted signed-int64 generators. Drafts cannot be assigned an imported ID. It keeps explicit source ordering even when UUID or timestamp ordering differs, proposes only missing mappings/links, and never proposes a generator floor below the source high-water or its exported current value. Sorted input digests bind the full exports to the plan; `activation_authorized` is always false. A future executor must revalidate under the product writer fence and retain the existing migration/rollback safeguards from [the G-6 contract](PRODUCT-GAPS.md#g-6-후속-결정--공개-uuid--내부-postorder-2026-10-30).

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
