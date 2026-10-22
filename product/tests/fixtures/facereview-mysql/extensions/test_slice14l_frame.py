import asyncio
import hashlib
import importlib.util
import json
import pathlib
import unittest


MODULE = pathlib.Path(__file__).with_name("slice14l_frame.py")
VIEW_LOG = "66666666-6666-4666-8666-666666666666"
VIDEO = "eeeeeeee-0000-4000-8000-000000000001"


def staged_key(view_log_id, centi):
    return "facereview:watch:frame:" + hashlib.sha256(view_log_id.encode("utf-8")).hexdigest() + ":" + str(centi)


class Data:
    def __init__(self, ctx):
        self.ctx = ctx

    async def read(self, query):
        self.ctx.reads.append(query)
        if query["read"] == "VideoViewLog":
            row = self.ctx.view_logs.get(query["filter"][0]["value"])
            return {"rows": [row] if row and row["user"] == self.ctx.actor else []}
        if query["read"] == "Video":
            row = self.ctx.videos.get(query["filter"][0]["value"])
            return {"rows": [row] if row else []}
        raise AssertionError(query)


class Redis:
    def __init__(self, staged):
        self.staged = staged
        self.taken = []

    async def getdel(self, key, compensate=False, effect_key=None):
        self.taken.append((key, compensate, effect_key))
        return self.staged.pop(key, None)


class Mongo:
    def __init__(self):
        self.updates = []

    async def update_one(self, collection, filter, update, upsert=False, compensate=False, key=None):
        self.updates.append((collection, filter, update, upsert, compensate, key))
        return {"matched": 0, "modified": 0, "upserted_id": "x"}


class Context:
    def __init__(self, actor, staged=None, view_logs=None, videos=None):
        self.actor = actor
        self.view_logs = view_logs if view_logs is not None else {VIEW_LOG: {"id": VIEW_LOG, "video": VIDEO, "user": "alice"}}
        self.videos = videos if videos is not None else {VIDEO: {"id": VIDEO, "duration": 120}}
        self.reads = []
        self.data = Data(self)
        self.redis = Redis(staged if staged is not None else {})
        self.mongo = Mongo()


spec = importlib.util.spec_from_file_location("slice14l_frame", MODULE)
frame = importlib.util.module_from_spec(spec)

SCORES = {"neutral": 10.5, "happy": 60.25, "surprise": 5.0, "sad": 4.25, "angry": 20.0}


class RecordFrameTests(unittest.TestCase):
    def setUp(self):
        spec.loader.exec_module(frame)

    def test_a_staged_frame_is_written_to_the_timeline_in_the_originals_score_order(self):
        async def run():
            ctx = Context("alice", {staged_key(VIEW_LOG, 2029): json.dumps(SCORES)})
            result = await frame.recordFrame({"id": VIEW_LOG, "centi": 2029}, ctx)
            self.assertEqual(result, {"recorded": True})
            self.assertEqual(ctx.redis.taken, [(staged_key(VIEW_LOG, 2029), True, None)])
            (collection, filt, update, upsert, compensate, key) = ctx.mongo.updates[0]
            self.assertEqual((collection, filt, upsert, compensate), ("youtube_watching_data", {"video_view_log_id": VIEW_LOG}, True, False))
            self.assertEqual(update["$set"]["emotion_score_timeline.2029"], [10.5, 60.25, 5.0, 4.25, 20.0])
            self.assertEqual(update["$set"]["most_emotion_timeline.2029"], "happy")
            insert = update["$setOnInsert"]
            self.assertEqual((insert["video_id"], insert["video_view_log_id"], insert["duration"]), (VIDEO, VIEW_LOG, 120))
            self.assertEqual((insert["completion_rate"], insert["dominant_emotion"]), (0.0, "neutral"))
            self.assertNotIn("$inc", update, "a counter is not idempotent: the timeline alone is written")
            self.assertNotIn("user_id", insert, "the server forces the user of the session document")

        asyncio.run(run())

    def test_without_staged_scores_nothing_is_written(self):
        async def run():
            ctx = Context("alice")
            self.assertEqual(await frame.recordFrame({"id": VIEW_LOG, "centi": 5}, ctx), {"recorded": False})
            self.assertEqual(ctx.mongo.updates, [])

        asyncio.run(run())

    def test_a_session_of_another_member_is_not_recorded_and_its_scores_stay_staged(self):
        async def run():
            key = staged_key(VIEW_LOG, 7)
            ctx = Context("bob", {key: json.dumps(SCORES)})
            with self.assertRaisesRegex(Exception, "session not found"):
                await frame.recordFrame({"id": VIEW_LOG, "centi": 7}, ctx)
            self.assertIn(key, ctx.redis.staged, "the staged scores were not taken")
            self.assertEqual(ctx.mongo.updates, [])

        asyncio.run(run())

    def test_the_same_key_twice_writes_the_same_slot_and_keys_may_come_out_of_order(self):
        async def run():
            ctx = Context("alice")
            for centi in (300, 100, 300):
                ctx.redis.staged[staged_key(VIEW_LOG, centi)] = json.dumps(SCORES)
                self.assertEqual(await frame.recordFrame({"id": VIEW_LOG, "centi": centi}, ctx), {"recorded": True})
            slots = [
                [path for path in update["$set"] if path.startswith("emotion_score_timeline.")]
                for (_, _, update, _, _, _) in ctx.mongo.updates
            ]
            self.assertEqual(slots, [["emotion_score_timeline.300"], ["emotion_score_timeline.100"], ["emotion_score_timeline.300"]])

        asyncio.run(run())

    def test_malformed_scores_are_refused_and_nothing_is_written(self):
        async def run():
            bad_values = [
                "not json",
                json.dumps([1, 2, 3]),
                json.dumps({**SCORES, "happy": "60"}),
                json.dumps({**SCORES, "happy": -1}),
                json.dumps({**SCORES, "happy": 100.5}),
                json.dumps({k: v for k, v in SCORES.items() if k != "sad"}),
                json.dumps({**SCORES, "extra": 1}),
                json.dumps({**SCORES, "happy": float("nan")}),
            ]
            for value in bad_values:
                ctx = Context("alice", {staged_key(VIEW_LOG, 1): value})
                with self.assertRaisesRegex(Exception, "invalid staged scores"):
                    await frame.recordFrame({"id": VIEW_LOG, "centi": 1}, ctx)
                self.assertEqual(ctx.mongo.updates, [], value)

        asyncio.run(run())

    def test_inputs_outside_the_declared_ranges_are_refused(self):
        async def run():
            for bad in ({"id": "", "centi": 1}, {"id": VIEW_LOG, "centi": -1}, {"id": VIEW_LOG, "centi": True}, {"id": VIEW_LOG, "centi": 1.5}):
                ctx = Context("alice")
                with self.assertRaises(Exception):
                    await frame.recordFrame(bad, ctx)
                self.assertEqual((ctx.redis.taken, ctx.mongo.updates), ([], []))

        asyncio.run(run())


if __name__ == "__main__":
    unittest.main()
