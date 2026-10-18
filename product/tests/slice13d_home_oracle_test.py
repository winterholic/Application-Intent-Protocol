# Slice13-D: compare the AIP-side recommendation math with the ORIGINAL FaceReview functions.
#
# Oracle = common/utils/recommendation_alg.py at the pinned original revision, extracted with
# `git show <sha>:<path>` from a read-only checkout and executed as-is. Nothing here copies
# the original algorithm into the test. Time and randomness are frozen identically for both sides.
#
#   FACEREVIEW_SOURCE_PATH=/path/to/new-facereview python3 product/tests/slice13d_home_oracle_test.py
import importlib.util
import os
import random
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone

sys.dont_write_bytecode = True

ORIGINAL_SHA = '7948b7e721837fdb73c427cb54e12cad4ec2d5af'
ORIGINAL_PATH = 'common/utils/recommendation_alg.py'
HERE = os.path.dirname(os.path.abspath(__file__))
EXT_DIR = os.environ.get('SLICE13D_EXT_DIR') or os.path.join(HERE, 'fixtures', 'facereview-mysql', 'extensions')
FIXED_NOW = datetime(2026, 10, 18, 12, 0, 0)


class _FrozenDatetime(datetime):
    @classmethod
    def utcnow(cls):
        return FIXED_NOW


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def load_original():
    source = os.environ.get('FACEREVIEW_SOURCE_PATH')
    if not source:
        raise unittest.SkipTest('FACEREVIEW_SOURCE_PATH not set; the original oracle is required')
    head = subprocess.run(['git', '-C', source, 'cat-file', '-e', f'{ORIGINAL_SHA}^{{commit}}'], capture_output=True)
    if head.returncode != 0:
        raise AssertionError(f'pinned original revision {ORIGINAL_SHA} not present in {source}')
    blob = subprocess.run(['git', '-C', source, 'show', f'{ORIGINAL_SHA}:{ORIGINAL_PATH}'], capture_output=True, check=True).stdout
    tmp = tempfile.mkdtemp(prefix='slice13d-original-')
    path = os.path.join(tmp, 'recommendation_alg_original.py')
    with open(path, 'wb') as f:
        f.write(blob)
    mod = _load('recommendation_alg_original', path)
    mod.datetime = _FrozenDatetime
    return mod


ORIG = load_original()
PORT = _load('recommendation_math', os.path.join(EXT_DIR, 'recommendation_math.py'))
GENRES = ['drama', 'eating', 'travel', 'cook', 'show', 'information', 'game', 'sports', 'music', 'animal', 'beauty', 'comedy', 'horror', 'exercise', 'vlog', 'etc']
EMOS = ['neutral', 'happy', 'surprise', 'sad', 'angry']


def dist(rng):
    raw = [rng.random() ** 2 for _ in EMOS]
    s = sum(raw)
    return {e: round(r / s, 4) for e, r in zip(EMOS, raw)}


def pool_video(rng, i, category=None, base=None):
    d = dist(rng)
    dominant = max(d, key=d.get)
    return {
        'video_id': f'v{i:04d}', 'youtube_url': f'u{i}', 'title': f't{i}',
        'category': category or rng.choice(GENRES), 'dominant_emotion': dominant,
        'dominant_emotion_per': round(d[dominant] * 100, 2), 'emotion_distribution': d,
        'base_score': base if base is not None else round(rng.uniform(0, 100), 4),
    }


def make_pool(rng, n):
    pool = [pool_video(rng, i) for i in range(n)]
    pool.sort(key=lambda v: v['base_score'], reverse=True)
    return pool


def summaries(rng, pool, n):
    # Shape produced by HomeService.get_personalized_videos from Mongo summaries.
    cat = {v['video_id']: v['category'] for v in pool}
    out = []
    for _ in range(n):
        v = rng.choice(pool)
        d = dist(rng)
        out.append({'category': cat.get(v['video_id']), 'dominant_emotion': max(d, key=d.get), 'emotion_percentages': d})
    return out


def both_rank(pool, recent, fav, viewed, seed, **kw):
    random.seed(seed)
    a = ORIG.rank_personalized(pool, recent, fav, set(viewed) if viewed is not None else None, **kw)
    random.seed(seed)
    b = PORT.rank_personalized(pool, recent, fav, set(viewed) if viewed is not None else None, **kw)
    return a, b


class BaseScoreOracle(unittest.TestCase):
    def check(self, stats):
        self.assertEqual(ORIG.compute_base_score(dict(stats)), PORT.compute_base_score(dict(stats), now=FIXED_NOW), stats)

    def test_edge_cases(self):
        iso = lambda days: (FIXED_NOW - timedelta(days=days, seconds=1)).isoformat()
        cases = [
            {}, {'emotion_distribution': {}}, {'emotion_distribution': None, 'sample_frames': None},
            {'emotion_distribution': {'neutral': 1.0}, 'sample_frames': 10_000, 'average_completion_rate': 1.0},
            {'emotion_distribution': {'neutral': 1.7}, 'sample_frames': 5, 'average_completion_rate': 9},
            {'emotion_distribution': {'happy': 0.5, 'sad': 0.5}, 'sample_frames': 0, 'average_completion_rate': 0.5},
            {'emotion_distribution': {'happy': 1}, 'sample_frames': 360, 'view_count': -5, 'like_count': -1},
            {'emotion_distribution': {'angry': 1}, 'sample_frames': 360, 'view_count': 10**9, 'like_count': 10**9},
            {'emotion_distribution': {'angry': 1}, 'sample_frames': 360, 'view_count': None, 'like_count': None},
            {'emotion_distribution': {'surprise': 0.4}, 'sample_frames': 1, 'created_at': 'not-a-date'},
            {'emotion_distribution': {'surprise': 0.4}, 'sample_frames': 1, 'created_at': ''},
            {'emotion_distribution': {'surprise': 0.4}, 'sample_frames': 1, 'created_at': None},
            {'emotion_distribution': {'surprise': 0.4}, 'sample_frames': 1, 'created_at': FIXED_NOW - timedelta(days=3)},
            {'emotion_distribution': {'surprise': 0.4}, 'sample_frames': 1, 'created_at': datetime(2026, 10, 1, tzinfo=timezone.utc)},
            {'emotion_distribution': {'surprise': 0.4}, 'sample_frames': 1, 'created_at': '2026-10-17T00:00:00Z'},
            {'emotion_distribution': {'surprise': 0.4}, 'sample_frames': 1, 'created_at': '2026-10-17T00:00:00+09:00'},
            {'emotion_distribution': {'surprise': 0.4}, 'sample_frames': 1, 'created_at': FIXED_NOW + timedelta(days=5)},
        ]
        # Freshness bucket edges: days <=2, <=7, <=14, <=30, then the exponential tail and its 0.1 floor.
        for days in (0, 1, 2, 3, 7, 8, 14, 15, 30, 31, 90, 365, 3650):
            cases.append({'emotion_distribution': {'happy': 0.3, 'neutral': 0.4}, 'sample_frames': 200, 'created_at': iso(days)})
        for stats in cases:
            self.check(stats)

    def test_fuzz(self):
        rng = random.Random(1018)
        for _ in range(2000):
            d = dist(rng) if rng.random() > 0.1 else {}
            stats = {
                'emotion_distribution': d, 'average_completion_rate': rng.choice([0, 0.2, 0.9999, 1, 1.5, None, rng.random()]),
                'sample_frames': rng.choice([0, 1, 199, 200, 201, 360, 5000, None, rng.randint(0, 10**5)]),
                'view_count': rng.choice([0, 1, 999999, 10**6, 10**7, None, rng.randint(0, 10**8)]),
                'like_count': rng.choice([0, 1, 9999, 10**4, None, rng.randint(0, 10**6)]),
                'created_at': rng.choice([None, (FIXED_NOW - timedelta(days=rng.randint(0, 800), hours=rng.randint(0, 23))).isoformat()]),
            }
            self.check(stats)


class UserProfileOracle(unittest.TestCase):
    def test_cases(self):
        rng = random.Random(7)
        pool = make_pool(rng, 40)
        cases = [
            [], [{'emotion_percentages': None}], [{'emotion_percentages': {}}], [{'emotion_percentages': {'happy': 0}}],
            [{'emotion_percentages': {'bogus': 1.0, 'happy': 0.5}}], [{}],
            summaries(rng, pool, 1), summaries(rng, pool, 20), summaries(rng, pool, 25),
        ]
        for recent in cases:
            self.assertEqual(ORIG.build_user_emotion_profile(recent), PORT.build_user_emotion_profile(recent))

    def test_cold_start_profile_is_uniform_non_neutral(self):
        self.assertEqual(PORT.build_user_emotion_profile([]), {'neutral': 0.0, 'happy': 0.25, 'surprise': 0.25, 'sad': 0.25, 'angry': 0.25})


class RankOracle(unittest.TestCase):
    def assertSame(self, pool, recent, fav, viewed, seed=1, **kw):
        a, b = both_rank(pool, recent, fav, viewed, seed, **kw)
        self.assertEqual([v.get('video_id') for v in a], [v.get('video_id') for v in b])
        self.assertEqual(a, b)
        return a

    def test_twenty_mongo_summaries_with_favorites_and_viewed_exclusion(self):
        rng = random.Random(20)
        pool = make_pool(rng, 400)
        recent = summaries(rng, pool, 20)
        viewed = {v['video_id'] for v in rng.sample(pool, 60)}
        out = self.assertSame(pool, recent, ['music', 'game', 'horror', 'vlog'], viewed)
        self.assertEqual(len(out), 20)
        self.assertFalse({v['video_id'] for v in out} & viewed)

    def test_favorite_genre_rank_bonus_capped_at_third_tier(self):
        rng = random.Random(3)
        pool = make_pool(rng, 30)
        for v in pool:
            v['base_score'] = 50.0
            v['emotion_distribution'] = {}
        for fav in (['drama'], ['drama', 'eating'], ['x', 'y', 'z', 'drama'], ['x', 'y', 'z', 'w', 'drama'], []):
            self.assertSame(pool, [], fav, set())

    def test_cold_start_no_recent_no_favorites(self):
        rng = random.Random(5)
        pool = make_pool(rng, 120)
        self.assertSame(pool, [], [], set())
        self.assertSame(pool, [], [], None)

    def test_all_viewed_and_empty_pool_return_empty(self):
        rng = random.Random(6)
        pool = make_pool(rng, 15)
        self.assertEqual(self.assertSame(pool, [], ['music'], {v['video_id'] for v in pool}), [])
        self.assertEqual(self.assertSame([], [], [], set()), [])

    def test_pool_larger_than_head_uses_seeded_exploration(self):
        rng = random.Random(8)
        pool = make_pool(rng, 600)
        # The pool contract is "sorted by base_score", but nothing enforces it; scramble the tail so that
        # exploration candidates can reach the final top 20 and the seed visibly matters.
        for v in pool[150:]:
            v['base_score'] = round(rng.uniform(60, 100), 4)
        recent = summaries(rng, pool, 20)
        outs = [tuple(v['video_id'] for v in self.assertSame(pool, recent, ['comedy'], set(), seed=s)) for s in range(12)]
        self.assertGreater(len(set(outs)), 1, 'seed must change the exploration draw, otherwise this test proves nothing')
        self.assertSame(pool, recent, [], set(), seed=3, top_n=10, random_n=5, limit=7)
        self.assertSame(pool, recent, [], set(), seed=3, top_n=0, random_n=30, limit=25)

    def test_diversity_first_pass_never_three_same_category_in_a_row(self):
        rng = random.Random(9)
        pool = []
        for i in range(60):
            v = pool_video(rng, i, category=('music', 'game', 'music')[i % 3], base=100.0 - i)
            v['dominant_emotion'] = ('happy', 'sad', 'angry', 'surprise')[i % 4]
            v['emotion_distribution'] = {}
            pool.append(v)
        out = self.assertSame(pool, [], [], set(), limit=12)
        cats = [v['category'] for v in out]
        self.assertTrue(all(not (cats[i] == cats[i + 1] == cats[i + 2]) for i in range(len(cats) - 2)), cats)

    def test_diversity_is_single_pass_so_backfill_can_restore_a_streak(self):
        # Pinned original quirk: the filter never revisits skipped items, so clustered scores leave the first pass
        # short and the backfill appends same-category runs. The port must reproduce it, not "fix" it.
        rng = random.Random(14)
        pool = []
        for i in range(60):
            v = pool_video(rng, i, category='music' if i < 40 else 'game', base=100.0 - i)
            v['dominant_emotion'] = 'happy' if i % 2 == 0 else 'sad'
            v['emotion_distribution'] = {}
            pool.append(v)
        out = self.assertSame(pool, [], [], set(), limit=12)
        cats = [v['category'] for v in out]
        self.assertTrue(any(cats[i] == cats[i + 1] == cats[i + 2] for i in range(len(cats) - 2)), cats)

    def test_same_emotion_streak_and_backfill_when_filter_cannot_fill_limit(self):
        rng = random.Random(10)
        pool = [pool_video(rng, i, category=f'c{i}', base=90.0 - i) for i in range(10)]
        for v in pool:
            v['dominant_emotion'] = 'happy'
            v['emotion_distribution'] = {}
        out = self.assertSame(pool, [], [], set(), limit=8)
        self.assertEqual(len(out), 8, 'backfill from remaining candidates must still reach the limit')
        # all-same category too
        same = [pool_video(rng, 100 + i, category='music', base=80.0 - i) for i in range(6)]
        self.assertEqual(len(self.assertSame(same, [], [], set(), limit=5)), 5)

    def test_absent_dominant_emotion_key_counts_as_neutral_for_streaks(self):
        rng = random.Random(15)
        pool = []
        for i in range(24):
            v = pool_video(rng, i, category=f'c{i}', base=90.0 - i)
            v['emotion_distribution'] = {}
            v['dominant_emotion'] = ('happy', 'sad')[i % 2]
            pool.append(v)
        del pool[0]['dominant_emotion']            # original default: 'neutral'
        pool[1]['dominant_emotion'] = 'neutral'
        del pool[2]['dominant_emotion']
        del pool[3]['dominant_emotion']
        out = self.assertSame(pool, [], [], set(), limit=4)
        self.assertEqual([v['video_id'] for v in out], ['v0000', 'v0001', 'v0002', 'v0004'], 'fourth neutral-equivalent after three in a row is skipped')

    def test_zero_magnitude_video_distribution_has_zero_affinity(self):
        rng = random.Random(16)
        pool = [pool_video(rng, i, category=f'c{i}', base=50.0) for i in range(12)]
        for i, v in enumerate(pool):
            v['emotion_distribution'] = [{'happy': 0}, {'bogus': 1.0}, {'neutral': 0.0, 'sad': 0.0}][i % 3]
        self.assertSame(pool, summaries(rng, pool, 5), [], set(), limit=12)
        self.assertEqual(ORIG.emotion_cosine({'happy': 1.0}, {}), PORT.emotion_cosine({'happy': 1.0}, {}))
        self.assertEqual(PORT.emotion_cosine({'happy': 1.0}, {'happy': 0}), 0.0)

    def test_ties_keep_stable_order(self):
        rng = random.Random(11)
        pool = [pool_video(rng, i, category=GENRES[i % 16], base=42.0) for i in range(30)]
        for v in pool:
            v['emotion_distribution'] = {}
            v['dominant_emotion'] = None
        out = self.assertSame(pool, [], [], set(), limit=30)
        self.assertEqual(len(out), 30)

    def test_missing_optional_fields(self):
        pool = [{'video_id': 'a'}, {'video_id': 'b', 'category': 'music', 'base_score': 3}, {'category': 'game'}, {'video_id': 'c', 'dominant_emotion': None}]
        self.assertSame(pool, [{'category': None, 'emotion_percentages': {'happy': 1}}], ['music'], set())

    def test_limit_edges(self):
        rng = random.Random(12)
        pool = make_pool(rng, 40)
        for limit in (0, 1, 2, 39, 40, 41, 500):
            self.assertSame(pool, summaries(rng, pool, 3), ['sports'], set(), limit=limit)

    def test_fuzz_against_original(self):
        rng = random.Random(1019)
        for case in range(400):
            n = rng.choice([0, 1, 5, 19, 20, 21, 80, 151, 152, 200, 260, 400])
            pool = make_pool(rng, n)
            if n and case % 5 == 0:
                for v in pool:
                    v['base_score'] = round(v['base_score'] / 10) * 10  # force many ties
                pool.sort(key=lambda v: v['base_score'], reverse=True)
            recent = summaries(rng, pool, rng.choice([0, 1, 3, 10, 11, 20])) if n else []
            fav = rng.sample(GENRES, rng.choice([0, 1, 2, 3, 5]))
            viewed = {v['video_id'] for v in rng.sample(pool, rng.randint(0, n))} if n else set()
            self.assertSame(pool, recent, fav, viewed, seed=case, limit=rng.choice([1, 5, 20, 30]))

    def test_injected_rng_instance_matches_global_seeded_original(self):
        rng = random.Random(13)
        pool = make_pool(rng, 500)
        recent = summaries(rng, pool, 20)
        random.seed(77)
        a = ORIG.rank_personalized(pool, recent, ['vlog'], set())
        b = PORT.rank_personalized(pool, recent, ['vlog'], set(), rng=random.Random(77))
        self.assertEqual(a, b)


if __name__ == '__main__':
    unittest.main(verbosity=2)
