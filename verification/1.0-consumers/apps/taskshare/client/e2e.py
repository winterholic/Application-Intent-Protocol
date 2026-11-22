"""TaskShare 소비자 e2e 두 번째 언어: Python SDK(wheel 설치)와 생성된 Python binding으로 같은 서버를 호출한다.
TypeScript 시나리오(e2e.ts)와 같은 업무(공유, 권한 거부, 멱등, 낙관적 잠금, 확장)를 Python 호출 경로로 확인한다."""
import os
import time
from aip_sdk import connect, AipError
from todo_binding import CONTRACT_FINGERPRINT

URL = os.environ["AIP_URL"]
RUN = os.environ.get("RUN_ID", str(int(time.time())))
alice = connect(URL, os.environ["TOKEN_ALICE"], CONTRACT_FINGERPRINT)
bob = connect(URL, os.environ["TOKEN_BOB"], CONTRACT_FINGERPRINT)
carol = connect(URL, os.environ["TOKEN_CAROL"], CONTRACT_FINGERPRINT)
anon = connect(URL, None, CONTRACT_FINGERPRINT)
n = 0


def step(name, extra=None):
    global n
    n += 1
    print(f"PASS {n:02d} {name}" + (f" {extra}" if extra is not None else ""))


assert alice.session() == {"actorId": "1"}
step("session alice (python)")
members = alice.read({"read": "Member", "select": ["id", "name", "email"]})
mine = [m for m in members if m["id"] == "1"][0]
other = [m for m in members if m["id"] == "2"][0]
assert mine["email"] == "alice@example.test" and other["email"] is None
step("field policy: own email visible, others hidden")

made = alice.bundle([{"create": "TodoList", "values": {"owner": "1", "title": f"PyList {RUN}", "archived": False}}], key=f"pylist-{RUN}")
assert made["ok"], made
list_id = made["created"][0]["id"]
items = alice.bundle([
    {"create": "TodoItem", "values": {"list": list_id, "createdBy": "1", "title": "tea", "notes": None, "priority": "HIGH", "dueAt": "2026-12-01T00:00:00Z", "done": False, "version": 0, "assignee": None}},
    {"create": "TodoItem", "values": {"list": list_id, "createdBy": "1", "title": "jam", "notes": "plum", "priority": "LOW", "dueAt": None, "done": False, "version": 0, "assignee": None}},
], key=f"pyitems-{RUN}")
assert items["ok"], items
tea, jam = [c["id"] for c in items["created"]]
step("create list and 2 items in one transaction", {"tea": tea, "jam": jam})

edit = alice.apply({"apply": "TodoItem.edit", "target": {"ids": [tea]}, "values": {"title": "green tea"}, "expect": {"version": 0}}, key=f"pyedit-{RUN}")
assert edit["ok"], edit
stale = alice.apply({"apply": "TodoItem.edit", "target": {"ids": [tea]}, "values": {"title": "stale"}, "expect": {"version": 0}}, key=f"pystale-{RUN}")
assert stale["ok"] is False and stale["code"] == "WIRE.CONFLICT", stale
step("patch edit then stale expect -> WIRE.CONFLICT", stale["code"])
d1 = alice.apply({"apply": "TodoItem.complete", "target": {"ids": [jam]}}, key=f"pydone-{RUN}")
d2 = alice.apply({"apply": "TodoItem.complete", "target": {"ids": [jam]}}, key=f"pydone-{RUN}")
assert d1["ok"] and d2["ok"] and d2.get("replayed") is True
step("same key replayed", {"replayed": d2["replayed"]})

lists = alice.read({"read": "TodoList", "select": ["id", "title", "openCount", {"owner": {"select": ["id", "name"]}}, {"TodoItem": {"select": ["id", "title", "done"], "limit": 10}}],
                    "filter": [{"field": "title", "op": "prefix", "value": f"PyList {RUN}"}]})
row = lists[0]
assert row["openCount"] == 1 and row["owner"]["name"] == "Alice" and len(row["TodoItem"]) == 2, row
step("relation read with openCount aggregate", {"openCount": row["openCount"]})

assert carol.read({"read": "TodoList", "select": ["id"], "filter": [{"field": "title", "op": "prefix", "value": f"PyList {RUN}"}]}) == []
step("carol sees no list")
denied = carol.apply({"apply": "TodoItem.complete", "target": {"ids": [tea]}}, key=f"pycarol-{RUN}")
assert denied["ok"] is False
step("carol complete -> denied", denied["code"])
assert anon.read({"read": "TodoList", "select": ["id"]}) == []
step("anonymous read -> no rows")
try:
    alice.read({"read": "Member", "select": ["id", "secret"]})
    raise SystemExit("unexposed field accepted")
except AipError as error:
    step("unexposed field -> rejected", error.code)

share = alice.bundle([{"create": "ListShare", "values": {"list": list_id, "member": "2", "role": "VIEWER"}}], key=f"pyshare-{RUN}")
assert share["ok"], share
share_id = share["created"][0]["id"]
assert len(bob.read({"read": "TodoList", "select": ["id"], "filter": [{"field": "title", "op": "prefix", "value": f"PyList {RUN}"}]})) == 1
step("bob reads the shared list")
viewer = bob.apply({"apply": "TodoItem.complete", "target": {"ids": [tea]}}, key=f"pybob-{RUN}")
assert viewer["ok"] is False
step("viewer complete -> denied", viewer["code"])
viewer_create = bob.bundle([{"create": "TodoItem", "values": {"list": list_id, "createdBy": "2", "title": "x", "notes": None, "priority": "LOW", "dueAt": None, "done": False, "version": 0, "assignee": None}}], key=f"pybobc-{RUN}")
assert viewer_create["ok"] is False
step("viewer create item -> denied", viewer_create["code"])

ext = alice.apply({"extension": "TodoItem.reschedule", "input": {"id": tea, "rule": "WEEKLY"}}, key=f"pyresched-{RUN}")
assert ext["ok"] and ext["output"]["dueAt"] == "2026-12-08T00:00:00Z", ext
step("node write extension reschedule from python", ext["output"])
ext_replay = alice.apply({"extension": "TodoItem.reschedule", "input": {"id": tea, "rule": "WEEKLY"}}, key=f"pyresched-{RUN}")
assert ext_replay.get("replayed") is True
after = alice.read({"read": "TodoItem", "select": ["id", "dueAt", "version"], "filter": [{"field": "id", "op": "eq", "value": tea}]})
assert after[0]["dueAt"] == "2026-12-08T00:00:00Z" and after[0]["version"] == 2, after
step("extension replay does not re-run; result persisted once", after[0])

unshare = alice.apply({"apply": "ListShare.unshare", "target": {"ids": [share_id]}}, key=f"pyunshare-{RUN}")
assert unshare["ok"]
assert bob.read({"read": "TodoList", "select": ["id"], "filter": [{"field": "title", "op": "prefix", "value": f"PyList {RUN}"}]}) == []
step("unshare removes bob's access")
print("ALL PASS")
