"""Write-extension deployments must opt into enable_writes.

Locked-in root cause (2026-10-27): mysql_facereview_auth.rs served the
password/email browser journeys that call User.myEmotionSummary but omitted
"enable_writes", so the explicit-config gate refused every such call with
WIRE.NOT_FOUND (spikes/spike-v6-transport/src/lib.rs). Plain transitions and
read /extension calls need no flag, so the rule only fires on write-extension
/apply calls, made directly from Rust or from an invoked browser journey.
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TESTS_DIR = ROOT / "crates/aip-cli/tests"
SCRIPTS_DIR = ROOT / "product/tests"

DIRECT_WRITE_APPLY = re.compile(r'"request"\s*:\s*\{\s*"extension"\s*:')
SCRIPT_REF = re.compile(r"(facereview-ui-[a-z-]*browser\.py)")
SCRIPT_TAGS_WRITE = re.compile(r'\["write_extension"\]\s*=')
FLAG_TRUE = re.compile(r'"enable_writes"\s*:\s*true')

# mysql_worker_write_e2e.rs builds the write request inside an `apply` helper, so
# the direct marker cannot see it. Pin the requirement instead of widening the
# marker to helper-shaped calls (read /extension goldens share that shape).
PINNED_WRITE_DEPLOYMENTS = {"mysql_worker_write_e2e.rs"}


def invoked_scripts(rs_source: str) -> list[str]:
    return sorted(set(SCRIPT_REF.findall(rs_source)))


def script_tags_write(script_source: str) -> bool:
    return SCRIPT_TAGS_WRITE.search(script_source) is not None


def needs_write_flag(rs_source: str, script_source_of) -> bool:
    if DIRECT_WRITE_APPLY.search(rs_source):
        return True
    return any(script_tags_write(script_source_of(name)) for name in invoked_scripts(rs_source))


def has_write_flag(rs_source: str) -> bool:
    return FLAG_TRUE.search(rs_source) is not None


def read_script(name: str) -> str:
    return (SCRIPTS_DIR / name).read_text()


class WriteFlagRuleTest(unittest.TestCase):
    def test_direct_write_apply_needs_the_flag(self):
        source = 'post(&address, "/apply", &json!({"key": k, "request": {"extension": "User.x", "input": {}}}))'
        self.assertTrue(needs_write_flag(source, read_script))

    def test_plain_transition_does_not_need_the_flag(self):
        source = 'post(&address, "/apply", &json!({"key": k, "request": {"apply": "User.changeRole"}}))'
        self.assertFalse(needs_write_flag(source, read_script))

    def test_read_extension_does_not_need_the_flag(self):
        source = 'post(&address, "/extension", &json!({"extension": extension, "input": input}))'
        self.assertFalse(needs_write_flag(source, read_script))

    def test_script_tagging_a_write_needs_the_flag(self):
        source = '.arg(root.join("product/tests/facereview-ui-password-browser.py"))'
        self.assertTrue(needs_write_flag(source, lambda name: 'row["write_extension"] = "User.x"\n'))

    def test_script_without_write_tags_does_not_need_the_flag(self):
        source = '.arg(root.join("product/tests/facereview-ui-member-browser.py"))'
        self.assertFalse(needs_write_flag(source, lambda name: "row = {}\n"))

    def test_flag_detection(self):
        self.assertTrue(has_write_flag('"workers": {"enable_writes": true}'))
        self.assertFalse(has_write_flag('"workers": {"enable_writes": false}'))
        self.assertFalse(has_write_flag('"workers": {}'))


class WriteDeploymentAuditTest(unittest.TestCase):
    def test_every_write_serving_mysql_deployment_opts_in(self):
        failures = []
        checked = []
        for path in sorted(TESTS_DIR.glob("mysql_*.rs")):
            source = path.read_text()
            for name in invoked_scripts(source):
                self.assertTrue((SCRIPTS_DIR / name).is_file(), f"{path.name} invokes missing {name}")
            if path.name in PINNED_WRITE_DEPLOYMENTS or needs_write_flag(source, read_script):
                checked.append(path.name)
                if not has_write_flag(source):
                    failures.append(path.name)
        self.assertIn("mysql_facereview_auth.rs", checked)
        self.assertIn("mysql_worker_write_e2e.rs", checked)
        self.assertEqual(failures, [], f"write-serving deployments without enable_writes: {failures}")

    def test_guard_detects_the_original_auth_omission(self):
        auth = (TESTS_DIR / "mysql_facereview_auth.rs").read_text()
        self.assertTrue(needs_write_flag(auth, read_script))
        stripped = FLAG_TRUE.sub('"enable_writes": false', auth, count=1)
        self.assertNotEqual(stripped, auth)
        self.assertFalse(has_write_flag(stripped))


if __name__ == "__main__":
    unittest.main()
