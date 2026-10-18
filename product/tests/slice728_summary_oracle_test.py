"""Oracle tests for the FaceReview emotion summary fold and emotion DNA port (WBS 7.2.8).

The ORIGINAL functions are loaded from the pinned FaceReview checkout by AST extraction
(no Flask/SQLAlchemy/Mongo import) and run next to the port in
`fixtures/facereview-mysql/extensions/slice728_summary.py`.

Faked, not original: the Mongo query matcher, the SQLAlchemy models and session, and
`UserEmotionSummary.apply_delta` (its SQL compare-and-swap is re-modelled). Nothing here
talks to MySQL or Mongo, so MySQL column precision (DATETIME/TIMESTAMP seconds) is unverified.

Run: python3 -m unittest product/tests/slice728_summary_oracle_test.py
Set FACEREVIEW_SOURCE to override the source checkout; the pinned revision is enforced.
"""
import ast
import copy
import importlib.util
import os
import random
import subprocess
import sys
import types
import unittest
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, List, Optional

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(HERE / "fixtures" / "facereview-mysql" / "extensions"))
import slice728_summary as port  # noqa: E402

PINNED_REV = "7948b7e721837fdb73c427cb54e12cad4ec2d5af"
SOURCE = Path(os.environ.get(
    "FACEREVIEW_SOURCE", "/Users/winterholic/development/projects/facereview/new-facereview"))

T0 = datetime(2026, 10, 18, 12, 0, 0, 123456)


# ------------------------------------------------------------------ original loading

def _source_rev() -> str:
    out = subprocess.run(["git", "-C", str(SOURCE), "rev-parse", "HEAD"],
                         capture_output=True, text=True, check=True)
    return out.stdout.strip()


def _parse(rel: str) -> ast.Module:
    return ast.parse((SOURCE / rel).read_text(encoding="utf-8"), filename=rel)


class Clock:
    now = T0


class FrozenDatetime(datetime):
    @classmethod
    def utcnow(cls):
        return Clock.now


class BusinessError(Exception):
    pass


class IntegrityError(Exception):
    pass


class _APIError:
    USER_NOT_FOUND = "USER_NOT_FOUND"


class EmotionSummaryDto:
    def __init__(self, emotion_percentages, emotion_seconds):
        self.emotion_percentages = emotion_percentages
        self.emotion_seconds = emotion_seconds

    def to_dict(self):
        return {"emotion_percentages": self.emotion_percentages,
                "emotion_seconds": self.emotion_seconds}


def _load_original():
    if not SOURCE.is_dir():
        raise unittest.SkipTest(f"FaceReview source not found: {SOURCE}")
    rev = _source_rev()
    if rev != PINNED_REV:
        raise AssertionError(f"FaceReview source is at {rev}, expected {PINNED_REV}")

    util_spec = importlib.util.spec_from_file_location(
        "orig_emotion_summary", SOURCE / "common/utils/emotion_summary.py")
    util = importlib.util.module_from_spec(util_spec)
    util_spec.loader.exec_module(util)

    service = _parse("app/services/mypage_service.py")
    wanted_defs = {
        "_extract_emotion_scores", "_estimate_watch_seconds_from_summary",
        "_build_emotion_summary_from_docs", "_determine_dna_type", "_compute_traits",
        "_compute_fun_facts", "_build_default_dna",
    }
    wanted_assigns = {"_DNA_TYPES", "_TRAIT_POOL", "_DNA_TRAIT_KEYS"}
    body: List[ast.stmt] = []
    for node in service.body:
        if isinstance(node, ast.FunctionDef) and node.name in wanted_defs:
            body.append(node)
        elif isinstance(node, ast.Assign) and any(
                isinstance(t, ast.Name) and t.id in wanted_assigns for t in node.targets):
            body.append(node)
        elif isinstance(node, ast.ClassDef) and node.name == "MypageService":
            for item in node.body:
                if isinstance(item, ast.FunctionDef) and item.name in (
                        "get_emotion_summary", "get_emotion_dna"):
                    item.decorator_list = []
                    body.append(item)
    found = {n.name for n in body if isinstance(n, ast.FunctionDef)}
    assert found == wanted_defs | {"get_emotion_summary", "get_emotion_dna"}, found

    repo_tree = _parse("app/models/mongodb/youtube_watching_data.py")
    repo_method = None
    for node in repo_tree.body:
        if isinstance(node, ast.ClassDef) and node.name == "YoutubeWatchingDataRepository":
            for item in node.body:
                if isinstance(item, ast.FunctionDef) and item.name == "find_finalized_emotion_summaries_since":
                    repo_method = item
    assert repo_method is not None

    return util, ast.Module(body=body, type_ignores=[]), ast.Module(body=[repo_method], type_ignores=[])


# ------------------------------------------------------------------ fakes

def _match(doc: Dict, query: Dict) -> bool:
    for key, cond in query.items():
        if key == "$or":
            if not any(_match(doc, sub) for sub in cond):
                return False
            continue
        if isinstance(cond, dict) and cond and all(k.startswith("$") for k in cond):
            if key not in doc:
                return False
            for op, operand in cond.items():
                assert op == "$gt", op
                if not doc[key] > operand:
                    return False
        elif key not in doc or doc[key] != cond:
            return False
    return True


def _project(doc: Dict, projection: Optional[Dict]) -> Dict:
    if not projection:
        return dict(doc)
    include = any(v for k, v in projection.items() if k != "_id")
    if include:
        out = {k: v for k, v in doc.items() if projection.get(k)}
        if projection.get("_id", 1) and "_id" in doc:
            out["_id"] = doc["_id"]
        return out
    return {k: v for k, v in doc.items() if projection.get(k, 1)}


class Cursor:
    def __init__(self, docs):
        self.docs = docs

    def sort(self, spec):
        for key, direction in reversed(spec):
            self.docs.sort(key=lambda d: d[key], reverse=direction < 0)
        return self

    def __iter__(self):
        return iter(self.docs)


class Collection:
    def __init__(self, world):
        self.world = world

    def find(self, query, projection=None):
        self.world.find_log.append((copy.deepcopy(query), copy.deepcopy(projection)))
        return Cursor([_project(copy.deepcopy(d), projection)
                       for d in self.world.sessions if _match(d, query)])

    def count_documents(self, query):
        self.world.count_log.append(copy.deepcopy(query))
        return sum(1 for d in self.world.sessions if _match(d, query))


class SummaryRow:
    COLS = ("neutral_seconds", "happy_seconds", "surprise_seconds", "sad_seconds",
            "angry_seconds", "last_finalized_at", "last_session_id", "lock_version",
            "created_at", "updated_at")

    def __init__(self, user_id, **values):
        self.user_id = user_id
        for col in self.COLS:
            setattr(self, col, values.get(col))

    def emotion_seconds_dict(self):
        return {e: int(getattr(self, f"{e}_seconds")) for e in port.EMOTIONS}

    def snapshot(self):
        return {col: getattr(self, col) for col in ("user_id",) + self.COLS}


class Query:
    def __init__(self, table):
        self.table = table

    def filter_by(self, **kw):
        user_id = kw["user_id"]
        extra = {k: v for k, v in kw.items() if k not in ("user_id",)}
        return _Result(self.table, user_id, extra)


class _Result:
    def __init__(self, table, user_id, extra):
        self.table, self.user_id, self.extra = table, user_id, extra

    def first(self):
        row = self.table.get(self.user_id)
        if row is None:
            return None
        for k, v in self.extra.items():
            if getattr(row, k, None) != v:
                return None
        return row

    def update(self, values, synchronize_session=False):
        row = self.first()
        if row is None:
            return 0
        for column, value in values.items():
            setattr(row, column, value)
        return 1


class World:
    """One actor-scoped data set shared by the original and the port."""

    def __init__(self):
        self.users = {"u1": {"is_deleted": 0}, "u2": {"is_deleted": 0}}
        self.sessions: List[Dict] = []
        self.summary: Dict[str, SummaryRow] = {}
        self.dna: Dict[str, types.SimpleNamespace] = {}
        self.videos: Dict[str, object] = {}
        self.find_log: List = []
        self.count_log: List = []
        self.before_cas = []  # callables run once before each CAS attempt
        self.before_flush = []  # callables run before the first-create flush
        self.pending: List[SummaryRow] = []
        self.cas_attempts = 0

    def snapshot_summary(self, user_id):
        row = self.summary.get(user_id)
        return None if row is None else row.snapshot()


def _install_models(world: World, ns: Dict):
    class Col:
        def in_(self, ids):
            return ("in", list(ids))

    class VideoQuery:
        def filter(self, cond):
            assert cond[0] == "in"
            return types.SimpleNamespace(all=lambda: [
                types.SimpleNamespace(video_id=v, category=world.videos[v])
                for v in cond[1] if v in world.videos])

    class Video:
        video_id = Col()
        query = VideoQuery()

    class UserQuery:
        def filter_by(self, user_id, is_deleted):
            info = world.users.get(user_id)
            ok = info is not None and info["is_deleted"] == is_deleted
            return types.SimpleNamespace(first=lambda: types.SimpleNamespace(user_id=user_id) if ok else None)

    class User:
        query = UserQuery()

    class SummaryQuery(Query):
        pass

    class UserEmotionSummary:
        query = SummaryQuery(world.summary)

        def __new__(cls, user_id):
            return SummaryRow(user_id)

        @classmethod
        def apply_delta(cls, user_id, expected_version, emotion_seconds, checkpoint_at,
                        checkpoint_session_id):
            # Re-model of the original SQL: UPDATE ... WHERE user_id AND lock_version = expected.
            world.cas_attempts += 1
            if world.before_cas:
                world.before_cas.pop(0)(world)
            row = world.summary.get(user_id)
            if row is None or row.lock_version != expected_version:
                return False
            for e in port.EMOTIONS:
                col = f"{e}_seconds"
                setattr(row, col, getattr(row, col) + int(emotion_seconds.get(e, 0)))
            row.last_finalized_at = checkpoint_at
            row.last_session_id = checkpoint_session_id
            row.lock_version += 1
            row.updated_at = Clock.now
            return True

    class DnaRow(types.SimpleNamespace):
        pass

    class DnaQuery:
        def filter_by(self, user_id):
            return types.SimpleNamespace(first=lambda: world.dna.get(user_id))

    class UserEmotionDna:
        query = DnaQuery()

        def __new__(cls, **kw):
            return DnaRow(**kw)

    class Session:
        def add(self, obj):
            if isinstance(obj, SummaryRow):
                world.pending.append(obj)
            else:
                world.dna[obj.user_id] = obj

        def flush(self):
            for hook in world.before_flush:
                hook(world)
            world.before_flush.clear()
            for row in world.pending:
                if row.user_id in world.summary:
                    world.pending.clear()
                    raise IntegrityError("duplicate primary key")
                for e in port.EMOTIONS:
                    setattr(row, f"{e}_seconds", 0)
                row.lock_version = 0
                row.created_at = Clock.now
                row.updated_at = Clock.now
                world.summary[row.user_id] = row
            world.pending.clear()

        def rollback(self):
            world.pending.clear()

    ns.update(User=User, UserEmotionSummary=UserEmotionSummary, UserEmotionDna=UserEmotionDna,
              Video=Video, db=types.SimpleNamespace(session=Session()),
              IntegrityError=IntegrityError, BusinessError=BusinessError, APIError=_APIError,
              EmotionSummaryDto=EmotionSummaryDto, mongo_db=None)


class Original:
    """Pinned original functions bound to a World."""

    _cache = None

    def __init__(self, world: World):
        if Original._cache is None:
            Original._cache = _load_original()
        util, service_ast, repo_ast = Original._cache
        self.util = util
        ns: Dict = {"Dict": Dict, "List": List, "Optional": Optional,
                    "datetime": FrozenDatetime, "timedelta": timedelta,
                    "build_finalized_session_query": util.build_finalized_session_query}
        _install_models(world, ns)

        repo_ns = dict(ns)
        exec(compile(ast.fix_missing_locations(copy.deepcopy(repo_ast)), "orig_repo", "exec"), repo_ns)
        find_since = repo_ns["find_finalized_emotion_summaries_since"]

        class Repository:
            def __init__(self, _db):
                self.collection = Collection(world)

            find_finalized_emotion_summaries_since = find_since

        ns["YoutubeWatchingDataRepository"] = Repository
        exec(compile(ast.fix_missing_locations(copy.deepcopy(service_ast)), "orig_service", "exec"), ns)
        self.ns = ns

    def __getattr__(self, name):
        try:
            return self.__dict__["ns"][name]
        except KeyError:
            raise AttributeError(name)


# ------------------------------------------------------------------ port adapters

class WorldSummaryStore(port.SummaryStore):
    """Port storage over the same World; every read is predicated on the actor id."""

    def __init__(self, world: World, user_id: str):
        self.world, self.user_id = world, user_id
        self.ns = {}
        _install_models(world, self.ns)
        self.model = self.ns["UserEmotionSummary"]
        self.db = self.ns["db"]
        self.collection = Collection(world)

    def _row(self):
        return self.model.query.filter_by(user_id=self.user_id).first()

    def ensure_aggregate(self):
        aggregate = self._row()
        if aggregate is None:
            try:
                self.db.session.add(self.model(user_id=self.user_id))
                self.db.session.flush()
            except IntegrityError:
                self.db.session.rollback()
            aggregate = self._row()
        return aggregate.snapshot()

    def reload_aggregate(self):
        return self._row().snapshot()

    def first_fold_docs(self):
        return list(self.collection.find(
            port.build_finalized_session_query(self.user_id), port.FIRST_FOLD_PROJECTION))

    def docs_since(self, checkpoint_at, checkpoint_session_id):
        return list(self.collection.find(
            port.build_finalized_session_query(self.user_id, checkpoint_at, checkpoint_session_id),
            port.SINCE_PROJECTION).sort(list(port.SINCE_SORT)))

    def apply_delta(self, expected_version, delta, checkpoint_at, checkpoint_session_id):
        return self.model.apply_delta(self.user_id, expected_version, delta, checkpoint_at,
                                      checkpoint_session_id)

    def rollback(self):
        self.db.session.rollback()


def port_summary(world: World, user_id: str):
    return port.refresh_summary(WorldSummaryStore(world, user_id), lambda: Clock.now)


def port_dna(world: World, user_id: str):
    cached = world.dna.get(user_id)
    cached_view = None if cached is None else {
        "expires_at": cached.expires_at, "based_on_videos": cached.based_on_videos,
        "dna_data": cached.dna_data}
    collection = Collection(world)
    total = collection.count_documents({"user_id": user_id})

    def docs():
        return list(collection.find({"user_id": user_id}))

    def categories(ids):
        return {v: world.videos[v] for v in ids if v in world.videos}

    return port.plan_dna(cached_view, total, docs, categories, Clock.now)


# ------------------------------------------------------------------ helpers

def session(user, sid, finalized_at=None, **extra):
    doc = {"user_id": user, "video_view_log_id": sid, "video_id": extra.pop("video_id", "v1"),
           "created_at": extra.pop("created_at", datetime(2026, 10, 1, 12, 0, 0))}
    if finalized_at is not None:
        doc["finalized_at"] = finalized_at
    doc.update(extra)
    return doc


class Category:
    def __init__(self, value):
        self.value = value


def run_both(test, build_world, step):
    """Run `step` once on the original and once on the port, on identical worlds."""
    results = []
    for kind in ("original", "port"):
        Clock.now = T0
        world = build_world()
        try:
            out = step(kind, world)
        except Exception as exc:  # noqa: BLE001 - the exception type is part of the contract
            out = ("raised", type(exc).__name__)
        results.append((out, world))
    return results


class OracleCase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.orig_world = World()
        cls.orig = Original(cls.orig_world)


# ------------------------------------------------------------------ pure fold

FOLD_CASES = {
    "empty list": [],
    "empty doc": [{}],
    "empty docs": [{}, {}, {}],
    "precomputed seconds": [{"emotion_seconds": {"neutral": 3, "happy": 4, "angry": 1}}],
    "precomputed beats timeline": [{"emotion_seconds": {"happy": 2},
                                    "most_emotion_timeline": {"0": "sad"}}],
    "precomputed bad value skipped": [{"emotion_seconds": {"neutral": "x", "happy": None, "sad": "5"}}],
    "precomputed empty dict is still precomputed": [{"emotion_seconds": {},
                                                     "most_emotion_timeline": {"0": "sad"}}],
    "timeline last key of a second wins": [{"most_emotion_timeline": {"150": "happy", "199": "sad", "200": "sad"}}],
    "timeline second zero and boundary": [{"most_emotion_timeline": {"99": "happy", "100": "sad", "0": "neutral"}}],
    "timeline negative key": [{"most_emotion_timeline": {"-1": "happy", "-100": "sad", "-101": "sad"}}],
    "timeline float string": [{"most_emotion_timeline": {"250.9": "angry", "1e3": "surprise"}}],
    "timeline junk keys and unknown emotion": [{"most_emotion_timeline": {"a": "happy", "5": "joy", "": "sad", "7": "sad"}}],
    "timeline nan and inf": [{"most_emotion_timeline": {"nan": "happy", "5": "sad"}}],
    "percentages times duration": [{"emotion_percentages": {"happy": 0.5, "sad": 0.5},
                                    "duration": 100, "completion_rate": 0.4}],
    "completion over 1 is capped": [{"emotion_percentages": {"happy": 1.0},
                                     "duration": 10, "completion_rate": 7}],
    "completion zero falls to timeline_len": [{"emotion_percentages": {"happy": 1.0}, "completion_rate": 0,
                                               "duration": 10, "timeline_len": 9}],
    "frame_count fallback": [{"emotion_percentages": {"happy": 1.0}, "frame_count": 7}],
    "score timeline fallback": [{"emotion_percentages": {"happy": 1.0},
                                 "emotion_score_timeline": {"300": [1], "150": [2], "x": [3]}}],
    "no watch estimate": [{"emotion_percentages": {"happy": 1.0}}],
    "percentages without watch estimate via bad types": [{"emotion_percentages": {"happy": 1.0},
                                                          "duration": "a", "completion_rate": None}],
    "truncation not rounding": [{"emotion_percentages": {"happy": 0.29}, "duration": 100, "completion_rate": 1.0}],
    "float sum then one truncation": [{"emotion_percentages": {"happy": 0.1}, "duration": 7, "completion_rate": 1.0}] * 3,
    "half percentage rounding": [{"emotion_seconds": {"neutral": 1, "happy": 7}},
                                 {"emotion_seconds": {"sad": 0, "angry": 0}}],
    "bigint beyond float precision": [{"emotion_seconds": {"neutral": 2 ** 53 + 1}}],
    "mixed sessions": [
        {"most_emotion_timeline": {"0": "happy", "100": "happy"}},
        {"emotion_seconds": {"sad": 3}},
        {"emotion_percentages": {"neutral": 0.5}, "duration": 20, "completion_rate": 1},
    ],
}


class FoldOracleTest(OracleCase):
    def test_fold_cases_match_original(self):
        for name, docs in FOLD_CASES.items():
            with self.subTest(name):
                expected = self._call(self.orig._build_emotion_summary_from_docs, docs)
                actual = self._call(port.fold_summary, docs)
                self.assertEqual(expected, actual)

    @staticmethod
    def _call(fn, docs):
        try:
            return fn(copy.deepcopy(docs))
        except Exception as exc:  # noqa: BLE001
            return ("raised", type(exc).__name__)

    def test_pinned_values_do_not_depend_on_the_port(self):
        got = self.orig._build_emotion_summary_from_docs(FOLD_CASES["truncation not rounding"])
        self.assertEqual(got["emotion_seconds"]["happy"], 28)  # 0.29 * 100 = 28.999999999999996
        got = self.orig._build_emotion_summary_from_docs(FOLD_CASES["bigint beyond float precision"])
        self.assertEqual(got["emotion_seconds"]["neutral"], 2 ** 53)  # int > 2**53 loses its last unit
        got = self.orig._build_emotion_summary_from_docs(FOLD_CASES["timeline last key of a second wins"])
        self.assertEqual(got["emotion_seconds"], {"neutral": 0, "happy": 0, "surprise": 0, "sad": 2, "angry": 0})
        got = self.orig._build_emotion_summary_from_docs(FOLD_CASES["half percentage rounding"])
        self.assertEqual(got["emotion_percentages"]["happy"], 87.5)
        got = self.orig._build_emotion_summary_from_docs(FOLD_CASES["empty list"])
        self.assertEqual(got["emotion_percentages"], {e: 0.0 for e in port.EMOTIONS})

    def test_fold_fuzz_matches_original(self):
        rng = random.Random(728)
        emotions = list(port.EMOTIONS) + ["joy", None]
        keys = ["0", "50", "99", "100", "101", "199", "250.5", "-1", "x", "", "1e2", "nan", "3000"]
        for i in range(3000):
            docs = []
            for _ in range(rng.randint(0, 4)):
                doc = {}
                if rng.random() < 0.3:
                    doc["emotion_seconds"] = {e: rng.choice([0, 1, 5, 2.5, "3", None, 10 ** 6]) for e in
                                              rng.sample(port.EMOTIONS, rng.randint(0, 5))}
                if rng.random() < 0.5:
                    doc["most_emotion_timeline"] = {rng.choice(keys): rng.choice(emotions)
                                                    for _ in range(rng.randint(0, 5))}
                if rng.random() < 0.7:
                    doc["emotion_percentages"] = {e: rng.choice([0, 0.1, 0.29, 0.3333, 0.5, 1, 1.5])
                                                  for e in rng.sample(port.EMOTIONS, rng.randint(0, 5))}
                for field, choices in (("duration", [None, 0, 10, 33.3, "12", "a", 600]),
                                       ("completion_rate", [None, 0, 0.3, 0.97, 1, 2, "0.5"]),
                                       ("timeline_len", [None, 0, 5, "7", "a"]),
                                       ("frame_count", [None, 0, 9, "x"])):
                    if rng.random() < 0.5:
                        doc[field] = rng.choice(choices)
                if rng.random() < 0.2:
                    doc["emotion_score_timeline"] = {rng.choice(keys): [1] for _ in range(rng.randint(0, 3))}
                docs.append(doc)
            expected = self._call(self.orig._build_emotion_summary_from_docs, docs)
            actual = self._call(port.fold_summary, docs)
            self.assertEqual(expected, actual, f"case {i}: {docs}")

    def test_timeline_helper_and_query_builder_match_original_util(self):
        util = self.orig.util
        for timeline in ({}, None, {"0": "happy", "99": "sad", "100": "sad", "x": "sad", "4": "joy"}):
            self.assertEqual(util.build_emotion_seconds_from_timeline(timeline),
                             port.fold_summary([{"most_emotion_timeline": timeline}])["emotion_seconds"]
                             if timeline else util.empty_emotion_seconds())
        for args in (("u1",), ("u1", T0, "s5"), ("u1", T0, None), ("u1", T0, "")):
            self.assertEqual(util.build_finalized_session_query(*args),
                             port.build_finalized_session_query(*args))


# ------------------------------------------------------------------ summary orchestration

def base_world():
    world = World()
    world.sessions = [
        session("u1", "a", None, most_emotion_timeline={"0": "happy", "100": "happy", "200": "sad"},
                emotion_percentages={"happy": 0.7, "sad": 0.3}, duration=3, completion_rate=1.0),
        session("u1", "b", None, emotion_percentages={"neutral": 1.0}, duration=10, completion_rate=0.5),
        session("u2", "z", None, most_emotion_timeline={"0": "angry", "100": "angry"}),
    ]
    return world


def summary_step(kind, world, user="u1"):
    if kind == "original":
        return Original(world).get_emotion_summary(user)
    return port_summary(world, user)


class SummaryOracleTest(OracleCase):
    def both(self, build_world, step=None, user="u1"):
        step = step or (lambda kind, world: summary_step(kind, world, user))
        (o_out, o_world), (p_out, p_world) = run_both(self, build_world, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(o_world.snapshot_summary(user), p_world.snapshot_summary(user))
        return o_out, o_world

    def test_first_fold_counts_every_session_and_stamps_now(self):
        out, world = self.both(base_world)
        self.assertEqual(out["emotion_seconds"], {"neutral": 5, "happy": 2, "surprise": 0, "sad": 1, "angry": 0})
        row = world.snapshot_summary("u1")
        self.assertEqual((row["last_finalized_at"], row["last_session_id"], row["lock_version"]),
                         (T0, "", 1))

    def test_first_fold_projection_hides_precomputed_seconds_and_timeline_len(self):
        def build():
            world = base_world()
            world.sessions[0]["emotion_seconds"] = {"angry": 99}
            world.sessions[1]["timeline_len"] = 40
            return world

        out, _ = self.both(build)
        self.assertEqual(out["emotion_seconds"]["angry"], 0)
        self.assertEqual(out["emotion_seconds"]["neutral"], 5)  # duration*completion, not timeline_len/2

    def test_empty_user_still_creates_the_cache_row(self):
        def build():
            world = World()
            return world

        out, world = self.both(build)
        self.assertEqual(out["emotion_seconds"], {e: 0 for e in port.EMOTIONS})
        self.assertEqual(world.snapshot_summary("u1")["lock_version"], 1)

    def test_repeat_without_new_sessions_changes_nothing(self):
        def step(kind, world):
            first = summary_step(kind, world)
            Clock.now = T0 + timedelta(hours=1)
            before = world.snapshot_summary("u1")
            second = summary_step(kind, world)
            assert world.snapshot_summary("u1") == before
            return first, second

        (o_out, o_world), (p_out, p_world) = run_both(self, base_world, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(o_out[0], o_out[1])
        self.assertEqual(o_world.snapshot_summary("u1"), p_world.snapshot_summary("u1"))
        self.assertEqual(o_world.snapshot_summary("u1")["lock_version"], 1)

    def test_incremental_uses_precomputed_seconds_and_moves_the_cursor(self):
        later = T0 + timedelta(minutes=5)

        def step(kind, world):
            summary_step(kind, world)
            world.sessions.append(session("u1", "c", later, emotion_seconds={"happy": 4, "sad": 2},
                                          most_emotion_timeline={"0": "angry"}))
            world.sessions.append(session("u1", "d", later + timedelta(seconds=1),
                                          emotion_percentages={"surprise": 1.0}, duration=8, completion_rate=0.5))
            Clock.now = later + timedelta(minutes=1)
            return summary_step(kind, world)

        (o_out, o_world), (p_out, p_world) = run_both(self, base_world, step)
        self.assertEqual(o_out, p_out)
        row = o_world.snapshot_summary("u1")
        self.assertEqual(row, p_world.snapshot_summary("u1"))
        self.assertEqual(o_out["emotion_seconds"], {"neutral": 5, "happy": 6, "surprise": 4, "sad": 3, "angry": 0})
        self.assertEqual((row["last_finalized_at"], row["last_session_id"], row["lock_version"]),
                         (later + timedelta(seconds=1), "d", 2))

    def test_cursor_tie_on_finalized_at_is_decided_by_session_id(self):
        def step(kind, world):
            summary_step(kind, world)
            tie = T0  # equals the first-fold checkpoint
            world.sessions.append(session("u1", "", tie, emotion_seconds={"happy": 1}))   # id == '' : not > ''
            world.sessions.append(session("u1", "m", tie, emotion_seconds={"happy": 10}))  # id > '' : included
            world.sessions.append(session("u1", "n", tie - timedelta(microseconds=1),
                                          emotion_seconds={"happy": 100}))                # older : excluded
            Clock.now = T0 + timedelta(minutes=1)
            out = summary_step(kind, world)
            # second pass with the cursor now at (tie, 'm'): same-instant 'm' must not repeat, 'n2' must count
            world.sessions.append(session("u1", "n2", tie, emotion_seconds={"sad": 1000}))
            return out, summary_step(kind, world)

        (o_out, o_world), (p_out, p_world) = run_both(self, base_world, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(o_out[0]["emotion_seconds"]["happy"], 2 + 10)
        self.assertEqual(o_out[1]["emotion_seconds"]["sad"], 1 + 1000)
        self.assertEqual(o_world.snapshot_summary("u1"), p_world.snapshot_summary("u1"))

    def test_partial_finalization_is_double_counted_by_the_original_and_pinned(self):
        # The first fold has no finalized_at filter, so a session still open then is counted.
        # When it finalizes after the checkpoint the incremental read counts it again.
        # This is the ORIGINAL behaviour; the port keeps it and does not invent a correction.
        def step(kind, world):
            first = summary_step(kind, world)
            open_session = world.sessions[1]
            open_session["finalized_at"] = T0 + timedelta(minutes=2)
            open_session["emotion_seconds"] = {"neutral": 5}
            Clock.now = T0 + timedelta(minutes=3)
            return first, summary_step(kind, world)

        (o_out, o_world), (p_out, p_world) = run_both(self, base_world, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(o_out[0]["emotion_seconds"]["neutral"], 5)
        self.assertEqual(o_out[1]["emotion_seconds"]["neutral"], 10)
        self.assertEqual(o_world.snapshot_summary("u1"), p_world.snapshot_summary("u1"))

    def test_unfinalized_sessions_after_the_first_fold_are_ignored(self):
        def step(kind, world):
            summary_step(kind, world)
            world.sessions.append(session("u1", "late", None, emotion_seconds={"happy": 50}))
            Clock.now = T0 + timedelta(minutes=3)
            return summary_step(kind, world)

        (o_out, _), (p_out, _) = run_both(self, base_world, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(o_out["emotion_seconds"]["happy"], 2)

    def test_one_cas_conflict_is_retried_with_the_new_row(self):
        def bump(world):
            row = world.summary["u1"]
            row.happy_seconds += 100
            row.lock_version += 1

        def step(kind, world):
            summary_step(kind, world)  # first fold, version 1
            world.sessions.append(session("u1", "c", T0 + timedelta(minutes=1), emotion_seconds={"sad": 2}))
            world.before_cas.append(bump)
            Clock.now = T0 + timedelta(minutes=2)
            return summary_step(kind, world)

        (o_out, o_world), (p_out, p_world) = run_both(self, base_world, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(o_out["emotion_seconds"]["happy"], 102)
        self.assertEqual(o_out["emotion_seconds"]["sad"], 3)
        self.assertEqual(o_world.snapshot_summary("u1"), p_world.snapshot_summary("u1"))
        self.assertEqual(o_world.cas_attempts, p_world.cas_attempts)
        self.assertEqual(o_world.cas_attempts, 3)  # first fold + failed + retry

    def test_three_cas_conflicts_return_the_stored_totals_without_writing(self):
        def bump(world):
            row = world.summary["u1"]
            row.angry_seconds += 1
            row.lock_version += 1

        def step(kind, world):
            world.before_cas[:] = [bump, bump, bump]
            return summary_step(kind, world)

        (o_out, o_world), (p_out, p_world) = run_both(self, base_world, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(o_out["emotion_seconds"], {"neutral": 0, "happy": 0, "surprise": 0, "sad": 0, "angry": 3})
        self.assertEqual(o_world.snapshot_summary("u1"), p_world.snapshot_summary("u1"))
        self.assertEqual(o_world.cas_attempts, 3)
        self.assertEqual(p_world.cas_attempts, 3)
        self.assertIsNone(o_world.snapshot_summary("u1")["last_finalized_at"])

    def test_concurrent_first_create_reuses_the_row_that_won(self):
        def winner(world):
            world.summary["u1"] = SummaryRow(
                "u1", neutral_seconds=7, happy_seconds=0, surprise_seconds=0, sad_seconds=0,
                angry_seconds=0, last_finalized_at=None, last_session_id=None, lock_version=0,
                created_at=T0, updated_at=T0)

        def step(kind, world):
            world.before_flush.append(winner)
            return summary_step(kind, world)

        (o_out, o_world), (p_out, p_world) = run_both(self, base_world, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(o_out["emotion_seconds"]["neutral"], 7 + 5)
        self.assertEqual(o_world.snapshot_summary("u1"), p_world.snapshot_summary("u1"))

    def test_foreign_actor_sessions_are_never_read_or_folded(self):
        (o_out, o_world), (p_out, p_world) = run_both(
            self, base_world, lambda kind, world: (summary_step(kind, world, "u1"), world))
        self.assertEqual(o_out[0], p_out[0])
        self.assertEqual(o_out[0]["emotion_seconds"]["angry"], 0)
        for world in (o_world, p_world):
            self.assertTrue(world.find_log)
            for query, _projection in world.find_log:
                self.assertEqual(query["user_id"], "u1")
            self.assertNotIn("u2", world.summary)
        # the other actor sees only its own sessions
        (o2, _), (p2, _) = run_both(self, base_world, lambda kind, world: summary_step(kind, world, "u2"))
        self.assertEqual(o2, p2)
        self.assertEqual(o2["emotion_seconds"]["angry"], 2)
        self.assertEqual(o2["emotion_seconds"]["happy"], 0)

    def test_unknown_and_deleted_user_raise_in_the_original_only(self):
        # The port has no user check: the AIP row policy `active(actor)` decides that.
        world = base_world()
        world.users["u1"]["is_deleted"] = 1
        orig = Original(world)
        with self.assertRaises(BusinessError):
            orig.get_emotion_summary("u1")
        with self.assertRaises(BusinessError):
            orig.get_emotion_summary("nobody")
        with self.assertRaises(BusinessError):
            orig.get_emotion_dna("nobody")
        self.assertEqual(world.summary, {})

    def test_summary_fuzz_matches_original_over_call_sequences(self):
        rng = random.Random(7280)
        for case in range(150):
            def build(case=case):
                world = World()
                local = random.Random(case)
                for i in range(local.randint(0, 5)):
                    world.sessions.append(self._random_session(local, f"s{i:02d}", "u1", with_final=local.random() < 0.6))
                return world

            def step(kind, world, case=case):
                local = random.Random(case * 31 + 1)
                outs = []
                for round_no in range(local.randint(1, 4)):
                    Clock.now = T0 + timedelta(seconds=round_no * 90)
                    for j in range(local.randint(0, 3)):
                        world.sessions.append(self._random_session(
                            local, f"r{round_no}{j}", "u1", with_final=True,
                            final_at=T0 + timedelta(seconds=round_no * 90 - local.randint(0, 120))))
                    if local.random() < 0.3:
                        world.before_cas.append(lambda w, k=local.randint(1, 5): self._bump(w, k))
                    outs.append(summary_step(kind, world))
                return outs

            (o_out, o_world), (p_out, p_world) = run_both(self, build, step)
            self.assertEqual(o_out, p_out, f"case {case}")
            self.assertEqual(o_world.snapshot_summary("u1"), p_world.snapshot_summary("u1"), f"case {case}")
        del rng

    @staticmethod
    def _bump(world, amount):
        row = world.summary.get("u1")
        if row is not None:
            row.sad_seconds += amount
            row.lock_version += 1

    @staticmethod
    def _random_session(rng, sid, user, with_final, final_at=None):
        doc = {"user_id": user, "video_view_log_id": sid, "video_id": "v1",
               "created_at": datetime(2026, 10, 1, 12, 0, 0)}
        if with_final:
            doc["finalized_at"] = final_at or T0 + timedelta(seconds=rng.choice([-30, -1, 0, 0, 5, 60]))
        kind = rng.choice(["seconds", "timeline", "percent"])
        if kind == "seconds":
            doc["emotion_seconds"] = {e: rng.randint(0, 9) for e in rng.sample(port.EMOTIONS, 3)}
        elif kind == "timeline":
            doc["most_emotion_timeline"] = {str(rng.randint(0, 900)): rng.choice(port.EMOTIONS) for _ in range(4)}
        doc["emotion_percentages"] = {"happy": 0.6, "neutral": 0.4}
        doc["duration"] = rng.choice([5, 12, 40])
        doc["completion_rate"] = rng.choice([0.25, 0.8, 1.0])
        return doc


# ------------------------------------------------------------------ DNA

def dna_world(n=3, sad=False):
    world = World()
    world.videos = {"v1": Category("drama"), "v2": Category("music"), "v3": "game"}
    for i in range(n):
        world.sessions.append(session(
            "u1", f"s{i}", None, video_id=["v1", "v2", "v3"][i % 3],
            created_at=datetime(2026, 10, 1, [12, 23, 3][i % 3], 0, 0),
            emotion_percentages={"happy": 0.4, "sad": 0.1, "neutral": 0.2, "surprise": 0.1, "angry": 0.2},
            completion_rate=0.5))
    world.sessions.append(session("u2", "z", None, video_id="v1", completion_rate=1.0,
                                  emotion_percentages={"surprise": 1.0}))
    return world


def dna_step(kind, world, user="u1"):
    if kind == "original":
        return Original(world).get_emotion_dna(user)
    return port_dna(world, user)[0]


def apply_port_write(world, user, write):
    if write is None:
        return
    row = types.SimpleNamespace(user_id=user, dna_type=write["dna_type"], dna_data=write["dna_data"],
                                based_on_videos=write["based_on_videos"], generated_at=write["generated_at"],
                                expires_at=write["expires_at"])
    world.dna[user] = row


def dna_row(world, user="u1"):
    row = world.dna.get(user)
    return None if row is None else {k: getattr(row, k) for k in (
        "dna_type", "dna_data", "based_on_videos", "generated_at", "expires_at")}


def dna_run(kind, world, user="u1"):
    if kind == "original":
        return Original(world).get_emotion_dna(user)
    out, write = port_dna(world, user)
    apply_port_write(world, user, write)
    return out


class DnaOracleTest(OracleCase):
    def both(self, build, step=None):
        step = step or (lambda kind, world: dna_run(kind, world))
        (o_out, o_world), (p_out, p_world) = run_both(self, build, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(dna_row(o_world), dna_row(p_world))
        return o_out, o_world

    def test_no_sessions_returns_the_default_and_caches_nothing(self):
        out, world = self.both(World)
        self.assertEqual(out["based_on_videos"], 0)
        self.assertEqual(out["generated_at"], T0.isoformat())
        self.assertIsNone(dna_row(world))

    def test_first_computation_inserts_a_seven_day_cache_row(self):
        out, world = self.both(dna_world)
        row = dna_row(world)
        self.assertEqual(row["expires_at"], T0 + timedelta(days=7))
        self.assertEqual((row["based_on_videos"], row["generated_at"]), (3, T0))
        self.assertEqual(row["dna_data"], out)
        self.assertEqual(out["based_on_videos"], 3)
        self.assertEqual(row["dna_type"], out["dna_type"])

    def test_missing_video_is_not_counted_and_enum_category_uses_value(self):
        def build():
            world = dna_world()
            for doc in world.sessions:
                doc["created_at"] = datetime(2026, 10, 1, 12)  # keep NIGHT_OWL out of the way
            del world.videos["v2"]
            return world

        out, _ = self.both(build)
        self.assertEqual(out["dna_type"], "JOYFUL_EXPLORER")
        self.assertEqual({t["trait"]: t["score"] for t in out["traits"]}["장르 탐험가"],
                         int(2 / 16 * 100))

    def test_cache_hit_returns_the_stored_data_without_reading_sessions(self):
        def step(kind, world):
            dna_run(kind, world)
            world.find_log.clear()
            Clock.now = T0 + timedelta(days=7) - timedelta(microseconds=1)
            world.sessions.extend(dna_world(7).sessions[3:7])  # 4 new sessions of u1
            world.sessions[:] = [s for s in world.sessions if s["user_id"] == "u1"][:3 + 4]
            out = dna_run(kind, world)
            return out, list(world.find_log)

        (o_out, o_world), (p_out, p_world) = run_both(self, dna_world, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(o_out[1], [])  # no session read on a hit
        self.assertEqual(dna_row(o_world), dna_row(p_world))
        self.assertEqual(dna_row(o_world)["based_on_videos"], 3)

    def test_cache_miss_boundaries(self):
        cases = {
            "expires_exactly_now": (T0, 0, True),
            "expires_one_microsecond_later": (T0 + timedelta(microseconds=1), 0, False),
            "four_new_sessions_hit": (T0 + timedelta(days=1), 4, False),
            "five_new_sessions_miss": (T0 + timedelta(days=1), 5, True),
            "fewer_sessions_than_cached_hit": (T0 + timedelta(days=1), -2, False),
        }
        for name, (expires_at, extra, expect_recompute) in cases.items():
            with self.subTest(name):
                def build(extra=extra, expires_at=expires_at):
                    world = dna_world(3)
                    based_on = 3 if extra >= 0 else 5
                    total = based_on + extra
                    own = [s for s in world.sessions if s["user_id"] == "u1"]
                    template = own[0]
                    world.sessions = [s for s in world.sessions if s["user_id"] != "u1"]
                    for i in range(total):
                        world.sessions.append(dict(template, video_view_log_id=f"t{i}"))
                    world.dna["u1"] = types.SimpleNamespace(
                        user_id="u1", dna_type="NIGHT_OWL", dna_data={"cached": True}, based_on_videos=based_on,
                        generated_at=T0 - timedelta(days=1), expires_at=expires_at)
                    return world

                out, world = self.both(build)
                self.assertEqual(out != {"cached": True}, expect_recompute)

    def test_recompute_updates_the_existing_row_in_place(self):
        def build():
            world = dna_world(3)
            world.dna["u1"] = types.SimpleNamespace(
                user_id="u1", dna_type="NIGHT_OWL", dna_data={"old": True}, based_on_videos=0,
                generated_at=T0 - timedelta(days=9), expires_at=T0 - timedelta(days=2))
            return world

        out, world = self.both(build)
        row = dna_row(world)
        self.assertEqual((row["generated_at"], row["expires_at"]), (T0, T0 + timedelta(days=7)))
        self.assertEqual(row["dna_data"], out)

    def test_expired_cache_with_no_sessions_left_returns_default_and_keeps_the_stale_row(self):
        def build():
            world = World()
            world.dna["u1"] = types.SimpleNamespace(
                user_id="u1", dna_type="NIGHT_OWL", dna_data={"old": True}, based_on_videos=3,
                generated_at=T0 - timedelta(days=9), expires_at=T0 - timedelta(days=2))
            return world

        out, world = self.both(build)
        self.assertEqual(out["based_on_videos"], 0)
        self.assertEqual(dna_row(world)["dna_data"], {"old": True})

    def test_missing_session_fields_raise_the_same_error(self):
        for missing in ("created_at", "video_id"):
            with self.subTest(missing):
                def build(missing=missing):
                    world = dna_world(2)
                    del world.sessions[0][missing]
                    return world

                (o_out, _), (p_out, _) = run_both(self, build, lambda kind, world: dna_run(kind, world))
                self.assertEqual(o_out, ("raised", "KeyError"))
                self.assertEqual(o_out, p_out)

    def test_foreign_actor_sessions_do_not_change_the_dna_or_the_count(self):
        def build():
            world = dna_world(3)
            world.sessions.extend(dict(s, user_id="u2", video_view_log_id=f"x{i}") for i, s in enumerate(world.sessions[:3]))
            return world

        def step(kind, world):
            out = dna_run(kind, world)
            return out, [q for q, _ in world.find_log], list(world.count_log)

        (o_out, _), (p_out, _) = run_both(self, build, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(o_out[0]["based_on_videos"], 3)
        for query in o_out[1] + o_out[2]:
            self.assertEqual(query, {"user_id": "u1"})

    def test_seven_day_cache_expires_exactly_at_the_boundary(self):
        def step(kind, world):
            dna_run(kind, world)
            Clock.now = T0 + timedelta(days=7) - timedelta(microseconds=1)
            hit = dna_run(kind, world)
            Clock.now = T0 + timedelta(days=7)
            miss = dna_run(kind, world)
            return hit, miss

        (o_out, o_world), (p_out, p_world) = run_both(self, dna_world, step)
        self.assertEqual(o_out, p_out)
        self.assertEqual(o_out[0]["generated_at"], T0.isoformat())
        self.assertEqual(o_out[1]["generated_at"], (T0 + timedelta(days=7)).isoformat())
        self.assertEqual(dna_row(o_world), dna_row(p_world))

    def test_dna_rule_table_matches_original(self):
        orig = self.orig
        grid = [0.0, 0.1, 0.29, 0.3, 0.35, 0.39, 0.4, 0.5, 0.69, 0.7, 0.84, 0.85, 1.0]
        for completion in grid:
            for night in (0.0, 0.39, 0.4):
                for happy in grid:
                    for sad in (0.0, 0.15, 0.5):
                        for surprise in (0.0, 0.29, 0.3):
                            for neutral in (0.0, 0.39, 0.4):
                                for genres, total in ((1, 2), (2, 3), (3, 9)):
                                    args = (completion, night, happy, sad, surprise, neutral, genres, total)
                                    self.assertEqual(orig._determine_dna_type(*args),
                                                     port.determine_dna_type(*args), args)

    def test_every_dna_type_is_reachable_and_matches(self):
        probes = {
            "BINGE_MASTER": (0.85, 0, 0, 0, 0, 0, 5, 9),
            "NIGHT_OWL": (0.5, 0.4, 0, 0, 0, 0, 5, 9),
            "THRILL_SEEKER": (0.5, 0, 0, 0, 0.3, 0, 5, 9),
            "CALM_OBSERVER": (0.5, 0, 0, 0, 0, 0.4, 5, 9),
            "EMOTIONAL_DIVER": (0.7, 0, 0.3, 0.2, 0, 0, 5, 9),
            "JOYFUL_EXPLORER": (0.5, 0, 0.35, 0, 0, 0, 5, 9),
            "COMFORT_LOVER": (0.5, 0, 0.3, 0, 0, 0, 2, 3),
            "MOOD_SURFER": (0.5, 0, 0.2, 0.2, 0.2, 0.2, 5, 9),
        }
        for expected, args in probes.items():
            with self.subTest(expected):
                self.assertEqual(self.orig._determine_dna_type(*args), expected)
                self.assertEqual(port.determine_dna_type(*args), expected)
        # fall-through: happy 0.30..0.35 with many genres goes to the final JOYFUL_EXPLORER
        args = (0.5, 0, 0.32, 0.1, 0.1, 0.1, 5, 9)
        self.assertEqual(self.orig._determine_dna_type(*args), "JOYFUL_EXPLORER")
        self.assertEqual(port.determine_dna_type(*args), "JOYFUL_EXPLORER")

    def test_traits_fun_facts_and_default_match_original(self):
        rng = random.Random(7281)
        for _ in range(2000):
            happy, sad, surprise, neutral, completion, night = (rng.random() for _ in range(6))
            genres = rng.randint(0, 20)
            for dna_type in list(port.DNA_TYPES) + ["UNKNOWN"]:
                self.assertEqual(
                    self.orig._compute_traits(dna_type, happy, sad, surprise, neutral, completion, genres, night),
                    port.compute_traits(dna_type, happy, sad, surprise, neutral, completion, genres, night))
            n = rng.randint(1, 50)
            self.assertEqual(self.orig._compute_fun_facts(happy, completion, n),
                             port.compute_fun_facts(happy, completion, n))
        self.assertEqual(self.orig._DNA_TYPES, {k: {"title": v[0], "description": v[1]}
                                                for k, v in port.DNA_TYPES.items()})
        self.assertEqual(self.orig._TRAIT_POOL, port.TRAIT_POOL)
        self.assertEqual(self.orig._DNA_TRAIT_KEYS, port.DNA_TRAIT_KEYS)
        Clock.now = T0
        self.assertEqual(self.orig._build_default_dna("u1"), port.default_dna(T0))

    def test_dna_night_hours_and_float_truncation(self):
        for hour, is_night in ((21, False), (22, True), (23, True), (0, True), (3, True), (4, False)):
            with self.subTest(hour):
                def build(hour=hour):
                    world = World()
                    world.videos = {"v1": "drama"}
                    world.sessions = [session("u1", "s", None, created_at=datetime(2026, 10, 1, hour),
                                              emotion_percentages={"happy": 0.29}, completion_rate=0.29)]
                    return world

                out, _ = self.both(build)
                scores = {t["trait"]: t["score"] for t in out["traits"]}
                self.assertEqual(out["dna_type"], "NIGHT_OWL" if is_night else out["dna_type"])
                self.assertEqual(out["emotion_radar"]["happy"], 28)  # int(0.29 * 100)
                self.assertEqual(out["based_on_videos"], 1)
                del scores

    def test_dna_fuzz_matches_original(self):
        for case in range(120):
            def build(case=case):
                local = random.Random(case)
                world = World()
                world.videos = {f"v{i}": local.choice([Category(f"c{i}"), f"c{i % 4}", None]) for i in range(8)}
                world.videos = {k: v for k, v in world.videos.items() if v is not None}
                for i in range(local.randint(0, 9)):
                    percentages = {e: local.choice([0, 0.05, 0.2, 0.33, 0.6]) for e in port.EMOTIONS if local.random() < 0.8}
                    world.sessions.append(session(
                        "u1", f"s{i}", None, video_id=f"v{local.randint(0, 9)}",
                        created_at=datetime(2026, 10, 1, local.randint(0, 23)),
                        emotion_percentages=percentages, completion_rate=local.choice([0, 0.3, 0.7, 0.9, 1.0])))
                return world

            self.both(build)


# ------------------------------------------------------------------ platform gap canaries

class PlatformGapCanaryTest(unittest.TestCase):
    """These pass while the platform lacks what a one-action Mongo-read + cache-write needs.

    A failure means the platform changed: re-evaluate the report, do not just delete the test.
    """

    def read(self, rel):
        return (REPO / rel).read_text(encoding="utf-8")

    def test_ctx_data_apply_carries_ids_and_key_but_no_values(self):
        worker = self.read("spikes/spike-v4-worker/workers/worker.mjs")
        self.assertIn("apply: (res, name, ids, key)", worker)
        write = self.read("spikes/spike-v4-worker/src/write.rs")
        self.assertIn('json!({"apply":message["name"],"target":target})', write)
        self.assertNotIn('"values"', write.split("fn ctx_request")[1].split("\n}\n")[0])

    def test_write_action_ctx_accepts_apply_only(self):
        write = self.read("spikes/spike-v4-worker/src/write.rs")
        self.assertIn('message["op"] != "apply"', write)
        self.assertIn("쓰기 action ctx는 apply만", write)

    def test_read_extensions_reject_apply_and_write_extensions_have_no_mongo_grant_in_docs(self):
        docs = self.read("product/MANAGED-EXECUTION.md")
        self.assertIn("Read extensions continue to reject `ctx.data.apply`", docs)
        self.assertIn("Extension reads still reject `ctx.data.apply` with `WIRE.ACCESS_DENIED`", docs)
        self.assertIn("Mongo and Redis read adapters", docs.replace("MongoDB and Redis", "Mongo and Redis"))

    def test_numeric_transitions_only_step_by_a_literal(self):
        syntax = self.read("spec/definition-syntax.md")
        self.assertIn("숫자는 f = f + 1처럼만 바꾼다", syntax)


if __name__ == "__main__":
    unittest.main()
