"""Protocol tests for the Python caller SDK with an injected transport (no network)."""
import http.server
import json
import os
import sys
import threading
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
    def test_session_returns_the_verified_actor_id_from_aip(self):
        server = FakeServer()
        client = connect("http://aip", "t1", transport=server)

        self.assertEqual(client.session(), {"actorId": "42"})
        self.assertEqual([call[0] for call in server.calls], ["/session"])

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
        self.assertEqual(caught.exception.code, "WIRE.CONTRACT_MISMATCH")
        with self.assertRaises(AipError) as caught:
            client.operation("textLength", {"text": "abc"})
        self.assertEqual(caught.exception.code, "WIRE.CONTRACT_MISMATCH")

    def test_lost_response_is_resent_with_the_same_key_and_body(self):
        server = FakeServer()
        server.script("/apply", ConnectionResetError(), {"ok": False, "code": "WIRE.COMMIT_UNKNOWN"},
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
        server.script("/apply", TimeoutError(), {"ok": False, "code": "WIRE.DB_UNAVAILABLE"}, {"ok": False, "code": "WIRE.DB_UNAVAILABLE"})
        client = connect("http://aip", "t1", FP, transport=server)
        with self.assertRaises(WriteUnsettled) as caught:
            client.apply({"apply": "Event.mark", "target": {"ids": ["11"]}}, key="k2")
        self.assertEqual(caught.exception.key, "k2")
        self.assertEqual(client.pending(), ["k2"])
        mismatch = client.apply({"apply": "Event.mark", "target": {"ids": ["12"]}}, key="k2")
        self.assertEqual(mismatch["code"], "WIRE.IDEMPOTENCY_MISMATCH")

    def test_first_attempt_business_rejection_is_final(self):
        server = FakeServer()
        server.script("/apply", {"ok": False, "code": "WIRE.INVALID_STATE"})
        client = connect("http://aip", "t1", FP, transport=server)
        self.assertEqual(client.apply({"apply": "Event.mark", "target": {"ids": ["11"]}}, key="k3")["code"], "WIRE.INVALID_STATE")
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

    def test_bundle_sends_one_atomic_request_and_a_lost_response_replays_the_stored_result(self):
        server = FakeServer()
        stored = {"ok": True, "applied": [{"changed": ["10"], "unchanged": []}], "created": [{"resource": "Task", "id": "5"}],
                  "tags": ["Project", "Task"], "replayed": True}
        server.script("/apply", ConnectionResetError(), stored)
        client = connect("http://aip", "t1", FP, transport=server)
        operations = [{"create": "Task", "values": {"project": "10", "title": "t"}}, {"apply": "Project.archive", "target": {"ids": ["10"]}}]
        result = client.bundle(operations, key="b1")
        self.assertTrue(result["recovered"] and result["replayed"])
        self.assertEqual(result["created"], [{"resource": "Task", "id": "5"}])
        bodies = [c[2] for c in server.calls if c[0] == "/apply"]
        self.assertEqual([b["request"] for b in bodies], [{"atomic": operations}] * 2)
        self.assertEqual({b["key"] for b in bodies}, {"b1"})
        self.assertEqual(client.pending(), [])
        with self.assertRaises(AipError) as caught:
            client.bundle([])
        self.assertEqual(caught.exception.code, "WIRE.BAD_REQUEST")

    def test_a_malformed_bundle_result_is_not_trusted_and_the_key_stays_pending(self):
        server = FakeServer()
        # a success that does not say what it created is unconfirmed; every attempt is the same request
        server.script("/apply", {"ok": True, "changed": ["10"], "unchanged": [], "tags": []}, {"ok": True, "applied": "none", "created": []},
                      {"ok": True, "applied": [], "created": [{"id": "5"}]})
        client = connect("http://aip", "t1", FP, transport=server)
        with self.assertRaises(WriteUnsettled) as caught:
            client.bundle([{"apply": "Project.archive", "target": {"ids": ["10"]}}], key="b2")
        self.assertEqual(caught.exception.key, "b2")
        self.assertEqual(client.pending(), ["b2"])

    def test_bundle_business_rejection_is_final_and_compose_builds_its_request(self):
        server = FakeServer()
        server.script("/apply", {"ok": False, "code": "WIRE.INVALID_STATE"}, {"ok": True, "applied": [{"changed": ["11"], "unchanged": []}],
                      "created": [{"resource": "Log", "id": "7"}], "tags": ["Project", "Log"]})
        client = connect("http://aip", "t1", FP, transport=server)
        self.assertEqual(client.bundle([{"apply": "Project.archive", "target": {"ids": ["10"]}}], key="b3")["code"], "WIRE.INVALID_STATE")
        steps = [{"transition": "archive"}, {"create": "Log", "values": {"owner": {"item": "owner"}, "name": {"item": "name"}}}]
        done = client.compose("Project", ["11"], steps, key="c1")
        self.assertEqual(done["created"], [{"resource": "Log", "id": "7"}])
        request = [c[2] for c in server.calls if c[0] == "/apply"][-1]["request"]
        self.assertEqual(request, {"compose": "Project", "targets": {"ids": ["11"]}, "steps": steps})
        self.assertEqual(client.pending(), [])

    def test_rejects_malformed_fingerprint_and_server_errors(self):
        with self.assertRaises(AipError):
            connect("http://aip", "t1", "not-a-fingerprint")
        server = FakeServer()
        server.script("/read", {"ok": False, "code": "WIRE.ACCESS_DENIED"})
        with self.assertRaises(AipError) as caught:
            connect("http://aip", "t1", FP, transport=server).read({"read": "Event"})
        self.assertEqual(caught.exception.code, "WIRE.ACCESS_DENIED")


class RequestIdTest(unittest.TestCase):
    """The server names every request in the `x-request-id` response header; a failure carries it (the body is unchanged)."""

    ID = "req_0123456789abcdef01234567"

    def serve(self, reply, header=ID):
        def transport(url, headers, body, timeout):
            path = url.split("http://aip", 1)[1]
            if path == "/session":
                return json.dumps({"ok": True, "principal": {"actorId": "42"}, "remainingMs": 1000}).encode()
            raw = json.dumps(reply).encode()
            return raw if header is None else (raw, {"X-Request-Id": header})

        return connect("http://aip", "t1", FP, transport=transport)

    def test_a_rejection_carries_the_request_id(self):
        with self.assertRaises(AipError) as caught:
            self.serve({"ok": False, "code": "WIRE.FILTER_NOT_ALLOWED"}).read({"read": "Event", "select": ["id"]})
        self.assertEqual((caught.exception.code, caught.exception.request_id), ("WIRE.FILTER_NOT_ALLOWED", self.ID))

    def test_a_contract_mismatch_and_a_managed_failure_carry_it_too(self):
        with self.assertRaises(AipError) as caught:
            self.serve({"ok": True, "rows": [], "deps": [], "contractFingerprint": OTHER}).read({"read": "Event", "select": ["id"]})
        self.assertEqual((caught.exception.code, caught.exception.request_id), ("WIRE.CONTRACT_MISMATCH", self.ID))
        with self.assertRaises(AipError) as caught:
            self.serve({"ok": False, "code": "WIRE.NOT_FOUND"}).jobs.status("x")
        self.assertEqual(caught.exception.request_id, self.ID)

    def test_a_failed_write_outcome_and_an_unsettled_write_carry_it(self):
        outcome = self.serve({"ok": False, "code": "WIRE.BAD_VALUE"}).apply({"apply": "A.b"}, key="k1")
        self.assertEqual((outcome["ok"], outcome["code"], outcome["requestId"]), (False, "WIRE.BAD_VALUE", self.ID))
        with self.assertRaises(WriteUnsettled) as caught:
            self.serve({"ok": False, "code": "WIRE.COMMIT_UNKNOWN"}).apply({"apply": "A.b"}, key="k2", retries=0)
        self.assertEqual(caught.exception.request_id, self.ID)

    def test_a_success_and_a_missing_or_unsafe_header_add_nothing(self):
        ok = self.serve({"ok": True, "tags": [], "changed": [], "unchanged": []}).apply({"apply": "A.b"}, key="k3")
        self.assertNotIn("requestId", ok)
        for header in (None, "short", "has space in it", "x" * 65, "semi;colon-1234"):
            with self.assertRaises(AipError) as caught:
                self.serve({"ok": False, "code": "WIRE.NOT_FOUND"}, header=header).read({"read": "Event", "select": ["id"]})
            self.assertIsNone(caught.exception.request_id, header)

    def test_the_body_requestId_and_retry_win_over_the_header_and_reach_the_error(self):
        body_id = "req_aaaaaaaaaaaaaaaaaaaaaaaa"
        # CD-2: the failure body names its request and says whether to send it again; the body value is preferred
        with self.assertRaises(AipError) as caught:
            self.serve({"ok": False, "code": "WIRE.DB_UNAVAILABLE", "requestId": body_id, "retry": "later"}, header=None).read({"read": "Event", "select": ["id"]})
        self.assertEqual((caught.exception.code, caught.exception.request_id, caught.exception.retry), ("WIRE.DB_UNAVAILABLE", body_id, "later"))
        with self.assertRaises(AipError) as caught:
            self.serve({"ok": False, "code": "WIRE.NOT_FOUND", "requestId": body_id, "retry": "never"}).read({"read": "Event", "select": ["id"]})
        self.assertEqual((caught.exception.request_id, caught.exception.retry), (body_id, "never"))
        # a failed write outcome keeps both fields in its body; an unknown or malformed retry value is not trusted
        outcome = self.serve({"ok": False, "code": "WIRE.BAD_VALUE", "requestId": body_id, "retry": "never"}).apply({"apply": "A.b"}, key="k9")
        self.assertEqual((outcome["requestId"], outcome["retry"]), (body_id, "never"))
        for bad in ("sometimes", ["later"], None):
            with self.assertRaises(AipError) as caught:
                self.serve({"ok": False, "code": "WIRE.NOT_FOUND", "retry": bad}).read({"read": "Event", "select": ["id"]})
            self.assertIsNone(caught.exception.retry, bad)

    def test_the_default_transport_reads_the_header_of_a_real_reply(self):
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers.get("content-length", 0)))
                body = json.dumps({"ok": True, "principal": {"actorId": "42"}, "remainingMs": 1000} if self.path == "/session" else {"ok": False, "code": "WIRE.ACCESS_DENIED"}).encode()
                self.send_response(200)
                self.send_header("content-length", str(len(body)))
                self.send_header("x-request-id", outer.ID)
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            client = connect(f"http://127.0.0.1:{server.server_port}", "t1", FP)
            with self.assertRaises(AipError) as caught:
                client.read({"read": "Event", "select": ["id"]})
            self.assertEqual((caught.exception.code, caught.exception.request_id), ("WIRE.ACCESS_DENIED", self.ID))
        finally:
            server.shutdown()


if __name__ == "__main__":
    unittest.main()


class TransportBoundaryTest(unittest.TestCase):
    def test_refuses_clear_text_remote_and_non_http_schemes(self):
        for base in ("http://example.com", "ftp://example.com", "file:///etc/passwd"):
            client = connect(base, "t1", FP)
            with self.assertRaises(AipError) as raised:
                client.read({"read": "Event", "select": ["id"]})
            self.assertEqual(raised.exception.code, "CLIENT.PROTOCOL_ERROR", base)

    def test_does_not_follow_redirects_with_the_bearer_token(self):
        seen = []

        class Sink(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                seen.append(self.headers.get("authorization"))
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b'{"ok":true}')

            def log_message(self, *args):
                pass

        sink = http.server.HTTPServer(("127.0.0.1", 0), Sink)
        target = f"http://127.0.0.1:{sink.server_port}/steal"

        class Redirect(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers.get("content-length", 0)))
                self.send_response(302)
                self.send_header("location", target)
                self.end_headers()

            def log_message(self, *args):
                pass

        redirector = http.server.HTTPServer(("127.0.0.1", 0), Redirect)
        for server in (sink, redirector):
            threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            client = connect(f"http://127.0.0.1:{redirector.server_port}", "secret-token", FP)
            with self.assertRaises(AipError):
                client.read({"read": "Event", "select": ["id"]})
            self.assertEqual(seen, [], "the redirect target must never receive the bearer token")
        finally:
            for server in (sink, redirector):
                server.shutdown()
                server.server_close()

class RetryMetadataTest(unittest.TestCase):
    def test_unsettled_write_keeps_server_retry_guidance(self):
        server = FakeServer()
        server.script('/apply', {'ok': False, 'code': 'WIRE.COMMIT_UNKNOWN', 'retry': 'check-first', 'requestId': 'req_0123456789abcdef01234567'})
        client = connect('http://aip', 't1', FP, transport=server)
        with self.assertRaises(WriteUnsettled) as caught:
            client.apply({'apply': 'Event.mark'}, key='retry-metadata', retries=0)
        self.assertEqual(caught.exception.retry, 'check-first')
        self.assertEqual(caught.exception.request_id, 'req_0123456789abcdef01234567')

    def test_rejected_write_discards_unknown_retry_value(self):
        server = FakeServer()
        server.script('/apply', {'ok': False, 'code': 'WIRE.BAD_VALUE', 'retry': 'sometimes'})
        client = connect('http://aip', 't1', FP, transport=server)
        self.assertNotIn('retry', client.apply({'apply': 'Event.mark'}, key='bad-retry'))
