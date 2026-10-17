-- Minimal Infisical-shaped data set for the port definition (app.aip). Run by tests/live_harness.py and by
-- DB-RUNBOOK.md after `aip service init`. :schema is replaced by the deployment schema name.
--
-- Users come from the identity provider, so rows are created by the operator, then bound to token subjects with
-- `aip service principal bind`. Ids are explicit so subjects, tests and the runbook can name them.
--
--   id  subject   place
--   1   alice     org Acme: admin, org Other: plain member (so she can see eve)
--   2   bob       org Acme: member, project Web: member
--   3   carol     org Acme: member, project Web: viewer
--   4   mallory   no organization at all
--   5   eve       org Other: admin
--   6   dan       org Acme: member, no project membership
INSERT INTO :schema."user" (id, email, username) OVERRIDING SYSTEM VALUE VALUES
  (1, 'alice@acme.test', 'alice'), (2, 'bob@acme.test', 'bob'), (3, 'carol@acme.test', 'carol'),
  (4, 'mallory@elsewhere.test', 'mallory'), (5, 'eve@other.test', 'eve'), (6, 'dan@acme.test', 'dan');
INSERT INTO :schema.organization (id, name, slug, created_by_id) OVERRIDING SYSTEM VALUE VALUES
  (1, 'Acme', 'acme', 1), (2, 'Other', 'other', 5);
INSERT INTO :schema.org_membership (user_id, org_id, role) VALUES
  (1, 1, 'ADMIN'), (2, 1, 'MEMBER'), (3, 1, 'MEMBER'), (6, 1, 'MEMBER'), (5, 2, 'ADMIN'), (1, 2, 'MEMBER');
INSERT INTO :schema.project (id, org_id, name, slug) OVERRIDING SYSTEM VALUE VALUES
  (1, 1, 'Web', 'web'), (2, 2, 'Secret Lab', 'lab');
INSERT INTO :schema.project_membership (user_id, project_id, role) VALUES
  (2, 1, 'MEMBER'), (3, 1, 'VIEWER'), (5, 2, 'ADMIN');
-- Root folders ("/") per environment, as upstream has.
INSERT INTO :schema.folder (id, project_id, path, env) OVERRIDING SYSTEM VALUE VALUES
  (1, 1, '/', 'dev'), (2, 1, '/', 'prod'), (3, 2, '/', 'dev');
SELECT setval(pg_get_serial_sequence(':schema."user"', 'id'), 100);
SELECT setval(pg_get_serial_sequence(':schema.organization', 'id'), 100);
SELECT setval(pg_get_serial_sequence(':schema.project', 'id'), 100);
SELECT setval(pg_get_serial_sequence(':schema.folder', 'id'), 100);
