"""Project removal/leave contracts, exercised through the SDK against an isolated PostgreSQL.

The pinned upstream reference is project-membership-service.ts deleteProjectMemberships/leaveProject.
These tests cover the port's existing fixed-role permission model, not upstream EE permissions.
"""

import unittest
import uuid
import sys

import live_harness as lh
from test_live_db import APP, SEED, PRINCIPALS, eq


class ProjectMembershipTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            # Membership operations need no worker. The Linux CI also runs the isolated job check below.
            cls.dep = lh.Deployment(APP, SEED, PRINCIPALS, workers=sys.platform == "linux")
        except lh.HarnessUnavailable as why:
            raise unittest.SkipTest(str(why))
        try:
            cls.dep.start()
        except BaseException:
            cls.dep.stop()
            raise
        cls.c = {name: cls.dep.client(name) for name, _ in PRINCIPALS}

    @classmethod
    def tearDownClass(cls):
        cls.dep.stop()

    def project(self, roles=None):
        slug = "membership-" + uuid.uuid4().hex
        response = lh.create(self.c["alice"], "Project", {"org": "1", "name": slug, "slug": slug})
        self.assertTrue(response.get("ok"), response)
        project_id = response["created"][0]["id"]
        memberships = {}
        for who, role in (roles or {"bob": "MEMBER", "carol": "VIEWER", "dan": "NO_ACCESS"}).items():
            user_id = dict(PRINCIPALS)[who]
            response = lh.create(self.c["alice"], "ProjectMembership",
                                 {"user": str(user_id), "project": project_id, "role": role})
            self.assertTrue(response.get("ok"), response)
            memberships[who] = response["created"][0]["id"]
        return project_id, memberships

    def stored(self, membership_id):
        return int(self.dep.sql(f"SELECT count(*) FROM :schema.project_membership WHERE id = {membership_id}"))

    def test_only_project_admin_can_remove_another_member(self):
        project, members = self.project()
        for who in ("bob", "carol", "dan", "eve", "mallory"):
            with self.subTest(who=who):
                denied = lh.transition(self.c[who], "ProjectMembership.remove", [members["bob"]])
                self.assertFalse(denied.get("ok"), denied)
                self.assertEqual(self.stored(members["bob"]), 1)
        removed = lh.transition(self.c["alice"], "ProjectMembership.remove", [members["bob"]])
        self.assertTrue(removed.get("ok"), removed)
        self.assertEqual(self.stored(members["bob"]), 0)
        self.assertEqual(self.stored(members["carol"]), 1)
        self.assertEqual(self.c["bob"].read({"read": "Project", "select": ["id"], "filter": eq("id", project)}), [])

    def test_project_admin_can_remove_a_peer_in_the_same_project(self):
        _, members = self.project({"bob": "ADMIN", "carol": "MEMBER"})
        removed = lh.transition(self.c["bob"], "ProjectMembership.remove", [members["carol"]])
        self.assertTrue(removed.get("ok"), removed)
        self.assertEqual(self.stored(members["carol"]), 0)
        self.assertEqual(self.stored(members["bob"]), 1)

    def test_remove_refuses_self_and_rolls_back_the_entire_bulk(self):
        _, members = self.project({"bob": "ADMIN", "carol": "VIEWER"})
        denied = lh.transition(self.c["bob"], "ProjectMembership.remove", [members["carol"], members["bob"]])
        self.assertFalse(denied.get("ok"), denied)
        self.assertEqual(self.stored(members["bob"]), 1)
        self.assertEqual(self.stored(members["carol"]), 1)

    def test_members_leave_their_own_membership_only(self):
        _, members = self.project()
        for who in ("alice", "carol", "eve", "mallory"):
            with self.subTest(who=who):
                denied = lh.transition(self.c[who], "ProjectMembership.leave", [members["bob"]])
                self.assertFalse(denied.get("ok"), denied)
                self.assertEqual(self.stored(members["bob"]), 1)
        left = lh.transition(self.c["bob"], "ProjectMembership.leave", [members["bob"]])
        self.assertTrue(left.get("ok"), left)
        self.assertEqual(self.stored(members["bob"]), 0)
        self.assertEqual(self.stored(members["carol"]), 1)

    def test_no_access_member_can_leave_without_listing_other_memberships(self):
        project, members = self.project()
        own = self.c["dan"].read({"read": "ProjectMembership", "select": ["id", "user", "role"],
                                  "filter": eq("user", "6")})
        self.assertIn({"id": members["dan"], "user": "6", "role": "NO_ACCESS"}, own)
        self.assertTrue(all(row["user"] == "6" for row in own), own)
        self.assertEqual(self.c["dan"].read({"read": "ProjectMembership", "select": ["id"],
                                            "filter": eq("user", "2")}), [])
        self.assertEqual(self.c["dan"].read({"read": "Project", "select": ["id"], "filter": eq("id", project)}), [])
        left = lh.transition(self.c["dan"], "ProjectMembership.leave", [members["dan"]])
        self.assertTrue(left.get("ok"), left)
        self.assertEqual(self.stored(members["dan"]), 0)
        self.assertEqual(self.stored(members["bob"]), 1)

    def test_secret_manager_admin_may_leave_without_another_direct_admin(self):
        project, members = self.project({"bob": "ADMIN", "carol": "MEMBER"})
        left = lh.transition(self.c["bob"], "ProjectMembership.leave", [members["bob"]])
        self.assertTrue(left.get("ok"), left)
        self.assertEqual(self.stored(members["bob"]), 0)
        self.assertEqual(self.c["bob"].read({"read": "Project", "select": ["id"], "filter": eq("id", project)}), [])
        self.assertEqual(self.stored(members["carol"]), 1)

    def test_removal_revokes_secret_access_but_preserves_private_rows(self):
        project, members = self.project()
        env = lh.create(self.c["alice"], "Environment", {"project": project, "slug": "dev", "name": "Dev", "position": 1})
        self.assertTrue(env.get("ok"), env)
        folder = lh.create(self.c["alice"], "Folder", {"env": env["created"][0]["id"], "name": "root"})
        self.assertTrue(folder.get("ok"), folder)
        folder_id = folder["created"][0]["id"]
        secret = lh.create(self.c["bob"], "Secret", {"folder": folder_id, "key": "REVOKED", "owner": "2"})
        self.assertTrue(secret.get("ok"), secret)
        secret_id = secret["created"][0]["id"]
        override = lh.create(self.c["bob"], "PersonalSecret", {"secret": secret_id, "user": "2"})
        self.assertTrue(override.get("ok"), override)
        override_id = override["created"][0]["id"]
        self.assertEqual(len(self.c["bob"].read({"read": "Secret", "select": ["id"], "filter": eq("id", secret_id)})), 1)

        removed = lh.transition(self.c["alice"], "ProjectMembership.remove", [members["bob"]])
        self.assertTrue(removed.get("ok"), removed)
        # Use the same SDK client: a previously successful read must not keep authorization alive.
        self.assertEqual(self.c["bob"].read({"read": "Secret", "select": ["id"], "filter": eq("id", secret_id)}), [])
        self.assertEqual(self.c["bob"].read({"read": "PersonalSecret", "select": ["id"], "filter": eq("id", override_id)}), [])
        self.assertFalse(lh.create(self.c["bob"], "Secret", {"folder": folder_id, "key": "DENIED", "owner": "2"}).get("ok"))
        self.assertEqual(self.dep.sql(f"SELECT count(*) FROM :schema.personal_secret WHERE id = {override_id}").strip(), "1")
        rejoined = lh.create(self.c["alice"], "ProjectMembership", {"user": "2", "project": project, "role": "MEMBER"})
        self.assertTrue(rejoined.get("ok"), rejoined)
        self.assertEqual(self.c["bob"].read({"read": "PersonalSecret", "select": ["id"], "filter": eq("id", override_id)}),
                         [{"id": override_id}])

    @unittest.skipUnless(sys.platform == "linux", "isolated worker execution is verified on Linux CI")
    def test_removal_revokes_job_enqueue_after_a_successful_job(self):
        project, members = self.project()
        env = lh.create(self.c["alice"], "Environment", {"project": project, "slug": "dev", "name": "Dev", "position": 1})
        self.assertTrue(env.get("ok"), env)
        folder = lh.create(self.c["alice"], "Folder", {"env": env["created"][0]["id"], "name": "root"})
        self.assertTrue(folder.get("ok"), folder)
        secret = lh.create(self.c["bob"], "Secret", {"folder": folder["created"][0]["id"], "key": "JOB_REVOKED", "owner": "2"})
        self.assertTrue(secret.get("ok"), secret)
        secret_id = secret["created"][0]["id"]
        receipt = lh.job_receipt(self.c["bob"], "secretReminder", {"secret": secret_id}, "before-removal")
        self.assertEqual(receipt["state"], "SUCCEEDED", receipt)
        removed = lh.transition(self.c["alice"], "ProjectMembership.remove", [members["bob"]])
        self.assertTrue(removed.get("ok"), removed)
        self.assertEqual(lh.error_code(self.c["bob"].jobs.enqueue, "secretReminder", {"secret": secret_id}, key="removed-job"),
                         "WIRE.ACCESS_DENIED")

    def test_bulk_remove_is_atomic_across_projects(self):
        _, members = self.project()
        foreign = lh.create(self.c["eve"], "ProjectMembership", {"user": "1", "project": "2", "role": "VIEWER"})
        self.assertTrue(foreign.get("ok"), foreign)
        foreign_id = foreign["created"][0]["id"]
        denied = lh.transition(self.c["alice"], "ProjectMembership.remove", [members["bob"], foreign_id])
        self.assertFalse(denied.get("ok"), denied)
        self.assertEqual(self.stored(members["bob"]), 1)
        self.assertEqual(self.stored(foreign_id), 1)
        removed = lh.transition(self.c["alice"], "ProjectMembership.remove", [members["bob"], members["carol"]])
        self.assertTrue(removed.get("ok"), removed)
        self.assertEqual(self.stored(members["bob"]), 0)
        self.assertEqual(self.stored(members["carol"]), 0)
        self.assertEqual(self.stored(members["dan"]), 1)

    def test_remove_and_leave_keep_audit_before_images_and_idempotent_receipts(self):
        _, members = self.project()
        for who, transition, target in (("alice", "remove", members["bob"]), ("carol", "leave", members["carol"])):
            key = "membership-" + transition
            first = lh.transition(self.c[who], "ProjectMembership." + transition, [target], key=key)
            self.assertTrue(first.get("ok"), first)
            replay = lh.transition(self.c[who], "ProjectMembership." + transition, [target], key=key)
            self.assertTrue(replay.get("ok"), replay)
            self.assertEqual(self.stored(target), 0)
            listing = self.dep.cli("audit", "--config", str(self.dep.work / "service.json"), "list",
                                   "--resource", "ProjectMembership", "--target-id", target)
            rows = [row for row in listing["audit"] if row["requestKey"] == key]
            self.assertEqual(len(rows), 1, listing)
            self.assertEqual(rows[0]["transition"], transition)
            self.assertEqual(rows[0]["before"]["user"], 2 if who == "alice" else 3)
            self.assertIsNone(rows[0]["after"])
