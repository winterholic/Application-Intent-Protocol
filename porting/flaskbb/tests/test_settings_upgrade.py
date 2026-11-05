"""Offline fixture-upgrade planning; live service permissions are tested in Node."""
import copy
import unittest

from porting.flaskbb.seed.install import SettingValueType, _load_fixture
from porting.flaskbb.seed import upgrade_settings as upgrade


FIXTURE = (("general", {"name": "General", "description": "Group", "settings": (
    ("per_page", {"value": 10, "value_type": SettingValueType.integer,
                  "name": "Per page", "description": "Count", "extra": {"min": 5}}),
)}),)
GROUP = {"id": "7", "key": "general", "name": "General", "description": "Group"}
SETTING = {"id": "9", "key": "per_page", "value_json": "10", "group": "7",
           "name": "Per page", "description": "Count", "value_type": "integer",
           "extra_json": '{"min": 5}'}


class SettingsUpgradeTests(unittest.TestCase):
    def test_refused_group_create_stops_before_settings_and_reports_failure(self):
        class DeniedClient:
            def __init__(self):
                self.events = []

            def read(self, query):
                self.events.append(query["read"])
                return []

            def bundle(self, operations):
                self.events.append(operations[0]["create"])
                return {"ok": False, "code": "WIRE.FORBIDDEN"}

        client = DeniedClient()
        with self.assertRaisesRegex(RuntimeError, "WIRE.FORBIDDEN"):
            upgrade.upgrade_settings(client, FIXTURE)
        self.assertEqual(client.events, ["SettingsGroup", "SettingsGroup"])

    def test_key_reads_do_not_use_an_unbounded_or_truncated_listing(self):
        class ReadOnlyClient:
            def __init__(self):
                self.queries = []

            def read(self, query):
                self.queries.append(query)
                return [GROUP] if query["read"] == "SettingsGroup" else [SETTING]

            def bundle(self, operations):
                raise AssertionError("unchanged fixture must issue no write")

        client = ReadOnlyClient()
        self.assertEqual(upgrade.upgrade_settings(client, FIXTURE, True), {
            "groups_written": 0, "settings_written": 0})
        self.assertEqual(len(client.queries), 3)
        for query in client.queries:
            self.assertEqual(query["limit"], 1)
            self.assertEqual(query["filter"][0]["field"], "key")
            self.assertEqual(query["filter"][0]["op"], "eq")

    def test_adds_missing_group_through_exposed_create(self):
        self.assertEqual(upgrade.plan_groups(FIXTURE, []), [{"create": "SettingsGroup", "values": {
            "key": "general", "name": "General", "description": "Group"}}])

    def test_preserves_existing_operator_values_without_force(self):
        group = {**GROUP, "name": "Custom"}
        setting = {**SETTING, "value_json": "55"}
        self.assertEqual(upgrade.plan_groups(FIXTURE, [group]), [])
        self.assertEqual(upgrade.plan_settings(FIXTURE, [group], [setting]), [])

    def test_force_updates_only_changed_groups_and_settings(self):
        self.assertEqual(upgrade.plan_groups(FIXTURE, [{**GROUP, "name": "Custom"}], True), [{
            "apply": "SettingsGroup.edit", "target": {"ids": ["7"]},
            "values": {"name": "General", "description": "Group"}}])
        changes = upgrade.plan_settings(FIXTURE, [GROUP], [{**SETTING, "value_json": "55"}], True)
        self.assertEqual(len(changes), 1)
        self.assertEqual(changes[0]["apply"], "Setting.edit")
        self.assertEqual(changes[0]["target"], {"ids": ["9"]})
        self.assertEqual(changes[0]["values"]["value_json"], "10")
        self.assertNotIn("key", changes[0]["values"])

    def test_adds_missing_setting_using_resolved_group_id(self):
        changes = upgrade.plan_settings(FIXTURE, [GROUP], [])
        self.assertEqual(changes, [{"create": "Setting", "values": {
            key: value for key, value in SETTING.items() if key != "id"}}])

    def test_unchanged_settings_are_not_written_even_with_force(self):
        self.assertEqual(upgrade.plan_groups(FIXTURE, [GROUP], True), [])
        self.assertEqual(upgrade.plan_settings(FIXTURE, [GROUP], [SETTING], True), [])

    def test_force_moves_group_and_preserves_unrelated_rows(self):
        other = {**SETTING, "id": "11", "key": "custom"}
        changes = upgrade.plan_settings(FIXTURE, [GROUP], [{**SETTING, "group": "8"}, other], True)
        self.assertEqual(len(changes), 1)
        self.assertEqual(changes[0]["values"]["group"], "7")

    def test_unresolved_group_fails_before_any_setting_writes(self):
        with self.assertRaisesRegex(ValueError, "general"):
            upgrade.plan_settings(FIXTURE, [], [])

    def test_does_not_mutate_input_rows(self):
        rows = [copy.deepcopy(SETTING)]
        previous = copy.deepcopy(rows)
        upgrade.plan_settings(FIXTURE, [GROUP], rows, True)
        self.assertEqual(rows, previous)

    def test_pinned_fixture_all_settings_and_optional_extra_are_represented(self):
        fixture = _load_fixture("settings")
        groups = [{"id": str(i), "key": key} for i, (key, _) in enumerate(fixture, 1)]
        changes = upgrade.plan_settings(fixture, groups, [])
        self.assertEqual(len(changes), sum(len(group["settings"]) for _, group in fixture))
        values = {item["values"]["key"]: item["values"] for item in changes}
        self.assertEqual(values["posts_per_page"]["value_json"], "10")
        self.assertIsNone(values["project_title"]["extra_json"])


if __name__ == "__main__":
    unittest.main()
