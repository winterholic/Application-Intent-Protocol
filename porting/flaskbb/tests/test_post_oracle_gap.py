"""Run the pinned FlaskBB ORM against an isolated PostgreSQL schema.

The tests deliberately import the preserved upstream application and models.
They are not an AST/SQLite model of SQLAlchemy behavior.
"""
import os
import re
import sys
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from uuid import uuid4
from unittest.mock import patch

UPSTREAM = Path(__file__).resolve().parents[1] / "upstream"
sys.path.insert(0, str(UPSTREAM))

from flaskbb import create_app  # noqa: E402
from flaskbb.configs.testing import TestingConfig  # noqa: E402
from flaskbb.extensions import db  # noqa: E402
from flaskbb.forum.models import Category, Forum, Post, Topic  # noqa: E402
from flaskbb.user.models import Group, User  # noqa: E402
from sqlalchemy import event  # noqa: E402
from sqlalchemy.engine import Engine  # noqa: E402
import sqlalchemy  # noqa: E402


class OracleConfig(TestingConfig):
    SQLALCHEMY_DATABASE_URI = os.environ.get("FLASKBB_ORACLE_DATABASE_URI", "")


def postgres_uri_is_scoped(uri):
    return (uri.startswith("postgresql://runner@127.0.0.1:") and
            "/aip_test_" in uri and "search_path" in uri)


class PostOracleGap(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        uri = OracleConfig.SQLALCHEMY_DATABASE_URI
        if not postgres_uri_is_scoped(uri):
            raise RuntimeError("refusing ORM test without isolated aip_test_* PostgreSQL schema URI")
        if create_app.__module__ != "flaskbb.app" or sqlalchemy.__version__ != "1.3.10":
            raise RuntimeError("expected preserved FlaskBB app with pinned SQLAlchemy 1.3.10")
        # The pinned FlaskBB requirements include Celery 4.3.0, whose
        # ``pytz (>dev)`` metadata is rejected by pkg_resources when
        # FlaskBB discovers installed plugin entry points. Plugins are outside
        # this Post/Topic ORM oracle, so keep the original create_app path while
        # isolating this test from external plugin discovery.
        with patch("flaskbb.plugins.manager.iter_entry_points", return_value=[]):
            cls.app = create_app(
                OracleConfig,
                instance_path="/tmp/flaskbb-orm-oracle-" + uuid4().hex,
            )
        cls.ctx = cls.app.app_context()
        cls.ctx.push()
        cls.statements = []

        @event.listens_for(Engine, "before_cursor_execute")
        def capture_sql(conn, cursor, statement, parameters, context, executemany):
            if "FROM posts" in statement or "FROM topics" in statement:
                cls.statements.append(statement)

        cls.capture_sql = capture_sql
        db.create_all()

    @classmethod
    def tearDownClass(cls):
        db.session.remove()
        db.drop_all()
        event.remove(Engine, "before_cursor_execute", cls.capture_sql)
        cls.ctx.pop()

    def setUp(self):
        db.session.remove()
        db.drop_all()
        db.create_all()
        self.request_context = self.app.test_request_context()
        self.request_context.push()
        self.statements = self.__class__.statements
        self.statements[:] = []

    def tearDown(self):
        db.session.remove()
        self.request_context.pop()

    def seed(self, posts):
        """Create real upstream rows; posts are (id, day, hidden, topic_hidden)."""
        group = Group(name="ordinary", description="oracle fixture")
        db.session.add(group)
        db.session.flush()
        user = User(username="oracle-user", email="oracle@example.invalid",
                    _password="not-used", primary_group=group)
        category = Category(title="oracle-category")
        db.session.add_all([user, category])
        db.session.flush()
        forum = Forum(category_id=category.id, title="oracle-forum")
        db.session.add(forum)
        db.session.flush()
        topic = Topic(title="oracle-topic", user=user)
        topic.forum = forum
        db.session.add(topic)
        db.session.flush()
        rows = []
        for post_id, day, hidden, topic_hidden in posts:
            if topic_hidden:
                topic.hidden = True
            row = Post(content="body", user=user, topic=topic)
            row.id = post_id
            row.date_created = (
                datetime(2020, 1, 1, tzinfo=timezone.utc) +
                timedelta(days=day)
            )
            row.hidden = hidden
            row.topic = topic
            row.user = user
            rows.append(row)
            db.session.add(row)
            db.session.flush()
            if hidden is None:
                # SQLAlchemy applies the mapped False default when None is
                # present at INSERT time; persist NULL with a real UPDATE.
                row.hidden = None
                db.session.flush()
        topic.first_post = rows[0]
        topic.last_post = rows[-1]
        topic.last_updated = rows[-1].date_created
        forum.last_post = rows[-1]
        forum.last_post_user = user
        forum.last_post_title = topic.title
        forum.last_post_username = rows[-1].username
        forum.last_post_created = rows[-1].date_created
        db.session.commit()
        return forum, topic, rows

    def test_relationship_result_is_observed_without_inventing_order(self):
        _, topic, posts = self.seed([(10, 0, False, False),
                                     (30, 2, False, False),
                                     (20, 1, False, False)])
        self.statements[:] = []
        result = topic.posts.all()
        second_last = topic.second_last_post
        sql = "\n".join(self.statements)
        self.assertEqual(len(result), 3)
        self.assertEqual(second_last, result[-2].id)
        self.assertNotRegex(sql.upper(), r"ORDER\s+BY")
        print("ORM_RELATIONSHIP ids={} second_last={} has_order_by={}".format(
            [post.id for post in result], second_last,
            bool(re.search(r"ORDER\s+BY", sql, re.I))))

    def test_delete_autoflush_and_id_ordered_forum_candidate(self):
        forum, topic, posts = self.seed([(10, 0, False, False),
                                         (20, 9, False, False),
                                         (30, 1, False, False)])
        target = posts[-1]
        # Post.delete marks the row, then its original candidate query triggers
        # SQLAlchemy autoflush before selecting the replacement.
        target.delete()
        self.assertIsNone(db.session.query(Post.id).filter_by(id=30).first())
        self.assertEqual(forum.last_post_id, 20)
        self.assertEqual(forum.last_post_created, posts[1].date_created)
        self.assertEqual(topic.last_post_id, topic.second_last_post)
        db.session.commit()
        self.assertIsNone(Post.query.with_hidden().filter_by(id=30).first())
        print("DELETE_AUTOFLUSH deleted=30 forum_last={} topic_last={}".format(
            forum.last_post_id, topic.last_post_id))

    def test_delete_with_one_remaining_candidate(self):
        forum, _, posts = self.seed([(15, 0, False, False),
                                     (25, 1, False, False)])
        posts[-1].delete()
        self.assertEqual(forum.last_post_id, 15)
        self.assertIsNone(Post.query.with_hidden().filter_by(id=25).first())
        print("SINGLE_CANDIDATE deleted=25 forum_last=15")

    def test_nonlast_delete_keeps_existing_pointers(self):
        forum, topic, posts = self.seed([(11, 0, False, False),
                                         (12, 1, False, False),
                                         (13, 2, False, False)])
        before = (forum.last_post_id, topic.last_post_id, topic.last_updated)
        posts[1].delete()
        self.assertEqual((forum.last_post_id, topic.last_post_id, topic.last_updated), before)
        db.session.commit()
        print("NONLAST_DELETE pointer_before_after={}".format(before))

    def test_hidden_post_is_excluded_but_hidden_topic_post_remains_candidate(self):
        forum, topic, posts = self.seed([(40, 0, False, False),
                                         (50, 1, True, False),
                                         (55, 3, None, False),
                                         (60, 2, False, True)])
        # The forum query is rooted at Post, so HideableQuery removes hidden
        # Posts; the joined Topic has no corresponding hidden predicate.
        posts[-1].delete()
        self.assertEqual(forum.last_post_id, 40)
        self.assertTrue(topic.hidden)
        self.assertIsNone(Post.query.filter_by(id=50).first())
        self.assertIsNone(Post.query.filter_by(id=55).first())
        self.assertIsNotNone(Post.query.with_hidden().filter_by(id=55).first())
        self.assertIsNone(Post.query.with_hidden().filter_by(id=60).first())
        print("HIDDEN_CANDIDATES hidden_post=50 null_hidden=55 selected=40 selected_topic_hidden=true")

    def test_first_post_delete_delegates_to_topic_delete(self):
        _, topic, posts = self.seed([(71, 0, False, False),
                                     (72, 1, False, False)])
        calls = []
        original = topic.delete

        def observe_delete(*args, **kwargs):
            calls.append((args, kwargs))
            return "delegated"

        topic.delete = observe_delete
        result = posts[0].delete()
        topic.delete = original
        self.assertIs(result, posts[0])
        self.assertEqual(len(calls), 1)
        print("FIRST_POST_DELEGATION calls={}".format(len(calls)))

    def test_wrong_date_sort_negative_control_is_detected(self):
        forum, _, _ = self.seed([(80, 0, False, False),
                                 (90, 9, False, False),
                                 (100, 1, False, False)])
        source_query = Post.query.filter(
            Post.topic_id == Topic.id, Topic.forum_id == forum.id,
            Post.hidden != True,
        )
        by_source_id = source_query.order_by(Post.id.desc()).first()
        wrong_by_date = source_query.order_by(Post.date_created.desc()).first()
        self.assertEqual(by_source_id.id, 100)
        if os.environ.get("FLASKBB_ORACLE_NEGATIVE_CONTROL") == "date-sort":
            self.assertEqual(wrong_by_date.id, by_source_id.id,
                             "negative control: date sort must be rejected")
        with self.assertRaises(AssertionError):
            self.assertEqual(wrong_by_date.id, by_source_id.id)
        print("NEGATIVE_CONTROL date_sort_rejected expected_id=100 actual_id={}".format(
            wrong_by_date.id))

    def test_wrong_visibility_filter_negative_control_is_detected(self):
        forum, _, _ = self.seed([(110, 0, False, False),
                                 (120, 1, False, True),
                                 (130, 2, True, False)])
        source_query = Post.query.filter(
            Post.topic_id == Topic.id, Topic.forum_id == forum.id,
            Post.hidden != True,
        )
        source_candidate = source_query.order_by(Post.id.desc()).first()
        wrong_filter = source_query.filter(Topic.hidden != True).order_by(
            Post.id.desc()
        ).first()
        self.assertEqual(source_candidate.id, 120)
        wrong_filter_id = wrong_filter.id if wrong_filter is not None else None
        if os.environ.get("FLASKBB_ORACLE_NEGATIVE_CONTROL") == "topic-filter":
            self.assertEqual(wrong_filter_id, source_candidate.id,
                             "negative control: Topic visibility filter must be rejected")
        with self.assertRaises(AssertionError):
            self.assertEqual(wrong_filter_id, source_candidate.id)
        print("NEGATIVE_CONTROL topic_visibility_filter_rejected expected_id=120 actual_id={}".format(
            wrong_filter_id))


if __name__ == "__main__":
    unittest.main(verbosity=2)
