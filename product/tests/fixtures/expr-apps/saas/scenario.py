"""협업형 SaaS 시나리오(Python SDK). TypeScript 시나리오와 같은 의미를 생성 helper로 확인한다."""
from aip_sdk import connect
from binding import (CONTRACT_FINGERPRINT, apply_project_archive, apply_task_complete, apply_task_edit, apply_task_remove,
                     apply_task_start, create_comment, create_project, create_task, read_comment, read_org, read_task, read_user)
from check import BASE_URL, SCHEMA, TOKENS, scenario, sql

assert CONTRACT_FINGERPRINT


def as_(who):
    return connect(BASE_URL, TOKENS[who], CONTRACT_FINGERPRINT)


alice, bob, vera, carol, dave = (as_(n) for n in ("alice", "bob", "vera", "carol", "dave"))
state = {}


def ok(result, what):
    assert result["ok"] is True, f"{what}: {result}"
    return result


def denied(result, code, what):
    assert result["ok"] is False and result["code"] == code, f"{what}: {result}"


def task_values(**kw):
    base = {"org": "1", "title": "t", "assignee": None, "creator": "2", "status": "TODO", "priority": 1, "version": 0}
    base.update(kw)
    return base


def one(client, select, **query):
    rows = read_task(client, {"read": "Task", "select": select, **query})
    assert len(rows) == 1, rows
    return rows[0]


def t_isolation():
    assert [r["name"] for r in read_org(alice, {"read": "Org", "select": ["id", "name"]})] == ["Acme"]
    users = read_user(bob, {"read": "User", "select": ["id", "name", "email"], "sort": [{"field": "id", "dir": "asc"}]})
    assert [u["name"] for u in users] == ["Alice", "Bob", "Vera"]
    assert [u["email"] for u in users] == [None, "bob@acme.test", None]
    assert read_task(alice, {"read": "Task", "select": ["id"]}) == []
    assert [r["title"] for r in read_task(carol, {"read": "Task", "select": ["id", "title"]})] == ["globex-secret-task"]


def t_create_and_relations():
    made = ok(create_project(alice, {"org": "1", "name": "Launch", "status": "ACTIVE", "owner": "1", "version": 0}, key="py-proj"), "project")
    state["project"] = made["created"][0]["id"]
    denied(create_project(bob, {"org": "1", "name": "Rogue", "status": "ACTIVE", "owner": "2", "version": 0}, key="py-proj-bob"), "WIRE.FORBIDDEN", "member project")
    made = ok(create_task(bob, task_values(project=state["project"], title="write spec", description="first draft", assignee="2"), key="py-task-1"), "task")
    state["task"] = made["created"][0]["id"]
    row = one(bob, ["id", "title", "commentCount", {"project": {"select": ["id", "name"]}}, {"assignee": {"select": ["id", "name"]}}],
              filter=[{"field": "id", "op": "eq", "value": state["task"]}])
    assert row["project"] == {"id": state["project"], "name": "Launch"} and row["assignee"] == {"id": "2", "name": "Bob"}
    assert row["commentCount"] == 0


def t_partial_update_and_lock():
    t = state["task"]
    ok(apply_task_edit(bob, {"ids": [t]}, {"title": "v2"}, {"version": 0}, key="py-edit-1"), "partial")
    row = one(bob, ["id", "title", "description", "assignee", "version"], filter=[{"field": "id", "op": "eq", "value": t}])
    assert (row["title"], row["description"], row["assignee"], row["version"]) == ("v2", "first draft", "2", 1), row
    denied(apply_task_edit(alice, {"ids": [t]}, {"priority": 5}, {"version": 0}, key="py-edit-stale"), "WIRE.CONFLICT", "stale")
    assert sql(f"SELECT priority||'|'||version FROM {SCHEMA}.task WHERE id={t}") == "1|1"
    ok(apply_task_edit(alice, {"ids": [t]}, {"priority": 5, "assignee": None}, {"version": 1}, key="py-edit-fresh"), "retry")
    assert sql(f"SELECT priority||'|'||coalesce(assignee_id::text,'null')||'|'||version FROM {SCHEMA}.task WHERE id={t}") == "5|null|2"


def t_replay():
    t = state["task"]
    a = apply_task_edit(alice, {"ids": [t]}, {"description": "replayed"}, {"version": 2}, key="py-replay")
    b = apply_task_edit(alice, {"ids": [t]}, {"description": "replayed"}, {"version": 2}, key="py-replay")
    assert a["ok"] is True and b.get("replayed") is True, (a, b)
    assert sql(f"SELECT version FROM {SCHEMA}.task WHERE id={t}") == "3"


def t_transitions_and_roles():
    t = state["task"]
    denied(apply_task_start(bob, {"ids": [t]}, key="py-start-bob"), "WIRE.FORBIDDEN", "non-assignee member")
    denied(apply_task_start(vera, {"ids": [t]}, key="py-start-vera"), "WIRE.FORBIDDEN", "viewer")
    ok(apply_task_start(alice, {"ids": [t]}, key="py-start"), "start")
    assert apply_task_start(alice, {"ids": [t]}, key="py-start-2")["ok"] is False
    ok(apply_task_complete(alice, {"ids": [t]}, key="py-complete"), "complete")
    assert apply_task_edit(alice, {"ids": [t]}, {"title": "late"}, {"version": 5}, key="py-edit-done")["ok"] is False
    denied(create_task(vera, task_values(project=state["project"], creator="3"), key="py-vera-create"), "WIRE.FORBIDDEN", "viewer create")


def t_cross_tenant():
    t = state["task"]
    denied(apply_task_edit(carol, {"ids": [t]}, {"title": "pwn"}, {"version": 0}, key="py-x-edit"), "WIRE.MISSING_TARGET", "carol edits acme")
    denied(apply_task_edit(alice, {"ids": ["1"]}, {"title": "pwn"}, {"version": 0}, key="py-x-edit2"), "WIRE.MISSING_TARGET", "alice edits globex")
    denied(apply_project_archive(carol, {"ids": [state["project"]]}, key="py-x-archive"), "WIRE.MISSING_TARGET", "carol archives acme project")
    assert read_task(carol, {"read": "Task", "select": ["id"], "filter": [{"field": "id", "op": "eq", "value": t}]}) == []
    denied(create_task(bob, task_values(project="1", title="into globex"), key="py-x-create"), "WIRE.MISSING_TARGET", "foreign project")
    denied(create_comment(carol, {"org": "2", "task": t, "author": "4", "body": "hi"}, key="py-x-comment"), "WIRE.MISSING_TARGET", "foreign task comment")
    assert sql(f"SELECT count(*) FROM {SCHEMA}.task WHERE title IN ('pwn','into globex')") == "0"
    assert sql(f"SELECT title||'|'||version FROM {SCHEMA}.task WHERE id=1") == "globex-secret-task|0"


def t_aggregate_cursor_delete():
    t = state["task"]
    ok(create_comment(bob, {"org": "1", "task": t, "author": "2", "body": "first"}, key="py-c1"), "c1")
    ok(create_comment(bob, {"org": "1", "task": t, "author": "2", "body": "second"}, key="py-c2"), "c2")
    row = one(alice, ["id", "commentCount", {"comments": {"select": ["id", "body"], "limit": 10}}], filter=[{"field": "id", "op": "eq", "value": t}])
    assert row["commentCount"] == 2 and [c["body"] for c in row["comments"]] == ["first", "second"]
    assert read_comment(carol, {"read": "Comment", "select": ["id"]}) == []
    for i in range(7):
        ok(create_task(alice, task_values(project=state["project"], title=f"bulk-{i}", creator="1", priority=i % 5 + 1), key=f"py-bulk-{i}"), "bulk")
    seen, after = [], None
    for _ in range(10):
        q = {"read": "Task", "select": ["id"], "filter": [{"field": "project", "op": "eq", "value": state["project"]}], "sort": [{"field": "id", "dir": "desc"}], "limit": 3}
        if after:
            q["after"] = after
        rows = read_task(alice, q)
        if not rows:
            break
        seen += [r["id"] for r in rows]
        after = {"id": rows[-1]["id"]}
        if len(rows) < 3:
            break
    expected = sql(f"SELECT string_agg(id::text, ',' ORDER BY id DESC) FROM {SCHEMA}.task WHERE project_id={state['project']}").split(",")
    assert seen == expected and len(seen) == 8, (seen, expected)
    denied(apply_task_remove(bob, {"ids": [t]}, key="py-rm-bob"), "WIRE.FORBIDDEN", "member delete")
    denied(apply_task_remove(dave, {"ids": [t]}, key="py-rm-dave"), "WIRE.MISSING_TARGET", "foreign delete")
    ok(apply_task_remove(alice, {"ids": [t]}, key="py-rm-alice"), "admin delete")
    assert sql(f"SELECT count(*) FROM {SCHEMA}.task WHERE id={t}") == "0" and sql(f"SELECT count(*) FROM {SCHEMA}.comment") == "0"


assert alice.session() == {"actorId": "1"}
scenario("tenant-read-isolation+field-policy", t_isolation)
scenario("create-relations-and-role-guard", t_create_and_relations)
scenario("partial-update-optimistic-lock", t_partial_update_and_lock)
scenario("idempotent-replay-single-effect", t_replay)
scenario("transitions-and-role-guards", t_transitions_and_roles)
scenario("cross-tenant-negatives", t_cross_tenant)
scenario("aggregate-cursor-delete-cascade", t_aggregate_cursor_delete)
