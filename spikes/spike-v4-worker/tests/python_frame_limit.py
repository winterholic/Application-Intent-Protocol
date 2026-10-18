import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
WORKER = ROOT / "workers" / "worker.py"
FRAME_LIMIT = 1024 * 1024


def frame_for_value_size(size):
    message = {
        "type": "reply",
        "call": 1,
        "value": {"docs": [{"value": "x" * size}]},
    }
    return (json.dumps(message, separators=(",", ":")) + "\n").encode()


def frame_at_size(size):
    base = frame_for_value_size(0)
    payload_size = size - len(base)
    if payload_size < 0:
        raise ValueError("requested frame is shorter than its protocol envelope")
    frame = frame_for_value_size(payload_size)
    if len(frame) != size:
        raise AssertionError(f"frame size drift: expected {size}, got {len(frame)}")
    return frame


class PythonWorkerFrameLimitTest(unittest.TestCase):
    def exchange(self, reply_frame):
        with tempfile.TemporaryDirectory(prefix="aip-python-frame-") as extension_dir:
            Path(extension_dir, "probe.py").write_text(
                "async def fetch(input, ctx):\n"
                "    docs = await ctx.mongo.find('items')\n"
                "    return {'length': len(docs[0]['value'])}\n",
                encoding="utf-8",
            )
            worker = subprocess.Popen(
                [sys.executable, "-P", "-c", WORKER.read_text(encoding="utf-8"), extension_dir],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            try:
                invocation = {
                    "type": "invoke",
                    "invoke": 1,
                    "impl": "probe.fetch",
                    "input": {},
                    "token": "test-token",
                }
                worker.stdin.write((json.dumps(invocation) + "\n").encode())
                worker.stdin.flush()
                call = json.loads(worker.stdout.readline())
                self.assertEqual(call["type"], "call")
                self.assertEqual(call["op"], "mongo")
                self.assertEqual(call["call"], 1)

                # Match the actual response envelope returned by the Rust host.
                reply_frame = reply_frame.replace(b'"call":1', f'"call":{call["call"]}'.encode(), 1)
                try:
                    worker.stdin.write(reply_frame)
                    worker.stdin.flush()
                except BrokenPipeError:
                    # An oversized input frame closes the worker pipe as part of rejection.
                    pass
                try:
                    worker.stdin.close()
                except BrokenPipeError:
                    pass
                output = worker.stdout.read()
                error = worker.stderr.read()
                status = worker.wait(timeout=5)
                worker.stdout.close()
                worker.stderr.close()
                return status, output, error
            finally:
                if worker.poll() is None:
                    worker.kill()
                    worker.wait()

    def test_64kib_boundary_accepts_below_and_above_default_reader_limit(self):
        for size in (60 * 1024, 75 * 1024):
            with self.subTest(reply_bytes=size):
                status, output, error = self.exchange(frame_for_value_size(size))
                self.assertEqual(status, 0, error.decode(errors="replace"))
                self.assertEqual(json.loads(output.splitlines()[-1]), {"type": "done", "invoke": 1, "output": {"length": size}})

    def test_existing_one_mib_frame_boundary_accepts_exact_limit(self):
        status, output, error = self.exchange(frame_at_size(FRAME_LIMIT))
        self.assertEqual(status, 0, error.decode(errors="replace"))
        self.assertEqual(json.loads(output.splitlines()[-1])["output"]["length"], FRAME_LIMIT - len(frame_for_value_size(0)))

    def test_existing_one_mib_frame_boundary_rejects_one_byte_over(self):
        status, output, error = self.exchange(frame_at_size(FRAME_LIMIT + 1))
        self.assertNotEqual(status, 0)
        self.assertEqual(output, b"")
        self.assertIn(b"ValueError", error)


if __name__ == "__main__":
    unittest.main()
