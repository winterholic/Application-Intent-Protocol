#!/usr/bin/env python3
"""FlaskBB `install` data seed for an AIP-owned schema (F13).

Original `flaskbb install` creates the tables, then the default groups, the
default settings, an admin user and (optionally) a welcome forum. In AIP the
tables come from `aip service init`; this script emits the remaining rows as
SQL, reading the pinned upstream fixtures themselves instead of copying them:

    aip service init porting/flaskbb/app.aip --config service.json
    python3 porting/flaskbb/seed/install.py --schema <schema> \
        --admin-username admin --admin-email admin@example.test --welcome \
        | psql -v ON_ERROR_STOP=1 -d "$DATABASE_URL"
    aip service principal --config service.json bind --subject <idp-sub> --actor 1

There is no admin password: authentication belongs to the identity provider and
the admin account is reached by binding an IdP subject to actor 1. Only the
standard library is used. Group ids follow the fixture order (Administrator=1,
Super Moderator=2, Moderator=3, Member=4, Banned=5, Guest=6) like the original.
"""

import argparse
import enum
import importlib.util
import json
import os
import re
import sys
import types
import uuid

UPSTREAM = os.path.join(os.path.dirname(__file__), "..", "upstream", "flaskbb", "fixtures")


class SettingValueType(enum.Enum):
    """Mirror of flaskbb.utils.forms.SettingValueType (values are not stored; names are)."""

    string = 0
    integer = 1
    float = 3
    boolean = 4
    select = 5
    selectmultiple = 6


def _load_fixture(name):
    """Import an upstream fixture module with the two flaskbb helpers it needs stubbed."""
    forms = types.ModuleType("flaskbb.utils.forms")
    forms.SettingValueType = SettingValueType
    helpers = types.ModuleType("flaskbb.utils.helpers")
    # Callable `choices` are resolved at runtime in the original; the seed records their names.
    helpers.get_available_themes = lambda: []
    helpers.get_available_languages = lambda: []
    for pkg in ("flaskbb", "flaskbb.utils"):
        sys.modules.setdefault(pkg, types.ModuleType(pkg))
    sys.modules["flaskbb.utils.forms"] = forms
    sys.modules["flaskbb.utils.helpers"] = helpers
    spec = importlib.util.spec_from_file_location(f"flaskbb_fixture_{name}", os.path.join(UPSTREAM, f"{name}.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.fixture


def literal(value):
    if value is None:
        return "NULL"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return str(value)
    return "'" + str(value).replace("'", "''") + "'"


def jsonable(value):
    if callable(value):
        return value.__name__
    if isinstance(value, dict):
        return {k: jsonable(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [jsonable(v) for v in value]
    return value


def insert(schema, table, columns, rows):
    cols = ",".join(f'"{c}"' for c in columns)
    values = ",\n  ".join("(" + ",".join(literal(v) for v in row) + ")" for row in rows)
    return f'INSERT INTO "{schema}"."{table}" ({cols}) VALUES\n  {values};'


def build(schema, admin_username, admin_email, welcome):
    groups = _load_fixture("groups")
    settings = _load_fixture("settings")
    out = ["BEGIN;"]

    group_columns = ["id", "name", "description", "admin", "super_mod", "mod", "banned", "guest", "editpost", "deletepost",
                     "deletetopic", "posttopic", "postreply", "mod_edituser", "mod_banuser", "viewhidden", "makehidden"]
    group_ids = {}
    group_rows = []
    for index, (name, flags) in enumerate(groups.items(), start=1):
        group_ids[name] = index
        group_rows.append([index, name, flags["description"]] + [flags[c] for c in group_columns[3:]])
    out.append(insert(schema, "group", group_columns, group_rows))

    setting_groups, setting_rows = [], []
    for gid, (key, group) in enumerate(settings, start=1):
        setting_groups.append([gid, key, group["name"], group["description"]])
        for skey, setting in group["settings"]:
            extra = setting.get("extra")
            setting_rows.append([
                len(setting_rows) + 1, skey, json.dumps(jsonable(setting["value"])), gid, setting["name"],
                setting["description"], setting["value_type"].name, json.dumps(jsonable(extra)) if extra is not None else None,
            ])
    out.append(insert(schema, "settings_group", ["id", "key", "name", "description"], setting_groups))
    out.append(insert(schema, "setting", ["id", "key", "value_json", "group_id", "name", "description", "value_type", "extra_json"], setting_rows))

    admin_group = group_ids["Administrator"]
    out.append(insert(schema, "user", ["id", "username", "email", "date_joined", "lastseen", "primary_group_id", "activated", "language"],
                      [[1, admin_username, admin_email, "now()", "now()", admin_group, True, "en"]]).replace("'now()'", "now()"))

    if welcome:
        topic, post = str(uuid.uuid4()), str(uuid.uuid4())
        out.append(insert(schema, "category", ["id", "title", "description", "position"], [[1, "My Category", None, 1]]))
        out.append(insert(schema, "forum", ["id", "category_id", "title", "description", "position", "locked", "show_moderators"],
                          [[1, 1, "Welcome", "Your first forum", 1, False, False]]))
        out.append(insert(schema, "forum_group_access", ["id", "forum_id", "group_id"],
                          [[i, 1, gid] for i, gid in enumerate(group_ids.values(), start=1)]))
        out.append(insert(schema, "topic", ["id", "forum_id", "title", "user_id", "username", "date_created", "last_updated", "locked",
                                             "important", "views", "published", "deleted", "hidden"],
                          [[topic, 1, "Welcome!", 1, admin_username, "now()", "now()", False, False, 0, True, False, False]]).replace("'now()'", "now()"))
        out.append(insert(schema, "post", ["id", "topic_id", "forum_id", "user_id", "username", "content", "date_created", "hidden",
                                            "published", "version", "deleted"],
                          [[post, topic, 1, 1, admin_username, "Have fun with your new FlaskBB Forum!", "now()", False, True, 0, False]]).replace("'now()'", "now()"))
        out.append(insert(schema, "topic_first_post", ["id", "topic_id", "post_id"], [[1, topic, post]]))
        out.append(f'UPDATE "{schema}"."forum" SET last_post_id = {literal(post)}, last_post_user_id = 1, '
                   f'last_post_title = \'Welcome!\', last_post_username = {literal(admin_username)}, last_post_created = now() WHERE id = 1;')

    tables = ["group", "settings_group", "setting", "user"] + (["category", "forum", "forum_group_access", "topic_first_post"] if welcome else [])
    for table in tables:
        out.append(f"SELECT setval(pg_get_serial_sequence('\"{schema}\".\"{table}\"', 'id'), (SELECT max(id) FROM \"{schema}\".\"{table}\"));")
    out.append("COMMIT;")
    return "\n".join(out) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--schema", required=True)
    parser.add_argument("--admin-username", default="admin")
    parser.add_argument("--admin-email", required=True)
    parser.add_argument("--welcome", action="store_true", help="create the welcome category, forum and topic")
    args = parser.parse_args(argv)
    if not re.fullmatch(r"[a-z_][a-z0-9_]{0,62}", args.schema):
        parser.error("schema must be a plain lowercase identifier")
    sys.stdout.write(build(args.schema, args.admin_username, args.admin_email, args.welcome))


if __name__ == "__main__":
    main()
