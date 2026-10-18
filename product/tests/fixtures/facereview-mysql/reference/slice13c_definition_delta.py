"""slice13c_watch.facereview.aip must be facereview.aip plus only the marked slice13c blocks and the two header lines.

Anything else that differs is a silent change to shared fixture semantics. Also runs `aip service check` when AIP_BIN is set.
"""
import os
import re
import subprocess
import sys
import unittest
from pathlib import Path

FIXTURE = Path(__file__).resolve().parent.parent
BASE = (FIXTURE / "facereview.aip").read_text(encoding="utf-8")
SLICE = (FIXTURE / "slice13c_watch.facereview.aip").read_text(encoding="utf-8")
# The one in-place edit: view_count loses its `?` so that `view_count + 1` type-checks (increment needs a non-null Int).
VIEW_COUNT_BASE = "    view_count: Int(0..1000000000)?\n"
VIEW_COUNT_SLICE = "    view_count: Int(0..1000000000) // slice13c: non-null so that view_count + 1 is expressible; the live column is nullable (see delta note)\n"


def strip_slice(text):
    text = re.sub(r"  // slice13c begin:.*?  // slice13c end\n", "", text, flags=re.S)
    lines = text.split("\n")
    assert lines[0].startswith("// Slice13-C:") and lines[1].startswith("// Everything outside"), "header lines missing"
    return "\n".join(lines[2:])


class DefinitionDelta(unittest.TestCase):
    def test_only_marked_blocks_and_view_count_differ(self):
        self.assertEqual(BASE.count(VIEW_COUNT_BASE), 1)
        self.assertEqual(SLICE.count(VIEW_COUNT_SLICE), 1)
        self.assertEqual(strip_slice(SLICE).replace(VIEW_COUNT_SLICE, VIEW_COUNT_BASE), BASE)

    def test_added_blocks_are_the_expected_declarations(self):
        blocks = re.findall(r"  // slice13c begin:.*?  // slice13c end\n", SLICE, flags=re.S)
        self.assertEqual(len(blocks), 3)
        joined = "".join(blocks)
        for needle in ("transition incrementViewCount", "extension write recordView", "extension read watchDetail",
                       "extension read watchRecommended", "aggregate countForVideo"):
            self.assertIn(needle, joined)
        # nothing widens a read policy: no new `rows read` and no change to expose read lists
        self.assertNotIn("rows read", joined)
        self.assertNotIn("expose read", joined)
        self.assertNotIn("expose create", joined)

    @unittest.skipUnless(os.environ.get("AIP_BIN"), "set AIP_BIN to also run `aip service check`")
    def test_aip_service_check_passes(self):
        out = subprocess.run([os.environ["AIP_BIN"], "service", "check", str(FIXTURE / "slice13c_watch.facereview.aip")],
                             capture_output=True, text=True)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertIn('"ok":true', out.stdout)


if __name__ == "__main__":
    unittest.main()
