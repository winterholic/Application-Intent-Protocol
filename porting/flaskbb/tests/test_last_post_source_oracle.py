"""Execute pinned upstream method bodies with a tiny SQLite query adapter.

This is a source algorithm oracle, NOT SQLAlchemy, PostgreSQL or AIP parity.
The adapter supplies HideableQuery's root-only filter for each viewer. It does
not simulate flush/relationship ordering: second_last_post is an explicit input.
No upstream module is imported or modified; only named method ASTs are compiled.
"""
import ast
from pathlib import Path
import sqlite3
from types import SimpleNamespace
import unittest


SOURCE = Path(__file__).resolve().parents[1] / "upstream/flaskbb/forum/models.py"


class Column:
    def __init__(self, sql):
        self.sql = sql

    def compare(self, op, other):
        value = other.sql if isinstance(other, Column) else str(int(other))
        return f"{self.sql} {op} {value}"

    def __eq__(self, other):
        return self.compare("=", other)

    def __ne__(self, other):
        return self.compare("!=", other)

    def desc(self):
        return f"{self.sql} DESC"


def method(owner, name):
    tree = ast.parse(SOURCE.read_text())
    cls = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == owner)
    node = next(n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == name)
    node.decorator_list = []
    module = ast.Module(body=[node], type_ignores=[])
    namespace = {}
    exec(compile(module, str(SOURCE), "exec"), namespace)
    return namespace[name]


class Query:
    def __init__(self, db, table, objects, viewhidden):
        self.db, self.table, self.objects = db, table, objects
        self.clauses = [] if viewhidden else [f"{table}.hidden != 1"]
        self.order, self.count, self.start = [], None, 0

    def filter(self, *clauses):
        self.clauses.extend(clauses)
        return self

    def order_by(self, *order):
        self.order.extend(order)
        return self

    def limit(self, count):
        self.count = count
        return self

    def offset(self, start):
        self.start = start
        return self

    def all(self):
        tables = self.table
        if self.table == "posts" and any("topics." in c for c in self.clauses):
            tables += ", topics"
        sql = f"SELECT {self.table}.id FROM {tables}"
        if self.clauses:
            sql += " WHERE " + " AND ".join(self.clauses)
        if self.order:
            sql += " ORDER BY " + ", ".join(self.order)
        if self.count is not None:
            sql += f" LIMIT {self.count} OFFSET {self.start}"
        return [self.objects[row[0]] for row in self.db.execute(sql)]

    def first(self):
        result = self.all()
        return result[0] if result else None


class Oracle(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(":memory:")
        self.addCleanup(self.db.close)
        self.db.executescript("""
            CREATE TABLE topics(id INTEGER PRIMARY KEY, forum_id INT, last_post_id INT, hidden INT);
            CREATE TABLE posts(id INTEGER PRIMARY KEY, topic_id INT, date_created INT, hidden INT);
        """)
        self.posts, self.topics = {}, {}
        self.forum = SimpleNamespace(id=1, last_post=None)

    def topic(self, key, hidden=False):
        row = SimpleNamespace(id=key, forum_id=1, forum=self.forum, hidden=hidden,
                              title=f"topic-{key}", user=f"topic-user-{key}",
                              username=f"topic-name-{key}", last_updated=0,
                              second_last_post=None, last_post=None, first_post=None)
        self.topics[key] = row
        return row

    def post(self, key, topic, time, hidden=False):
        row = SimpleNamespace(id=key, topic=topic, topic_id=topic.id, date_created=time,
                              hidden=hidden, user=f"post-user-{key}", user_id=key,
                              username=f"post-name-{key}")
        self.posts[key] = row
        if topic.first_post is None:
            topic.first_post = row
        topic.last_post, topic.last_updated = row, time
        return row

    def run_method(self, owner, name, row, viewhidden=False):
        self.db.execute("DELETE FROM posts")
        self.db.execute("DELETE FROM topics")
        self.db.executemany("INSERT INTO posts VALUES (?,?,?,?)",
                            [(p.id, p.topic.id, p.date_created, p.hidden) for p in self.posts.values()])
        self.db.executemany("INSERT INTO topics VALUES (?,?,?,?)",
                            [(t.id, 1, t.last_post.id if t.last_post else None, t.hidden)
                             for t in self.topics.values()])
        fn = method(owner, name)
        for cls, table, objects in [("Post", "posts", self.posts), ("Topic", "topics", self.topics)]:
            columns = ("id", "topic_id", "date_created", "hidden") if cls == "Post" else ("id", "forum_id", "last_post_id", "hidden")
            fn.__globals__[cls] = SimpleNamespace(query=Query(self.db, table, objects, viewhidden),
                                                  **{c: Column(f"{table}.{c}") for c in columns})
        if owner == "Forum":
            fn(row, commit=False)
        else:
            fn(row)

    def test_post_requires_both_pointer_conditions_for_forum_update(self):
        t = self.topic(1)
        old, current = self.post(1, t, 10), self.post(2, t, 20)
        self.forum.last_post = current
        self.run_method("Post", "_deal_with_last_post", old)
        self.assertIs(self.forum.last_post, current)
        self.assertIs(t.last_post, current)
        self.forum.last_post = old  # topic-last matches; forum-last does not
        self.run_method("Post", "_deal_with_last_post", current)
        self.assertIs(self.forum.last_post, old)
        self.assertIs(t.last_post, t.first_post)

    def test_post_id_order_includes_posts_in_hidden_topics(self):
        t, hidden_topic = self.topic(1), self.topic(2, hidden=True)
        self.post(41, t, 200)
        selected = self.post(42, hidden_topic, 100)
        current = self.post(43, t, 300, hidden=True)
        self.forum.last_post = current
        self.run_method("Post", "_deal_with_last_post", current, viewhidden=True)
        self.assertIs(self.forum.last_post, selected)
        self.assertEqual(self.forum.last_post_username, selected.username)
        self.assertEqual(self.forum.last_post_created, 100)

    def test_post_hidden_null_is_not_false_in_sql_predicate(self):
        t = self.topic(1)
        self.post(1, t, 10, hidden=None)
        current = self.post(2, t, 20, hidden=True)
        self.forum.last_post = current
        self.run_method("Post", "_deal_with_last_post", current)
        for field in ("last_post", "last_post_user", "last_post_title", "last_post_username", "last_post_created"):
            self.assertIsNone(getattr(self.forum, field))

    def test_topic_hide_uses_last_post_id_but_topic_author_projection(self):
        other, current = self.topic(1), self.topic(2)
        replacement = self.post(41, other, 200)
        latest = self.post(42, current, 100)
        self.forum.last_post = latest
        self.run_method("Topic", "_remove_topic_from_forum", current)
        self.assertIs(self.forum.last_post, replacement)
        self.assertEqual(self.forum.last_post_user, other.user)
        self.assertEqual(self.forum.last_post_created, other.last_updated)
        self.assertNotEqual(self.forum.last_post_user, replacement.user)

    def test_topic_hide_non_top_preserves_existing_projection(self):
        other, current = self.topic(1), self.topic(2)
        self.post(42, other, 100)
        self.post(41, current, 200)
        sentinel = object()
        self.forum.last_post = sentinel
        self.run_method("Topic", "_remove_topic_from_forum", current)
        self.assertIs(self.forum.last_post, sentinel)

    def test_forum_update_root_hidden_filter_depends_on_viewer(self):
        t = self.topic(1, hidden=True)
        visible, hidden = self.post(1, t, 100), self.post(2, t, 200, hidden=True)
        self.run_method("Forum", "update_last_post", self.forum, viewhidden=False)
        self.assertIs(self.forum.last_post, visible)  # Topic.hidden is not joined into policy
        self.run_method("Forum", "update_last_post", self.forum, viewhidden=True)
        self.assertIs(self.forum.last_post, hidden)

    def test_forum_same_pointer_does_not_refresh_projection(self):
        t = self.topic(1)
        self.forum.last_post = self.post(1, t, 100)
        self.forum.last_post_title = "preserve-existing"
        self.run_method("Forum", "update_last_post", self.forum)
        self.assertEqual(self.forum.last_post_title, "preserve-existing")

    def test_unhide_strict_time_comparison_preserves_forum_on_tie(self):
        old, restored = self.topic(1), self.topic(2)
        self.forum.last_post = self.post(1, old, 100)
        self.forum.last_post_created = 100
        self.post(2, restored, 100)
        self.run_method("Topic", "_restore_topic_to_forum", restored)
        self.assertIs(self.forum.last_post, old.last_post)
        restored.last_updated = 101
        self.run_method("Topic", "_restore_topic_to_forum", restored)
        self.assertIs(self.forum.last_post, restored.last_post)
        self.assertEqual(self.forum.last_post_user, restored.user)


if __name__ == "__main__":
    unittest.main()
