"""Python SDK consumer run by product/tools/smoke.mjs against a real `aip service serve`.

It runs after the TypeScript consumer on the same deployment and principal, so it also checks
that both SDKs share one idempotency namespace and one contract fingerprint.
"""
import json
import os

from aip_sdk import AipError, connect

base = os.environ["AIP_SMOKE_BASE"]
token = os.environ["AIP_SMOKE_TOKEN"]
fingerprint = os.environ["AIP_SMOKE_FINGERPRINT"]
wire = os.environ["AIP_SMOKE_WIRE"]
target = "11" if wire == "decimal" else 11

client = connect(base, token, fingerprint)
catalog = client.discover()
rows = client.read({"read": "Event", "select": ["id", "checked"]})
if rows != [{"id": target, "checked": True}]:
    raise SystemExit(f"python read mismatch: {rows}")
request = {"apply": "Event.mark", "target": {"ids": [target]}}
# The TypeScript consumer committed this key; the Python client must observe the same write.
replay = client.apply(request, key="product-sdk-smoke-write")
status = client.status(request, "product-sdk-smoke-write")
drift = connect(base, token, "0" * 64)
try:
    drift.read({"read": "Event", "select": ["id"]})
    drift_rejected = False
except AipError as error:
    drift_rejected = error.code == "CONTRACT_MISMATCH"

report = {
    "catalog": catalog.get("catalogVersion") == "aip-capabilities/1",
    "readRows": len(rows),
    "sharedReplay": replay.get("ok") is True and replay.get("replayed") is True,
    "statusReplay": status.get("ok") is True and status.get("replayed") is True,
    "driftRejected": drift_rejected,
}
if os.environ.get("AIP_SMOKE_WORKER"):
    length = client.operation("textLength", {"text": "가😀A"})["length"]
    try:
        client.operation("textLength", {"text": ""})
        invalid = False
    except AipError as error:
        invalid = error.code == "BAD_VALUE"
    job = client.jobs.enqueue("textLengthJob", {"text": "가😀A"}, key="product-python-smoke-job")
    again = client.jobs.enqueue("textLengthJob", {"text": "가😀A"}, key="product-python-smoke-job")
    run = client.jobs.wait(job["id"], timeout=20)
    result = client.jobs.result(job["id"]) if run["state"] == "SUCCEEDED" else None
    report["operation"] = {
        "length": length,
        "invalidInputRejected": invalid,
        "job": {"status": run["state"], "length": (result or {}).get("length"), "replayedId": job["id"] == again["id"]},
    }
print(json.dumps(report, ensure_ascii=False))
