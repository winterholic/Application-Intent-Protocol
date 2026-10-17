"""Regeneration check for the pinned Infisical inventory (facereview convention)."""

import json
import os
import pathlib
import subprocess
import unittest

REPO = pathlib.Path(__file__).resolve().parent.parent
TOOL = REPO / "tools" / "infisical_inventory.py"
COMMITTED = REPO / "plan-docs" / "porting" / "infisical-inventory.json"
DEFAULT_ROOT = pathlib.Path.home() / "development" / "projects" / "aip" / "infisical-upstream"


class InfisicalInventoryTests(unittest.TestCase):
    def test_inventory_regenerates_identically(self):
        root = pathlib.Path(os.environ.get("INFISICAL_UPSTREAM", str(DEFAULT_ROOT)))
        if not (root / "backend" / "src").is_dir():
            self.skipTest(f"Infisical checkout not found at {root}")
        out = pathlib.Path("/tmp/infisical-inventory-check.json")
        subprocess.run(["python3", str(TOOL), "--root", str(root), "--output", str(out)],
                       check=True, capture_output=True, text=True)
        fresh = json.loads(out.read_text(encoding="utf-8"))
        committed = json.loads(COMMITTED.read_text(encoding="utf-8"))
        self.assertEqual(fresh["revision"]["sha"], committed["revision"]["sha"],
                         "upstream revision moved; re-pin and review the diff")
        self.assertEqual(fresh, committed)

    def test_every_mit_router_is_assigned(self):
        committed = json.loads(COMMITTED.read_text(encoding="utf-8"))
        self.assertEqual(committed["totals"]["unassigned_router_files"], [])
        assigned = sum(m.get("route_count", 0) for m in committed["modules"])
        self.assertEqual(assigned, committed["totals"]["mit_routes"])


if __name__ == "__main__":
    unittest.main()
