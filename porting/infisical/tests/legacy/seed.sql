-- Fixture for the legacy (commit 53b1537) definition, used only by tests/legacy_repro.py.
-- :schema is replaced by the deployment schema. Ids are explicit so the tests can name them.
INSERT INTO :schema."user" (id, email, username) OVERRIDING SYSTEM VALUE VALUES
  (1, 'alice@acme.test', 'alice'), (2, 'bob@acme.test', 'bob'), (3, 'carol@acme.test', 'carol'),
  (4, 'mallory@elsewhere.test', 'mallory'), (5, 'eve@other.test', 'eve');
INSERT INTO :schema.organization (id, name, slug) OVERRIDING SYSTEM VALUE VALUES
  (1, 'Acme', 'acme'), (2, 'Other', 'other');
INSERT INTO :schema.org_membership (user_id, org_id, role) VALUES
  (1, 1, 'admin'), (2, 1, 'member'), (3, 1, 'member'), (5, 2, 'admin');
INSERT INTO :schema.project (id, org_id, name, slug) OVERRIDING SYSTEM VALUE VALUES
  (1, 1, 'Web', 'web'), (2, 2, 'Secret Lab', 'lab');
INSERT INTO :schema.project_membership (user_id, project_id, role) VALUES
  (1, 1, 'admin'), (2, 1, 'member'), (3, 1, 'viewer'), (5, 2, 'admin');
INSERT INTO :schema.folder (id, project_id, path, env) OVERRIDING SYSTEM VALUE VALUES
  (1, 1, '/', 'dev');
-- explicit ids do not advance the identity sequences
SELECT setval(pg_get_serial_sequence(':schema."user"', 'id'), 100);
SELECT setval(pg_get_serial_sequence(':schema.organization', 'id'), 100);
SELECT setval(pg_get_serial_sequence(':schema.project', 'id'), 100);
SELECT setval(pg_get_serial_sequence(':schema.folder', 'id'), 100);
