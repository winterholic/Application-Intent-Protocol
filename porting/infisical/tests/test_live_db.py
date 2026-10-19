"""Real-PostgreSQL behavior of porting/infisical/app.aip.

Skips unless AIP_TEST_DATABASE_URL names a loopback, non-5432 database (see live_harness.py). Each test
that changes data uses its own key or organization so the cases do not depend on each other.

  AIP_TEST_DATABASE_URL=postgresql://... python3 -m unittest discover -s porting/infisical/tests -p 'test_live_db.py' -v
"""

import os
import pathlib
import re
import unittest

import live_harness as lh

PORT = pathlib.Path(__file__).resolve().parent.parent
# INFISICAL_APP_AIP lets tests/mutation_check.py run the same cases against a deliberately weakened definition.
APP = pathlib.Path(os.environ.get("INFISICAL_APP_AIP", PORT / "app.aip"))
PRINCIPALS = [("alice", 1), ("bob", 2), ("carol", 3), ("mallory", 4), ("eve", 5), ("dan", 6)]
ALICE, BOB, CAROL, MALLORY, EVE, DAN = "alice", "bob", "carol", "mallory", "eve", "dan"


def eq(field, value):
    return [{"field": field, "op": "eq", "value": value}]


class LiveDbTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.dep = lh.Deployment(APP, PORT / "seed.sql", PRINCIPALS, workers=True).start()
        except lh.HarnessUnavailable as why:
            raise unittest.SkipTest(str(why))
        cls.c = {name: cls.dep.client(name) for name, _ in PRINCIPALS}

    @classmethod
    def tearDownClass(cls):
        cls.dep.stop()

    # -- helpers -----------------------------------------------------------------------------------------------
    def create_secret(self, who, key, folder="1"):
        """One secret row (name + owner in a folder); returns the create response."""
        owner = {"alice": "1", "bob": "2", "carol": "3", "dan": "6"}[who]
        return lh.create(self.c[who], "Secret", {"folder": folder, "key": key, "owner": owner})

    def secrets(self, who, key):
        return self.c[who].read({"read": "Secret", "select": ["id", "key"], "filter": eq("key", key)})

    def rows_named(self, key, folder="1"):
        return int(self.dep.sql(f"SELECT count(*) FROM :schema.secret WHERE folder_id = {folder} AND key = '{key}'").strip())

    # -- roles -------------------------------------------------------------------------------------------------
    def test_viewer_cannot_create_folder_or_secret(self):
        folder = lh.create(self.c[CAROL], "Folder", {"project": "1", "path": "/viewer", "env": "dev"})
        self.assertFalse(folder.get("ok"), folder)
        made = self.create_secret(CAROL, "VIEWER_NAME")
        self.assertFalse(made.get("ok"), made)
        self.assertEqual(self.rows_named("VIEWER_NAME"), 0)

    def test_viewer_can_read_but_not_delete(self):
        made = self.create_secret(BOB, "VIEWER_READS")
        self.assertTrue(made.get("ok"), made)
        secret_id = made["created"][0]["id"]
        self.assertEqual([r["key"] for r in self.secrets(CAROL, "VIEWER_READS")], ["VIEWER_READS"])
        denied = lh.transition(self.c[CAROL], "Secret.delete", [secret_id])
        self.assertFalse(denied.get("ok"), denied)
        self.assertEqual(self.rows_named("VIEWER_READS"), 1)

    def test_member_can_create_folder_and_secret(self):
        folder = lh.create(self.c[BOB], "Folder", {"project": "1", "path": "/member-made", "env": "dev"})
        self.assertTrue(folder.get("ok"), folder)
        made = self.create_secret(BOB, "MEMBER_KEY", folder=folder["created"][0]["id"])
        self.assertTrue(made.get("ok"), made)

    def test_secret_cannot_be_created_in_someone_elses_name(self):
        spoof = lh.create(self.c[BOB], "Secret", {"folder": "1", "key": "SPOOFED_OWNER", "owner": "1"})
        self.assertFalse(spoof.get("ok"), spoof)
        self.assertEqual(self.rows_named("SPOOFED_OWNER"), 0)

    def test_no_access_role_reads_nothing(self):
        # dan is an organization member with no project membership: no project, folder or secret is visible.
        self.assertEqual(self.c[DAN].read({"read": "Project", "select": ["id"]}), [])
        self.assertEqual(self.c[DAN].read({"read": "Folder", "select": ["id"]}), [])
        refused = self.create_secret(DAN, "DAN_KEY")
        self.assertFalse(refused.get("ok"), refused)

    def test_other_organization_is_isolated(self):
        made = self.create_secret(BOB, "ACME_ONLY")
        self.assertEqual(self.secrets(EVE, "ACME_ONLY"), [])
        refused = lh.create(self.c[EVE], "Secret", {"folder": "1", "key": "EVE_IN_ACME", "owner": "5"})
        self.assertFalse(refused.get("ok"), refused)
        denied = lh.transition(self.c[EVE], "Secret.delete", [made["created"][0]["id"]])
        self.assertFalse(denied.get("ok"), denied)
        self.assertEqual(self.rows_named("ACME_ONLY"), 1)

    # -- uniqueness and deletion (upstream: unique index uq_secrets_v2_key_folder_id_shared + hard delete) -----
    def test_secret_name_is_unique_in_a_folder_including_the_root(self):
        first = lh.create(self.c[BOB], "Secret", {"folder": "1", "key": "ROOT_DUP", "owner": "2"})
        second = lh.create(self.c[BOB], "Secret", {"folder": "1", "key": "ROOT_DUP", "owner": "2"}, key="dup-name")
        self.assertTrue(first.get("ok"), first)
        self.assertFalse(second.get("ok"), second)
        self.assertEqual(second.get("code"), "WIRE.ALREADY_EXISTS")
        self.assertEqual(self.rows_named("ROOT_DUP"), 1)

    def test_same_name_is_allowed_in_another_folder(self):
        folder = lh.create(self.c[BOB], "Folder", {"project": "1", "path": "/other", "env": "dev"})
        self.assertTrue(folder.get("ok"), folder)
        self.assertTrue(self.create_secret(BOB, "SAME_NAME").get("ok"))
        self.assertTrue(self.create_secret(BOB, "SAME_NAME", folder=folder["created"][0]["id"]).get("ok"))

    def test_delete_removes_the_row_and_the_name_can_be_recreated_twice(self):
        ids = []
        for round_ in (1, 2, 3):
            made = self.create_secret(BOB, "RECYCLE")
            self.assertTrue(made.get("ok"), f"round {round_} create: {made}")
            secret_id = made["created"][0]["id"]
            ids.append(secret_id)
            self.assertEqual(self.rows_named("RECYCLE"), 1)
            deleted = lh.transition(self.c[BOB], "Secret.delete", [secret_id])
            self.assertTrue(deleted.get("ok"), deleted)
            self.assertEqual(self.secrets(BOB, "RECYCLE"), [], f"round {round_}: deleted secret still listed")
            self.assertEqual(self.rows_named("RECYCLE"), 0, f"round {round_}: row must be physically gone")
        self.assertEqual(len(set(ids)), 3, "each recreate is a new row")

    def test_bulk_delete_removes_every_named_secret_and_no_other(self):
        ids = [self.create_secret(BOB, f"BULK_{n}")["created"][0]["id"] for n in range(3)]
        keep = self.create_secret(BOB, "BULK_KEEP")["created"][0]["id"]
        deleted = lh.transition(self.c[BOB], "Secret.delete", ids[:2] + [ids[2]])
        self.assertTrue(deleted.get("ok"), deleted)
        for n in range(3):
            self.assertEqual(self.rows_named(f"BULK_{n}"), 0)
        self.assertEqual(self.rows_named("BULK_KEEP"), 1)
        self.assertEqual([r["id"] for r in self.secrets(BOB, "BULK_KEEP")], [keep])

    def test_delete_leaves_a_before_image_in_the_audit_log(self):
        made = self.create_secret(BOB, "AUDITED_DELETE")
        secret_id = made["created"][0]["id"]
        self.assertTrue(lh.transition(self.c[BOB], "Secret.delete", [secret_id], key="audited-delete").get("ok"))
        listing = self.dep.cli("audit", "--config", str(self.dep.work / "service.json"), "list",
                               "--resource", "Secret", "--target-id", secret_id)
        rows = [r for r in listing["audit"] if r["requestKey"] == "audited-delete"]
        self.assertEqual(len(rows), 1, listing)
        row = rows[0]
        self.assertEqual((row["transition"], row["via"], row["actorId"]), ("delete", "apply", 2))
        self.assertEqual(row["before"]["key"], "AUDITED_DELETE")
        self.assertIsNone(row["after"])

    # -- users and organizations -------------------------------------------------------------------------------
    def test_non_member_sees_only_themselves(self):
        emails = {r["email"] for r in self.c[MALLORY].read({"read": "User", "select": ["id", "email"]})}
        self.assertEqual(emails, {"mallory@elsewhere.test"})

    def test_members_see_their_organization_only(self):
        emails = {r["email"] for r in self.c[BOB].read({"read": "User", "select": ["id", "email"]})}
        self.assertEqual(emails, {"alice@acme.test", "bob@acme.test", "carol@acme.test", "dan@acme.test"})

    def test_organization_creator_becomes_admin_exactly_once(self):
        mallory = self.c[MALLORY]
        made = lh.create(mallory, "Organization", {"name": "Mallory Co", "slug": "mallory-co", "createdBy": "4"})
        self.assertTrue(made.get("ok"), made)
        org = made["created"][0]["id"]
        self.assertEqual(len(mallory.read({"read": "Organization", "select": ["id"], "filter": eq("id", org)})), 1)
        # someone else cannot take the open seat of an organization they did not create
        thief = lh.create(self.c[DAN], "OrgMembership", {"user": "6", "org": org, "role": "ADMIN"})
        self.assertFalse(thief.get("ok"), thief)
        claim = lh.create(mallory, "OrgMembership", {"user": "4", "org": org, "role": "ADMIN"})
        self.assertTrue(claim.get("ok"), claim)
        again = lh.create(mallory, "OrgMembership", {"user": "4", "org": org, "role": "ADMIN"}, key="claim-2")
        self.assertFalse(again.get("ok"), again)
        project = lh.create(mallory, "Project", {"org": org, "name": "Mine", "slug": "mine"})
        self.assertTrue(project.get("ok"), project)

    def test_organization_cannot_be_created_in_someone_elses_name(self):
        spoof = lh.create(self.c[BOB], "Organization", {"name": "Spoof", "slug": "spoof", "createdBy": "1"})  # alice is visible to bob
        self.assertFalse(spoof.get("ok"), spoof)

    def test_cannot_claim_an_organization_created_by_another_user(self):
        claim = lh.create(self.c[MALLORY], "OrgMembership", {"user": "4", "org": "2", "role": "ADMIN"})
        self.assertFalse(claim.get("ok"), claim)

    def test_project_membership_rules(self):
        outsider = lh.create(self.c[ALICE], "ProjectMembership", {"user": "5", "project": "1", "role": "MEMBER"})
        self.assertFalse(outsider.get("ok"), "a user outside the organization cannot join its project")
        by_member = lh.create(self.c[BOB], "ProjectMembership", {"user": "6", "project": "1", "role": "MEMBER"})
        self.assertFalse(by_member.get("ok"), by_member)
        joined = lh.create(self.c[ALICE], "ProjectMembership", {"user": "6", "project": "1", "role": "VIEWER"})
        self.assertTrue(joined.get("ok"), joined)
        membership_id = joined["created"][0]["id"]
        escalate = lh.transition(self.c[DAN], "ProjectMembership.setRole", [membership_id], values={"role": "ADMIN"})
        self.assertFalse(escalate.get("ok"), "a viewer cannot promote themselves")
        promoted = lh.transition(self.c[ALICE], "ProjectMembership.setRole", [membership_id], values={"role": "NO_ACCESS"})
        self.assertTrue(promoted.get("ok"), promoted)
        self.assertEqual(self.c[DAN].read({"read": "Project", "select": ["id"]}), [])

    # -- jobs --------------------------------------------------------------------------------------------------
    def test_reminder_job_runs_for_a_project_reader(self):
        made = self.create_secret(BOB, "REMIND_ME")
        secret_id = made["created"][0]["id"]
        receipt = lh.job_receipt(self.c[CAROL], "secretReminder", {"secret": secret_id}, "carol-reminds")
        self.assertEqual(receipt["state"], "SUCCEEDED", receipt)
        rows = self.c[BOB].read({"read": "Secret", "select": ["id", "remindedAt"], "filter": eq("id", secret_id)})
        self.assertIsNotNone(rows[0]["remindedAt"])
        queued = self.dep.sql("SELECT count(*) FROM :schema.aip_outbox").strip()
        self.assertGreaterEqual(int(queued), 1, "the reminder must leave an outbox notification")

    def test_reminder_job_rejects_callers_without_project_access(self):
        made = self.create_secret(BOB, "REMIND_PRIVATE")
        secret_id = made["created"][0]["id"]
        for who in (DAN, MALLORY, EVE):
            code = lh.error_code(self.c[who].jobs.enqueue, "secretReminder", {"secret": secret_id}, key=f"{who}-probe")
            self.assertIsNotNone(code, f"{who} enqueued a job for a project they cannot read")

    def test_every_served_job_has_a_worker_function(self):
        text = APP.read_text(encoding="utf-8")
        worker = (PORT / "workers" / "infisical_jobs.py").read_text(encoding="utf-8")
        for module, function in re.findall(r'implementation "([a-z_]+)\.([a-z_]+)"', text):
            self.assertEqual(module, "infisical_jobs")
            self.assertRegex(worker, rf"(?m)^(async )?def {function}\(", f"{function} is declared but not implemented")


if __name__ == "__main__":
    unittest.main()
