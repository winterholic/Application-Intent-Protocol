# FlaskBB 2019 AIP porting track

This track keeps the original FlaskBB source snapshot under `upstream/` and does not edit it. Read [the source decision](../../plan-docs/porting/FLASKBB-SELECTION.md), then [status](STATUS.md) and the forthcoming [full inventory](INVENTORY.md).

To build and run the upstream baseline, see the commands recorded in `STATUS.md` after the first actual run. All users, data, credentials, service ports, and databases are test-only and isolated. This application is not the FaceReview port and does not close its open WBS.

`runtime/aip-introspect.json` uses only test configuration and a no-key JWKS file. `tests/aip-read.test.mjs` creates its own ephemeral RSA key/JWKS, database schema and principal mapping per run; the empty fixture JWKS is not an identity provider and cannot authenticate a user.
