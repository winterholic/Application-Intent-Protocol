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
SEED = pathlib.Path(os.environ.get("INFISICAL_SEED_SQL", PORT / "seed.sql"))
PRINCIPALS = [("alice", 1), ("bob", 2), ("carol", 3), ("mallory", 4), ("eve", 5), ("dan", 6)]
ALICE, BOB, CAROL, MALLORY, EVE, DAN = "alice", "bob", "carol", "mallory", "eve", "dan"


def eq(field, value):
    return [{"field": field, "op": "eq", "value": value}]


class LiveDbTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.dep = lh.Deployment(APP, SEED, PRINCIPALS, workers=True).start()
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
        folder = lh.create(self.c[CAROL], "Folder", {"env": "1", "parent": "1", "name": "viewer"})
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
        folder = lh.create(self.c[BOB], "Folder", {"env": "1", "parent": "1", "name": "member-made"})
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
        folder = lh.create(self.c[BOB], "Folder", {"env": "1", "parent": "1", "name": "other"})
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

    # -- environments (upstream project_environments + the root folder created with each, project-env-service.ts) ------
    def make_env(self, who, slug, position, project="1", name=None, key=None):
        return lh.create(self.c[who], "Environment",
                         {"project": project, "slug": slug, "name": name or slug.upper(), "position": position}, key=key)

    def make_root(self, who, env_id, key=None):
        return lh.create(self.c[who], "Folder", {"env": env_id, "name": "root"}, key=key)

    def env_with_root(self, who, slug, position, project="1"):
        env_id = self.make_env(who, slug, position, project=project)["created"][0]["id"]
        return env_id, self.make_root(who, env_id)["created"][0]["id"]

    def new_environment_with_root(self, who, project_slug, env_slug):
        project = lh.create(self.c[who], "Project", {"org": "1", "name": project_slug, "slug": project_slug})
        self.assertTrue(project.get("ok"), project)
        return self.env_with_root(who, env_slug, 1, project=project["created"][0]["id"])

    def env_row(self, env_id):
        row = self.dep.sql("SELECT slug || '|' || name || '|' || position || '|' || project_id || '|' || (soft_deleted_at IS NOT NULL)"
                           " || '|' || coalesce(deleted_by_id::text, '<null>') FROM :schema.environment "
                           f"WHERE id = {env_id}").strip()
        return row.split("|") if row else None

    def env_children(self, env_id):
        folders = int(self.dep.sql(f"SELECT count(*) FROM :schema.folder WHERE env_id = {env_id}").strip())
        secrets = int(self.dep.sql("SELECT count(*) FROM :schema.secret s JOIN :schema.folder f ON f.id = s.folder_id "
                                   f"WHERE f.env_id = {env_id}").strip())
        return folders, secrets

    def env_reads(self, who, **filters):
        flt = [{"field": k, "op": "eq", "value": v} for k, v in filters.items()]
        return self.c[who].read({"read": "Environment", "select": ["id", "slug", "position", "softDeletedAt"], "filter": flt})

    def test_environment_is_created_by_a_project_writer_with_a_unique_slug_and_position(self):
        made = self.make_env(BOB, "staging", 10)
        self.assertTrue(made.get("ok"), made)
        env_id = made["created"][0]["id"]
        self.assertEqual(self.env_row(env_id), ["staging", "STAGING", "10", "1", "false", "<null>"])
        self.assertEqual(self.make_env(CAROL, "viewer-env", 11).get("code"), "WIRE.FORBIDDEN", "a viewer cannot create")
        self.assertEqual(self.make_env(DAN, "dan-env", 12).get("code"), "WIRE.MISSING_TARGET", "no project access")
        self.assertEqual(self.make_env(EVE, "eve-env", 13).get("code"), "WIRE.MISSING_TARGET", "another organization")
        same_slug = self.make_env(BOB, "staging", 14, key="env-dup-slug")
        self.assertEqual(same_slug.get("code"), "WIRE.ALREADY_EXISTS", same_slug)
        same_position = self.make_env(BOB, "other-staging", 10, key="env-dup-position")
        self.assertEqual(same_position.get("code"), "WIRE.ALREADY_EXISTS", same_position)
        for slug in ("viewer-env", "dan-env", "eve-env", "other-staging"):
            self.assertEqual(int(self.dep.sql(f"SELECT count(*) FROM :schema.environment WHERE slug = '{slug}'").strip()), 0, slug)
        project_two = self.make_env(EVE, "staging", 10, project="2")
        self.assertTrue(project_two.get("ok"), "the same slug and position are fine in another project")

    def test_environment_field_limits(self):
        for slug, position, name in (("", 20, "x"), ("s" * 65, 21, "x"), ("ok-slug", 0, "x"), ("ok-slug", 22, ""), ("ok-slug", 23, "n" * 256)):
            made = lh.create(self.c[BOB], "Environment", {"project": "1", "slug": slug, "name": name, "position": position})
            self.assertEqual(made.get("code"), "WIRE.BAD_VALUE", (slug[:8], position, name[:8], made))
        edge = lh.create(self.c[BOB], "Environment", {"project": "1", "slug": "s" * 64, "name": "n" * 255, "position": 24})
        self.assertTrue(edge.get("ok"), "64 and 255 characters are the upper bounds")

    def test_environment_is_read_by_project_readers_only(self):
        seen = lambda who: sorted((r["slug"], r["position"]) for r in self.env_reads(who, project="1") if r["position"] < 10)
        self.assertEqual(seen(CAROL), [("dev", 1), ("prod", 2)])
        self.assertEqual(seen(BOB), [("dev", 1), ("prod", 2)])
        self.assertEqual(self.env_reads(DAN), [], "no project access")
        self.assertEqual([r["slug"] for r in self.env_reads(EVE, project="1")], [], "another organization")
        self.assertEqual([r["id"] for r in self.env_reads(EVE, project="2") if r["position"] < 10], ["3"])

    # upstream: the first folder of an environment is its root (parentId null, name "root"), created with the environment
    def test_the_root_folder_of_an_environment_is_created_once_by_a_writer(self):
        env_id = self.make_env(BOB, "rooted", 30)["created"][0]["id"]
        self.assertEqual(self.make_root(CAROL, env_id).get("code"), "WIRE.FORBIDDEN", "a viewer cannot create the root")
        not_root = lh.create(self.c[BOB], "Folder", {"env": env_id, "name": "NOT_ROOT_NAME"})
        self.assertEqual(not_root.get("code"), "WIRE.FORBIDDEN", not_root)
        root = self.make_root(BOB, env_id)
        self.assertTrue(root.get("ok"), root)
        again = self.make_root(BOB, env_id, key="second-root")
        self.assertEqual(again.get("code"), "WIRE.FORBIDDEN", "an environment has one root")
        self.assertEqual(self.env_children(env_id), (1, 0))
        child = self.make_folder(BOB, "UNDER_NEW_ROOT", parent=root["created"][0]["id"], env=env_id)
        self.assertTrue(child.get("ok"), child)
        self.assertTrue(self.create_secret(BOB, "IN_NEW_ENV", folder=child["created"][0]["id"]).get("ok"))

    def test_environment_update_changes_name_slug_and_position_for_a_writer_only(self):
        a = self.make_env(BOB, "upd-a", 40)["created"][0]["id"]
        b = self.make_env(BOB, "upd-b", 41)["created"][0]["id"]
        denied = lh.transition(self.c[CAROL], "Environment.update", [a], values={"slug": "viewer-slug"})
        self.assertEqual(denied.get("code"), "WIRE.FORBIDDEN", denied)
        done = lh.transition(self.c[BOB], "Environment.update", [a], values={"name": "Renamed", "slug": "upd-a2", "position": 42}, key="env-update")
        self.assertTrue(done.get("ok"), done)
        self.assertEqual(self.env_row(a), ["upd-a2", "Renamed", "42", "1", "false", "<null>"])
        only_name = lh.transition(self.c[BOB], "Environment.update", [a], values={"name": "Only the name"})
        self.assertTrue(only_name.get("ok"), only_name)
        self.assertEqual(self.env_row(a)[:3], ["upd-a2", "Only the name", "42"], "omitted fields keep their value")
        taken_slug = lh.transition(self.c[BOB], "Environment.update", [a], values={"slug": "upd-b"}, key="env-taken-slug")
        self.assertEqual(taken_slug.get("code"), "WIRE.ALREADY_EXISTS", taken_slug)
        # upstream shifts the others when the position is taken; the port refuses (INFISICAL-GAPS G16)
        taken_position = lh.transition(self.c[BOB], "Environment.update", [a], values={"position": 41}, key="env-taken-pos")
        self.assertEqual(taken_position.get("code"), "WIRE.ALREADY_EXISTS", taken_position)
        self.assertEqual(self.env_row(b)[:3], ["upd-b", "UPD-B", "41"], "the neighbour is not shifted")
        self.assertEqual(self.env_row(a)[:3], ["upd-a2", "Only the name", "42"])
        self.assertEqual(lh.transition(self.c[EVE], "Environment.update", [a], values={"name": "x"}).get("code"), "WIRE.MISSING_TARGET")

    def test_environment_update_cannot_move_it_to_another_project(self):
        env_id = self.make_env(BOB, "stays-here", 43)["created"][0]["id"]
        other = lh.create(self.c[ALICE], "Project", {"org": "1", "name": "Env move target", "slug": "env-move-target"})
        target = other["created"][0]["id"]  # alice may write both projects, so only the patch list can stop the move
        refused = lh.transition(self.c[ALICE], "Environment.update", [env_id], values={"name": "x", "project": target})
        self.assertFalse(refused.get("ok"), refused)
        self.assertEqual(self.env_row(env_id)[3], "1")

    def test_environment_update_leaves_a_before_and_after_image_in_the_audit_log(self):
        env_id = self.make_env(BOB, "audit-env", 44)["created"][0]["id"]
        self.assertTrue(lh.transition(self.c[BOB], "Environment.update", [env_id], values={"name": "Audited", "slug": "audit-env2", "position": 45}, key="env-audit").get("ok"))
        listing = self.dep.cli("audit", "--config", str(self.dep.work / "service.json"), "list", "--resource", "Environment", "--target-id", env_id)
        rows = [r for r in listing["audit"] if r["requestKey"] == "env-audit"]
        self.assertEqual(len(rows), 1, listing)
        self.assertEqual((rows[0]["transition"], rows[0]["actorId"]), ("update", 2))
        self.assertEqual(rows[0]["before"], {"slug": "audit-env", "name": "AUDIT-ENV", "position": 44})
        self.assertEqual(rows[0]["after"], {"slug": "audit-env2", "name": "Audited", "position": 45})

    # upstream deleteEnvironment without hardDelete: mark the row, hide it, keep its slug reserved (project-env-service.ts:311-345)
    def test_soft_delete_hides_the_environment_and_everything_below_it_from_readers(self):
        env_id, root = self.env_with_root(BOB, "soft-me", 50)
        folder = self.make_folder(BOB, "SOFT_CHILD", parent=root, env=env_id)["created"][0]["id"]
        self.assertTrue(self.create_secret(BOB, "SOFT_SECRET", folder=folder).get("ok"))
        denied = lh.transition(self.c[CAROL], "Environment.softDelete", [env_id])
        self.assertEqual(denied.get("code"), "WIRE.FORBIDDEN", denied)
        done = lh.transition(self.c[BOB], "Environment.softDelete", [env_id], key="soft-me")
        self.assertTrue(done.get("ok"), done)
        self.assertEqual(self.env_row(env_id)[4:], ["true", "2"], "marked, with the deleting user")
        self.assertEqual(self.env_reads(CAROL, id=env_id), [], "a reader no longer sees it")
        self.assertEqual(len(self.env_reads(BOB, id=env_id)), 1, "a writer still sees it, to restore or purge it")
        self.assertIsNotNone(self.env_reads(BOB, id=env_id)[0]["softDeletedAt"])
        for who in (CAROL, BOB):
            self.assertEqual(self.c[who].read({"read": "Folder", "select": ["id"], "filter": eq("env", env_id)}), [], f"{who}: folders are hidden")
            self.assertEqual(self.secrets(who, "SOFT_SECRET"), [], f"{who}: secrets are hidden")
        self.assertEqual(self.env_children(env_id), (2, 1), "nothing is deleted")
        new_folder = self.make_folder(BOB, "SOFT_NEW", parent=root, env=env_id)
        self.assertEqual(new_folder.get("code"), "WIRE.MISSING_TARGET", new_folder)
        new_secret = self.create_secret(BOB, "SOFT_NEW_SECRET", folder=folder)
        self.assertEqual(new_secret.get("code"), "WIRE.MISSING_TARGET", new_secret)

    def test_soft_deleted_environment_keeps_its_slug_and_position_and_cannot_be_changed(self):
        env_id = self.make_env(BOB, "soft-held", 51)["created"][0]["id"]
        self.assertTrue(lh.transition(self.c[BOB], "Environment.softDelete", [env_id]).get("ok"))
        slug = self.make_env(BOB, "soft-held", 52, key="soft-slug")
        self.assertEqual(slug.get("code"), "WIRE.ALREADY_EXISTS", "upstream: the slug is held by a pending-deletion environment")
        position = self.make_env(BOB, "soft-other", 51, key="soft-position")
        self.assertEqual(position.get("code"), "WIRE.ALREADY_EXISTS", position)
        again = lh.transition(self.c[BOB], "Environment.softDelete", [env_id], key="soft-twice")
        self.assertEqual(again.get("code"), "WIRE.INVALID_STATE", again)
        update = lh.transition(self.c[BOB], "Environment.update", [env_id], values={"name": "Edited"}, key="soft-update")
        self.assertEqual(update.get("code"), "WIRE.INVALID_STATE", update)
        self.assertEqual(self.env_row(env_id)[:3], ["soft-held", "SOFT-HELD", "51"])
        late_root = self.make_root(BOB, env_id)
        self.assertEqual(late_root.get("code"), "WIRE.FORBIDDEN", "no folder is created in a soft-deleted environment")

    def test_restore_brings_back_the_environment_with_its_contents(self):
        env_id, root = self.env_with_root(BOB, "restore-me", 53)
        folder = self.make_folder(BOB, "RESTORED_CHILD", parent=root, env=env_id)["created"][0]["id"]
        self.assertTrue(self.create_secret(BOB, "RESTORED_SECRET", folder=folder).get("ok"))
        active = lh.transition(self.c[BOB], "Environment.restore", [env_id], key="restore-active")
        self.assertEqual(active.get("code"), "WIRE.INVALID_STATE", "only a soft-deleted environment is restored")
        self.assertTrue(lh.transition(self.c[BOB], "Environment.softDelete", [env_id]).get("ok"))
        viewer = lh.transition(self.c[CAROL], "Environment.restore", [env_id], key="restore-viewer")
        self.assertEqual(viewer.get("code"), "WIRE.MISSING_TARGET", "a reader cannot even see it")
        outsider = lh.transition(self.c[EVE], "Environment.restore", [env_id], key="restore-eve")
        self.assertEqual(outsider.get("code"), "WIRE.MISSING_TARGET", outsider)
        self.assertEqual(self.env_row(env_id)[4:], ["true", "2"])
        done = lh.transition(self.c[BOB], "Environment.restore", [env_id], key="restore-ok")
        self.assertTrue(done.get("ok"), done)
        self.assertEqual(self.env_row(env_id), ["restore-me", "RESTORE-ME", "53", "1", "false", "<null>"])
        self.assertEqual(len(self.env_reads(CAROL, id=env_id)), 1)
        self.assertEqual([r["key"] for r in self.secrets(CAROL, "RESTORED_SECRET")], ["RESTORED_SECRET"])
        self.assertTrue(lh.transition(self.c[BOB], "Environment.update", [env_id], values={"name": "Back"}).get("ok"))

    # upstream deleteEnvironment with hardDelete = true deletes the row and the database cascades (project-env-service.ts:298-309)
    def test_hard_delete_removes_the_environment_and_all_its_folders(self):
        env_id, root = self.env_with_root(BOB, "purge-me", 60)
        child = self.make_folder(BOB, "PURGE_CHILD", parent=root, env=env_id)["created"][0]["id"]
        self.make_folder(BOB, "PURGE_GRANDCHILD", parent=child, env=env_id)
        other_id, other_root = self.env_with_root(BOB, "purge-neighbour", 61)
        self.assertEqual(self.env_children(env_id), (3, 0))
        denied = lh.transition(self.c[CAROL], "Environment.hardDelete", [env_id])
        self.assertEqual(denied.get("code"), "WIRE.FORBIDDEN", denied)
        self.assertEqual(self.env_children(env_id), (3, 0))
        done = lh.transition(self.c[BOB], "Environment.hardDelete", [env_id], key="purge-me")
        self.assertTrue(done.get("ok"), done)
        self.assertIsNone(self.env_row(env_id))
        self.assertEqual(self.env_children(env_id), (0, 0))
        self.assertEqual(self.env_children(other_id), (1, 0), "the neighbour keeps its folders")
        # upstream closes the position gap (closePositionGap, :306); the port cannot, the neighbour keeps its position (G16)
        self.assertEqual(self.env_row(other_id)[2], "61")
        again = self.make_env(BOB, "purge-me", 60)
        self.assertTrue(again.get("ok"), "slug and position are free again")

    def test_hard_delete_works_on_a_soft_deleted_environment_too(self):
        env_id, root = self.env_with_root(BOB, "purge-soft", 62)
        self.assertTrue(lh.transition(self.c[BOB], "Environment.softDelete", [env_id]).get("ok"))
        self.assertEqual(lh.transition(self.c[CAROL], "Environment.hardDelete", [env_id]).get("code"), "WIRE.MISSING_TARGET")
        done = lh.transition(self.c[BOB], "Environment.hardDelete", [env_id], key="purge-soft")
        self.assertTrue(done.get("ok"), done)
        self.assertEqual((self.env_row(env_id), self.env_children(env_id)), (None, (0, 0)))

    # secrets are two hops below the environment, so the delete effect cannot reach them (INFISICAL-GAPS G16)
    def test_hard_delete_refuses_an_environment_that_still_holds_a_secret(self):
        env_id, root = self.env_with_root(BOB, "purge-held", 63)
        deep = self.make_folder(BOB, "DEEP", parent=root, env=env_id)["created"][0]["id"]
        secret_id = self.create_secret(BOB, "KEEPS_ENV", folder=deep)["created"][0]["id"]
        refused = lh.transition(self.c[BOB], "Environment.hardDelete", [env_id], key="purge-held")
        self.assertEqual(refused.get("code"), "WIRE.INVALID_STATE", refused)
        self.assertEqual((self.env_row(env_id) is not None, self.env_children(env_id)), (True, (2, 1)))
        self.assertTrue(lh.transition(self.c[BOB], "Environment.softDelete", [env_id]).get("ok"))
        hidden = lh.transition(self.c[BOB], "Environment.hardDelete", [env_id], key="purge-held-soft")
        self.assertEqual(hidden.get("code"), "WIRE.INVALID_STATE", "secrets hidden by the soft delete still block the purge")
        self.assertEqual(self.env_children(env_id), (2, 1))
        self.assertTrue(lh.transition(self.c[BOB], "Environment.restore", [env_id]).get("ok"))
        self.assertTrue(lh.transition(self.c[BOB], "Secret.delete", [secret_id]).get("ok"))
        self.assertTrue(lh.transition(self.c[BOB], "Environment.hardDelete", [env_id], key="purge-held-empty").get("ok"))
        self.assertEqual(self.env_children(env_id), (0, 0))

    def test_hard_delete_refuses_more_folders_than_the_effect_cap_and_changes_nothing(self):
        env_id, root = self.env_with_root(BOB, "purge-big", 64)
        self.dep.sql("INSERT INTO :schema.folder (env_id, parent_id, name) "
                     f"SELECT {env_id}, {root}, 'BIG_' || g FROM generate_series(1, 1000) g")
        self.assertEqual(self.env_children(env_id), (1001, 0))
        refused = lh.transition(self.c[BOB], "Environment.hardDelete", [env_id], key="purge-big")
        self.assertEqual(refused.get("code"), "WIRE.EFFECT_TARGET_MISSING", refused)
        self.assertEqual((self.env_row(env_id) is not None, self.env_children(env_id)), (True, (1001, 0)))
        self.dep.sql(f"DELETE FROM :schema.folder WHERE env_id = {env_id} AND name = 'BIG_1000'")
        done = lh.transition(self.c[BOB], "Environment.hardDelete", [env_id], key="purge-big-1000")
        self.assertTrue(done.get("ok"), "exactly the cap still goes through")
        self.assertEqual(self.env_children(env_id), (0, 0))

    def test_hard_delete_audits_the_environment_and_each_folder(self):
        env_id, root = self.env_with_root(BOB, "purge-audit", 65)
        self.assertTrue(lh.transition(self.c[BOB], "Environment.hardDelete", [env_id], key="purge-audit").get("ok"))
        listing = self.dep.cli("audit", "--config", str(self.dep.work / "service.json"), "list", "--resource", "Environment", "--target-id", env_id)
        rows = [r for r in listing["audit"] if r["requestKey"] == "purge-audit"]
        self.assertEqual(len(rows), 1, listing)
        self.assertEqual((rows[0]["transition"], rows[0]["actorId"]), ("hardDelete", 2))
        self.assertEqual(rows[0]["before"]["slug"], "purge-audit")
        self.assertIsNone(rows[0]["after"])
        folder_listing = self.dep.cli("audit", "--config", str(self.dep.work / "service.json"), "list", "--resource", "Folder", "--target-id", root)
        folder_rows = [r for r in folder_listing["audit"] if r["requestKey"] == "purge-audit"]
        self.assertEqual([r["transition"] for r in folder_rows], ["hardDelete:delete"], folder_listing)

    # -- folders (upstream: secret_folders is a tree, root has parentId null; deleteFolder in secret-folder-service.ts) --
    def make_folder(self, who, name, parent="1", env="1", key=None):
        return lh.create(self.c[who], "Folder", {"env": env, "parent": parent, "name": name}, key=key)

    def folder_rows(self, folder_id):
        return int(self.dep.sql(f"SELECT count(*) FROM :schema.folder WHERE id = {folder_id}").strip())

    def secret_rows_in(self, folder_id):
        return int(self.dep.sql(f"SELECT count(*) FROM :schema.secret WHERE folder_id = {folder_id}").strip())

    def test_folder_must_hang_under_a_parent_of_the_same_environment(self):
        no_parent = lh.create(self.c[BOB], "Folder", {"env": "1", "name": "FAKE_ROOT"})
        self.assertEqual(no_parent.get("code"), "WIRE.FORBIDDEN", "a parentless folder must be the root of an environment without one")
        wrong_env = self.make_folder(BOB, "WRONG_ENV", parent="1", env="2")
        self.assertEqual(wrong_env.get("code"), "WIRE.FORBIDDEN", wrong_env)
        # a parent the caller cannot read does not exist for them (row policy)
        hidden = self.make_folder(BOB, "HIDDEN_PARENT", parent="3")
        self.assertEqual(hidden.get("code"), "WIRE.MISSING_TARGET", hidden)
        # a parent the caller can read but that belongs to another project's environment is refused by the create rule itself
        second_root = self.new_environment_with_root(ALICE, "second-folder-test", "dev")[1]
        foreign = lh.create(self.c[ALICE], "Folder", {"env": "1", "parent": second_root, "name": "FOREIGN_PARENT"})
        self.assertEqual(foreign.get("code"), "WIRE.FORBIDDEN", foreign)
        for name in ("FAKE_ROOT", "WRONG_ENV", "HIDDEN_PARENT", "FOREIGN_PARENT"):
            self.assertEqual(int(self.dep.sql(f"SELECT count(*) FROM :schema.folder WHERE name = '{name}'").strip()), 0, name)

    def test_folder_name_is_unique_under_one_parent_only(self):
        first = self.make_folder(BOB, "TREE_DUP")
        again = self.make_folder(BOB, "TREE_DUP", key="tree-dup-2")
        self.assertTrue(first.get("ok"), first)
        self.assertFalse(again.get("ok"), again)
        self.assertEqual(again.get("code"), "WIRE.ALREADY_EXISTS")
        nested = self.make_folder(BOB, "TREE_DUP", parent=first["created"][0]["id"])
        self.assertTrue(nested.get("ok"), "the same name is fine below another parent")

    def test_empty_folder_can_be_deleted_by_a_writer_and_not_by_a_viewer(self):
        folder_id = self.make_folder(BOB, "EMPTY_ME")["created"][0]["id"]
        denied = lh.transition(self.c[CAROL], "Folder.delete", [folder_id])
        self.assertEqual(denied.get("code"), "WIRE.FORBIDDEN", denied)
        self.assertEqual(self.folder_rows(folder_id), 1)
        deleted = lh.transition(self.c[BOB], "Folder.delete", [folder_id])
        self.assertTrue(deleted.get("ok"), deleted)
        self.assertEqual(self.folder_rows(folder_id), 0, "the row is physically gone")
        self.assertTrue(self.make_folder(BOB, "EMPTY_ME").get("ok"), "the name is free again")

    def test_root_folder_cannot_be_deleted(self):
        for transition in ("Folder.delete", "Folder.forceDelete"):
            refused = lh.transition(self.c[ALICE], transition, ["2"])
            self.assertEqual(refused.get("code"), "WIRE.INVALID_STATE", f"{transition}: {refused}")
        self.assertEqual(self.folder_rows("2"), 1)

    def test_plain_delete_refuses_a_folder_with_secrets_or_subfolders(self):
        with_secret = self.make_folder(BOB, "HAS_SECRET")["created"][0]["id"]
        self.assertTrue(self.create_secret(BOB, "INSIDE", folder=with_secret).get("ok"))
        refused = lh.transition(self.c[BOB], "Folder.delete", [with_secret])
        self.assertEqual(refused.get("code"), "WIRE.INVALID_STATE", refused)
        self.assertEqual((self.folder_rows(with_secret), self.secret_rows_in(with_secret)), (1, 1))
        with_child = self.make_folder(BOB, "HAS_CHILD")["created"][0]["id"]
        child = self.make_folder(BOB, "THE_CHILD", parent=with_child)["created"][0]["id"]
        refused = lh.transition(self.c[BOB], "Folder.delete", [with_child])
        self.assertEqual(refused.get("code"), "WIRE.INVALID_STATE", refused)
        self.assertEqual((self.folder_rows(with_child), self.folder_rows(child)), (1, 1))

    def test_force_delete_removes_the_folder_and_its_secrets_and_nothing_else(self):
        target = self.make_folder(BOB, "FORCE_ME")["created"][0]["id"]
        sibling = self.make_folder(BOB, "FORCE_SIBLING")["created"][0]["id"]
        for n in range(3):
            self.assertTrue(self.create_secret(BOB, f"F_{n}", folder=target).get("ok"))
        self.assertTrue(self.create_secret(BOB, "F_KEEP", folder=sibling).get("ok"))
        denied = lh.transition(self.c[CAROL], "Folder.forceDelete", [target])
        self.assertEqual(denied.get("code"), "WIRE.FORBIDDEN", denied)
        self.assertEqual((self.folder_rows(target), self.secret_rows_in(target)), (1, 3))
        done = lh.transition(self.c[BOB], "Folder.forceDelete", [target], key="force-me")
        self.assertTrue(done.get("ok"), done)
        self.assertEqual((self.folder_rows(target), self.secret_rows_in(target)), (0, 0))
        self.assertEqual((self.folder_rows(sibling), self.secret_rows_in(sibling)), (1, 1))

    def test_force_delete_audits_the_folder_removal(self):
        target = self.make_folder(BOB, "FORCE_AUDIT")["created"][0]["id"]
        self.assertTrue(lh.transition(self.c[BOB], "Folder.forceDelete", [target], key="force-audit").get("ok"))
        listing = self.dep.cli("audit", "--config", str(self.dep.work / "service.json"), "list",
                               "--resource", "Folder", "--target-id", target)
        rows = [r for r in listing["audit"] if r["requestKey"] == "force-audit"]
        self.assertEqual(len(rows), 1, listing)
        self.assertEqual((rows[0]["transition"], rows[0]["via"], rows[0]["actorId"]), ("forceDelete", "apply", 2))
        self.assertEqual(rows[0]["before"]["name"], "FORCE_AUDIT")
        self.assertIsNone(rows[0]["after"])

    # Upstream deletes a whole subtree through ON DELETE CASCADE on parentId and folderId. The product has no
    # two-hop delete effect, so a folder with subfolders is refused instead of being half-deleted (INFISICAL-GAPS G14).
    def test_force_delete_refuses_a_folder_that_still_has_subfolders(self):
        parent = self.make_folder(BOB, "FORCE_TREE")["created"][0]["id"]
        child = self.make_folder(BOB, "FORCE_TREE_CHILD", parent=parent)["created"][0]["id"]
        self.assertTrue(self.create_secret(BOB, "DEEP", folder=child).get("ok"))
        refused = lh.transition(self.c[BOB], "Folder.forceDelete", [parent])
        self.assertEqual(refused.get("code"), "WIRE.INVALID_STATE", refused)
        self.assertEqual((self.folder_rows(parent), self.folder_rows(child), self.secret_rows_in(child)), (1, 1, 1))
        # bottom-up still works
        self.assertTrue(lh.transition(self.c[BOB], "Folder.forceDelete", [child]).get("ok"))
        self.assertTrue(lh.transition(self.c[BOB], "Folder.delete", [parent]).get("ok"))
        self.assertEqual((self.folder_rows(parent), self.folder_rows(child)), (0, 0))

    # The delete effect is capped at 1000 rows per transition; a bigger folder is refused whole, never trimmed.
    def test_force_delete_refuses_a_folder_over_the_effect_cap_and_changes_nothing(self):
        target = self.make_folder(BOB, "FORCE_BIG")["created"][0]["id"]
        self.dep.sql("INSERT INTO :schema.secret (folder_id, key, owner_id) "
                     f"SELECT {target}, 'BIG_' || g, 2 FROM generate_series(1, 1001) g")
        refused = lh.transition(self.c[BOB], "Folder.forceDelete", [target])
        self.assertEqual(refused.get("code"), "WIRE.EFFECT_TARGET_MISSING", refused)
        self.assertEqual((self.folder_rows(target), self.secret_rows_in(target)), (1, 1001))
        self.dep.sql("DELETE FROM :schema.secret WHERE key = 'BIG_1001'")
        done = lh.transition(self.c[BOB], "Folder.forceDelete", [target], key="force-big-1000")
        self.assertTrue(done.get("ok"), "exactly the cap still goes through")
        self.assertEqual((self.folder_rows(target), self.secret_rows_in(target)), (0, 0))

    # -- folder update (upstream updateFolder in secret-folder-service.ts: name and description of a non-root folder) --
    def folder_row(self, folder_id):
        row = self.dep.sql("SELECT name || '|' || coalesce(description, '<null>') || '|' || coalesce(parent_id::text, '<null>')"
                           f" || '|' || env_id FROM :schema.folder WHERE id = {folder_id}").strip()
        return row.split("|")

    def test_folder_can_be_renamed_and_described_by_a_writer_only(self):
        folder_id = self.make_folder(BOB, "RENAME_ME")["created"][0]["id"]
        denied = lh.transition(self.c[CAROL], "Folder.update", [folder_id], values={"name": "VIEWER_NAME", "description": "x"})
        self.assertEqual(denied.get("code"), "WIRE.FORBIDDEN", denied)
        self.assertEqual(self.folder_row(folder_id), ["RENAME_ME", "<null>", "1", "1"])
        done = lh.transition(self.c[BOB], "Folder.update", [folder_id], values={"name": "RENAMED", "description": "kept apart"})
        self.assertTrue(done.get("ok"), done)
        self.assertEqual(self.folder_row(folder_id), ["RENAMED", "kept apart", "1", "1"], "parent, project and env are untouched")
        self.assertTrue(self.make_folder(BOB, "RENAME_ME").get("ok"), "the old name is free again")

    def test_folder_created_with_a_description_keeps_it(self):
        made = lh.create(self.c[BOB], "Folder", {"env": "1", "parent": "1", "name": "DESCRIBED", "description": "hello"})
        self.assertTrue(made.get("ok"), made)
        self.assertEqual(self.folder_row(made["created"][0]["id"])[:2], ["DESCRIBED", "hello"])

    def test_root_folder_cannot_be_renamed(self):
        refused = lh.transition(self.c[ALICE], "Folder.update", ["2"], values={"name": "NOT_ROOT", "description": "x"})
        self.assertEqual(refused.get("code"), "WIRE.INVALID_STATE", refused)
        self.assertEqual(self.folder_row("2"), ["root", "<null>", "<null>", "2"])

    def test_rename_to_a_sibling_name_is_refused_but_the_same_name_or_another_parent_is_fine(self):
        first = self.make_folder(BOB, "SIB_A")["created"][0]["id"]
        second = self.make_folder(BOB, "SIB_B")["created"][0]["id"]
        clash = lh.transition(self.c[BOB], "Folder.update", [second], values={"name": "SIB_A", "description": "x"}, key="rename-clash")
        self.assertEqual(clash.get("code"), "WIRE.ALREADY_EXISTS", clash)
        self.assertEqual(self.folder_row(second)[:2], ["SIB_B", "<null>"])
        same = lh.transition(self.c[BOB], "Folder.update", [second], values={"name": "SIB_B", "description": "only the text changed"})
        self.assertTrue(same.get("ok"), same)
        self.assertEqual(self.folder_row(second)[:2], ["SIB_B", "only the text changed"])
        nested = self.make_folder(BOB, "SIB_C", parent=first)["created"][0]["id"]
        moved_name = lh.transition(self.c[BOB], "Folder.update", [nested], values={"name": "SIB_B", "description": "x"})
        self.assertTrue(moved_name.get("ok"), "SIB_B exists only below the root, not below SIB_A")

    def test_rename_keeps_the_secrets_and_children_attached(self):
        folder_id = self.make_folder(BOB, "KEEP_TREE")["created"][0]["id"]
        child = self.make_folder(BOB, "KEEP_CHILD", parent=folder_id)["created"][0]["id"]
        self.assertTrue(self.create_secret(BOB, "KEPT", folder=folder_id).get("ok"))
        self.assertTrue(lh.transition(self.c[BOB], "Folder.update", [folder_id], values={"name": "KEPT_TREE", "description": "x"}).get("ok"))
        self.assertEqual(self.secret_rows_in(folder_id), 1)
        self.assertEqual(self.folder_row(child)[2], str(folder_id))

    # upstream updateFolder passes `description` straight to the update: undefined keeps the stored text, null clears it
    def test_update_without_a_description_keeps_it_and_null_clears_it(self):
        made = lh.create(self.c[BOB], "Folder", {"env": "1", "parent": "1", "name": "DESC_KEEP", "description": "keep me"})
        folder_id = made["created"][0]["id"]
        self.assertTrue(lh.transition(self.c[BOB], "Folder.update", [folder_id], values={"name": "DESC_KEEP_2"}).get("ok"))
        self.assertEqual(self.folder_row(folder_id)[:2], ["DESC_KEEP_2", "keep me"])
        self.assertTrue(lh.transition(self.c[BOB], "Folder.update", [folder_id], values={"name": "DESC_KEEP_2", "description": None}).get("ok"))
        self.assertEqual(self.folder_row(folder_id)[:2], ["DESC_KEEP_2", "<null>"])

    def test_update_cannot_move_a_folder_or_change_its_project_or_environment(self):
        target_parent = self.make_folder(BOB, "MOVE_TARGET")["created"][0]["id"]
        folder_id = self.make_folder(BOB, "STAYS_PUT")["created"][0]["id"]
        for extra in ({"parent": target_parent}, {"env": "2"}, {"project": "2"}):
            refused = lh.transition(self.c[BOB], "Folder.update", [folder_id], values={"name": "STAYS_PUT", "description": "x", **extra})
            self.assertFalse(refused.get("ok"), f"{extra}: {refused}")
            self.assertEqual(self.folder_row(folder_id), ["STAYS_PUT", "<null>", "1", "1"], extra)

    def test_folder_name_cannot_be_blank(self):
        folder_id = self.make_folder(BOB, "NOT_BLANK")["created"][0]["id"]
        blank = lh.transition(self.c[BOB], "Folder.update", [folder_id], values={"name": "", "description": "x"})
        self.assertEqual(blank.get("code"), "WIRE.BAD_VALUE", blank)
        too_long = lh.transition(self.c[BOB], "Folder.update", [folder_id], values={"name": "x" * 256, "description": "x"})
        self.assertEqual(too_long.get("code"), "WIRE.BAD_VALUE", too_long)
        self.assertEqual(self.folder_row(folder_id)[0], "NOT_BLANK")

    def test_folder_update_is_not_possible_from_another_project_or_organization(self):
        folder_id = self.make_folder(BOB, "FOREIGN_RENAME")["created"][0]["id"]
        refused = lh.transition(self.c[EVE], "Folder.update", [folder_id], values={"name": "TAKEN_OVER", "description": "x"})
        self.assertEqual(refused.get("code"), "WIRE.MISSING_TARGET", refused)
        self.assertEqual(self.folder_row(folder_id)[0], "FOREIGN_RENAME")

    def test_folder_update_leaves_a_before_and_after_image_in_the_audit_log(self):
        folder_id = self.make_folder(BOB, "AUDIT_RENAME")["created"][0]["id"]
        done = lh.transition(self.c[BOB], "Folder.update", [folder_id], values={"name": "AUDIT_RENAMED", "description": "d"}, key="audit-rename")
        self.assertTrue(done.get("ok"), done)
        listing = self.dep.cli("audit", "--config", str(self.dep.work / "service.json"), "list",
                               "--resource", "Folder", "--target-id", folder_id)
        rows = [r for r in listing["audit"] if r["requestKey"] == "audit-rename"]
        self.assertEqual(len(rows), 1, listing)
        self.assertEqual((rows[0]["transition"], rows[0]["via"], rows[0]["actorId"]), ("update", "apply", 2))
        self.assertEqual(rows[0]["before"], {"name": "AUDIT_RENAME", "description": None})
        self.assertEqual(rows[0]["after"], {"name": "AUDIT_RENAMED", "description": "d"})

    # -- personal overrides (upstream secrets_v2 rows with type = personal and userId = the owner) ----------------------
    def override(self, who, secret_id, user=None, key=None):
        user = user or {"alice": "1", "bob": "2", "carol": "3", "dan": "6", "eve": "5", "mallory": "4"}[who]
        return lh.create(self.c[who], "PersonalSecret", {"secret": secret_id, "user": user}, key=key)

    def override_rows(self, secret_id):
        return int(self.dep.sql(f"SELECT count(*) FROM :schema.personal_secret WHERE secret_id = {secret_id}").strip())

    def shared_secret(self, key, folder="1"):
        return self.create_secret(BOB, key, folder=folder)["created"][0]["id"]

    def test_override_needs_an_existing_shared_secret_and_its_own_user(self):
        secret_id = self.shared_secret("OVR_BASE")
        missing = self.override(BOB, "999999")
        self.assertEqual(missing.get("code"), "WIRE.MISSING_TARGET", missing)
        for_someone_else = self.override(BOB, secret_id, user="3")
        self.assertEqual(for_someone_else.get("code"), "WIRE.FORBIDDEN", for_someone_else)
        self.assertEqual(self.override_rows(secret_id), 0)
        self.assertTrue(self.override(BOB, secret_id).get("ok"))
        self.assertEqual(self.override_rows(secret_id), 1)

    # upstream needs only describe/read access for a personal override, so a viewer may have one; no access or another
    # organization may not (secret-v2-bridge-service.ts:420-424)
    def test_override_follows_read_access_not_write_access(self):
        secret_id = self.shared_secret("OVR_READERS")
        viewer = self.override(CAROL, secret_id)
        self.assertTrue(viewer.get("ok"), viewer)
        self.assertEqual(self.override(DAN, secret_id).get("code"), "WIRE.MISSING_TARGET")
        self.assertEqual(self.override(EVE, secret_id).get("code"), "WIRE.MISSING_TARGET")
        self.assertEqual(self.override_rows(secret_id), 1)

    def test_one_override_per_user_and_secret(self):
        secret_id = self.shared_secret("OVR_ONCE")
        self.assertTrue(self.override(BOB, secret_id).get("ok"))
        again = self.override(BOB, secret_id, key="ovr-once-2")
        self.assertEqual(again.get("code"), "WIRE.ALREADY_EXISTS", again)
        self.assertTrue(self.override(CAROL, secret_id).get("ok"), "another user may have their own")
        self.assertEqual(self.override_rows(secret_id), 2)

    def test_soft_deleted_environment_hides_personal_overrides_and_restore_keeps_them(self):
        env_id, root = self.env_with_root(BOB, "override-soft-hidden", 90)
        secret_id = self.shared_secret("OVR_SOFT_HIDDEN", folder=root)
        bob = self.override(BOB, secret_id)
        carol = self.override(CAROL, secret_id)
        self.assertTrue(bob.get("ok"), bob)
        self.assertTrue(carol.get("ok"), carol)
        ids = {BOB: bob["created"][0]["id"], CAROL: carol["created"][0]["id"]}

        def own_rows(who):
            # Read by the override's own id, without filtering on a hidden Secret reference.
            # A reference filter could hide a leaked row while a plain read still returned it.
            return self.c[who].read({"read": "PersonalSecret", "select": ["id", "secret", "user"],
                                     "filter": eq("id", ids[who])})

        for who in (BOB, CAROL):
            self.assertEqual([r["id"] for r in own_rows(who)], [ids[who]])
        self.assertTrue(lh.transition(self.c[BOB], "Environment.softDelete", [env_id]).get("ok"))
        for who in (BOB, CAROL):
            self.assertEqual(own_rows(who), [], f"{who}: the override itself must be hidden")
            hidden_delete = lh.transition(self.c[who], "PersonalSecret.delete", [ids[who]])
            self.assertEqual(hidden_delete.get("code"), "WIRE.MISSING_TARGET", hidden_delete)
        hidden_create = self.override(ALICE, secret_id)
        self.assertEqual(hidden_create.get("code"), "WIRE.MISSING_TARGET", hidden_create)
        self.assertEqual(self.override_rows(secret_id), 2, "hidden rows remain stored")
        self.assertTrue(lh.transition(self.c[BOB], "Environment.restore", [env_id]).get("ok"))
        for who in (BOB, CAROL):
            self.assertEqual([r["id"] for r in own_rows(who)], [ids[who]])
            self.assertEqual(own_rows(who)[0]["secret"], secret_id)
        self.assertEqual(self.override_rows(secret_id), 2)

    # upstream lists rows whose userId is null or the caller (secret-v2-bridge-dal.ts:381-413)
    def test_overrides_are_visible_to_their_owner_only(self):
        secret_id = self.shared_secret("OVR_PRIVATE")
        bob_row = self.override(BOB, secret_id)["created"][0]["id"]
        self.assertTrue(self.override(CAROL, secret_id).get("ok"))
        read = lambda who: [r["id"] for r in self.c[who].read({"read": "PersonalSecret", "select": ["id"], "filter": eq("secret", secret_id)})]
        self.assertEqual(read(BOB), [bob_row])
        self.assertEqual(len(read(CAROL)), 1)
        self.assertNotEqual(read(CAROL), [bob_row])
        self.assertEqual(read(ALICE), [], "not even an organization admin sees someone else's override")
        self.assertEqual(read(DAN), [])
        self.assertEqual(self.override_rows(secret_id), 2)

    def test_an_override_can_be_deleted_by_its_owner_only(self):
        secret_id = self.shared_secret("OVR_DELETE")
        bob_row = self.override(BOB, secret_id)["created"][0]["id"]
        carol_row = self.override(CAROL, secret_id)["created"][0]["id"]
        stolen = lh.transition(self.c[CAROL], "PersonalSecret.delete", [bob_row])
        self.assertEqual(stolen.get("code"), "WIRE.MISSING_TARGET", stolen)
        admin = lh.transition(self.c[ALICE], "PersonalSecret.delete", [bob_row])
        self.assertEqual(admin.get("code"), "WIRE.MISSING_TARGET", admin)
        self.assertEqual(self.override_rows(secret_id), 2)
        self.assertTrue(lh.transition(self.c[BOB], "PersonalSecret.delete", [bob_row]).get("ok"))
        self.assertEqual(self.override_rows(secret_id), 1)
        self.assertEqual(len(self.c[CAROL].read({"read": "PersonalSecret", "select": ["id"], "filter": eq("id", carol_row)})), 1)
        self.assertEqual(self.secrets(BOB, "OVR_DELETE")[0]["key"], "OVR_DELETE", "the shared secret stays")

    # upstream deleteMany also deletes every user's personal row of a deleted shared secret (dal :364-370)
    def test_deleting_the_shared_secret_deletes_every_users_override(self):
        first, second = self.shared_secret("OVR_CASCADE_1"), self.shared_secret("OVR_CASCADE_2")
        keep = self.shared_secret("OVR_CASCADE_KEEP")
        for sid in (first, second, keep):
            self.assertTrue(self.override(BOB, sid).get("ok"))
            self.assertTrue(self.override(CAROL, sid).get("ok"))
        done = lh.transition(self.c[BOB], "Secret.delete", [first, second], key="ovr-cascade")
        self.assertTrue(done.get("ok"), done)
        self.assertEqual((self.override_rows(first), self.override_rows(second), self.override_rows(keep)), (0, 0, 2))
        self.assertEqual(self.rows_named("OVR_CASCADE_1"), 0)
        again = self.create_secret(BOB, "OVR_CASCADE_1")
        self.assertTrue(again.get("ok"), again)
        self.assertEqual(self.override_rows(again["created"][0]["id"]), 0, "a recreated name starts without overrides")

    def test_secret_delete_audits_the_removed_overrides(self):
        secret_id = self.shared_secret("OVR_AUDIT")
        override_id = self.override(CAROL, secret_id)["created"][0]["id"]
        self.assertTrue(lh.transition(self.c[BOB], "Secret.delete", [secret_id], key="ovr-audit").get("ok"))
        listing = self.dep.cli("audit", "--config", str(self.dep.work / "service.json"), "list",
                               "--resource", "PersonalSecret", "--target-id", override_id)
        rows = [r for r in listing["audit"] if r["requestKey"] == "ovr-audit"]
        self.assertEqual(len(rows), 1, listing)
        self.assertEqual((rows[0]["transition"], rows[0]["actorId"]), ("delete:delete", 2))
        self.assertEqual(rows[0]["before"]["user"], 3)
        self.assertIsNone(rows[0]["after"])

    # the delete effect is capped at 1000 rows; more overrides than that make the delete refuse as a whole
    def test_secret_delete_refuses_more_overrides_than_the_effect_cap_and_changes_nothing(self):
        secret_id = self.shared_secret("OVR_BIG")
        self.dep.sql("INSERT INTO :schema.\"user\" (email) SELECT 'ovr-big-' || g || '@acme.test' FROM generate_series(1, 1001) g")
        self.dep.sql("INSERT INTO :schema.personal_secret (secret_id, user_id) "
                     f"SELECT {secret_id}, id FROM :schema.\"user\" WHERE email LIKE 'ovr-big-%'")
        refused = lh.transition(self.c[BOB], "Secret.delete", [secret_id])
        self.assertEqual(refused.get("code"), "WIRE.EFFECT_TARGET_MISSING", refused)
        self.assertEqual((self.rows_named("OVR_BIG"), self.override_rows(secret_id)), (1, 1001))
        self.dep.sql("DELETE FROM :schema.personal_secret WHERE secret_id = %s AND user_id = (SELECT min(id) FROM :schema.\"user\" WHERE email LIKE 'ovr-big-%%')" % secret_id)
        done = lh.transition(self.c[BOB], "Secret.delete", [secret_id], key="ovr-big-1000")
        self.assertTrue(done.get("ok"), done)
        self.assertEqual((self.rows_named("OVR_BIG"), self.override_rows(secret_id)), (0, 0))

    # Folder.forceDelete removes secrets with a plain delete effect, which cannot also remove their overrides
    # (INFISICAL-GAPS G15). It refuses instead of failing on the foreign key.
    def test_force_delete_refuses_a_folder_whose_secrets_still_have_overrides(self):
        folder_id = self.make_folder(BOB, "OVR_FORCE")["created"][0]["id"]
        secret_id = self.create_secret(BOB, "OVR_IN_FOLDER", folder=folder_id)["created"][0]["id"]
        override_id = self.override(CAROL, secret_id)["created"][0]["id"]
        refused = lh.transition(self.c[BOB], "Folder.forceDelete", [folder_id])
        self.assertEqual(refused.get("code"), "WIRE.INVALID_STATE", refused)
        self.assertEqual((self.folder_rows(folder_id), self.secret_rows_in(folder_id), self.override_rows(secret_id)), (1, 1, 1))
        self.assertTrue(lh.transition(self.c[CAROL], "PersonalSecret.delete", [override_id]).get("ok"))
        self.assertTrue(lh.transition(self.c[BOB], "Folder.forceDelete", [folder_id]).get("ok"))
        self.assertEqual((self.folder_rows(folder_id), self.secret_rows_in(folder_id)), (0, 0))

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
