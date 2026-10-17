"""Reproduces the independent-verification defects against the legacy app.aip (commit 53b1537).

Every test asserts the behavior the port is supposed to have, so against the legacy definition
each one FAILS (that is the "before" evidence); the current definition is covered by
test_live_db.py. Run:

  AIP_TEST_DATABASE_URL=postgresql://... python3 -m unittest porting/infisical/tests/legacy_repro.py -v
"""

import pathlib
import unittest

import live_harness as lh

LEGACY = pathlib.Path(__file__).resolve().parent / "legacy"
PRINCIPALS = [("alice", 1), ("bob", 2), ("carol", 3), ("mallory", 4), ("eve", 5)]


class LegacyDefects(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.dep = lh.Deployment(LEGACY / "app-53b1537.aip", LEGACY / "seed.sql", PRINCIPALS,
                                    workers=True, workers_dir=LEGACY / "workers").start()
        except lh.HarnessUnavailable as why:
            raise unittest.SkipTest(str(why))

    @classmethod
    def tearDownClass(cls):
        cls.dep.stop()

    def test_viewer_cannot_create_a_secret(self):
        carol = self.dep.client("carol")
        result = lh.create(carol, "Secret", {"project": "1", "folder": "1", "env": "dev", "key": "VIEWER_KEY",
                                              "valueCiphertext": "x", "version": 1})
        self.assertEqual(result.get("ok"), False, f"viewer created a secret: {result}")

    def test_viewer_cannot_create_a_folder(self):
        carol = self.dep.client("carol")
        result = lh.create(carol, "Folder", {"project": "1", "path": "/viewer", "env": "dev"})
        self.assertEqual(result.get("ok"), False, f"viewer created a folder: {result}")

    def test_viewer_cannot_rotate_a_secret(self):
        bob = self.dep.client("bob")
        made = lh.create(bob, "Secret", {"project": "1", "folder": "1", "env": "dev", "key": "ROTATE_ME",
                                          "valueCiphertext": "v1", "version": 1})
        self.assertTrue(made.get("ok"), made)
        secret_id = made["created"][0]["id"]
        carol = self.dep.client("carol")
        result = lh.transition(carol, "Secret.rotate", [secret_id], values={"valueCiphertext": "tampered"})
        self.assertEqual(result.get("ok"), False, f"viewer rotated a secret: {result}")

    def test_root_secret_key_is_unique(self):
        bob = self.dep.client("bob")
        values = {"project": "1", "env": "prod", "key": "ROOT_DUP", "valueCiphertext": "x", "version": 1}
        first = lh.create(bob, "Secret", values)
        second = lh.create(bob, "Secret", values, key="second-attempt")
        self.assertTrue(first.get("ok"), first)
        self.assertEqual(second.get("ok"), False, f"duplicate root secret accepted: {second}")

    def test_secret_value_is_readable_or_not_stored(self):
        bob = self.dep.client("bob")
        made = lh.create(bob, "Secret", {"project": "1", "folder": "1", "env": "dev", "key": "READ_ME",
                                          "valueCiphertext": "opaque", "version": 1})
        self.assertTrue(made.get("ok"), made)
        code = lh.error_code(bob.read, {"read": "Secret", "select": ["id", "key", "valueCiphertext"],
                                         "filter": [{"field": "key", "op": "eq", "value": "READ_ME"}]})
        self.assertIsNone(code, f"a stored value cannot be read back: {code}")

    def test_non_member_cannot_enumerate_user_emails(self):
        mallory = self.dep.client("mallory")
        rows = mallory.read({"read": "User", "select": ["id", "email"]})
        emails = {row["email"] for row in rows}
        self.assertTrue(emails <= {"mallory@elsewhere.test"}, f"mallory sees {sorted(emails)}")

    def test_declared_jobs_run(self):
        alice = self.dep.client("alice")
        receipt = lh.job_receipt(alice, "secretReminder", {"projectId": "1", "secretId": "1"}, "legacy-job")
        self.assertEqual(receipt["state"], "SUCCEEDED", f"job did not run: {receipt}")

    def test_deleted_secret_leaves_the_list_and_can_be_recreated(self):
        alice = self.dep.client("alice")
        values = {"project": "1", "folder": "1", "env": "dev", "key": "DELETE_ME", "valueCiphertext": "x", "version": 1}
        made = lh.create(alice, "Secret", values)
        self.assertTrue(made.get("ok"), made)
        deleted = lh.transition(alice, "Secret.delete", [made["created"][0]["id"]])
        self.assertTrue(deleted.get("ok"), deleted)
        rows = alice.read({"read": "Secret", "select": ["id", "key"], "filter": [{"field": "key", "op": "eq", "value": "DELETE_ME"}]})
        self.assertEqual(rows, [], f"deleted secret is still listed: {rows}")
        again = lh.create(alice, "Secret", values, key="recreate")
        self.assertTrue(again.get("ok"), f"deleted key cannot be recreated: {again}")

    def test_organization_creator_is_not_orphaned(self):
        mallory = self.dep.client("mallory")
        made = lh.create(mallory, "Organization", {"name": "Mallory Co", "slug": "mallory-co"})
        self.assertTrue(made.get("ok"), made)
        rows = mallory.read({"read": "Organization", "select": ["id", "slug"], "filter": [{"field": "slug", "op": "eq", "value": "mallory-co"}]})
        self.assertEqual(len(rows), 1, "the creator cannot see the organization it just created")
        members = self.dep.sql("SELECT count(*) FROM :schema.org_membership WHERE org_id = " + made["created"][0]["id"])
        self.assertEqual(members.strip(), "1", "the organization has no admin member")

    def test_job_enqueue_requires_project_access(self):
        mallory = self.dep.client("mallory")
        code = lh.error_code(mallory.jobs.enqueue, "secretReminder",
                             {"projectId": "1", "secretId": "1"}, key="mallory-job")
        self.assertIsNotNone(code, "a user with no access to the project enqueued a job for it")


if __name__ == "__main__":
    unittest.main()
