"""Protocol tests for the Python caller SDK with an injected transport (no network)."""
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

from aip_sdk import AipError, ScopeConflict, WriteUnsettled, connect  # noqa: E402

FP = "a" * 64
OTHER = "b" * 64


class FakeServer:
    """Records requests and answers from a scripted queue per path."""

    def __init__(self, actors=None):
        self.calls = []
        self.queue = {}
        self.actors = actors or {"t1": "42", "t2": "42", "x": "7"}

    def script(self, path, *responses):
        self.queue.setdefault(path, []).extend(responses)

    def __call__(self, url, headers, body, timeout):
        path = url.split("http://aip", 1)[1]
        self.calls.append((path, dict(headers), json.loads(body)))
        if path == "/session":
            token = headers.get("authorization", "").removeprefix("Bearer ")
            return json.dumps({"ok": True, "principal": {"actorId": self.actors[token]}, "remainingMs": 1000}).encode()
        item = self.queue[path].pop(0)
        if isinstance(item, Exception):
            raise item
        return json.dumps(item).encode()


class PythonSdkTest(unittest.TestCase):
    def test_discovers_fingerprint_and_sends_it_on_contract_paths(self):
        server = FakeServer()
        server.script("/capabilities", {"ok": True, "contractFingerprint": FP, "operations": {}})
        server.script("/read", {"ok": True, "rows": [{"id": "11"}], "deps": [], "contractFingerprint": FP})
        client = connect("http://aip", "t1", transport=server)
        self.assertEqual(client.read({"read": "Event", "select": ["id"]}), [{"id": "11"}])
        capabilities = next(c for c in server.calls if c[0] == "/capabilities")
        read = next(c for c in server.calls if c[0] == "/read")
        self.assertNotIn("x-aip-contract", capabilities[1])
        self.assertEqual(read[1]["x-aip-contract"], FP)
        self.assertEqual(read[1]["authorization"], "Bearer t1")

    def test_pinned_fingerprint_detects_a_different_deployment(self):
        server = FakeServer()
        server.script("/capabilities", {"ok": True, "contractFingerprint": OTHER})
        server.script("/operation", {"ok": True, "output": {"length": 3}, "contractFingerprint": OTHER})
        client = connect("http://aip", "t1", FP, transport=server)
        with self.assertRaises(AipError) as caught:
            client.discover()
        self.assertEqual(caught.exception.code, "CONTRACT_MISMATCH")
        with self.assertRaises(AipError) as caught:
            client.operation("textLength", {"text": "abc"})
        self.assertEqual(caught.exception.code, "CONTRACT_MISMATCH")

    def test_lost_response_is_resent_with_the_same_key_and_body(self):
        server = FakeServer()
        server.script("/apply", ConnectionResetError(), {"ok": False, "code": "COMMIT_UNKNOWN"},
                      {"ok": True, "changed": ["11"], "unchanged": [], "tags": ["Event"], "replayed": True})
        client = connect("http://aip", "t1", FP, transport=server)
        result = client.apply({"apply": "Event.mark", "target": {"ids": ["11"]}}, key="k1")
        self.assertTrue(result["recovered"])
        bodies = [c[2] for c in server.calls if c[0] == "/apply"]
        self.assertEqual(len(bodies), 3)
        self.assertTrue(all(body == bodies[0] and body["key"] == "k1" for body in bodies))
        self.assertEqual(client.pending(), [])

    def test_retry_rejected_before_execution_stays_unsettled(self):
        server = FakeServer()
        server.script("/apply", TimeoutError(), {"ok": False, "code": "DB_UNAVAILABLE"}, {"ok": False, "code": "DB_UNAVAILABLE"})
        client = connect("http://aip", "t1", FP, transport=server)
        with self.assertRaises(WriteUnsettled) as caught:
            client.apply({"apply": "Event.mark", "target": {"ids": ["11"]}}, key="k2")
        self.assertEqual(caught.exception.key, "k2")
        self.assertEqual(client.pending(), ["k2"])
        mismatch = client.apply({"apply": "Event.mark", "target": {"ids": ["12"]}}, key="k2")
        self.assertEqual(mismatch["code"], "IDEMPOTENCY_MISMATCH")

    def test_first_attempt_business_rejection_is_final(self):
        server = FakeServer()
        server.script("/apply", {"ok": False, "code": "INVALID_STATE"})
        client = connect("http://aip", "t1", FP, transport=server)
        self.assertEqual(client.apply({"apply": "Event.mark", "target": {"ids": ["11"]}}, key="k3")["code"], "INVALID_STATE")
        self.assertEqual(client.pending(), [])

    def test_session_renewal_keeps_principal_scope(self):
        server = FakeServer()
        server.script("/apply", OSError(), OSError(), OSError(), {"ok": True, "changed": [], "unchanged": ["11"], "tags": [], "replayed": True})
        client = connect("http://aip", "t1", FP, transport=server)
        with self.assertRaises(WriteUnsettled):
            client.apply({"apply": "Event.mark", "target": {"ids": ["11"]}}, key="k4")
        with self.assertRaises(ScopeConflict):
            client.replace_session("x")
        settled = client.replace_session("t2")
        self.assertEqual([item["key"] for item in settled], ["k4"])
        self.assertEqual(server.calls[-1][1]["authorization"], "Bearer t2")

    def test_jobs_files_and_events_use_managed_routes(self):
        server = FakeServer()
        server.script("/jobs/enqueue", {"ok": True, "run": {"id": "j1", "state": "QUEUED"}, "contractFingerprint": FP})
        server.script("/files/put", {"ok": True, "file": {"id": "f1"}, "contractFingerprint": FP})
        server.script("/files/get", {"ok": True, "file": {"id": "f1"}, "base64": "aGk=", "contractFingerprint": FP})
        server.script("/events/read", {"ok": True, "events": [{"cursor": "1", "payload": {}}], "cursor": "1", "contractFingerprint": FP})
        client = connect("http://aip", "t1", FP, transport=server)
        self.assertEqual(client.jobs.enqueue("report", {"text": "x"}, key="r1")["id"], "j1")
        self.assertEqual(client.files.put("reports", "a.txt", b"hi", "text/plain")["id"], "f1")
        self.assertEqual(client.files.get("f1"), b"hi")
        self.assertEqual(client.events.read("notifications")["cursor"], "1")
        enqueue = next(c for c in server.calls if c[0] == "/jobs/enqueue")
        self.assertEqual(enqueue[2], {"operation": "report", "input": {"text": "x"}, "key": "r1", "delayMs": 0})
        self.assertEqual(enqueue[1]["x-aip-contract"], FP)

    def test_rejects_malformed_fingerprint_and_server_errors(self):
        with self.assertRaises(AipError):
            connect("http://aip", "t1", "not-a-fingerprint")
        server = FakeServer()
        server.script("/read", {"ok": False, "code": "ACCESS_DENIED"})
        with self.assertRaises(AipError) as caught:
            connect("http://aip", "t1", FP, transport=server).read({"read": "Event"})
        self.assertEqual(caught.exception.code, "ACCESS_DENIED")


if __name__ == "__main__":
    unittest.main()
