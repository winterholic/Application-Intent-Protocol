# Slice13-D pool builder oracle: compare the pure AIP-side Tier1 pool builder with the
# ORIGINAL FaceReview HomeService._build_and_cache_ranked_pool.
#
# Oracle = app/services/home_service.py at the pinned original revision, extracted with
# `git show <sha>:<path>` from a read-only checkout and executed as-is. Nothing here copies
# the original scoring/pool/category/padding logic into the test. Fakes stand in for the
# I/O the original performs (Video.query, VideoLike GROUP BY, VideoDistributionRepository,
# redis pipeline); the module under test is the pure builder, which performs no I/O at all.
# Randomness is frozen identically for both sides: the original consumes the process-global
# `random` (seeded with `random.seed(seed)`), the builder consumes `random.Random(seed)`.
#
#   FACEREVIEW_SOURCE_PATH=/path/to/new-facereview python3 product/tests/slice13d_pool_builder_oracle_test.py
import importlib.util
import json
import logging
import os
import random
import subprocess
import sys
import tempfile
import types
import unittest
from datetime import datetime, timedelta

sys.dont_write_bytecode = True

ORIGINAL_SHA = '7948b7e721837fdb73c427cb54e12cad4ec2d5af'
ORIGINAL_HOME_PATH = 'app/services/home_service.py'
ORIGINAL_ALG_PATH = 'common/utils/recommendation_alg.py'
ORIGINAL_GENRE_PATH = 'common/enum/youtube_genre.py'
HERE = os.path.dirname(os.path.abspath(__file__))
EXT_DIR = os.environ.get('SLICE13D_EXT_DIR') or os.path.join(HERE, 'fixtures', 'facereview-mysql', 'extensions')

POOL_KEY = 'facereview:reco:pool'
CATEGORY_KEY = 'facereview:reco:category'


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _show(rev_path):
    source = os.environ.get('FACEREVIEW_SOURCE_PATH')
    if not source:
        raise unittest.SkipTest('FACEREVIEW_SOURCE_PATH not set; the original oracle is required')
    head = subprocess.run(['git', '-C', source, 'cat-file', '-e', f'{ORIGINAL_SHA}^{{commit}}'], capture_output=True)
    if head.returncode != 0:
        raise AssertionError(f'pinned original revision {ORIGINAL_SHA} not present in {source}')
    return subprocess.run(['git', '-C', source, 'show', f'{ORIGINAL_SHA}:{rev_path}'], capture_output=True, check=True).stdout


def _blob_module(name, rev_path):
    tmp = tempfile.mkdtemp(prefix='slice13d-pool-orig-')
    path = os.path.join(tmp, name.replace('.', '_') + '.py')
    with open(path, 'wb') as f:
        f.write(_show(rev_path))
    return _load(name, path)


ORIG_ALG = _blob_module('recommendation_alg_original', ORIGINAL_ALG_PATH)
GENRE_MOD = _blob_module('youtube_genre_original', ORIGINAL_GENRE_PATH)
GenreEnum = GENRE_MOD.GenreEnum
ALL_GENRES = [g.value for g in GenreEnum]


def _mod(name, **attrs):
    m = types.ModuleType(name)
    for k, v in attrs.items():
        setattr(m, k, v)
    sys.modules[name] = m
    return m


class _Col:
    """Minimal SQLAlchemy column stand-in: records == / in_ as tuples for the fake query."""

    def __init__(self, name):
        self.name = name

    def __eq__(self, other):
        return ('eq', self.name, other)

    def __hash__(self):
        return hash(self.name)

    def in_(self, values):
        return ('in', self.name, list(values))


class FakeVideo:
    """Row stand-in for the original Video ORM object (category kept as the real GenreEnum)."""

    is_deleted = _Col('is_deleted')
    category = _Col('category')
    video_id = _Col('video_id')

    def __init__(self, video_id, youtube_url, title, category, view_count=None, created_at=None, is_deleted=0):
        self.video_id = video_id
        self.youtube_url = youtube_url
        self.title = title
        self.category = category
        self.view_count = view_count
        self.created_at = created_at
        self.is_deleted = is_deleted


VIDEO_STATE = []
DIST_STATE = {}
LIKE_ROWS = {}


class _VideoQuery:
    def __init__(self, ops=()):
        self.ops = ops

    def filter_by(self, **kw):
        return _VideoQuery(self.ops + (('filter_by', kw),))

    def filter(self, *criteria):
        return _VideoQuery(self.ops + (('filter', criteria),))

    def all(self):
        rows = list(VIDEO_STATE)
        for op in self.ops:
            if op[0] == 'filter_by':
                for k, v in op[1].items():
                    rows = [r for r in rows if getattr(r, k) == v]
            else:
                for kind, col, val in op[1]:
                    if kind == 'eq':
                        rows = [r for r in rows if getattr(r, col) == val]
                    elif kind == 'in':
                        rows = [r for r in rows if getattr(r, col) in val]
        return rows


class VideoLike:
    video_id = _Col('video_id')
    video_like_id = _Col('video_like_id')


class _GroupQuery:
    def group_by(self, *args):
        return self

    def all(self):
        return list(LIKE_ROWS.items())


class _FakeSession:
    def query(self, *cols):
        return _GroupQuery()


class _FakeCollection:
    def find(self, query):
        ids = (query.get('video_id') or {}).get('$in', [])
        return [DIST_STATE[v] for v in ids if v in DIST_STATE]


class VideoDistributionRepository:
    def __init__(self, mongo_db):
        self.collection = _FakeCollection()


class FakePipeline:
    def __init__(self, owner):
        self.owner = owner
        self.ops = []

    def set(self, key, value, ex=None):
        self.ops.append(('set', key, value, ex))
        return True

    def delete(self, key):
        self.ops.append(('delete', key))
        return 1

    def hset(self, h, field, value):
        self.ops.append(('hset', h, field, value))
        return 1

    def expire(self, key, ttl):
        self.ops.append(('expire', key, ttl))
        return True

    def execute(self):
        self.owner.batches.append(list(self.ops))
        return []


class FakeRedis:
    def __init__(self):
        self.batches = []

    def pipeline(self):
        return FakePipeline(self)


EXT_STUB = types.ModuleType('common.extensions')


def _install_stubs():
    for name in ('common', 'common.decorator', 'common.enum', 'common.exception', 'common.utils', 'app', 'app.models',
                 'app.models.mongodb', 'app.dto'):
        if name not in sys.modules:
            sys.modules[name] = types.ModuleType(name)
    ident = lambda f: f
    _mod('common.decorator.db_decorators', transactional_readonly=ident, transactional=ident)
    _mod('app.models.user', User=type('User', (), {}))
    _mod('app.models.video', Video=type('Video', (), {'query': _VideoQuery()}))
    sys.modules['app.models.video'].Video.is_deleted = FakeVideo.is_deleted
    sys.modules['app.models.video'].Video.category = FakeVideo.category
    sys.modules['app.models.video'].Video.video_id = FakeVideo.video_id
    _mod('app.models', VideoViewLog=type('VideoViewLog', (), {}), VideoRequest=type('VideoRequest', (), {}),
         VideoBookmark=type('VideoBookmark', (), {}))
    _mod('app.models.video_like', VideoLike=VideoLike)
    _mod('app.models.mongodb.youtube_watching_data', YoutubeWatchingDataRepository=type('YoutubeWatchingDataRepository', (), {}))
    _mod('app.models.mongodb.video_distribution', VideoDistributionRepository=VideoDistributionRepository)
    _mod('common.enum.error_code', APIError=type('APIError', (Exception,), {}))
    sys.modules['common.enum.youtube_genre'] = GENRE_MOD
    _mod('common.exception.exceptions', BusinessError=type('BusinessError', (Exception,), {}))
    EXT_STUB.db = types.SimpleNamespace(session=_FakeSession())
    EXT_STUB.mongo_db = object()
    EXT_STUB.redis_client = FakeRedis()
    sys.modules['common.extensions'] = EXT_STUB
    alg = types.ModuleType('common.utils.recommendation_alg')
    alg.compute_base_score = ORIG_ALG.compute_base_score
    alg.rank_personalized = ORIG_ALG.rank_personalized
    sys.modules['common.utils.recommendation_alg'] = alg
    _mod('common.utils.logging_utils', get_logger=lambda name: logging.getLogger('orig-' + name))
    import dataclasses

    @dataclasses.dataclass
    class BaseVideoDataDto:
        video_id: str
        youtube_url: str
        title: str
        dominant_emotion: object = None
        dominant_emotion_per: object = None
        is_bookmarked: bool = False

    @dataclasses.dataclass
    class CategoryVideoDataDto:
        category_name: str
        videos: list = dataclasses.field(default_factory=list)

    @dataclasses.dataclass
    class CategoryVideoDataListDto:
        video_data: list = dataclasses.field(default_factory=list)

    @dataclasses.dataclass
    class AllVideoDataDto:
        videos: list = dataclasses.field(default_factory=list)
        total: int = 0
        page: int = 1
        size: int = 20
        has_next: bool = False

    _mod('app.dto.home', BaseVideoDataDto=BaseVideoDataDto, CategoryVideoDataDto=CategoryVideoDataDto,
         CategoryVideoDataListDto=CategoryVideoDataListDto, AllVideoDataDto=AllVideoDataDto)
    _mod('sqlalchemy', or_=lambda *a: ('or', a), func=types.SimpleNamespace(count=lambda c: ('count', c)))


_install_stubs()
HS = _blob_module('home_service_original', ORIGINAL_HOME_PATH)
BUILDER = _load('recommendation_pool_builder', os.path.join(EXT_DIR, 'recommendation_pool_builder.py'))

BASE_TIME = datetime(2026, 10, 10, 12, 0, 0)
EMOS = ['neutral', 'happy', 'surprise', 'sad', 'angry']


def make_dist(rng, dominant=None, zero_sum=False):
    if zero_sum:
        return {'dominant_emotion': dominant or 'happy', 'emotion_averages': {e: 0.0 for e in EMOS},
                'emotion_counts': {}, 'total_frames': 0, 'average_completion_rate': 0.5}
    raw = [rng.random() ** 2 for _ in EMOS]
    s = sum(raw)
    avgs = {e: round(v / s, 4) for e, v in zip(EMOS, raw)}
    dom = dominant or max(avgs, key=avgs.get)
    return {'dominant_emotion': dom, 'emotion_averages': avgs,
            'emotion_counts': {e: int(avgs[e] * 100) for e in EMOS},
            'total_frames': 500 + rng.randint(0, 500), 'average_completion_rate': round(rng.uniform(0, 1), 4)}


def make_fixture(rng, vid, category='music', with_dist=True, likes=None, created_at='default',
                 is_deleted=0, view_count=None, dist=None, no_dominant=False, zero_sum=False):
    if created_at == 'default':
        created_at = BASE_TIME - timedelta(days=rng.choice([0, 1, 5, 10, 40]))
    if view_count is None:
        view_count = rng.choice([0, 10, 5000, 10**6, None])
    if dist is None and with_dist:
        dist = make_dist(rng, zero_sum=zero_sum)
        if no_dominant:
            dist = dict(dist)
            dist.pop('dominant_emotion', None)
    return {'video_id': vid, 'youtube_url': f'https://youtu.be/{vid}', 'title': f'title-{vid}',
            'category': category, 'view_count': view_count, 'created_at': created_at,
            'is_deleted': is_deleted, 'dist': dist, 'likes': likes}


def install_fixture(fixtures):
    global VIDEO_STATE, DIST_STATE, LIKE_ROWS
    VIDEO_STATE = []
    DIST_STATE = {}
    LIKE_ROWS = {}
    for f in fixtures:
        VIDEO_STATE.append(FakeVideo(
            f['video_id'], f['youtube_url'], f['title'], GenreEnum(f['category']),
            view_count=f.get('view_count'), created_at=f.get('created_at'), is_deleted=f.get('is_deleted', 0)))
        if f.get('dist') is not None:
            doc = dict(f['dist'])
            doc['video_id'] = f['video_id']
            DIST_STATE[f['video_id']] = doc
        if f.get('likes') is not None:
            LIKE_ROWS[f['video_id']] = f['likes']


def builder_inputs(fixtures):
    videos = []
    for f in fixtures:
        created = f.get('created_at')
        videos.append({'video_id': f['video_id'], 'youtube_url': f['youtube_url'], 'title': f['title'],
                       'category': f['category'], 'view_count': f.get('view_count'),
                       'created_at': created.isoformat() if hasattr(created, 'isoformat') else created,
                       'is_deleted': f.get('is_deleted', 0)})
    dist_docs = {vid: doc for vid, doc in DIST_STATE.items()}
    return videos, dist_docs, dict(LIKE_ROWS)


class PoolBuilderOracle(unittest.TestCase):
    def run_both(self, fixtures, seed=1, raw_by_cat=None, as_generator=False):
        install_fixture(fixtures)
        videos, dist_docs, like_map = builder_inputs(fixtures)
        if as_generator:
            videos = iter(videos)
        redis = FakeRedis()
        EXT_STUB.redis_client = redis
        HS._MEM_CACHE.update(pool=None, categories=None, ts=0.0)
        random.seed(seed)
        orig_pool, orig_dtos = HS.HomeService._build_and_cache_ranked_pool()
        pool2, cats2 = BUILDER.build_ranked_pool(
            videos, dist_docs, like_map, ALL_GENRES, rng=random.Random(seed), raw_by_cat=raw_by_cat)
        self.assertEqual(pool2, orig_pool)
        # Pinned original quirk: deficient categories with no padding rows stay in the
        # in-memory dict as empty buckets, but Redis HSET and the category DTOs skip
        # them. The port reproduces the dict; only non-empty buckets are observable.
        nonempty2 = {c: es for c, es in cats2.items() if es}
        self.assertEqual(len(redis.batches), 1, 'original must write redis in exactly one pipeline')
        ops = redis.batches[0]
        self.assertEqual(ops[0][0], 'set')
        self.assertEqual(ops[0][1], POOL_KEY)
        self.assertEqual(json.loads(ops[0][2]), pool2)
        hsets = [o for o in ops if o[0] == 'hset']
        self.assertTrue(all(o[1] == CATEGORY_KEY for o in hsets))
        self.assertEqual([o[2] for o in hsets], list(nonempty2.keys()))
        for _, _, cat, payload in hsets:
            self.assertEqual(json.loads(payload), nonempty2[cat])
        self.assertEqual(
            [(d.category_name, [(v.video_id, v.youtube_url, v.title, v.dominant_emotion, v.dominant_emotion_per)
                                for v in d.videos]) for d in orig_dtos],
            [(c, [(e['video_id'], e['youtube_url'], e['title'], e['dominant_emotion'], e['dominant_emotion_per'])
                  for e in es]) for c, es in nonempty2.items()])
        self.assertTrue(all(len(es) <= 20 for es in cats2.values()))
        scores = [e['base_score'] for e in pool2]
        self.assertEqual(scores, sorted(scores, reverse=True))
        return pool2, cats2

    def test_empty_videos(self):
        pool, cats = self.run_both([], seed=1)
        self.assertEqual(pool, [])
        self.assertEqual(set(cats), set(ALL_GENRES))
        self.assertTrue(all(v == [] for v in cats.values()))

    def test_videos_without_dist_docs_are_skipped(self):
        rng = random.Random(21)
        fixtures = [make_fixture(rng, f'v{i:04d}', with_dist=False) for i in range(10)]
        pool, cats = self.run_both(fixtures, seed=2)
        self.assertEqual(pool, [])
        padded = cats['music']
        self.assertEqual(len(padded), len(fixtures))
        self.assertEqual({e['video_id'] for e in padded}, {f['video_id'] for f in fixtures})
        self.assertTrue(all(e['base_score'] == 0.0 and e['dominant_emotion'] is None for e in padded))

    def test_zero_sum_averages_are_skipped(self):
        rng = random.Random(22)
        fixtures = [make_fixture(rng, f'v{i:04d}', zero_sum=True) for i in range(8)]
        pool, cats = self.run_both(fixtures, seed=3)
        self.assertEqual(pool, [])
        padded = cats['music']
        self.assertEqual(len(padded), len(fixtures))
        self.assertEqual({e['video_id'] for e in padded}, {f['video_id'] for f in fixtures})
        self.assertTrue(all(e['base_score'] == 0.0 and e['dominant_emotion'] is None for e in padded))

    def test_missing_dominant_emotion_is_skipped(self):
        rng = random.Random(23)
        fixtures = [make_fixture(rng, f'v{i:04d}', no_dominant=True) for i in range(6)]
        fixtures.append(make_fixture(rng, 'vblank', dist={'emotion_averages': {'happy': 1.0}}))
        pool, cats = self.run_both(fixtures, seed=4)
        self.assertEqual(pool, [])
        padded = cats['music']
        self.assertEqual(len(padded), len(fixtures))
        self.assertEqual({e['video_id'] for e in padded}, {f['video_id'] for f in fixtures})
        self.assertTrue(all(e['base_score'] == 0.0 and e['dominant_emotion'] is None for e in padded))

    def test_deleted_videos_are_excluded(self):
        rng = random.Random(24)
        fixtures = [make_fixture(rng, f'v{i:04d}', is_deleted=1) for i in range(5)]
        fixtures += [make_fixture(rng, f'k{i:04d}') for i in range(3)]
        pool, cats = self.run_both(fixtures, seed=5)
        self.assertEqual({e['video_id'] for e in pool}, {'k0000', 'k0001', 'k0002'})
        self.assertEqual([e['video_id'] for e in cats['music']],
                         [e['video_id'] for e in pool if e['category'] == 'music'][:20])

    def test_pool_truncated_to_top_1000(self):
        rng = random.Random(25)
        fixtures = [make_fixture(rng, f'v{i:04d}', category=ALL_GENRES[i % len(ALL_GENRES)]) for i in range(1200)]
        pool, cats = self.run_both(fixtures, seed=6)
        self.assertEqual(len(pool), 1000)
        self.assertTrue(all(len(es) <= 20 for es in cats.values()))

    def test_category_20_boundary(self):
        rng = random.Random(26)
        fixtures = [make_fixture(rng, f'exact{i:02d}', category='music') for i in range(20)]
        fixtures += [make_fixture(rng, f'over{i:02d}', category='game') for i in range(25)]
        fixtures += [make_fixture(rng, f'short{i:02d}', category='vlog') for i in range(19)]
        fixtures += [make_fixture(rng, f'pad{i:02d}', category='vlog', with_dist=False) for i in range(10)]
        # Explicit raw_by_cat must mirror the original fill query: every video of the
        # deficient category in table order (scored rows first, then unscored rows).
        raw_vlog = [{'video_id': f['video_id'], 'youtube_url': f['youtube_url'], 'title': f['title']}
                    for f in fixtures if f['category'] == 'vlog']
        pool, cats = self.run_both(fixtures, seed=7, raw_by_cat={'vlog': raw_vlog})
        self.assertEqual(len(cats['music']), 20)
        self.assertEqual(len(cats['game']), 20)
        self.assertEqual(len(cats['vlog']), 20)
        scored_vlog = {f['video_id'] for f in fixtures if f['category'] == 'vlog' and f.get('dist') is not None}
        self.assertEqual({e['video_id'] for e in cats['vlog'][:19]}, scored_vlog)
        self.assertIn(cats['vlog'][19]['video_id'], {f'pad{i:02d}' for i in range(10)})
        self.assertEqual(cats['vlog'][19]['dominant_emotion'], None)
        self.assertEqual(cats['vlog'][19]['base_score'], 0.0)

    def test_seeded_random_padding_matches(self):
        rng = random.Random(27)
        fixtures = [make_fixture(rng, f'm{i:02d}', category='music') for i in range(3)]
        fixtures += [make_fixture(rng, f'u{i:02d}', category='music', with_dist=False) for i in range(8)]
        fixtures += [make_fixture(rng, f'g{i:02d}', category='game') for i in range(25)]
        _, cats_a = self.run_both(fixtures, seed=42)
        _, cats_b = self.run_both(fixtures, seed=43)
        # 3 scored entries keep their base_score order under both seeds; the 8 unscored
        # padding rows are shuffled, so the seeds visibly diverge while each still
        # matches the original draw for its own seed (checked inside run_both).
        self.assertEqual([e['video_id'] for e in cats_a['music'][:3]],
                         [e['video_id'] for e in cats_b['music'][:3]])
        self.assertEqual(len(cats_a['music']), 11)
        self.assertNotEqual([e['video_id'] for e in cats_a['music'][3:]],
                            [e['video_id'] for e in cats_b['music'][3:]])

    def test_ties_keep_stable_input_order(self):
        rng = random.Random(28)
        fixed = make_dist(rng)
        fixtures = []
        for i in range(30):
            f = make_fixture(rng, f't{i:02d}', category=ALL_GENRES[i % len(ALL_GENRES)],
                             dist=dict(fixed), view_count=100, created_at=BASE_TIME, likes=5)
            fixtures.append(f)
        pool, _ = self.run_both(fixtures, seed=8)
        self.assertEqual([e['video_id'] for e in pool], [f['video_id'] for f in fixtures])

    def test_like_zero_and_missing_and_none_counts(self):
        rng = random.Random(29)
        fixed = make_dist(rng)
        fixtures = [
            make_fixture(rng, 'lz', dist=dict(fixed), view_count=100, created_at=BASE_TIME, likes=0),
            make_fixture(rng, 'lm', dist=dict(fixed), view_count=100, created_at=BASE_TIME, likes=None),
            make_fixture(rng, 'ln', dist=dict(fixed), view_count=None, created_at=BASE_TIME, likes=None),
        ]
        pool, _ = self.run_both(fixtures, seed=9)
        by_id = {e['video_id']: e['base_score'] for e in pool}
        self.assertEqual(by_id['lz'], by_id['lm'])

    def test_null_is_deleted_matches_sql_exclusion(self):
        rng = random.Random(99)
        fixtures = [make_fixture(rng, 'null1', category='music', is_deleted=None)]
        pool, cats = self.run_both(fixtures, seed=1)
        self.assertEqual(pool, [])
        self.assertEqual(cats.get('music'), [])

    def test_generator_input_matches_list_for_pad_only(self):
        rng = random.Random(77)
        fixtures = [make_fixture(rng, 'padonly1', category='music', with_dist=False)]
        pool, cats = self.run_both(fixtures, seed=2, as_generator=True)
        self.assertEqual(pool, [])
        padded = cats['music']
        self.assertEqual(len(padded), 1)
        self.assertEqual(padded[0]['video_id'], 'padonly1')
        self.assertEqual(padded[0]['dominant_emotion'], None)
        self.assertEqual(padded[0]['base_score'], 0.0)

    def test_created_at_none(self):
        rng = random.Random(30)
        fixed = make_dist(rng)
        fixtures = [make_fixture(rng, f'n{i:02d}', dist=dict(fixed), view_count=50, created_at=None, likes=2)
                                 for i in range(5)]
        pool, _ = self.run_both(fixtures, seed=10)
        self.assertEqual(len(pool), 5)

    def test_seed_fixed_fuzz(self):
        rng = random.Random(1019)
        for case in range(60):
            n = rng.choice([0, 1, 5, 19, 20, 21, 60, 1050])
            fixtures = []
            for i in range(n):
                roll = rng.random()
                cat = rng.choice(ALL_GENRES)
                if roll < 0.08:
                    fixtures.append(make_fixture(rng, f'c{case:02d}v{i:04d}', category=cat, with_dist=False))
                elif roll < 0.12:
                    fixtures.append(make_fixture(rng, f'c{case:02d}v{i:04d}', category=cat, zero_sum=True))
                elif roll < 0.16:
                    fixtures.append(make_fixture(rng, f'c{case:02d}v{i:04d}', category=cat, no_dominant=True))
                elif roll < 0.20:
                    fixtures.append(make_fixture(rng, f'c{case:02d}v{i:04d}', category=cat, is_deleted=1))
                else:
                    like_roll = rng.random()
                    likes = None if like_roll < 0.4 else (0 if like_roll < 0.5 else rng.randint(0, 10**5))
                    created = None if rng.random() < 0.15 else BASE_TIME - timedelta(days=rng.randint(0, 400))
                    fixtures.append(make_fixture(rng, f'c{case:02d}v{i:04d}', category=cat,
                                                 likes=likes, created_at=created))
            use_explicit = (case % 3 == 0)
            raw = None
            if use_explicit:
                raw = {}
                for f in fixtures:
                    if f.get('is_deleted', 0):
                        continue
                    raw.setdefault(f['category'], []).append(
                        {'video_id': f['video_id'], 'youtube_url': f['youtube_url'], 'title': f['title']})
            self.run_both(fixtures, seed=1000 + case, raw_by_cat=raw)


if __name__ == '__main__':
    unittest.main(verbosity=2)
