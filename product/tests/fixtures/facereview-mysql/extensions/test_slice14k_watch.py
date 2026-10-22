import asyncio
import importlib.util
import pathlib
import unittest


MODULE = pathlib.Path(__file__).with_name("slice14k_watch.py")


class Data:
    def __init__(self, ctx):
        self.ctx = ctx
        self.rows = ctx.rows
        self.created = 0

    async def read(self, query):
        self.ctx.read_queries.append(query)
        row = self.rows.get(query["filter"][0]["value"])
        if row is None or row["user"] != self.ctx.actor:
            return {"rows": []}
        return {"rows": [row]}

    async def create(self, resource, values):
        self.created += 1
        if values["id"] in self.rows:
            raise Exception("WIRE.ALREADY_EXISTS")
        if values["user"] != self.ctx.actor:
            raise Exception("WIRE.ACCESS_DENIED")
        row = {**values}
        row.setdefault("created_at", "2026-10-22T00:00:00Z")
        self.rows[values["id"]] = row
        return values["id"]


class Redis:
    def __init__(self):
        self.values = {}

    async def set_ex(self, key, value, ttl, **kwargs):
        self.values[key] = (value, ttl, kwargs)


class Context:
    def __init__(self, actor, rows):
        self.actor = actor
        self.rows = rows
        self.read_queries = []
        self.data = Data(self)
        self.redis = Redis()


spec = importlib.util.spec_from_file_location("slice14k_watch", MODULE)
watch = importlib.util.module_from_spec(spec)


class StartWatchingTests(unittest.TestCase):
    def setUp(self):
        spec.loader.exec_module(watch)

    def test_first_frame_creates_log_and_session_then_reconnect_reuses_log(self):
        async def run():
            rows = {}
            first = Context("alice", rows)
            request = {"id": "view-1", "video": "video-1", "user": "alice", "duration": 120}
            result = await watch.startWatching(request, first)
            self.assertEqual(result["initialized"], True)
            self.assertEqual(first.data.created, 1)
            self.assertEqual(first.read_queries[0]["read"], "VideoViewLog")
            key, (value, ttl, options) = next(iter(first.redis.values.items()))
            self.assertTrue(key.startswith("facereview:watch:active:"))
            self.assertEqual(ttl, 180)
            self.assertTrue(options["compensate"])

            reconnect = Context("alice", rows)
            await watch.startWatching(request, reconnect)
            self.assertEqual(reconnect.data.created, 0)
            self.assertEqual(len(rows), 1)
            self.assertEqual(len(reconnect.redis.values), 1)
            self.assertEqual(reconnect.redis.values[key][0], value)

        asyncio.run(run())

    def test_actor_separation_and_session_identity_mismatch_are_rejected(self):
        async def run():
            rows = {"view-1": {"id": "view-1", "video": "video-1", "user": "alice", "created_at": "2026-10-22T00:00:00Z"}}
            bob = Context("bob", rows)
            with self.assertRaisesRegex(Exception, "WIRE.ALREADY_EXISTS"):
                await watch.startWatching({"id": "view-1", "video": "video-1", "user": "bob", "duration": 120}, bob)
            self.assertEqual(bob.redis.values, {})

            alice = Context("alice", rows)
            with self.assertRaisesRegex(Exception, "session does not match"):
                await watch.startWatching({"id": "view-1", "video": "video-2", "user": "alice", "duration": 120}, alice)
            self.assertEqual(alice.redis.values, {})

        asyncio.run(run())


if __name__ == "__main__":
    unittest.main()
