"""Proves test_live_db.py is not vacuous: each weakening of app.aip must make a named test fail.

  AIP_TEST_DATABASE_URL=postgresql://... python3 porting/infisical/tests/mutation_check.py

Prints one line per mutation and exits non-zero if any mutation survives (all tests still pass).
"""

import os
import pathlib
import re
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
APP = HERE.parent / "app.aip"
BINARY = os.environ.get("AIP_BIN", str(HERE.parent.parent.parent / "target" / "debug" / "aip"))

# (label, text to find, replacement, test that must fail)
MUTATIONS = [
    ("soft-deleted overrides visible", "rows read when user = actor and canReadProject(actor, secret.folder.env.project) and secret.folder.env.softDeletedAt = null",
     "rows read when user = actor and canReadProject(actor, secret.folder.env.project)",
     "test_soft_deleted_environment_hides_personal_overrides_and_restore_keeps_them"),
    ("viewer may write", "role in (ADMIN, MEMBER))\npredicate isProjectAdmin", "role in (ADMIN, MEMBER, VIEWER))\npredicate isProjectAdmin",
     "test_viewer_cannot_create_folder_or_secret"),
    ("every user visible", "rows read when this = actor or sharesOrgWith(actor, this)", "rows read when true",
     "test_non_member_sees_only_themselves"),
    ("duplicate secret names", "  unique folder, key\n", "",
     "test_secret_name_is_unique_in_a_folder_including_the_root"),
    ("org created in another name", "allow actor != null and createdBy = actor", "allow actor != null",
     "test_organization_cannot_be_created_in_someone_elses_name"),
    ("job open to any user", " and exists Secret where id = input.secret and canReadProject(actor, folder.env.project) and folder.env.softDeletedAt = null", "",
     "test_reminder_job_rejects_callers_without_project_access"),
    ("reminder enqueue ignores soft delete", " and canReadProject(actor, folder.env.project) and folder.env.softDeletedAt = null",
     " and canReadProject(actor, folder.env.project)",
     "test_reminder_job_rejects_a_soft_deleted_environment_before_enqueue"),
    ("outsider joins project", " and isOrgMember(user, project.org)", "",
     "test_project_membership_rules"),
    ("viewer may delete", "transition delete {\n    allow canWriteProject(actor, folder.env.project)", "transition delete {\n    allow canReadProject(actor, folder.env.project)",
     "test_viewer_can_read_but_not_delete"),
    ("secret owner is free", "allow canWriteProject(actor, folder.env.project) and owner = actor", "allow canWriteProject(actor, folder.env.project)",
     "test_secret_cannot_be_created_in_someone_elses_name"),
    ("org member reads any project", "rows read when canReadProject(actor, this)", "rows read when true",
     "test_no_access_role_reads_nothing"),
    ("plain folder delete ignores contents", "from parent != null and folderIsEmpty(this)", "from parent != null",
     "test_plain_delete_refuses_a_folder_with_secrets_or_subfolders"),
    ("root folder deletable", "from parent != null and folderIsEmpty(this)", "from folderIsEmpty(this)",
     "test_root_folder_cannot_be_deleted"),
    ("force delete over subfolders", "from parent != null and not (hasSubfolder(this))", "from parent != null",
     "test_force_delete_refuses_a_folder_that_still_has_subfolders"),
    ("force delete keeps secrets", "    delete many Secret maxRows 1000 where folder = this.id\n", "",
     "test_force_delete_removes_the_folder_and_its_secrets_and_nothing_else"),
    ("viewer may force delete", "transition forceDelete {\n    allow canWriteProject(actor, env.project)", "transition forceDelete {\n    allow canReadProject(actor, env.project)",
     "test_force_delete_removes_the_folder_and_its_secrets_and_nothing_else"),
    ("folder under a parent of another environment", "((parent != null and parent.env = env) or", "((parent != null) or",
     "test_folder_must_hang_under_a_parent_of_the_same_environment"),
    ("user-made root folder of any name", "(parent = null and name = \"root\" and envHasNoRoot(env))", "(parent = null and envHasNoRoot(env))",
     "test_the_root_folder_of_an_environment_is_created_once_by_a_writer"),
    ("second root folder", "(parent = null and name = \"root\" and envHasNoRoot(env))", "(parent = null and name = \"root\")",
     "test_the_root_folder_of_an_environment_is_created_once_by_a_writer"),
    ("concurrent root creates unrestricted", "(parent = null and name = \"root\" and envHasNoRoot(env))", "(parent = null and name = \"root\")",
     "test_concurrent_root_creates_commit_exactly_one_root"),
    ("folder in a soft-deleted environment", "allow canWriteProject(actor, env.project) and env.softDeletedAt = null and ((parent", "allow canWriteProject(actor, env.project) and ((parent",
     "test_soft_deleted_environment_keeps_its_slug_and_position_and_cannot_be_changed"),
    ("soft-deleted folders visible", "rows read when canReadProject(actor, env.project) and env.softDeletedAt = null", "rows read when canReadProject(actor, env.project)",
     "test_soft_delete_hides_the_environment_and_everything_below_it_from_readers"),
    ("soft-deleted secrets visible", "rows read when canReadProject(actor, folder.env.project) and folder.env.softDeletedAt = null", "rows read when canReadProject(actor, folder.env.project)",
     "test_soft_delete_hides_the_environment_and_everything_below_it_from_readers"),
    ("duplicate environment slugs", "  unique project, slug\n", "",
     "test_environment_is_created_by_a_project_writer_with_a_unique_slug_and_position"),
    ("duplicate environment positions", "  unique project, position\n", "",
     "test_environment_is_created_by_a_project_writer_with_a_unique_slug_and_position"),
    ("viewer may create environments", "expose create {\n    allow canWriteProject(actor, project)\n    fields project, slug, name, position", "expose create {\n    allow canReadProject(actor, project)\n    fields project, slug, name, position",
     "test_environment_is_created_by_a_project_writer_with_a_unique_slug_and_position"),
    ("soft-deleted environments visible to readers", "(softDeletedAt = null or canWriteProject(actor, project))", "true",
     "test_soft_delete_hides_the_environment_and_everything_below_it_from_readers"),
    ("update may move the environment", "    patch slug, name, position", "    patch project, slug, name, position",
     "test_environment_update_cannot_move_it_to_another_project"),
    ("soft-deleted environment editable", "    from softDeletedAt = null\n    patch slug, name, position", "    from true\n    patch slug, name, position",
     "test_soft_deleted_environment_keeps_its_slug_and_position_and_cannot_be_changed"),
    ("viewer may update environments", "transition update {\n    allow canWriteProject(actor, project)\n    from softDeletedAt", "transition update {\n    allow canReadProject(actor, project)\n    from softDeletedAt",
     "test_environment_update_changes_name_slug_and_position_for_a_writer_only"),
    ("soft delete twice", "    from softDeletedAt = null\n    to softDeletedAt = now, deletedBy = actor", "    from true\n    to softDeletedAt = now, deletedBy = actor",
     "test_soft_deleted_environment_keeps_its_slug_and_position_and_cannot_be_changed"),
    ("soft delete forgets the user", "to softDeletedAt = now, deletedBy = actor", "to softDeletedAt = now",
     "test_soft_delete_hides_the_environment_and_everything_below_it_from_readers"),
    ("viewer may soft delete", "transition softDelete {\n    allow canWriteProject(actor, project)", "transition softDelete {\n    allow canReadProject(actor, project)",
     "test_soft_delete_hides_the_environment_and_everything_below_it_from_readers"),
    ("restore an active environment", "    from softDeletedAt != null\n    to softDeletedAt = null, deletedBy = null", "    from true\n    to softDeletedAt = null, deletedBy = null",
     "test_restore_brings_back_the_environment_with_its_contents"),
    ("restore keeps the marker", "to softDeletedAt = null, deletedBy = null", "to deletedBy = null",
     "test_restore_brings_back_the_environment_with_its_contents"),
    ("restore keeps the deleting user", "to softDeletedAt = null, deletedBy = null", "to softDeletedAt = null",
     "test_restore_brings_back_the_environment_with_its_contents"),
    ("hard delete ignores secrets", "    from not (envHasSecret(this))\n    delete\n", "    from true\n    delete\n",
     "test_hard_delete_refuses_an_environment_that_still_holds_a_secret"),
    ("hard delete keeps folders", "    delete many Folder maxRows 1000 where env = this.id\n", "",
     "test_hard_delete_removes_the_environment_and_all_its_folders"),
    ("viewer may hard delete", "transition hardDelete {\n    allow canWriteProject(actor, project)", "transition hardDelete {\n    allow canReadProject(actor, project)",
     "test_hard_delete_removes_the_environment_and_all_its_folders"),
    ("duplicate folder names", "  unique parent, name\n", "",
     "test_folder_name_is_unique_under_one_parent_only"),
    ("viewer may rename folders", "transition update {\n    allow canWriteProject(actor, env.project)\n    from parent", "transition update {\n    allow canReadProject(actor, env.project)\n    from parent",
     "test_folder_can_be_renamed_and_described_by_a_writer_only"),
    ("root folder renamable", "    from parent != null\n    patch name, description", "    from true\n    patch name, description",
     "test_root_folder_cannot_be_renamed"),
    ("rename may move the folder", "    patch name, description", "    patch name, description, parent",
     "test_update_cannot_move_a_folder_or_change_its_project_or_environment"),
    ("override for another user", "allow user = actor and canReadProject(actor, secret.folder.env.project)", "allow canReadProject(actor, secret.folder.env.project)",
     "test_override_needs_an_existing_shared_secret_and_its_own_user"),
    ("override needs write access", "allow user = actor and canReadProject(actor, secret.folder.env.project)", "allow user = actor and canWriteProject(actor, secret.folder.env.project)",
     "test_override_follows_read_access_not_write_access"),
    ("overrides visible to everyone", "rows read when user = actor and canReadProject(actor, secret.folder.env.project)", "rows read when canReadProject(actor, secret.folder.env.project)",
     "test_overrides_are_visible_to_their_owner_only"),
    ("duplicate overrides", "  unique secret, user\n", "",
     "test_one_override_per_user_and_secret"),
    ("secret delete keeps overrides", "    delete many PersonalSecret maxRows 1000 where secret = this.id\n", "",
     "test_deleting_the_shared_secret_deletes_every_users_override"),
    ("force delete ignores overrides", " and not (hasOverriddenSecret(this))", "",
     "test_force_delete_refuses_a_folder_whose_secrets_still_have_overrides"),
]


# Equivalent mutants: weakening these clauses changes nothing observable today because another rule already
# refuses the request (an organization is visible only to its creator or members, and unique(user, org) blocks
# a second seat; an override is listed to its owner only, so deleting someone else's override already finds no
# target, and a secret the caller cannot read is no valid override target; a soft-deleted environment is invisible to readers, so a reader cannot target it with restore). They stay in the definition as defense in depth; this list fails if a mutant stops being
# equivalent without the test noticing.
EQUIVALENT = [
    ("claim without creator check", " and org.createdBy = actor and orgHasNoMember(org)", " and orgHasNoMember(org)"),
    ("claim without empty-org check", " and org.createdBy = actor and orgHasNoMember(org)", " and org.createdBy = actor"),
    ("override create without project check", "allow user = actor and canReadProject(actor, secret.folder.env.project)", "allow user = actor"),
    ("restore without write check", "transition restore {\n    allow canWriteProject(actor, project)", "transition restore {\n    allow canReadProject(actor, project)"),
    ("override delete without owner check", "transition delete {\n    allow user = actor\n", "transition delete {\n    allow canReadProject(actor, secret.folder.env.project)\n"),
]


def main():
    source = APP.read_text(encoding="utf-8")
    survivors = 0
    for label, old, new, test in MUTATIONS:
        if source.count(old) != 1:
            print(f"BROKEN   {label}: pattern matches {source.count(old)} times")
            survivors += 1
            continue
        with tempfile.TemporaryDirectory() as tmp:
            mutated = pathlib.Path(tmp) / "app.aip"
            mutated.write_text(source.replace(old, new), encoding="utf-8")
            env = dict(os.environ, INFISICAL_APP_AIP=str(mutated))
            if '"ok":true' not in subprocess.run([BINARY, "service", "check", str(mutated)], capture_output=True, text=True).stdout:
                # a definition the product rejects makes every test error out, which is not a caught mutation
                print(f"BROKEN   {label}: the mutated definition does not pass `aip service check`")
                survivors += 1
                continue
            run = subprocess.run([sys.executable, "-m", "unittest", f"test_live_db.LiveDbTests.{test}"],
                                 cwd=HERE, env=env, capture_output=True, text=True)
        if run.returncode == 0:
            print(f"SURVIVED {label}: {test} still passes")
            survivors += 1
        else:
            reason = "skipped" if "skipped" in run.stderr else "caught"
            named_failure = re.search(rf"^(?:FAIL|ERROR): {re.escape(test)} \(", run.stderr, re.MULTILINE)
            executed_one = re.search(r"^Ran 1 test\b", run.stderr, re.MULTILINE)
            fixture_failure = re.search(r"^(?:FAIL|ERROR): (?:setUpClass|tearDownClass|setUpModule|tearDownModule)\b", run.stderr, re.MULTILINE)
            if not named_failure or not executed_one or fixture_failure or "AttributeError" in run.stderr or "unittest.loader._FailedTest" in run.stderr:
                reason = "BROKEN"  # fixture/import failures are not evidence against the mutated policy
            print(f"{reason:8} {label}: {test}")
            if reason in ("skipped", "BROKEN"):
                print(run.stderr[-2000:])
                survivors += 1
    for label, old, new in EQUIVALENT:
        if source.count(old) != 1:
            print(f"BROKEN   {label}: pattern matches {source.count(old)} times")
            survivors += 1
            continue
        with tempfile.TemporaryDirectory() as tmp:
            mutated = pathlib.Path(tmp) / "app.aip"
            mutated.write_text(source.replace(old, new), encoding="utf-8")
            env = dict(os.environ, INFISICAL_APP_AIP=str(mutated))
            run = subprocess.run([sys.executable, "-m", "unittest", "test_live_db"],
                                 cwd=HERE, env=env, capture_output=True, text=True)
        executed = re.search(r"^Ran [1-9][0-9]* tests?\b", run.stderr, re.MULTILINE) and "skipped=" not in run.stderr
        verdict = "EQUIV" if run.returncode == 0 and executed else ("BROKEN" if run.returncode == 0 else "CHANGED")
        print(f"{verdict:8} {label}: whole suite {'unchanged' if verdict == 'EQUIV' else 'failed or did not execute, review the rule'}")
        if verdict != "EQUIV":
            print(run.stderr[-2000:])
            survivors += 1
    return 1 if survivors else 0


if __name__ == "__main__":
    sys.exit(main())
