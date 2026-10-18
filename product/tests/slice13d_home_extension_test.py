# Slice13-D: scripted-peer tests of the home/category and home/personalized read extensions.
#
# EVIDENCE SCOPE: a scripted ctx stands in for the AIP server, MySQL, Mongo and Redis. This proves the extension's
# request shapes, error propagation and use of the math module. It is NOT real-database, real-driver or HTTP evidence.
# The ranking itself is checked against the original functions through slice13d_home_oracle_test.
# UID below is a fabricated UUID served by the scripted User row: results here are NOT evidence of equivalence for a real
# user profile (the real private-Mongo identity path is blocked by D3 and unverified).
#
#   FACEREVIEW_SOURCE_PATH=/path/to/new-facereview python3 product/tests/slice13d_home_extension_test.py
import asyncio
import json
import os
import random
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import slice13d_home_oracle_test as oracle  # noqa: E402  (loads the pinned original once)

HOME = oracle._load('slice13d_home', os.path.join(oracle.EXT_DIR, 'slice13d_home.py'))
UID = '0b9c1d22-4e5f-4a61-8c7d-1234567890ab'


class WireError(Exception):
    def __init__(self, code):
        super().__init__(code)
        self.code = code


class FakeCtx:
    """Scripted peer: records every call and answers from fixed state. Any non-read op fails the test."""

    def __init__(self, pool=None, summaries=None, favorites=(), viewed=(), bookmarks=(), categories=None,
                 redis_error=None, mongo_error=None, actor_rows=True, page_override=None, user_rows=None, own_user=None):
        self.calls = []
        self.pool, self.summaries = pool, summaries or []
        self.favorites, self.viewed, self.bookmarks = list(favorites), list(viewed), list(bookmarks)
        self.categories, self.redis_error, self.mongo_error = categories, redis_error, mongo_error
        self.actor_rows, self.page_override = actor_rows, page_override
        # user_rows: what a scripted server would return for `read User` (it honours `limit`, like the real planner).
        self.user_rows, self.own_user = user_rows, own_user if own_user is not None else UID
        ctx = self
        self.data = type('D', (), {'read': staticmethod(ctx._read)})()
        self.redis = type('R', (), {'get': staticmethod(ctx._get), 'hgetall': staticmethod(ctx._hgetall)})()

    async def _read(self, q):
        self.calls.append(('read', q))
        res = q['read']
        if res == 'User':
            if self.user_rows is not None:
                return {'rows': self.user_rows[:q['limit']]}
            return {'rows': [{'id': UID}] if self.actor_rows else []}
        source = {'UserFavoriteGenre': [{'genre': g, 'user': self.own_user} for g in self.favorites],
                  'VideoViewLog': [{'video': v, 'user': self.own_user} for v in self.viewed],
                  'VideoBookmark': [{'video': v} for v in self.bookmarks]}[res]
        if self.page_override is not None:
            return {'rows': [source[0]] * q['limit'] if source else [{'video': 'x', 'genre': 'drama'}] * q['limit']}
        off = q.get('offset', 0)
        return {'rows': source[off:off + q['limit']]}

    async def _get(self, key):
        self.calls.append(('redis.get', key))
        if self.redis_error:
            raise WireError(self.redis_error)
        return None if self.pool is None else json.dumps(self.pool)

    async def _hgetall(self, key):
        self.calls.append(('redis.hgetall', key))
        if self.redis_error:
            raise WireError(self.redis_error)
        return self.categories

    async def call(self, op, name, inp, **extra):
        self.calls.append((op, name, inp))
        assert op == 'mongo' and inp['command'] == 'find', f'unexpected capability call {op} {inp}'
        if self.mongo_error:
            raise WireError(self.mongo_error)
        return {'docs': self.summaries}


def run(coro):
    out = asyncio.run(coro)
    # the extension returns its list as one JSON Text value; decode for assertions
    return {k: json.loads(v) for k, v in out.items()} if isinstance(out, dict) else out


def seeded_home(seed):
    HOME._rng = random.Random(seed)


class PersonalizedFlow(unittest.TestCase):
    def setUp(self):
        self.rng = random.Random(41)
        self.pool = oracle.make_pool(self.rng, 400)
        self.summaries = []
        cat = {v['video_id']: v['category'] for v in self.pool}
        for _ in range(20):
            v = self.rng.choice(self.pool)
            d = oracle.dist(self.rng)
            self.summaries.append({'video_id': v['video_id'], 'dominant_emotion': max(d, key=d.get), 'emotion_percentages': d})
        self.cat = cat

    def expected(self, favorites, viewed, summaries=None, seed=5):
        # Original recent_watching assembly (home_service.py 367-376) feeding the ORIGINAL rank_personalized.
        recent = []
        for wd in (self.summaries if summaries is None else summaries):
            if wd.get('dominant_emotion') is None:
                continue
            recent.append({'category': self.cat.get(wd.get('video_id')), 'dominant_emotion': wd['dominant_emotion'],
                           'emotion_percentages': wd.get('emotion_percentages', {})})
        random.seed(seed)
        return [v['video_id'] for v in oracle.ORIG.rank_personalized(
            pool=self.pool, recent_watching=recent, favorite_genres=favorites, viewed_ids=set(viewed), limit=20, top_n=150, random_n=50)]

    def test_matches_original_rank_with_twenty_summaries_favorites_and_viewed(self):
        favorites, viewed = ['music', 'game', 'vlog'], [v['video_id'] for v in self.pool[:30]]
        ctx = FakeCtx(self.pool, self.summaries, favorites, viewed, bookmarks=[self.pool[40]['video_id']])
        seeded_home(5)
        out = run(HOME.personalized({}, ctx))['videos']
        self.assertEqual([v['video_id'] for v in out], self.expected(favorites, viewed))
        self.assertTrue(all(v['video_id'] not in set(viewed) for v in out))
        flagged = {v['video_id'] for v in out if v['is_bookmarked']}
        self.assertTrue(flagged <= {self.pool[40]['video_id']})

    def test_cold_start_without_summaries_or_favorites(self):
        ctx = FakeCtx(self.pool, [], [], [])
        seeded_home(5)
        out = run(HOME.personalized({}, ctx))['videos']
        self.assertEqual([v['video_id'] for v in out], self.expected([], [], summaries=[]))

    def test_summaries_without_dominant_emotion_are_skipped(self):
        mixed = [dict(s, dominant_emotion=None) if i % 2 else s for i, s in enumerate(self.summaries)]
        ctx = FakeCtx(self.pool, mixed, ['comedy'], [])
        seeded_home(5)
        out = run(HOME.personalized({}, ctx))['videos']
        self.assertEqual([v['video_id'] for v in out], self.expected(['comedy'], [], summaries=mixed))

    def test_all_watched_returns_empty(self):
        ctx = FakeCtx(self.pool, self.summaries, ['music'], [v['video_id'] for v in self.pool])
        self.assertEqual(run(HOME.personalized({}, ctx)), {'videos': []})

    def test_fallback_branch_is_unreachable_in_the_original_and_stays_equivalent(self):
        # rank_personalized returns [] only when every pool entry is viewed, and the original's fallback then also
        # finds no unwatched candidate. The branch is dead code upstream; it is ported to keep the same observable
        # result (empty list) and is exercised here directly so a later change cannot make it return viewed videos.
        small = [oracle.pool_video(random.Random(i), i, category='music' if i < 2 else 'game') for i in range(6)]
        seeded_home(1)
        self.assertEqual(len(HOME.rank_with_fallback(small, [], ['music'], set(), 20)), 6)
        self.assertEqual(HOME.rank_with_fallback([], [], ['music'], set(), 20), [])
        self.assertEqual(HOME.rank_with_fallback(small, [], ['music'], {v['video_id'] for v in small}, 20), [])

    def test_request_shapes_and_read_only(self):
        ctx = FakeCtx(self.pool, self.summaries, [], [])
        run(HOME.personalized({}, ctx))
        mongo = [c for c in ctx.calls if c[0] == 'mongo'][0]
        self.assertEqual(mongo[1], 'youtube_watching_data')
        self.assertEqual(mongo[2], {'command': 'find', 'filter': {'user_id': UID}, 'projection': {'emotion_score_timeline': 0},
                                    'sort': {'created_at': -1}, 'limit': 20})
        self.assertEqual({c[0] for c in ctx.calls}, {'read', 'redis.get', 'mongo'})
        self.assertEqual([c[1] for c in ctx.calls if c[0] == 'redis.get'], ['facereview:reco:pool'])

    def test_pool_cache_miss_is_an_explicit_error_not_an_empty_list(self):
        with self.assertRaises(HOME.Slice13dError) as cm:
            run(HOME.personalized({}, FakeCtx(None, self.summaries, [], [])))
        self.assertEqual(cm.exception.code, 'SLICE13D.POOL_CACHE_MISS')

    def test_corrupt_pool_cache_is_an_explicit_error(self):
        ctx = FakeCtx(self.pool, self.summaries, [], [])
        ctx.pool = None

        async def bad(key):
            return '{not json'
        ctx.redis.get = bad
        with self.assertRaises(HOME.Slice13dError) as cm:
            run(HOME.personalized({}, ctx))
        self.assertEqual(cm.exception.code, 'SLICE13D.POOL_CACHE_CORRUPT')

    def test_adapter_and_policy_errors_propagate_unchanged(self):
        for kw, code in (({'redis_error': 'WIRE.ADAPTER_UNAVAILABLE'}, 'WIRE.ADAPTER_UNAVAILABLE'),
                         ({'mongo_error': 'WIRE.ADAPTER_UNAVAILABLE'}, 'WIRE.ADAPTER_UNAVAILABLE'),
                         ({'mongo_error': 'WIRE.ACCESS_DENIED'}, 'WIRE.ACCESS_DENIED')):
            with self.assertRaises(WireError) as cm:
                run(HOME.personalized({}, FakeCtx(self.pool, self.summaries, [], [], **kw)))
            self.assertEqual(cm.exception.code, code)

    def test_missing_actor_row_is_user_not_found(self):
        with self.assertRaises(HOME.Slice13dError) as cm:
            run(HOME.personalized({}, FakeCtx(self.pool, self.summaries, actor_rows=False)))
        self.assertEqual(cm.exception.code, 'SLICE13D.USER_NOT_FOUND')

    def test_viewed_ids_beyond_one_page_are_all_excluded(self):
        viewed = [v['video_id'] for v in self.pool[:250]]
        ctx = FakeCtx(self.pool, self.summaries, [], viewed)
        out = run(HOME.personalized({}, ctx))['videos']
        self.assertFalse({v['video_id'] for v in out} & set(viewed))
        offsets = [c[1].get('offset', 0) for c in ctx.calls if c[0] == 'read' and c[1]['read'] == 'VideoViewLog']
        self.assertEqual(offsets, [0, 100, 200])

    def test_read_cap_fails_loudly_instead_of_truncating(self):
        with self.assertRaises(HOME.Slice13dError) as cm:
            run(HOME.personalized({}, FakeCtx(self.pool, self.summaries, [], [], page_override=True)))
        self.assertEqual(cm.exception.code, 'SLICE13D.READ_TRUNCATED')


class CategoryFlow(unittest.TestCase):
    def setUp(self):
        rng = random.Random(51)
        self.cats = {}
        for c in ('music', 'game', 'drama'):
            self.cats[c] = json.dumps([
                {k: v for k, v in oracle.pool_video(rng, hash((c, i)) % 9999, category=c).items()
                 if k in ('video_id', 'youtube_url', 'title', 'dominant_emotion', 'dominant_emotion_per')}
                for i in range(4)])
        self.ids = [e['video_id'] for c in self.cats.values() for e in json.loads(c)]

    def test_hit_returns_original_dto_shape_and_marks_bookmarks(self):
        ctx = FakeCtx(categories=self.cats, bookmarks=[self.ids[1], 'unrelated'])
        out = run(HOME.category({}, ctx))
        self.assertEqual([g['category_name'] for g in out['video_data']], ['music', 'game', 'drama'])
        flat = [v for g in out['video_data'] for v in g['videos']]
        self.assertEqual([v['video_id'] for v in flat], self.ids)
        self.assertEqual([v['video_id'] for v in flat if v['is_bookmarked']], [self.ids[1]])
        self.assertEqual(set(flat[0]), {'video_id', 'youtube_url', 'title', 'dominant_emotion', 'dominant_emotion_per', 'is_bookmarked'})

    def test_guest_without_bookmark_rows_sees_all_false(self):
        out = run(HOME.category({}, FakeCtx(categories=self.cats)))
        self.assertFalse(any(v['is_bookmarked'] for g in out['video_data'] for v in g['videos']))

    def test_cache_miss_and_adapter_absence_are_explicit(self):
        for cats in (None, {}):
            with self.assertRaises(HOME.Slice13dError) as cm:
                run(HOME.category({}, FakeCtx(categories=cats)))
            self.assertEqual(cm.exception.code, 'SLICE13D.CATEGORY_CACHE_MISS')
        with self.assertRaises(WireError) as cm:
            run(HOME.category({}, FakeCtx(redis_error='WIRE.ADAPTER_UNAVAILABLE')))
        self.assertEqual(cm.exception.code, 'WIRE.ADAPTER_UNAVAILABLE')

    def test_missing_dominant_fields_default_like_the_original(self):
        entry = {'video_id': 'a', 'youtube_url': 'u', 'title': 't'}
        out = run(HOME.category({}, FakeCtx(categories={'etc': json.dumps([entry])})))
        v = out['video_data'][0]['videos'][0]
        self.assertEqual((v['dominant_emotion'], v['dominant_emotion_per'], v['is_bookmarked']), (None, 0.0, False))

    def test_only_reads_are_issued(self):
        ctx = FakeCtx(categories=self.cats)
        run(HOME.category({}, ctx))
        self.assertEqual({c[0] for c in ctx.calls}, {'redis.hgetall', 'read'})


class SelfIdentity(unittest.TestCase):
    """Reviewer REPORT4 inference, reproduced against a SCRIPTED server.

    `User` rows are visible as `(id = actor and active(actor)) or isAdmin(actor)` (facereview.aip line 30), so an ADMIN's
    `read User limit 1` can return someone else's row. These tests script that multi-row answer; they show what the
    extension does with it. They are NOT a run against a real MySQL, a real admin, or the real planner's SQL.
    """

    OTHER = '00000000-0000-4000-8000-000000000001'      # sorts before UID, so it is the "first row" an id-ordered read returns
    BYSTANDER = '00000000-0000-4000-8000-000000000002'

    def setUp(self):
        self.pool = oracle.make_pool(random.Random(71), 40)

    def mongo_filters(self, ctx):
        return [c[2]['filter'] for c in ctx.calls if c[0] == 'mongo']

    def test_admin_whose_first_visible_row_is_someone_else_still_queries_only_their_own_id(self):
        ctx = FakeCtx(self.pool, [], ['music'], [self.pool[0]['video_id']], user_rows=[{'id': self.OTHER}, {'id': UID}, {'id': self.BYSTANDER}])
        run(HOME.personalized({}, ctx))
        self.assertEqual(self.mongo_filters(ctx), [{'user_id': UID}])
        user_read = [c[1] for c in ctx.calls if c[0] == 'read' and c[1]['read'] == 'User'][0]
        self.assertEqual(user_read['limit'], 2, 'two rows are enough to tell a member from an admin')

    def test_admin_with_no_own_only_rows_gets_an_explicit_error_and_no_private_query(self):
        ctx = FakeCtx(self.pool, [], [], [], user_rows=[{'id': self.OTHER}, {'id': UID}])
        with self.assertRaises(HOME.Slice13dError) as cm:
            run(HOME.personalized({}, ctx))
        self.assertEqual(cm.exception.code, 'SLICE13D.SELF_IDENTITY_UNRESOLVED')
        self.assertEqual(self.mongo_filters(ctx), [], 'a guessed id must never reach the private collection')
        self.assertFalse([c for c in ctx.calls if c[0] == 'redis.get'])

    def test_single_visible_row_that_contradicts_the_own_only_rows_is_rejected(self):
        ctx = FakeCtx(self.pool, [], ['music'], [], user_rows=[{'id': self.OTHER}], own_user=UID)
        with self.assertRaises(HOME.Slice13dError) as cm:
            run(HOME.personalized({}, ctx))
        self.assertEqual(cm.exception.code, 'SLICE13D.SELF_IDENTITY_MISMATCH')
        self.assertEqual(self.mongo_filters(ctx), [])

    def test_member_with_single_row_and_no_own_rows_still_works(self):
        ctx = FakeCtx(self.pool, [], [], [], user_rows=[{'id': UID}])
        run(HOME.personalized({}, ctx))
        self.assertEqual(self.mongo_filters(ctx), [{'user_id': UID}])

    def test_resolve_self_unit(self):
        r = HOME._resolve_self
        self.assertEqual(r([{'id': 'a'}], []), 'a')
        self.assertEqual(r([{'id': 'a'}, {'id': 'b'}], [{'user': 'b'}, {'user': 'b'}]), 'b')
        for rows, own in (([{'id': 'a'}, {'id': 'b'}], []), ([{'id': 'a'}, {'id': 'b'}], [{'user': 'a'}, {'user': 'b'}]), ([{'id': 'a'}], [{'user': 'b'}])):
            with self.assertRaises(HOME.Slice13dError):
                r(rows, own)


class DeclarationContract(unittest.TestCase):
    """The extension blocks in the real facereview.aip must match the worker entries they name."""

    DEFINITION = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'fixtures', 'facereview-mysql', 'facereview.aip')

    def blocks(self):
        import re
        source = open(self.DEFINITION, encoding='utf-8').read()
        found = {}
        for m in re.finditer(r'extension read (\w+) \{(.*?)\n  \}', source, re.S):
            body = m.group(2)
            found[m.group(1)] = {
                'input': re.search(r'input \{([^}]*)\}', body).group(1).strip(),
                'output': re.search(r'output \{([^}]*)\}', body).group(1).strip(),
                'effect': re.search(r'effect (\w+)', body).group(1),
                'deadline': re.search(r'deadline (\w+)', body).group(1),
                'implementation': re.search(r'implementation "([^"]+)"', body).group(1),
                'access': re.search(r'^\s*access\s', body, re.M) is not None,
            }
        return found

    def test_exactly_the_two_declared_extensions(self):
        self.assertEqual(sorted(self.blocks()), ['homeCategory', 'homePersonalized'])

    def test_contract_fields(self):
        for name, b in self.blocks().items():
            self.assertEqual((b['effect'], b['deadline'], b['input']), ('none', '5s', ''), name)
            self.assertFalse(b['access'], f'{name}: the language cannot declare grants yet; the tests attach them')

    def test_entry_exists_is_async_and_returns_exactly_the_declared_output(self):
        import inspect
        pool = oracle.make_pool(random.Random(61), 30)
        cats = {'music': json.dumps([{k: pool[0][k] for k in ('video_id', 'youtube_url', 'title', 'dominant_emotion', 'dominant_emotion_per')}])}
        for name, b in self.blocks().items():
            module, function = b['implementation'].split('.')
            self.assertEqual(module, 'slice13d_home')
            entry = getattr(HOME, function)
            self.assertTrue(inspect.iscoroutinefunction(entry), name)
            self.assertEqual(list(inspect.signature(entry).parameters), ['input', 'ctx'], name)
            key, ty = [p.strip() for p in b['output'].split(':')]
            self.assertEqual(ty, 'Text', name)
            ctx = FakeCtx(pool=pool, categories=cats)
            out = asyncio.run(entry({}, ctx))
            self.assertEqual(list(out), [key], name)
            self.assertIsInstance(json.loads(out[key]), list)


if __name__ == '__main__':
    unittest.main(verbosity=2)
