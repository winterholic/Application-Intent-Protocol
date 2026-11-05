"""Update the pinned settings fixture through the authenticated AIP SDK.

Missing rows are added; existing rows change only with --force. Groups commit
before settings so their server-assigned ids can be resolved. As upstream, this
is not one transaction across the complete fixture. No direct SQL is used.
"""
import argparse
import json
import os
import sys

if __package__:
    from .install import _load_fixture, jsonable
else:
    from install import _load_fixture, jsonable


GROUP_FIELDS = ["id", "key", "name", "description"]
SETTING_FIELDS = ["id", "key", "value_json", "group", "name", "description", "value_type", "extra_json"]


def _operation(resource, key, values, existing, force):
    if existing is None:
        return {"create": resource, "values": {"key": key, **values}}
    if force and any(existing[field] != value for field, value in values.items()):
        return {"apply": f"{resource}.edit", "target": {"ids": [existing["id"]]}, "values": values}
    return None

def plan_groups(fixture, rows, force=False):
    by_key = {row["key"]: row for row in rows}
    result = []
    for key, group in fixture:
        values = {field: group[field] for field in ("name", "description")}
        operation = _operation("SettingsGroup", key, values, by_key.get(key), force)
        if operation is not None:
            result.append(operation)
    return result


def plan_settings(fixture, groups, rows, force=False):
    group_ids = {row["key"]: row["id"] for row in groups}
    by_key = {row["key"]: row for row in rows}
    result = []
    for group_key, group in fixture:
        if group_key not in group_ids:
            raise ValueError(f"settings group was not resolved: {group_key}")
        for key, setting in group["settings"]:
            extra = setting.get("extra")
            values = {
                "value_json": json.dumps(jsonable(setting["value"])),
                "group": group_ids[group_key],
                "name": setting["name"], "description": setting["description"],
                "value_type": setting["value_type"].name,
                "extra_json": json.dumps(jsonable(extra)) if extra is not None else None,
            }
            operation = _operation("Setting", key, values, by_key.get(key), force)
            if operation is not None:
                result.append(operation)
    return result


def _read_fixture_rows(client, resource, fields, keys):
    # Key lookups avoid treating a budget-truncated listing as a complete
    # snapshot when an installation also contains custom settings.
    result = []
    for key in keys:
        rows = client.read({"read": resource, "select": fields,
                            "filter": [{"field": "key", "op": "eq", "value": key}], "limit": 1})
        result.extend(rows)
    return result


def upgrade_settings(client, fixture, force=False):
    def read_groups():
        return _read_fixture_rows(client, "SettingsGroup", GROUP_FIELDS, [key for key, _ in fixture])

    def commit(operations):
        # One operation per bundle stays within product budgets for arbitrary
        # fixture size and matches upstream's independently committed saves.
        for operation in operations:
            response = client.bundle([operation])
            if response.get("ok") is not True:
                raise RuntimeError(f"settings upgrade refused: {response.get('code', 'unknown')}")

    groups = plan_groups(fixture, read_groups(), force)
    commit(groups)
    keys = [key for _, group in fixture for key, _ in group["settings"]]
    settings = plan_settings(fixture, read_groups(), _read_fixture_rows(client, "Setting", SETTING_FIELDS, keys), force)
    commit(settings)
    return {"groups_written": len(groups), "settings_written": len(settings)}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--url", required=True)
    parser.add_argument("--token-env", default="AIP_FLASKBB_ADMIN_TOKEN")
    parser.add_argument("--fixture", choices=["settings"], default="settings")
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args(argv)
    token = os.environ.get(args.token_env)
    if not token:
        parser.error(f"missing credential environment variable: {args.token_env}")
    from aip_sdk import Client, WriteUnsettled
    try:
        result = upgrade_settings(Client(args.url, token), _load_fixture(args.fixture), args.force)
    except WriteUnsettled as failure:
        print(json.dumps({"ok": False, "code": failure.code, "key": failure.key,
                          "request": failure.request, "requestId": failure.request_id,
                          "retry": failure.retry}), file=sys.stderr)
        raise SystemExit(1) from failure
    except Exception as failure:
        raise SystemExit(f"settings upgrade failed: {failure}") from failure
    print(json.dumps({"ok": True, **result}))


if __name__ == "__main__":
    main()
