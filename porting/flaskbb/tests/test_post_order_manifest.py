"""Offline source-order preflight; this does not authorize or perform a DB migration."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import unittest

MODULE = Path(__file__).resolve().parents[1] / "seed/post_order_manifest.py"
spec = importlib.util.spec_from_file_location("post_order_manifest", MODULE)
manifest_tool = importlib.util.module_from_spec(spec)
spec.loader.exec_module(manifest_tool)

A = "ffffffff-ffff-4fff-8fff-ffffffffffff"
B = "00000000-0000-4000-8000-000000000001"
C = "00000000-0000-4000-8000-000000000002"
SOURCE = {"namespace": "flaskbb-fixture-1", "revision": "0" * 40, "high_water": 80}


class ManifestTests(unittest.TestCase):
    def setUp(self):
        self.manifest = {"version": 1, "source": dict(SOURCE), "posts": [
            {"source_id": 41, "post_uuid": A}, {"source_id": 42, "post_uuid": B}]}
        self.snapshot = {"version": 1, "source": dict(SOURCE), "posts": [
            {"post_uuid": A, "published": True, "source_order": None},
            {"post_uuid": B, "published": True, "source_order": None},
            {"post_uuid": C, "published": False, "source_order": None}],
            "mappings": [], "generator": {"last_value": 1, "is_called": False}}

    def plan(self):
        return manifest_tool.preflight(self.manifest, self.snapshot)

    def test_inverse_uuid_order_keeps_explicit_source_ids_and_source_high_water(self):
        before = copy.deepcopy((self.manifest, self.snapshot))
        plan = self.plan()
        self.assertEqual(plan["insert_mappings"], self.manifest["posts"])
        self.assertEqual(plan["link_posts"], self.manifest["posts"])
        self.assertEqual(plan["generator_floor"], 80)
        self.assertEqual(plan["next_id_min"], 81)
        self.assertFalse(plan["activation_authorized"])
        self.assertEqual((self.manifest, self.snapshot), before)

    def test_exact_replay_is_noop_and_never_lowers_generator(self):
        self.snapshot["mappings"] = copy.deepcopy(self.manifest["posts"])
        for post, mapping in zip(self.snapshot["posts"], self.manifest["posts"]):
            post["source_order"] = mapping["source_id"]
        self.snapshot["generator"] = {"last_value": 120, "is_called": True}
        plan = self.plan()
        self.assertEqual(plan["insert_mappings"], [])
        self.assertEqual(plan["link_posts"], [])
        self.assertEqual(plan["generator_floor"], 120)
        self.assertEqual(plan["next_id_min"], 121)

    def test_existing_mapping_can_be_linked_without_reinsertion(self):
        self.snapshot["mappings"] = [dict(self.manifest["posts"][0])]
        plan = self.plan()
        self.assertEqual(plan["insert_mappings"], [self.manifest["posts"][1]])
        self.assertEqual(len(plan["link_posts"]), 2)

    def test_manifest_order_does_not_change_digest(self):
        expected = self.plan()["digest"]
        self.manifest["posts"].reverse()
        self.snapshot["posts"].reverse()
        self.assertEqual(self.plan()["digest"], expected)

    def test_snapshot_and_source_high_water_are_bound_to_digest(self):
        expected = self.plan()["digest"]
        self.snapshot["generator"]["last_value"] = 90
        self.assertNotEqual(self.plan()["digest"], expected)
        self.snapshot["generator"]["last_value"] = 1
        for obj in [self.manifest, self.snapshot]:
            obj["source"]["high_water"] = 81
        self.assertNotEqual(self.plan()["digest"], expected)

    def test_duplicate_ids_and_uuids_are_rejected(self):
        for field in ["source_id", "post_uuid"]:
            with self.subTest(field=field):
                original = self.manifest["posts"][1][field]
                self.manifest["posts"][1][field] = self.manifest["posts"][0][field]
                with self.assertRaisesRegex(ValueError, "duplicate"):
                    self.plan()
                self.manifest["posts"][1][field] = original

    def test_unknown_or_missing_posts_are_rejected_including_unmapped_published_rows(self):
        for remove in [True, False]:
            with self.subTest(remove=remove):
                old = copy.deepcopy(self.manifest)
                if remove:
                    self.manifest["posts"].pop()
                else:
                    self.manifest["posts"].append({"source_id": 43, "post_uuid": "00000000-0000-4000-8000-000000000099"})
                with self.assertRaisesRegex(ValueError, "coverage"):
                    self.plan()
                self.manifest = old

    def test_drafts_cannot_be_claimed_as_source_saved_posts(self):
        self.manifest["posts"].append({"source_id": 43, "post_uuid": C})
        with self.assertRaisesRegex(ValueError, "coverage"):
            self.plan()

    def test_source_namespace_revision_and_high_water_must_match(self):
        for field, value in [("namespace", "another-source"), ("revision", "1" * 40), ("high_water", 81)]:
            with self.subTest(field=field):
                original = self.manifest["source"][field]
                self.manifest["source"][field] = value
                with self.assertRaisesRegex(ValueError, "source"):
                    self.plan()
                self.manifest["source"][field] = original

    def test_high_water_must_cover_manifest_ids(self):
        for obj in [self.manifest, self.snapshot]:
            obj["source"]["high_water"] = 41
        with self.assertRaisesRegex(ValueError, "high_water"):
            self.plan()

    def test_existing_id_or_uuid_pair_cannot_be_reassigned(self):
        for pair in [{"source_id": 41, "post_uuid": B}, {"source_id": 77, "post_uuid": A}]:
            with self.subTest(pair=pair):
                self.snapshot["mappings"] = [pair]
                with self.assertRaisesRegex(ValueError, "mapping"):
                    self.plan()

    def test_link_must_match_the_reverse_mapping(self):
        self.snapshot["posts"][0]["source_order"] = 42
        with self.assertRaisesRegex(ValueError, "link"):
            self.plan()

    def test_unclaimed_or_dangling_existing_mapping_is_rejected(self):
        for target in [C, "00000000-0000-4000-8000-000000000099"]:
            self.snapshot["mappings"] = [{"source_id": 79, "post_uuid": target}]
            with self.assertRaisesRegex(ValueError, "mapping"):
                self.plan()

    def test_ids_are_positive_int64_and_uuids_are_canonical(self):
        for invalid in [True, 0, -1, "41", 1 << 63]:
            with self.subTest(invalid=invalid):
                self.manifest["posts"][0]["source_id"] = invalid
                with self.assertRaises(ValueError):
                    self.plan()
        self.manifest["posts"][0]["source_id"] = 41
        self.manifest["posts"][0]["post_uuid"] = A.upper()
        with self.assertRaises(ValueError):
            self.plan()

    def test_unknown_fields_duplicate_snapshot_rows_and_exhausted_sequence_rejected(self):
        self.manifest["source"]["typo"] = 1
        with self.assertRaises(ValueError):
            self.plan()
        del self.manifest["source"]["typo"]
        self.snapshot["posts"].append(dict(self.snapshot["posts"][0]))
        with self.assertRaisesRegex(ValueError, "duplicate"):
            self.plan()
        self.snapshot["posts"].pop()
        self.snapshot["generator"]["last_value"] = (1 << 63) - 1
        with self.assertRaisesRegex(ValueError, "exhausted"):
            self.plan()

    def test_json_duplicate_keys_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "duplicate"):
            manifest_tool.loads('{"version":1,"version":1}')

    def test_empty_source_has_no_mapping_and_never_authorizes_allocation(self):
        self.manifest["posts"] = []
        for post in self.snapshot["posts"]:
            post["published"] = False
        for obj in [self.manifest, self.snapshot]:
            obj["source"]["high_water"] = 0
        plan = self.plan()
        self.assertEqual(plan["insert_mappings"], [])
        self.assertEqual(plan["link_posts"], [])
        self.assertEqual(plan["generator_floor"], 1)
        self.assertFalse(plan["activation_authorized"])

    def test_generator_and_published_metadata_must_not_coerce_strings_or_bools(self):
        for obj, field, value in [(self.snapshot["generator"], "last_value", True),
                                   (self.snapshot["generator"], "is_called", "false"),
                                   (self.snapshot["posts"][0], "published", 1)]:
            with self.subTest(field=field):
                original = obj[field]
                obj[field] = value
                with self.assertRaises(ValueError):
                    self.plan()
                obj[field] = original

    def test_duplicate_existing_reverse_mapping_is_rejected(self):
        self.snapshot["mappings"] = [dict(self.manifest["posts"][0])] * 2
        with self.assertRaisesRegex(ValueError, "duplicate"):
            self.plan()

    def test_deleted_source_id_high_water_and_preexisting_sequence_gap_are_preserved(self):
        for obj in [self.manifest, self.snapshot]:
            obj["source"]["high_water"] = 2000
        self.assertEqual(self.plan()["next_id_min"], 2001)
        self.snapshot["generator"] = {"last_value": 3000, "is_called": False}
        self.assertEqual(self.plan()["next_id_min"], 3001)

    def test_cli_is_stdin_only_and_fails_without_partial_plan(self):
        payload = {"manifest": self.manifest, "snapshot": self.snapshot}
        result = subprocess.run([sys.executable, str(MODULE)], input=json.dumps(payload), text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["generator_floor"], 80)
        del payload["manifest"]["posts"][0]
        result = subprocess.run([sys.executable, str(MODULE)], input=json.dumps(payload), text=True, capture_output=True)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertIn("coverage", result.stderr)


if __name__ == "__main__":
    unittest.main()
