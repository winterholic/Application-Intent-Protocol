"""The release candidate proves every required workflow on one SHA, and nothing in it can publish.

Two halves: scripts/release-candidate-required.py is run against recorded `needs` and job results, and the workflow files
are read as text (no YAML parser: the hosted runners do not promise one) to pin the structure that keeps the gate honest.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"
SCRIPT = ROOT / "scripts/release-candidate-required.py"
SHA = "a" * 40
OTHER_SHA = "b" * 40
# Called workflows whose jobs may skip inside a release-candidate run because the job-level `if:` cannot hold there
# (worker-jobs-linux's crypto repair job only runs for a `crypto_repair_only` dispatch of that workflow).
ALLOW_SKIPPED = ["worker-jobs-linux / crypto-compile-repair"]
# Called workflows without a main push trigger of their own. independent-consumer-rc.yml belongs to Track C; until it adds a
# `push: branches: [main]` trigger the consumers are proven by the release candidate and by dispatch only (reported to the lead).
MAIN_PUSH_EXEMPT = {"independent-consumer-rc.yml"}
PUBLISH_PATTERNS = [r"npm\s+publish", r"twine\s+upload", r"gh\s+release\s+(create|upload)", r"pypi-publish",
                    r"action-gh-release", r"git\s+push", r"git\s+tag\b", r"cargo\s+publish", r"id-token:\s*write"]


def read(name):
    return (WORKFLOWS / name).read_text()


def job_ids(text):
    ids, in_jobs = [], False
    for line in text.splitlines():
        if re.match(r"^jobs:\s*$", line):
            in_jobs = True
        elif in_jobs and re.match(r"^\S", line) and not line.startswith("#"):
            break
        elif in_jobs and re.match(r"^  [A-Za-z0-9_-]+:\s*$", line):
            ids.append(line.strip()[:-1])
    return ids


def job_block(text, job):
    lines, out, inside = text.splitlines(), [], False
    for line in lines:
        if re.match(rf"^  {re.escape(job)}:\s*$", line):
            inside = True
            continue
        if inside and re.match(r"^  \S", line):
            break
        if inside:
            out.append(line)
    return "\n".join(out)


def top_block(text, key):
    out, inside = [], False
    for line in text.splitlines():
        if re.match(rf"^{re.escape(key)}:", line):
            inside = True
            out.append(line)
            continue
        if inside and re.match(r"^\S", line) and not line.startswith("#"):
            break
        if inside:
            out.append(line)
    return "\n".join(out)


def trigger_block(text, event):
    out, inside = [], False
    for line in top_block(text, "on").splitlines()[1:]:
        if re.match(rf"^  {event}:", line):
            inside = True
            out.append(line)
            continue
        if inside and re.match(r"^  \S", line):
            break
        if inside:
            out.append(line)
    return "\n".join(out)


def release_candidate_jobs():
    text = read("release-candidate.yml")
    return [job for job in job_ids(text) if job != "required"]


def called_workflows():
    text = read("release-candidate.yml")
    files = []
    for job in release_candidate_jobs():
        match = re.search(r"uses:\s*\./\.github/workflows/([\w.-]+\.yml)", job_block(text, job))
        files.append(match.group(1) if match else None)
    return files


class GateScriptTest(unittest.TestCase):
    def run_gate(self, results, jobs=None, expected=None, allow=ALLOW_SKIPPED):
        expected = expected or list(results)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / "rc.yml"
            workflow.write_text("jobs:\n" + "".join(f"  {name}:\n    uses: ./x.yml\n" for name in expected) +
                                "  required:\n    needs: []\n")
            (root / "needs.json").write_text(json.dumps({name: {"result": value} for name, value in results.items()}))
            command = [sys.executable, str(SCRIPT), "--sha", SHA, "--needs", str(root / "needs.json"),
                       "--workflow", str(workflow)]
            if jobs is not None:
                (root / "jobs.jsonl").write_text("".join(json.dumps(job) + "\n" for job in jobs))
                command += ["--jobs", str(root / "jobs.jsonl")]
            for prefix in allow:
                command += ["--allow-skipped", prefix]
            return subprocess.run(command, capture_output=True, text=True)

    @staticmethod
    def job(workflow, name="core", conclusion="success", sha=SHA):
        return {"name": f"{workflow} / {name}", "conclusion": conclusion, "head_sha": sha, "status": "completed"}

    def all_success(self):
        return {"verify": "success", "release-gate": "success"}

    def all_jobs(self):
        return [self.job("verify"), self.job("release-gate"), {**self.job("required"), "name": "required (all workflows green on this SHA)",
                                                              "conclusion": None}]

    def test_every_workflow_and_job_green_on_the_same_sha_passes(self):
        result = self.run_gate(self.all_success(), self.all_jobs())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"ALL REQUIRED WORKFLOWS PASSED for {SHA}", result.stdout)

    def test_a_skipped_workflow_is_not_a_pass(self):
        result = self.run_gate({**self.all_success(), "verify": "skipped"})
        self.assertEqual(result.returncode, 1)
        self.assertIn("`verify` finished as `skipped`", result.stderr)

    def test_a_cancelled_workflow_is_not_a_pass(self):
        result = self.run_gate({**self.all_success(), "release-gate": "cancelled"})
        self.assertEqual(result.returncode, 1)
        self.assertIn("`release-gate` finished as `cancelled`", result.stderr)

    def test_a_failed_workflow_is_not_a_pass(self):
        result = self.run_gate({**self.all_success(), "verify": "failure"})
        self.assertEqual(result.returncode, 1)

    def test_a_declared_workflow_missing_from_needs_fails(self):
        result = self.run_gate(self.all_success(), expected=["verify", "release-gate", "independent-consumers"])
        self.assertEqual(result.returncode, 1)
        self.assertIn("`independent-consumers` is declared but not aggregated", result.stderr)

    def test_a_need_the_workflow_does_not_declare_fails(self):
        result = self.run_gate({**self.all_success(), "stray": "success"}, expected=["verify", "release-gate"])
        self.assertEqual(result.returncode, 1)
        self.assertIn("`stray`, which the workflow does not declare", result.stderr)

    def test_no_needs_at_all_fails(self):
        self.assertEqual(self.run_gate({}, expected=["verify"]).returncode, 1)

    def test_a_job_that_ran_for_another_sha_fails_even_when_its_workflow_is_green(self):
        jobs = self.all_jobs()
        jobs[1] = self.job("release-gate", sha=OTHER_SHA)
        result = self.run_gate(self.all_success(), jobs)
        self.assertEqual(result.returncode, 1)
        self.assertIn(f"ran for {OTHER_SHA}, not {SHA}", result.stderr)

    def test_a_job_skipped_inside_a_green_workflow_fails(self):
        jobs = self.all_jobs() + [self.job("verify", "browser", "skipped")]
        result = self.run_gate(self.all_success(), jobs)
        self.assertEqual(result.returncode, 1)
        self.assertIn("`verify / browser` concluded `skipped`", result.stderr)

    def test_a_job_cancelled_inside_a_green_workflow_fails(self):
        jobs = self.all_jobs() + [self.job("verify", "browser", "cancelled")]
        self.assertEqual(self.run_gate(self.all_success(), jobs).returncode, 1)

    def test_an_explicitly_allowed_skipped_job_is_tolerated_and_nothing_else(self):
        allowed = self.job("worker-jobs-linux", "crypto-compile-repair", "skipped")
        results = {**self.all_success(), "worker-jobs-linux": "success"}
        jobs = self.all_jobs() + [self.job("worker-jobs-linux", "worker-jobs"), allowed]
        self.assertEqual(self.run_gate(results, jobs).returncode, 0)
        jobs[-1] = self.job("worker-jobs-linux", "worker-jobs-2", "skipped")
        self.assertEqual(self.run_gate(results, jobs).returncode, 1)

    def test_a_workflow_that_ran_no_jobs_fails(self):
        jobs = [job for job in self.all_jobs() if not job["name"].startswith("release-gate")]
        result = self.run_gate(self.all_success(), jobs)
        self.assertEqual(result.returncode, 1)
        self.assertIn("no job was observed for workflow `release-gate`", result.stderr)


class WorkflowStructureTest(unittest.TestCase):
    def test_required_aggregates_exactly_the_declared_workflow_jobs(self):
        text = read("release-candidate.yml")
        needs = re.findall(r"^\s+- ([\w-]+)\s*$", re.search(r"needs:\n((?:\s+- [\w-]+\n)+)", job_block(text, "required") + "\n").group(1), re.M)
        self.assertEqual(sorted(needs), sorted(release_candidate_jobs()))

    def test_required_uses_the_gate_script_with_the_run_sha_and_the_api_job_list(self):
        block = job_block(read("release-candidate.yml"), "required")
        self.assertIn("if: ${{ always() }}", block)
        self.assertIn("scripts/release-candidate-required.py", block)
        self.assertIn('--sha "$GITHUB_SHA"', block)
        self.assertIn("--jobs", block)
        self.assertIn("actions: read", block)
        self.assertNotIn("continue-on-error", block)
        self.assertNotIn("|| true", block)

    def test_every_release_candidate_job_calls_a_workflow_that_exists_and_accepts_workflow_call(self):
        for job, file in zip(release_candidate_jobs(), called_workflows()):
            with self.subTest(job=job):
                self.assertIsNotNone(file, f"{job} must call a local reusable workflow")
                self.assertTrue((WORKFLOWS / file).exists(), file)
                self.assertIn("workflow_call:", trigger_block(read(file), "workflow_call"), file)

    def test_no_called_workflow_can_publish_or_tag(self):
        for file in set(called_workflows()) | {"release-candidate.yml"}:
            text = "\n".join(line for line in read(file).splitlines() if not line.lstrip().startswith("#"))
            for pattern in PUBLISH_PATTERNS:
                with self.subTest(file=file, pattern=pattern):
                    self.assertIsNone(re.search(pattern, text), f"{file} contains a publishing step: {pattern}")
        self.assertNotIn("release.yml", called_workflows())

    def test_a_release_candidate_run_checks_out_the_candidate_sha_only(self):
        for file in set(called_workflows()):
            text = read(file)
            for ref in re.findall(r"^\s+ref:\s*(.+)$", text, re.M):
                with self.subTest(file=file):
                    if ref.strip() == "${{ env.TARGET_SHA }}":
                        # Only acceptable when TARGET_SHA is the caller's SHA unless a core_sha input overrides it, and the
                        # release candidate never passes that input.
                        self.assertIn("TARGET_SHA: ${{ inputs.core_sha || github.sha }}", text)
                        self.assertNotIn("core_sha", job_block(read("release-candidate.yml"), "independent-consumers"))
                    else:
                        self.assertEqual(ref.strip(), "${{ github.sha }}", f"{file} pins a checkout ref other than the candidate")

    def test_every_required_workflow_runs_on_every_main_push_without_a_path_filter(self):
        for file in sorted(set(called_workflows()) - MAIN_PUSH_EXEMPT):
            push = trigger_block(read(file), "push")
            with self.subTest(file=file):
                self.assertNotEqual(push, "", f"{file} has no push trigger, so main commits skip it")
                self.assertRegex(push, r"branches:.*\bmain\b")
                self.assertNotRegex(push, r"(?m)^\s+paths(-ignore)?:", f"{file} filters main pushes by path")

    def test_push_and_dispatch_concurrency_never_cancels_another_sha(self):
        # A newer push must not cancel or replace the run of an older SHA, or that SHA is left without a verdict.
        for file in sorted(set(called_workflows()) | {"release-candidate.yml"}):
            block = top_block(read(file), "concurrency")
            with self.subTest(file=file):
                if block:  # no concurrency block means nothing is ever cancelled
                    self.assertIn("github.sha", block)
                    self.assertNotRegex(block, r"(?m)cancel-in-progress:\s*true\s*$")

    def test_every_continue_on_error_step_is_aggregated_into_a_failing_final_step(self):
        for path in sorted(WORKFLOWS.glob("*.yml")):
            text = path.read_text()
            steps = re.split(r"(?m)^      - ", text)
            masked = [step for step in steps if re.search(r"^\s+continue-on-error:\s*true", step, re.M)]
            for step in masked:
                step_id = re.search(r"^\s+id:\s*([\w-]+)", step, re.M)
                with self.subTest(file=path.name, step=step.splitlines()[0]):
                    self.assertIsNotNone(step_id, "a continue-on-error step needs an id so its outcome can be checked")
                    final = [s for s in steps if "if: ${{ always() }}" in s and f"steps.{step_id.group(1)}.outcome" in s]
                    self.assertTrue(final, f"{path.name}: outcome of {step_id.group(1)} is never re-checked, so its failure is masked")
                    self.assertIn("sys.exit(1 if failed else 0)", final[0])


class ContractWiringTest(unittest.TestCase):
    """B-05, B-06, F-05: tests that exist but ran nowhere are tied to a workflow, and the split is checked for drift."""

    def probe_lists(self):
        out = subprocess.run(["bash", str(ROOT / "scripts/run-concurrency-probes.sh"), "--list"],
                             capture_output=True, text=True, check=True).stdout
        lists = {"stable": [], "exploratory": []}
        for line in out.splitlines():
            kind, name = line.split()
            lists[kind].append(name)
        return lists

    def test_every_ignored_concurrency_probe_is_classified_stable_or_exploratory_exactly_once(self):
        source = (ROOT / "spikes/spike-v6-transport/tests/concurrency_probe_http.rs").read_text()
        ignored = re.findall(r"#\[ignore[^\n]*\]\nasync fn (\w+)", source)
        lists = self.probe_lists()
        self.assertTrue(ignored, "no ignored probes detected; the source selector may have drifted")
        self.assertEqual(sorted(lists["stable"] + lists["exploratory"]), sorted(ignored))
        self.assertFalse(set(lists["stable"]) & set(lists["exploratory"]))
        # The task's stable set: f1, i1-i5, n1, e1-e5. a1-a5 stay exploratory.
        self.assertEqual(sorted(name.split("_")[0] for name in lists["stable"]),
                         sorted(["f1", "n1", *(f"i{n}" for n in range(1, 6)), *(f"e{n}" for n in range(1, 6))]))
        self.assertEqual(lists["exploratory"].count("e6_saturated_replay_latency_by_database_round_trip"), 1)

    def test_stable_probes_are_required_and_exploratory_probes_are_not(self):
        self.assertIn("concurrency-contracts", release_candidate_jobs())
        self.assertIn("run-concurrency-probes.sh stable", read("concurrency-contracts.yml"))
        exploration = read("concurrency-exploration.yml")
        self.assertNotIn("concurrency-exploration.yml", called_workflows())
        self.assertNotIn("workflow_call:", exploration)
        self.assertIn("schedule:", exploration)
        self.assertIn("workflow_dispatch:", exploration)

    def test_macos_only_test_files_run_in_the_macos_workflow(self):
        macos = read("macos-platform-tests.yml")
        test_files = list((ROOT / "spikes/spike-v6-transport/tests").glob("*.rs"))
        gated = {path.stem for path in test_files if path.read_text().lstrip().startswith("#![cfg(target_os = \"macos\")]")}
        gated |= {path.stem for path in test_files if re.search(r"#\[cfg\(target_os = \"macos\"\)\]", path.read_text())}
        self.assertTrue(gated, "no macOS-only test target or in-file macOS cfg test detected")
        verify_head = (ROOT / "scripts/verify-head.sh").read_text()
        for stem in sorted(gated):
            with self.subTest(test_file=stem):
                self.assertRegex(macos, rf"--test {stem}\b", f"{stem}.rs compiles only for macOS and no macOS job runs it")
                self.assertRegex(verify_head, rf"--test {stem}\b", f"verify-head's macOS path does not run {stem}.rs")

    def test_default_verify_runs_all_v6_regression_targets(self):
        workflow = read("verify.yml")
        core = workflow.split("  core:\n", 1)[1]
        self.assertIn("bash tools/verify.sh", core)
        verify = (ROOT / "tools/verify.sh").read_text()
        self.assertRegex(verify, r'for engine in [^\n]*spike-v6-transport')
        self.assertIn('cargo test --manifest-path "spikes/$engine/Cargo.toml" --locked --target-dir target --no-fail-fast', verify)


class EntryPointTest(unittest.TestCase):
    """scripts/verify-head.sh records the SHA it ran on and never reports a pass for work it did not do (F-02)."""
    SCRIPT = ROOT / "scripts/verify-head.sh"

    def git(self, root, *args):
        return subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True, check=True).stdout.strip()

    def scratch_repo(self, directory, dirty=False):
        root = Path(directory) / "repo"
        (root / "scripts").mkdir(parents=True)
        for name in ("scripts/verify-head.sh", "scripts/release-candidate-required.py"):
            (root / name).write_text((ROOT / name).read_text())
        self.git(root, "init", "-q")
        self.git(root, "-c", "user.name=t", "-c", "user.email=t@t", "add", "-A")
        self.git(root, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "scratch")
        if dirty:
            (root / "uncommitted").write_text("x")
        return root

    def run_script(self, root, *args, env=None):
        full = {**os.environ, **(env or {})}
        full.pop("GITHUB_SHA", None) if not env or "GITHUB_SHA" not in env else None
        return subprocess.run(["bash", str(root / "scripts/verify-head.sh"), *args], capture_output=True, text=True, env=full)

    def test_dry_run_names_the_sha_plans_every_step_and_executes_nothing(self):
        with tempfile.TemporaryDirectory() as directory:
            root, out = self.scratch_repo(directory), Path(directory) / "out"
            result = self.run_script(root, "--dry-run", "--out", str(out))
            sha = self.git(root, "rev-parse", "HEAD")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn(f"sha={sha}", result.stdout)
            self.assertIn("VERDICT: NOT RUN", result.stdout)
            self.assertNotIn("PROVEN", result.stdout)
            self.assertEqual(sorted(path.name for path in (out / "logs").iterdir()), [], "a dry run must not run any step")
            record = json.loads((out / "result.json").read_text())
            self.assertEqual((record["sha"], record["dry_run"], record["proven"]), (sha, True, False))
            self.assertTrue(all(step["status"] == "NOT RUN" for step in record["steps"]))

    def test_a_passing_run_records_the_sha_and_is_proven_only_when_clean(self):
        with tempfile.TemporaryDirectory() as directory:
            root, out = self.scratch_repo(directory), Path(directory) / "out"
            result = self.run_script(root, "--stage", "fast", "--only", "scripts-syntax", "--out", str(out))
            sha = self.git(root, "rev-parse", "HEAD")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn(f"VERDICT: PROVEN for {sha}", result.stdout)
            record = json.loads((out / "result.json").read_text())
            self.assertEqual((record["sha"], record["dirty"], record["proven"]), (sha, False, True))
        with tempfile.TemporaryDirectory() as directory:
            root, out = self.scratch_repo(directory, dirty=True), Path(directory) / "out"
            result = self.run_script(root, "--stage", "fast", "--only", "scripts-syntax", "--out", str(out))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("PASSED ON A DIRTY TREE", result.stdout)
            self.assertFalse(json.loads((out / "result.json").read_text())["proven"])

    def test_a_failing_step_fails_the_run_and_is_not_proven(self):
        with tempfile.TemporaryDirectory() as directory:
            root, out = self.scratch_repo(directory), Path(directory) / "out"
            # The scratch repo has no product/tests, so the workflow-structure step cannot pass.
            result = self.run_script(root, "--stage", "fast", "--only", "workflow-structure", "--out", str(out))
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertIn("FAIL  [fast] workflow-structure", result.stdout)
            self.assertIn("NOT PROVEN", result.stdout)
            self.assertFalse(json.loads((out / "result.json").read_text())["proven"])

    def test_heavy_refuses_a_dirty_tree_and_a_checkout_that_is_not_the_workflow_sha(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self.scratch_repo(directory, dirty=True)
            result = self.run_script(root, "--stage", "heavy", "--out", str(Path(directory) / "out"))
            self.assertEqual(result.returncode, 2)
            self.assertIn("would not prove HEAD", result.stderr)
        with tempfile.TemporaryDirectory() as directory:
            root = self.scratch_repo(directory)
            result = self.run_script(root, "--dry-run", "--out", str(Path(directory) / "out"), env={"GITHUB_SHA": "c" * 40})
            self.assertEqual(result.returncode, 2)
            self.assertIn("is not the SHA this workflow runs for", result.stderr)

    def test_the_plan_only_names_files_that_exist_and_never_publishes(self):
        plan = subprocess.run(["bash", str(self.SCRIPT), "--list", "--with-release-gate"], capture_output=True, text=True, check=True).stdout
        self.assertIn("release-gate", plan)
        for path in set(re.findall(r"(?:scripts|tools|product|spikes|crates)/[\w./-]+\.(?:sh|mjs|py|toml)", plan)):
            self.assertTrue((ROOT / path).exists(), f"the plan names {path}, which does not exist")
        for pattern in PUBLISH_PATTERNS[:-1]:
            self.assertIsNone(re.search(pattern, self.SCRIPT.read_text()), pattern)

    def test_provenance_contract_runs_once_in_the_fast_path(self):
        plan = subprocess.run(["bash", str(self.SCRIPT), "--list"], capture_output=True, text=True, check=True).stdout
        test_path = "verification/1.0-consumers/test_provenance.py"
        self.assertIn(test_path, plan)
        skipped_env = {**os.environ, "GITHUB_ACTIONS": "true", "PROVENANCE_UNIT_RUNS_IN_INDEPENDENT_CONSUMERS": "true"}
        skipped_plan = subprocess.run(["bash", str(self.SCRIPT), "--list", "--skip-provenance-unit"],
                                      capture_output=True, text=True, env=skipped_env, check=True).stdout
        self.assertNotIn(test_path, skipped_plan)
        rejected_env = {**os.environ, "GITHUB_ACTIONS": "false", "PROVENANCE_UNIT_RUNS_IN_INDEPENDENT_CONSUMERS": "false"}
        rejected = subprocess.run(["bash", str(self.SCRIPT), "--list", "--skip-provenance-unit"],
                                  capture_output=True, text=True, env=rejected_env)
        self.assertEqual(rejected.returncode, 2)
        self.assertIn("reserved for the Release Candidate Verify call", rejected.stderr)
        verify = read("verify.yml")
        candidate = read("release-candidate.yml")
        consumer = read("independent-consumer-rc.yml")
        self.assertIn("--skip-provenance-unit", verify)
        self.assertIn("provenance_unit_runs_in_independent_consumers: true", candidate)
        self.assertEqual(consumer.count(test_path), 1, "the RC caller skips the fast copy; the required consumer job runs this suite once")

    def test_verify_sh_runs_the_tests_that_no_gate_ran_before(self):
        text = (ROOT / "tools/verify.sh").read_text()
        for needle in ("spikes/spike-v6-transport/client/uuid.test.ts", "spikes/spike-v6-transport/client/extension-rejection.test.ts",
                       "discover -s tools -p 'test_*.py'", "discover -s scripts -p 'test_*.py'", "product.tests.test_release_candidate_required",
                       "AIP_BIN=target/debug/aip node --test --test-name-pattern='^(a transition the caller may not make|a create the caller may not make)' product/tests/policy-residual-paths.test.mjs"):
            self.assertIn(needle, text)


if __name__ == "__main__":
    unittest.main()
