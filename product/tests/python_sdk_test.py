"""Protocol tests for the Python caller SDK with an injected transport (no network)."""
import http.server
import json
import os
from pathlib import Path
import sys
import threading
import typing
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
                self.send_header("connection", "close")
                self.send_header("x-request-id", outer.ID)
                self.end_headers()
                self.wfile.write(body)
                self.close_connection = True

            def log_message(self, *args):
                pass

        server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            client = connect(f"http://127.0.0.1:{server.server_port}", "t1", FP)
            with self.assertRaises(AipError) as caught:
                client.read({"read": "Event", "select": ["id"]})
            self.assertEqual((caught.exception.code, caught.exception.request_id), ("WIRE.ACCESS_DENIED", self.ID))
        finally:
            server.shutdown()
            server.server_close()
            thread.join()


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
                self.send_header("connection", "close")
                self.end_headers()
                self.wfile.write(b'{"ok":true}')
                self.close_connection = True

            def log_message(self, *args):
                pass

        sink = http.server.HTTPServer(("127.0.0.1", 0), Sink)
        target = f"http://127.0.0.1:{sink.server_port}/steal"

        class Redirect(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers.get("content-length", 0)))
                self.send_response(302)
                self.send_header("location", target)
                self.send_header("connection", "close")
                self.end_headers()
                self.close_connection = True

            def log_message(self, *args):
                pass

        redirector = http.server.HTTPServer(("127.0.0.1", 0), Redirect)
        threads = [threading.Thread(target=s.serve_forever, daemon=True) for s in (sink, redirector)]
        for t in threads:
            t.start()
        try:
            client = connect(f"http://127.0.0.1:{redirector.server_port}", "secret-token", FP)
            with self.assertRaises(AipError):
                client.read({"read": "Event", "select": ["id"]})
            self.assertEqual(seen, [], "the redirect target must never receive the bearer token")
        finally:
            for server in (sink, redirector):
                server.shutdown()
                server.server_close()
            for t in threads:
                t.join()

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


class GeneratedSdkTest(unittest.TestCase):
    """Verifies that generated python bindings provide typed ReadResult, ApplyInput, and Operation types,
    and that runtime helper functions dispatch to client methods correctly."""

    def _generate_sdk_binding(self, fixture_path: Path, out_path: Path) -> None:
        import subprocess
        repo_root = Path(__file__).resolve().parent.parent.parent
        aip_bin = repo_root / "target" / "debug" / "aip"
        if aip_bin.exists():
            cmd = [str(aip_bin), "service", "gen", str(fixture_path), "--lang", "python", "--out", str(out_path)]
        else:
            cmd = ["cargo", "run", "-p", "aip-cli", "--bin", "aip", "--", "service", "gen", str(fixture_path), "--lang", "python", "--out", str(out_path)]
        result = subprocess.run(cmd, cwd=str(repo_root), capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, f"Generator failed:\nstdout: {result.stdout}\nstderr: {result.stderr}")

    def test_generated_types_and_runtime_helpers_from_real_fixture(self):
        import importlib.util, shutil, subprocess, sys, tempfile
        repo_root = Path(__file__).resolve().parent.parent.parent
        app_fixture = repo_root / "prototype" / "example" / "app.aip"
        operations_fixture = repo_root / "spikes" / "spike-v6-transport" / "tests" / "fixtures" / "operations.aip"

        with tempfile.TemporaryDirectory() as tmpdir:
            app_out = Path(tmpdir) / "app_gen.py"
            ops_out = Path(tmpdir) / "ops_gen.py"

            self._generate_sdk_binding(app_fixture, app_out)
            self._generate_sdk_binding(operations_fixture, ops_out)

            # 1. Real import and runtime helper execution for app.aip
            sys_path_entry = str(repo_root / "product" / "python")
            sys.path.insert(0, sys_path_entry)
            try:
                spec_app = importlib.util.spec_from_file_location("app_gen", app_out)
                self.assertIsNotNone(spec_app and spec_app.loader)
                mod_app = importlib.util.module_from_spec(spec_app)
                spec_app.loader.exec_module(mod_app)

                self.assertTrue(hasattr(mod_app, "CONTRACT_FINGERPRINT"))
                self.assertEqual(len(mod_app.CONTRACT_FINGERPRINT), 64)
                self.assertTrue(hasattr(mod_app, "JobState"))
                self.assertEqual(
                    typing.get_args(mod_app.JobState),
                    ("QUEUED", "RUNNING", "CANCEL_REQUESTED", "SUCCEEDED", "FAILED", "CANCELLED"),
                )
                self.assertTrue(hasattr(mod_app, "JobRun"))
                self.assertTrue(hasattr(mod_app, "EventRow"))
                self.assertTrue(hasattr(mod_app, "EventReadResult"))
                self.assertTrue(hasattr(mod_app, "ReadResult"))
                self.assertTrue(hasattr(mod_app, "EventMarkApplyInput"))
                self.assertTrue(hasattr(mod_app, "ApplyInput"))
                self.assertTrue(hasattr(mod_app, "TextLengthOperationInput"))
                self.assertTrue(hasattr(mod_app, "TextLengthOperationOutput"))
                self.assertTrue(hasattr(mod_app, "OperationInput"))
                self.assertTrue(hasattr(mod_app, "OperationOutput"))

                calls = []

                class DummyClient:
                    def read(self, query):
                        calls.append(("read", query))
                        return [{"id": "1", "title": "t"}]

                    def apply(self, request, key=None):
                        calls.append(("apply", request, key))
                        return {"ok": True, "changed": ["1"], "unchanged": []}

                    def operation(self, name, input_dict):
                        calls.append(("operation", name, input_dict))
                        return {"length": len(input_dict.get("text", ""))}

                client = DummyClient()

                rows = mod_app.read_event(client, {"read": "Event", "select": ["id", "title"]})
                self.assertEqual(rows, [{"id": "1", "title": "t"}])
                self.assertEqual(calls[-1], ("read", {"read": "Event", "select": ["id", "title"]}))

                apply_res = mod_app.apply_event_mark(client, {"ids": ["1"]}, key="k1")
                self.assertTrue(apply_res["ok"])
                self.assertEqual(calls[-1], ("apply", {"apply": "Event.mark", "target": {"ids": ["1"]}}, "k1"))

                op_out = mod_app.call_text_length(client, {"text": "hello"})
                self.assertEqual(op_out, {"length": 5})
                self.assertEqual(calls[-1], ("operation", "textLength", {"text": "hello"}))

                # 2. Real import and principalId check for operations.aip
                spec_ops = importlib.util.spec_from_file_location("ops_gen", ops_out)
                self.assertIsNotNone(spec_ops and spec_ops.loader)
                mod_ops = importlib.util.module_from_spec(spec_ops)
                spec_ops.loader.exec_module(mod_ops)

                self.assertTrue(hasattr(mod_ops, "principalId"), "operations.aip with actor principal must generate principalId")
                self.assertTrue(hasattr(mod_ops, "LengthOperationInput"))
                owner_field = mod_ops.LengthOperationInput.__annotations__["owner"]
                self.assertEqual(owner_field, mod_ops.principalId)
                self.assertTrue(hasattr(mod_ops, "call_length"))
                op_res = mod_ops.call_length(client, {"text": "test", "owner": mod_ops.principalId("p1")})
                self.assertEqual(op_res, {"length": 4})
                self.assertEqual(calls[-1], ("operation", "length", {"text": "test", "owner": "p1"}))
            finally:
                if sys_path_entry in sys.path:
                    sys.path.remove(sys_path_entry)

            # 3. Static type check with mypy (--warn-return-any, typed consumer positive and negative checks)
            uv_path = shutil.which("uv")
            mypy_path = shutil.which("mypy")
            if not (uv_path or mypy_path):
                self.fail("정적 타입 검사 실패: uv 또는 mypy 도구가 환경(PATH)에 없어 타입 검사를 수행할 수 없습니다 (의존성 없음)")

            env = dict(sys.modules["os"].environ, PYTHONPATH=str(repo_root / "product" / "python"))
            mypy_base_cmd = [uv_path, "run"]
            if os.environ.get("UV_OFFLINE") == "1" or os.environ.get("CARGO_NET_OFFLINE") == "true":
                mypy_base_cmd.append("--offline")
            mypy_base_cmd.extend(["--with", "mypy", "mypy"])
            mypy_flags = ["--warn-return-any", "--show-error-codes"]

            def run_mypy(target_path: Path):
                cmd = (mypy_base_cmd if uv_path else [mypy_path]) + mypy_flags + [str(target_path)]
                return subprocess.run(cmd, capture_output=True, text=True, env=env)

            for gen_file in (app_out, ops_out):
                mypy_run = run_mypy(gen_file)
                self.assertEqual(
                    mypy_run.returncode,
                    0,
                    f"mypy typecheck (--warn-return-any) failed for {gen_file}:\n{mypy_run.stdout}\n{mypy_run.stderr}",
                )

            # Typed consumer checks for operations.aip
            # (1) Positive: correctly typed return inference and access
            consumer_ok = temp_dir / "typed_consumer_ok.py"
            consumer_ok.write_text(
                f"import sys\n"
                f"sys.path.insert(0, r'{temp_dir}')\n"
                f"import {ops_out.stem} as ops\n"
                f"from aip_sdk import Client\n"
                f"def check(c: Client) -> int:\n"
                f"    res: ops.LengthOperationOutput = ops.call_length(c, {{'text': 'abc', 'owner': ops.principalId('p1')}})\n"
                f"    length: int = res['length']\n"
                f"    return length\n",
                encoding="utf-8",
            )
            run_ok = run_mypy(consumer_ok)
            self.assertEqual(run_ok.returncode, 0, f"Valid typed consumer should pass mypy:\n{run_ok.stdout}\n{run_ok.stderr}")

            # (2) Negative: accessing nonexistent field on TypedDict output is rejected
            consumer_bad_field = temp_dir / "typed_consumer_bad_field.py"
            consumer_bad_field.write_text(
                f"import sys\n"
                f"sys.path.insert(0, r'{temp_dir}')\n"
                f"import {ops_out.stem} as ops\n"
                f"from aip_sdk import Client\n"
                f"def check(c: Client) -> None:\n"
                f"    res = ops.call_length(c, {{'text': 'abc', 'owner': ops.principalId('p1')}})\n"
                f"    bad = res['nonexistent_output_field']\n",
                encoding="utf-8",
            )
            run_bad_field = run_mypy(consumer_bad_field)
            self.assertNotEqual(run_bad_field.returncode, 0, "Typed consumer with bad output field must be rejected by mypy")
            self.assertTrue(
                "typeddict-item" in run_bad_field.stdout or "TypedDict" in run_bad_field.stdout,
                f"Expected typeddict error for bad field, got:\n{run_bad_field.stdout}",
            )

            # (3) Negative: assigning output field to incompatible type is rejected
            consumer_bad_type = temp_dir / "typed_consumer_bad_type.py"
            consumer_bad_type.write_text(
                f"import sys\n"
                f"sys.path.insert(0, r'{temp_dir}')\n"
                f"import {ops_out.stem} as ops\n"
                f"from aip_sdk import Client\n"
                f"def check(c: Client) -> None:\n"
                f"    res = ops.call_length(c, {{'text': 'abc', 'owner': ops.principalId('p1')}})\n"
                f"    wrong: str = res['length']\n",
                encoding="utf-8",
            )
            run_bad_type = run_mypy(consumer_bad_type)
            self.assertNotEqual(run_bad_type.returncode, 0, "Typed consumer with incompatible output type must be rejected by mypy")
            self.assertTrue(
                "assignment" in run_bad_type.stdout or "Incompatible types" in run_bad_type.stdout,
                f"Expected assignment error for bad type, got:\n{run_bad_type.stdout}",
            )


class PackageSmokeTest(unittest.TestCase):
    """Verifies that product/python package can be built, installed and imported in a clean venv
    outside the repository directory without leaving build artifacts in source tree."""

    def test_clean_venv_package_install_and_import_smoke(self):
        import shutil, subprocess, sys, tempfile, zipfile
        repo_root = Path(__file__).resolve().parent.parent.parent
        pkg_dir = repo_root / "product" / "python"

        with tempfile.TemporaryDirectory() as tmpdir:
            # Build wheel from an isolated source copy so source tree remains untouched
            isolated_src = Path(tmpdir) / "pkg_src"
            shutil.copytree(pkg_dir, isolated_src)

            venv_dir = Path(tmpdir) / "venv"
            subprocess.run([sys.executable, "-m", "venv", str(venv_dir)], check=True, capture_output=True)
            venv_python = venv_dir / "bin" / "python"
            venv_pip = venv_dir / "bin" / "pip"
            if not venv_python.exists():
                venv_python = venv_dir / "Scripts" / "python.exe"
                venv_pip = venv_dir / "Scripts" / "pip.exe"

            dist_dir = Path(tmpdir) / "dist"
            dist_dir.mkdir()

            uv_path = shutil.which("uv")
            if uv_path:
                subprocess.run(
                    [uv_path, "build", "--wheel", "--out-dir", str(dist_dir), str(isolated_src)],
                    check=True,
                    capture_output=True,
                )
            else:
                wheel_proc = subprocess.run(
                    [sys.executable, "-m", "pip", "wheel", "--no-deps", "-w", str(dist_dir), str(isolated_src)],
                    capture_output=True,
                )
                if wheel_proc.returncode != 0:
                    self.skipTest(f"pip wheel unavailable: {wheel_proc.stderr.decode()}")

            wheels = list(dist_dir.glob("*.whl"))
            self.assertTrue(len(wheels) > 0, "Wheel should be built")
            wheel_file = wheels[0]

            # Verify py.typed marker is packaged in the wheel
            with zipfile.ZipFile(wheel_file) as zf:
                wheel_members = zf.namelist()
            self.assertTrue(
                any(m == "aip_sdk/py.typed" or m.endswith("/aip_sdk/py.typed") for m in wheel_members),
                f"py.typed marker must be present in wheel contents: {wheel_members}",
            )

            # Install wheel into clean venv without index access
            subprocess.run(
                [str(venv_pip), "install", "--no-index", str(wheel_file)],
                check=True,
                capture_output=True,
            )

            # Test import from outside repository directory
            outside_dir = Path(tmpdir) / "outside"
            outside_dir.mkdir()
            script = "import aip_sdk\nassert hasattr(aip_sdk, 'connect')\nassert hasattr(aip_sdk, 'Client')\nprint('smoke_ok', aip_sdk.__file__)"
            result = subprocess.run(
                [str(venv_python), "-c", script],
                cwd=str(outside_dir),
                check=True,
                capture_output=True,
                text=True,
            )
            self.assertIn("smoke_ok", result.stdout)
            self.assertNotIn(str(repo_root / "product" / "python"), result.stdout)

        # Verify no build or egg-info directories leaked into source tree
        self.assertFalse((pkg_dir / "build").exists(), "build directory should not leak into product/python")
        self.assertFalse((pkg_dir / "dist").exists(), "dist directory should not leak into product/python")
        self.assertEqual(list(pkg_dir.glob("*.egg-info")), [], "egg-info should not leak into product/python")
