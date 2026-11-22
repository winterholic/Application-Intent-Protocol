"""D3: contract semantics of the Python caller SDK that must match the TypeScript transport (injected transport, no network)."""
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))
sys.path.insert(0, os.path.dirname(__file__))

from aip_sdk import AipError, WriteUnsettled, connect, created  # noqa: E402
from python_sdk_test import FP, FakeServer  # noqa: E402

REQUEST = {"apply": "Event.mark", "target": {"ids": ["11"]}}
STORED = {"ok": True, "changed": ["11"], "unchanged": [], "tags": ["Event"], "replayed": True}
# The server answers these before it reads the idempotency record (admission, deployment fence, concurrency caps).
BEFORE_THE_JOURNAL = ["WIRE.RATE_LIMITED", "WIRE.DEPLOYMENT_CHANGED", "WIRE.SCHEMA_MISMATCH", "WIRE.SERVER_BUSY", "WIRE.CIRCUIT_OPEN"]


class PythonSdkContractTest(unittest.TestCase):
    def test_a_rejection_the_server_gives_before_the_journal_does_not_prove_the_earlier_attempt_rolled_back(self):
        for code in BEFORE_THE_JOURNAL:
            with self.subTest(code=code):
                server = FakeServer()
                refusal = {"ok": False, "code": code, "retry": "later"}
                server.script("/apply", ConnectionResetError(), refusal, refusal, STORED)
                client = connect("http://aip", "t1", FP, transport=server)
                with self.assertRaises(WriteUnsettled) as caught:
                    client.apply(REQUEST, key="k-" + code)
                self.assertEqual(caught.exception.retry, "later")
                self.assertEqual(client.pending(), ["k-" + code], "the possibly committed write keeps its key")
                settled = client.retry_pending()
                self.assertEqual([r["replayed"] for r in settled], [True], "the stored result is replayed, not executed again")
                self.assertEqual(client.pending(), [])
                bodies = [c[2] for c in server.calls if c[0] == "/apply"]
                self.assertTrue(all(b["key"] == "k-" + code for b in bodies))

    def test_a_first_attempt_rate_limit_is_final_because_nothing_could_have_committed(self):
        server = FakeServer()
        server.script("/apply", {"ok": False, "code": "WIRE.RATE_LIMITED", "retry": "later"})
        client = connect("http://aip", "t1", FP, transport=server)
        result = client.apply(REQUEST, key="k-first")
        self.assertEqual((result["ok"], result["code"]), (False, "WIRE.RATE_LIMITED"))
        self.assertEqual(client.pending(), [])

    def test_calling_again_without_a_key_after_an_unsettled_write_reuses_the_pending_key(self):
        # TypeScript does this in transport.ts: a new random key would run the possibly committed write a second time.
        server = FakeServer()
        server.script("/apply", ConnectionResetError(), ConnectionResetError(), ConnectionResetError(), STORED)
        client = connect("http://aip", "t1", FP, transport=server)
        with self.assertRaises(WriteUnsettled) as caught:
            client.apply(REQUEST)
        pending_key = caught.exception.key
        result = client.apply(REQUEST)
        self.assertTrue(result["replayed"])
        keys = [c[2]["key"] for c in server.calls if c[0] == "/apply"]
        self.assertEqual(set(keys), {pending_key}, "every attempt, including the keyless re-call, used the pending key")
        self.assertEqual(client.pending(), [])

    def test_a_keyless_call_with_a_different_body_gets_a_new_key(self):
        server = FakeServer()
        server.script("/apply", ConnectionResetError(), ConnectionResetError(), ConnectionResetError(),
                      {"ok": True, "changed": ["12"], "unchanged": [], "tags": ["Event"]})
        client = connect("http://aip", "t1", FP, transport=server)
        with self.assertRaises(WriteUnsettled) as caught:
            client.apply(REQUEST)
        other = client.apply({"apply": "Event.mark", "target": {"ids": ["12"]}})
        self.assertTrue(other["ok"])
        keys = [c[2]["key"] for c in server.calls if c[0] == "/apply"]
        self.assertNotEqual(keys[-1], caught.exception.key)
        self.assertEqual(client.pending(), [caught.exception.key])

    def test_created_handles_are_operation_indexes_and_reject_anything_else(self):
        self.assertEqual(created(0), {"created": 0})
        for bad in (-1, True, 1.5, "0", None):
            with self.subTest(bad=bad), self.assertRaises(AipError):
                created(bad)


if __name__ == "__main__":
    unittest.main()
