"""Checks for the pinned Infisical inventory: scanner unit tests (no checkout needed), regeneration
against the pinned upstream checkout (skipped when it is absent), and consistency between the committed JSON
and the numbers quoted in plan-docs/porting/INFISICAL-SELECTION.md."""

import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

REPO = pathlib.Path(__file__).resolve().parent.parent
TOOL = REPO / "tools" / "infisical_inventory.py"
COMMITTED = REPO / "plan-docs" / "porting" / "infisical-inventory.json"
SELECTION = REPO / "plan-docs" / "porting" / "INFISICAL-SELECTION.md"
JOBS = REPO / "plan-docs" / "porting" / "INFISICAL-JOBS.md"
EE_BOUNDARY = REPO / "plan-docs" / "porting" / "INFISICAL-EE-BOUNDARY.md"
DEFAULT_ROOT = pathlib.Path.home() / "development" / "projects" / "aip" / "infisical-upstream"

sys.path.insert(0, str(REPO / "tools"))
import infisical_inventory as inv  # noqa: E402


def routes_of(source):
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "x-router.ts"
        path.write_text(source, encoding="utf-8")
        return inv.extract_routes(path)


class ScannerTests(unittest.TestCase):
    def test_quoted_and_template_urls_are_both_counted(self):
        found = routes_of('''
          server.route({ method: "GET", url: "/a", schema: {} });
          server.route({ method: 'POST', url: '/b' });
          server.route({ method: "PUT", url: `/c/${id}` });
          server.route({ method: "GET", url: `/d` });
        ''')
        self.assertEqual([(r["method"], r["url"], r["url_kind"]) for r in found],
                         [("GET", "/a", "string"), ("POST", "/b", "string"),
                          ("PUT", "/c/${id}", "template"), ("GET", "/d", "template")])

    def test_commented_out_routes_are_not_counted(self):
        found = routes_of('''
          // server.route({ method: "GET", url: "/gone" });
          /* server.route({ method: "GET", url: "/also-gone" }); */
          server.route({ method: "GET", url: "/kept" });
        ''')
        self.assertEqual([r["url"] for r in found], ["/kept"])

    def test_comments_between_properties_do_not_hide_the_url(self):
        found = routes_of('''
          server.route({
            method: "PUT", // using put since this endpoint creates, updates and deletes
            url: "/",
            config: { rateLimit: writeLimit },
            schema: { body: z.object({ mappings: z.object({ name: z.string() }).array() }) }
          });
          server.route({
            // TODO: re-think this
            method: "GET",
            url: "/x"
          });
        ''')
        self.assertEqual([(r["method"], r["url"]) for r in found], [("PUT", "/"), ("GET", "/x")])

    def test_url_after_a_large_schema_with_nested_braces_and_regexes(self):
        found = routes_of('''
          server.route({
            schema: { params: z.object({ id: z.string().regex(/^["'{}]+$/) }), response: { 200: z.object({ a: z.string() }) } },
            method: "DELETE",
            url: "/after-schema"
          });
        ''')
        self.assertEqual([(r["method"], r["url"]) for r in found], [("DELETE", "/after-schema")])

    def test_non_literal_url_is_reported_as_dynamic(self):
        found = routes_of('server.route({ method: "GET", url: buildUrl(prefix) });')
        self.assertEqual([(r["url"], r["url_kind"]) for r in found], [("buildUrl(prefix)", "dynamic")])

    def test_route_receiver_other_than_server_counts(self):
        found = routes_of('app.route({ method: "GET", url: "/health" });')
        self.assertEqual([r["url"] for r in found], ["/health"])


class CommittedInventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.data = json.loads(COMMITTED.read_text(encoding="utf-8"))
        cls.modules = {m["id"]: m for m in cls.data["modules"]}

    def test_every_mit_router_is_assigned(self):
        totals = self.data["totals"]
        self.assertEqual(totals["unassigned_router_files"], [])
        assigned = sum(m.get("route_count", 0) for m in self.data["modules"])
        self.assertEqual(assigned, totals["mit_routes"])
        self.assertEqual(totals["router_files_total"], totals["router_files_mit"] + totals["router_files_ee"])
        self.assertEqual(totals["router_files_assigned"] + len(totals["excluded_test_only_router_files"]),
                         totals["router_files_mit"])

    def test_every_service_dir_has_exactly_one_module(self):
        coverage = self.data["service_dir_coverage"]
        self.assertEqual(coverage["unclassified"], [])
        owners = {}
        for module in self.data["modules"]:
            for path in module.get("paths", []):
                if path.startswith("backend/src/services/"):
                    self.assertNotIn(path, owners, f"{path} is claimed by {owners.get(path)} and {module['id']}")
                    owners[path] = module["id"]
        self.assertEqual(len(owners), coverage["service_dirs_total"])

    def test_url_kinds_add_up(self):
        kinds = self.data["totals"]["mit_route_url_kinds"]
        self.assertEqual(sum(kinds.values()), self.data["totals"]["mit_routes"])
        for module in self.data["modules"]:
            if "url_kinds" in module:
                self.assertEqual(sum(module["url_kinds"].values()), module["route_count"])

    def test_selection_table_matches_the_json(self):
        text = SELECTION.read_text(encoding="utf-8")
        rows = dict()
        for line in text.splitlines():
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if len(cells) >= 4 and re.fullmatch(r"I\d\d", cells[0]):
                rows[cells[0]] = cells
        for mod_id in ("I01", "I02", "I03", "I04", "I05", "I06", "I07", "I08", "I09", "I10"):
            module = self.modules[mod_id]
            self.assertIn(mod_id, rows, f"{mod_id} row missing from INFISICAL-SELECTION.md")
            self.assertEqual(rows[mod_id][2], str(module["route_count"]), f"{mod_id} routes")
            self.assertEqual(rows[mod_id][3], str(module["service_files"]), f"{mod_id} service files")

    def test_selection_quotes_the_json_totals(self):
        text = SELECTION.read_text(encoding="utf-8")
        totals, queues = self.data["totals"], self.data["bullmq"]["totals"]
        kinds = totals["mit_route_url_kinds"]
        for needle in (f"MIT **{totals['router_files_mit']}**", f"EE {totals['router_files_ee']}",
                       f"**{totals['mit_routes']}** = 따옴표 문자열 url {kinds['string']} + 템플릿 리터럴 url {kinds['template']}",
                       f"**{queues['queue_names']}개**", f"MIT {queues['mit']}, EE {queues['ee']}, 양쪽 {queues['mixed']}, 미참조 {queues['unreferenced']}"):
            self.assertIn(needle, text)
        outside = sum(x["route_definitions"] for x in self.data["route_definitions_outside_router_files"])
        self.assertIn(f"정의 {outside}개가 더 있고", text)


    def test_jobs_doc_lists_every_queue_once(self):
        text = JOBS.read_text(encoding="utf-8")
        queues = self.data["bullmq"]["queues"]
        for queue in queues:
            prefix = "| `" + queue["queue"] + "` |"
            self.assertEqual(text.count(prefix), 1, queue["queue"] + " must have exactly one table row")
            row = next(line for line in text.splitlines() if line.startswith(prefix))
            self.assertIn("| " + queue["class"] + " |", row, "class of " + queue["queue"] + " differs from the inventory")
        self.assertIn("`QueueName`은 " + str(len(queues)) + "개", text)

    def test_ee_boundary_doc_matches_the_json(self):
        text = EE_BOUNDARY.read_text(encoding="utf-8")
        coupling = self.data["ee_coupling"]
        needles = ["**" + str(coupling["non_ee_files_importing_ee"]) + "개**",
                   str(coupling["non_ee_non_test_files_importing_ee"]) + "개",
                   format(coupling["import_statements"], ",") + "개",
                   "**" + str(coupling["non_test_files_importing_ee_permission"]) + "**",
                   str(coupling["ee_permission_file_count"]) + "개"]
        for needle in needles:
            self.assertIn(needle, text)
        for mod_id, row in coupling["by_module"].items():
            pattern = (r"\| " + mod_id + r" [^|]*\| " + str(row["service_files"]) + r" \| " + str(row["service_files_importing_ee"])
                       + r" \| " + str(row["router_files"]) + r" \| " + str(row["router_files_importing_ee"]) + r" \|")
            self.assertRegex(text, pattern, mod_id + " coupling row")


class RegenerationTests(unittest.TestCase):
    def test_inventory_regenerates_identically(self):
        root = pathlib.Path(os.environ.get("INFISICAL_UPSTREAM", str(DEFAULT_ROOT)))
        if not (root / "backend" / "src").is_dir():
            self.skipTest(f"Infisical checkout not found at {root}")
        with tempfile.TemporaryDirectory() as tmp:
            out = pathlib.Path(tmp) / "inventory.json"
            subprocess.run([sys.executable, str(TOOL), "--root", str(root), "--output", str(out)],
                           check=True, capture_output=True, text=True)
            fresh = json.loads(out.read_text(encoding="utf-8"))
        committed = json.loads(COMMITTED.read_text(encoding="utf-8"))
        self.assertEqual(fresh["revision"]["sha"], committed["revision"]["sha"],
                         "upstream revision moved; re-pin and review the diff")
        self.assertEqual(fresh, committed)


if __name__ == "__main__":
    unittest.main()
