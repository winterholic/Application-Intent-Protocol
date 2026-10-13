"""V16: the Python caller SDK sends bundles and composes to a real server (run by tests/v16_bundle.rs)."""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "..", "product", "python"))

import aip_sdk  # noqa: E402
from aip_sdk import connect  # noqa: E402

url, token = os.environ["AIP_V16_URL"], os.environ["AIP_V16_TOKEN"]
string_wire = os.environ["AIP_V16_WIRE"] == "string"


def ident(n):
    return str(n) if string_wire else n


def task(project, title, points=1):
    return {"create": "Task", "values": {"project": ident(project), "author": ident(1), "title": title, "points": points}}


client = connect(url, token)

# a bundle of a create and a transition commits together; the same key replays the stored result
operations = [task(12, "py-1", 2), {"apply": "Project.archive", "target": {"ids": [ident(12)]}}]
first = client.bundle(operations, key="py-b1")
assert first["ok"] is True, first
assert first["created"][0]["resource"] == "Task" and first["applied"] == [{"changed": [ident(12)], "unchanged": []}], first
replay = client.bundle(operations, key="py-b1")
assert replay["replayed"] is True and replay["created"] == first["created"], replay

# a lost response is recovered with the same key against the real server: the bundle commits once
real = aip_sdk._urllib_transport
dropped = []


def lossy(url_, headers, body, timeout):
    answer = real(url_, headers, body, timeout)
    if url_.endswith("/apply") and not dropped:
        dropped.append(True)
        raise ConnectionResetError("response lost after the commit")
    return answer


lossy_client = connect(url, token, transport=lossy)
recovered = lossy_client.bundle([task(13, "py-lost", 3)], key="py-lost")
assert recovered["ok"] is True and recovered["recovered"] is True and recovered["replayed"] is True, recovered
assert lossy_client.pending() == []

# a failing operation cancels the whole bundle and is final (no pending key)
rolled = client.bundle([task(13, "py-ghost"), {"apply": "Project.archive", "target": {"ids": [ident(12)]}}], key="py-b2")
assert rolled["ok"] is False and rolled["code"] == "WIRE.INVALID_STATE", rolled
assert client.pending() == []

# a create that points at a row the caller cannot read looks the same as a missing row
theirs = client.bundle([task(20, "py-theirs")], key="py-b3")
nothing = client.bundle([task(424242, "py-nothing")], key="py-b4")
assert theirs["code"] == nothing["code"] == "WIRE.MISSING_TARGET" and theirs["msg"] == nothing["msg"], (theirs, nothing)

# compose: the values come from the target row the server reads
steps = [{"transition": "archive"}, {"create": "Log", "values": {"owner": {"item": "owner"}, "name": {"item": "name"}}}]
composed = client.compose("Project", [ident(15)], steps, key="py-c1")
assert composed["ok"] is True and composed["created"][0]["resource"] == "Log", composed
again = client.compose("Project", [ident(15)], steps, key="py-c1")
assert again["replayed"] is True, again
print("python bundle ok")
