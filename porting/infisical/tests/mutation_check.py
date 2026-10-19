"""Proves test_live_db.py is not vacuous: each weakening of app.aip must make a named test fail.

  AIP_TEST_DATABASE_URL=postgresql://... python3 porting/infisical/tests/mutation_check.py

Prints one line per mutation and exits non-zero if any mutation survives (all tests still pass).
"""

import os
import pathlib
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
APP = HERE.parent / "app.aip"
BINARY = os.environ.get("AIP_BIN", str(HERE.parent.parent.parent / "target" / "debug" / "aip"))

# (label, text to find, replacement, test that must fail)
MUTATIONS = [
    ("viewer may write", "role in (ADMIN, MEMBER))\npredicate isProjectAdmin", "role in (ADMIN, MEMBER, VIEWER))\npredicate isProjectAdmin",
     "test_viewer_cannot_create_folder_or_secret"),
    ("every user visible", "rows read when this = actor or sharesOrgWith(actor, this)", "rows read when true",
     "test_non_member_sees_only_themselves"),
    ("duplicate secret names", "  unique folder, key\n", "",
     "test_secret_name_is_unique_in_a_folder_including_the_root"),
    ("org created in another name", "allow actor != null and createdBy = actor", "allow actor != null",
     "test_organization_cannot_be_created_in_someone_elses_name"),
    ("job open to any user", " and exists Secret where id = input.secret and canReadProject(actor, folder.project)", "",
     "test_reminder_job_rejects_callers_without_project_access"),
    ("outsider joins project", " and isOrgMember(user, project.org)", "",
     "test_project_membership_rules"),
    ("viewer may delete", "transition delete {\n    allow canWriteProject(actor, folder.project)", "transition delete {\n    allow canReadProject(actor, folder.project)",
     "test_viewer_can_read_but_not_delete"),
    ("secret owner is free", "allow canWriteProject(actor, folder.project) and owner = actor", "allow canWriteProject(actor, folder.project)",
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
    ("viewer may force delete", "transition forceDelete {\n    allow canWriteProject(actor, project)", "transition forceDelete {\n    allow canReadProject(actor, project)",
     "test_force_delete_removes_the_folder_and_its_secrets_and_nothing_else"),
    ("folder in another project", "parent.project = project and parent.env = env", "parent.env = env",
     "test_folder_must_hang_under_a_parent_of_the_same_project_and_environment"),
    ("folder in another environment", "parent.project = project and parent.env = env", "parent.project = project",
     "test_folder_must_hang_under_a_parent_of_the_same_project_and_environment"),
    ("user-made root folder", "parent != null and parent.project = project and parent.env = env", "(parent = null or (parent.project = project and parent.env = env))",
     "test_folder_must_hang_under_a_parent_of_the_same_project_and_environment"),
    ("duplicate folder names", "  unique parent, name\n", "",
     "test_folder_name_is_unique_under_one_parent_only"),
]


# Equivalent mutants: weakening these clauses changes nothing observable today because another rule already
# refuses the request (an organization is visible only to its creator or members, and unique(user, org) blocks
# a second seat). They stay in the definition as defense in depth; this list fails if a mutant stops being
# equivalent without the test noticing.
EQUIVALENT = [
    ("claim without creator check", " and org.createdBy = actor and orgHasNoMember(org)", " and orgHasNoMember(org)"),
    ("claim without empty-org check", " and org.createdBy = actor and orgHasNoMember(org)", " and org.createdBy = actor"),
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
            if "has no attribute" in run.stderr or "AttributeError" in run.stderr:
                reason = "BROKEN"  # the named test does not exist; an import error is not a caught mutation
            print(f"{reason:8} {label}: {test}")
            if reason in ("skipped", "BROKEN"):
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
        verdict = "EQUIV" if run.returncode == 0 else "CHANGED"
        print(f"{verdict:8} {label}: whole suite {'unchanged' if run.returncode == 0 else 'now fails, review the rule'}")
        if run.returncode != 0:
            survivors += 1
    return 1 if survivors else 0


if __name__ == "__main__":
    sys.exit(main())
