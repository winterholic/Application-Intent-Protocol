import sys
import unittest
from datetime import datetime, timedelta, timezone
from urllib.parse import parse_qs, urlparse
from pathlib import Path
from unittest.mock import Mock

UI_ROOT = Path(__file__).resolve().parents[1] / "ui"
sys.path.insert(0, str(UI_ROOT))

from app import create_app  # noqa: E402
import read_tracking  # noqa: E402


class UiHostTests(unittest.TestCase):
    def test_forum_topics_request_sticky_then_recent_order_from_aip(self):
        client_api = Mock()
        client_api.discover.return_value = {"ok": True}
        client_api.session.return_value = {"actorId": "2"}
        client_api.read.side_effect = lambda query: {
            "Forum": [{"id": "2", "title": "Support", "locked": False}],
            "Topic": [],
            "TopicRead": [],
            "ForumRead": [],
            "User": [{"id": "2", "username": "member", "activated": True}],
        }[query["read"]]
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret"},
            sdk_factory=Mock(return_value=client_api),
            sessions={"sticky-session": {"access_token": "test-only-token"}},
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "sticky-session")
        response = browser.get("/forum/2")
        self.assertEqual(response.status_code, 200)
        query = next(call.args[0] for call in client_api.read.call_args_list if call.args[0]["read"] == "Topic")
        self.assertEqual(query["sort"], [
            {"field": "important", "dir": "desc"},
            {"field": "last_updated", "dir": "desc"},
            {"field": "id", "dir": "asc"},
        ])

    def test_login_redirect_starts_oidc_code_flow_with_pkce(self):
        flows = {}
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"},
            flows=flows,
        )
        response = app.test_client().get("/auth/login")
        target = urlparse(response.headers["Location"])
        query = parse_qs(target.query)

        self.assertEqual(target.scheme, "https")
        self.assertEqual(query["response_type"], ["code"])
        self.assertEqual(query["code_challenge_method"], ["S256"])
        self.assertEqual(len(query["code_challenge"][0]), 43)
        self.assertEqual(len(query["state"][0]), 43)
        self.assertEqual(len(query["nonce"][0]), 43)
        self.assertEqual(len(flows), 1)

    def test_oidc_callback_rejects_state_not_bound_to_browser_flow(self):
        flows = {"browser-flow": {"state": "expected", "nonce": "n", "verifier": "v"}}
        app = create_app({"TESTING": True, "SECRET_KEY": "test-only-secret"}, flows=flows)
        browser = app.test_client()
        browser.set_cookie("aip_oidc_flow", "browser-flow")

        response = browser.get("/oidc/callback?state=attacker&code=not-exchanged")

        self.assertEqual(response.status_code, 400)
        self.assertEqual(flows, {})

    def test_logout_requires_session_bound_csrf_token(self):
        sessions = {"session": {"access_token": "fixture", "csrf_token": "expected"}}
        app = create_app({"TESTING": True, "SECRET_KEY": "test-only-secret"}, sessions=sessions)
        browser = app.test_client()
        browser.set_cookie("aip_session", "session")

        denied = browser.post("/auth/logout", data={"csrf_token": "wrong"})
        self.assertEqual(denied.status_code, 403)
        self.assertIn("session", sessions)
        logged_out = browser.post("/auth/logout", data={"csrf_token": "expected"})
        self.assertEqual(logged_out.status_code, 303)
        self.assertNotIn("session", sessions)

    def test_authenticated_forum_home_reads_only_aip_forum_resource(self):
        client_api = Mock()
        client_api.discover.return_value = {"ok": True}
        client_api.session.return_value = {"actorId": "6"}
        client_api.read.side_effect = lambda query: {
            "User": [{"id": "6", "username": "porting-user", "primary_group": "3", "activated": True}],
            "Category": [{"id": "1", "title": "Support", "description": "Questions", "position": 1}],
            "Forum": [{"id": "2", "category": "1", "title": "Support", "description": "Questions", "position": 1,
                       "locked": False, "topic_count": 2, "post_count": 4}],
            "ForumRead": [],
        }[query["read"]]
        client_factory = Mock(return_value=client_api)
        sessions = {"opaque-test-session": {"access_token": "test-only-token"}}
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"},
            sdk_factory=client_factory,
            sessions=sessions,
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "opaque-test-session")
        response = browser.get("/")

        self.assertEqual(response.status_code, 200)
        self.assertIn(b"Support", response.data)
        client_factory.assert_called_once_with("http://aip.test", "test-only-token")
        client_api.session.assert_called_once_with()
        self.assertEqual([call.args[0]["read"] for call in client_api.read.call_args_list], ["User", "Category", "Forum", "ForumRead"])
        forum_query = next(call.args[0] for call in client_api.read.call_args_list if call.args[0]["read"] == "Forum")
        self.assertIn("title", forum_query["select"])
        self.assertIn("last_post_created", forum_query["select"])

    def test_new_topic_posts_one_authorized_atomic_aip_bundle(self):
        client_api = Mock()
        client_api.session.return_value = {"actorId": "6"}
        client_api.read.return_value = [{"id": "6", "username": "member", "activated": True}]
        client_api.bundle.return_value = {"ok": True, "created": []}
        sessions = {"session": {"access_token": "test-only-token", "csrf_token": "csrf"}}
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"},
            sdk_factory=Mock(return_value=client_api),
            sessions=sessions,
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "session")

        response = browser.post(
            "/forum/2/topic",
            data={"csrf_token": "csrf", "title": "AIP topic", "content": "First post"},
        )

        self.assertEqual(response.status_code, 303)
        client_api.bundle.assert_called_once()
        workflow = client_api.bundle.call_args.args[0]
        self.assertEqual([operation.get("create") or operation.get("apply") for operation in workflow],
                         ["Topic", "Post", "Post.publish", "TopicFirstPost", "Topic.publish"])
        self.assertTrue(all(operation["values"]["username"] == "member" for operation in workflow[:2]))

    def test_topic_reply_uses_aip_atomic_create_publish_and_bump(self):
        client_api = Mock()
        client_api.session.return_value = {"actorId": "6"}
        client_api.read.side_effect = [
            [{"id": "6", "username": "member", "activated": True}],
            [{"id": "topic-1", "forum": "2", "locked": False}],
        ]
        client_api.bundle.return_value = {"ok": True, "created": []}
        sessions = {"session": {"access_token": "test-only-token", "csrf_token": "csrf"}}
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"},
            sdk_factory=Mock(return_value=client_api),
            sessions=sessions,
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "session")

        response = browser.post(
            "/topic/topic-1/reply",
            data={"csrf_token": "csrf", "content": "Reply body"},
        )

        self.assertEqual(response.status_code, 303)
        workflow = client_api.bundle.call_args.args[0]
        self.assertEqual([operation.get("create") or operation.get("apply") for operation in workflow],
                         ["Post", "Post.publish", "Topic.bump"])
        self.assertEqual(workflow[0]["values"]["username"], "member")
        self.assertEqual(workflow[0]["values"]["topic"], "topic-1")

    def test_topic_post_without_csrf_is_rejected_before_aip_mutation(self):
        client_api = Mock()
        sessions = {"session": {"access_token": "test-only-token", "csrf_token": "csrf"}}
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"},
            sdk_factory=Mock(return_value=client_api),
            sessions=sessions,
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "session")

        response = browser.post("/forum/2/topic", data={"title": "No CSRF", "content": "Denied"})

        self.assertEqual(response.status_code, 403)
    def test_auth_routes_redirect_to_oidc_endpoints(self):
        app = create_app({"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"})
        browser = app.test_client()

        reg_res = browser.get("/auth/register")
        self.assertEqual(reg_res.status_code, 302)
        self.assertIn("kc_action=register", reg_res.headers["Location"])

        forgot_res = browser.get("/auth/forgot-password")
        self.assertEqual(forgot_res.status_code, 302)
        self.assertIn("kc_action=UPDATE_PASSWORD", forgot_res.headers["Location"])

        reset_res = browser.get("/auth/reset-password")
        self.assertEqual(reset_res.status_code, 302)
        self.assertIn("kc_action=UPDATE_PASSWORD", reset_res.headers["Location"])

        sessions = {"sess": {"access_token": "token", "csrf_token": "csrf"}}
        app_with_sess = create_app({"TESTING": True, "SECRET_KEY": "test-only-secret"}, sessions=sessions)
        b2 = app_with_sess.test_client()
        b2.set_cookie("aip_session", "sess")
        logout_res = b2.get("/auth/logout")
        self.assertEqual(logout_res.status_code, 303)
        self.assertNotIn("sess", sessions)

    def test_search_route_invokes_aip_filters(self):
        client_api = Mock()
        client_api.read.return_value = []
        sessions = {"session": {"access_token": "token", "csrf_token": "csrf"}}
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"},
            sdk_factory=Mock(return_value=client_api),
            sessions=sessions,
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "session")

        res = browser.get("/search?q=testkeyword&type=all")
        self.assertEqual(res.status_code, 200)
        self.assertIn(b"Search Results for \"testkeyword\"", res.data)
        # Upstream whoosheers index several fields per type (utils/search.py) and
        # whooshee_search matches any of them, so each field gets its own read.
        issued = sorted(
            (c.args[0]["read"], c.args[0]["filter"][0]["field"])
            for c in client_api.read.call_args_list
        )
        self.assertEqual(issued, [
            ("Forum", "description"), ("Forum", "title"),
            ("Post", "content"), ("Post", "modified_by"), ("Post", "username"),
            ("Topic", "title"), ("Topic", "username"),
            ("User", "username"),
        ])
        for call in client_api.read.call_args_list:
            self.assertEqual(call.args[0]["filter"][0]["op"], "contains")
            self.assertEqual(call.args[0]["filter"][0]["value"], "testkeyword")

    def _search_browser(self, read):
        client_api = Mock()
        client_api.read.side_effect = read
        sessions = {"session": {"access_token": "token", "csrf_token": "csrf"}}
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"},
            sdk_factory=Mock(return_value=client_api),
            sessions=sessions,
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "session")
        return browser, client_api

    def test_search_merges_field_matches_without_duplicates(self):
        def read(query):
            if query["read"] == "Topic":
                # Same topic matches by title and by author name; another only by author.
                rows = {"title": [{"id": "t1", "title": "Alpha", "username": "x", "forum": "1"}],
                        "username": [{"id": "t1", "title": "Alpha", "username": "x", "forum": "1"},
                                     {"id": "t2", "title": "Beta", "username": "keyword-user", "forum": "1"}]}
                return rows[query["filter"][0]["field"]]
            return []
        browser, _ = self._search_browser(read)
        res = browser.get("/search?q=keyword&type=topic")
        self.assertEqual(res.status_code, 200)
        self.assertEqual(res.data.count(b"Alpha"), 1)
        self.assertEqual(res.data.count(b"Beta"), 1)
        self.assertIn(b"Matching Topics (2)", res.data)

    def test_search_rejects_query_outside_original_length_bounds_without_reading(self):
        # Upstream SearchPageForm: DataRequired + Length(min=3, max=50) (forum/forms.py).
        for q, text in (("ab", b"between 3 and 50"), ("x" * 51, b"between 3 and 50"), ("   ", b"required")):
            with self.subTest(q=q):
                browser, client_api = self._search_browser(lambda query: [])
                res = browser.get("/search", query_string={"q": q, "type": "all"})
                self.assertEqual(res.status_code, 200)
                self.assertIn(text, res.data)
                self.assertNotIn(b"Search Results for", res.data)
                client_api.read.assert_not_called()

    def test_search_accepts_query_at_original_length_bounds(self):
        for q in ("abc", "x" * 50):
            with self.subTest(length=len(q)):
                browser, client_api = self._search_browser(lambda query: [])
                res = browser.get("/search", query_string={"q": q, "type": "user"})
                self.assertEqual(res.status_code, 200)
                self.assertNotIn(b"between 3 and 50", res.data)
                self.assertEqual(client_api.read.call_count, 1)

    def test_search_rejects_unknown_type_without_reading(self):
        browser, client_api = self._search_browser(lambda query: [])
        res = browser.get("/search?q=keyword&type=bogus")
        self.assertEqual(res.status_code, 200)
        self.assertIn(b"not a valid", res.data)
        client_api.read.assert_not_called()

    def test_member_list_and_user_profile(self):
        client_api = Mock()
        client_api.read.side_effect = [
            [{"id": "6", "username": "porting-user", "primary_group": "3", "post_count": 5}],
            [{"id": "6", "username": "porting-user", "primary_group": {"name": "Member"}, "activated": True, "signature": "Sig"}],
            [{"id": "t1", "title": "My Topic", "date_created": "2026-10-17T00:00:00Z"}],
            [{"id": "p1", "topic": "t1", "content": "My Post", "date_created": "2026-10-17T00:00:00Z"}],
        ]
        sessions = {"session": {"access_token": "token", "csrf_token": "csrf"}}
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"},
            sdk_factory=Mock(return_value=client_api),
            sessions=sessions,
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "session")

        users_res = browser.get("/users")
        self.assertEqual(users_res.status_code, 200)
        self.assertIn(b"porting-user", users_res.data)

        profile_res = browser.get("/user/porting-user")
        self.assertEqual(profile_res.status_code, 200)
        self.assertIn(b"porting-user", profile_res.data)
        self.assertIn(b"My Topic", profile_res.data)
        self.assertIn(b"My Post", profile_res.data)

    def test_user_settings_post_calls_update_profile_bundle(self):
        client_api = Mock()
        client_api.session.return_value = {"actorId": "6"}
        client_api.read.return_value = [{"id": "6", "username": "porting-user", "activated": True}]
        client_api.bundle.return_value = {"ok": True}
        sessions = {"session": {"access_token": "token", "csrf_token": "csrf"}}
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"},
            sdk_factory=Mock(return_value=client_api),
            sessions=sessions,
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "session")

        res = browser.post(
            "/user/settings",
            data={
                "csrf_token": "csrf",
                "location": "Seoul",
                "signature": "My Cool Signature",
            },
        )
        self.assertEqual(res.status_code, 303)
        client_api.bundle.assert_called_once()
        ops = client_api.bundle.call_args.args[0]
        self.assertEqual(ops[0]["apply"], "User.updateProfile")
        self.assertEqual(ops[0]["target"], {"ids": ["6"]})
        self.assertEqual(ops[0]["values"]["location"], "Seoul")
        self.assertEqual(ops[0]["values"]["signature"], "My Cool Signature")

    def test_create_conversation_bundle(self):
        client_api = Mock()
        client_api.session.return_value = {"actorId": "6"}
        client_api.read.side_effect = [
            [{"id": "6", "username": "sender", "activated": True}],
            [{"id": "7", "username": "recipient", "activated": True}],
        ]
        client_api.bundle.side_effect = [
            {"ok": True, "created": [{"resource": "Conversation", "id": "101"}, {"resource": "Conversation", "id": "102"}]},
            {"ok": True, "created": [{"resource": "Message", "id": "201"}, {"resource": "Message", "id": "202"}]},
        ]
        sessions = {"session": {"access_token": "token", "csrf_token": "csrf"}}
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"},
            sdk_factory=Mock(return_value=client_api),
            sessions=sessions,
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "session")

        res = browser.post(
            "/conversations/new",
            data={
                "csrf_token": "csrf",
                "to_username": "recipient",
                "subject": "Hello",
                "message": "Testing private message",
            },
        )
        self.assertEqual(res.status_code, 303)
        self.assertEqual(client_api.bundle.call_count, 2)
        conv_ops = client_api.bundle.call_args_list[0].args[0]
        self.assertEqual(len(conv_ops), 2)
        self.assertEqual(conv_ops[0]["create"], "Conversation")
        self.assertEqual(conv_ops[0]["values"]["user"], "6")
        self.assertEqual(conv_ops[1]["create"], "Conversation")
        self.assertEqual(conv_ops[1]["values"]["user"], "7")

        msg_ops = client_api.bundle.call_args_list[1].args[0]
        self.assertEqual(len(msg_ops), 2)
        self.assertEqual(msg_ops[0]["create"], "Message")
        self.assertEqual(msg_ops[0]["values"]["conversation"], "101")
        self.assertEqual(msg_ops[0]["values"]["message"], "Testing private message")
        self.assertEqual(msg_ops[1]["create"], "Message")
        self.assertEqual(msg_ops[1]["values"]["conversation"], "102")
        self.assertEqual(msg_ops[1]["values"]["message"], "Testing private message")

    def test_reply_conversation_bundle(self):
        client_api = Mock()
        client_api.session.return_value = {"actorId": "6"}
        client_api.read.side_effect = [
            [{"id": "6", "username": "sender", "activated": True}],
            [{"id": "101", "user": "6", "shared_id": "shared-uuid-1"}],
            [
                {"id": "101", "user": "6", "shared_id": "shared-uuid-1"},
                {"id": "102", "user": "7", "shared_id": "shared-uuid-1"},
            ],
        ]
        client_api.bundle.return_value = {"ok": True}
        sessions = {"session": {"access_token": "token", "csrf_token": "csrf"}}
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"},
            sdk_factory=Mock(return_value=client_api),
            sessions=sessions,
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "session")

        res = browser.post(
            "/conversations/101/reply",
            data={
                "csrf_token": "csrf",
                "message": "Reply message text",
            },
        )
        self.assertEqual(res.status_code, 303)
        client_api.bundle.assert_called_once()
        ops = client_api.bundle.call_args.args[0]
        self.assertEqual(len(ops), 3)
        self.assertEqual(ops[0]["create"], "Message")
        self.assertEqual(ops[0]["values"]["conversation"], "101")
        self.assertEqual(ops[0]["values"]["message"], "Reply message text")
        self.assertEqual(ops[1]["create"], "Message")
        self.assertEqual(ops[1]["values"]["conversation"], "102")
        self.assertEqual(ops[1]["values"]["message"], "Reply message text")
        self.assertEqual(ops[2]["apply"], "Conversation.bump")
        self.assertEqual(ops[2]["target"], {"ids": ["102"]})


class TokenConsumeRoutePinTests(unittest.TestCase):
    """PRODUCT-GAPS G-13: mailed activation/reset links have no port consumer.

    The original consumes tokens at /auth/reset-password/<token> and
    /auth/activate/confirm; the port's mails link to /auth/activate/<token>
    and /auth/reset/<token>, which the renderer does not serve (its
    /auth/reset-password is an IdP redirect, pinned separately). If consume
    routes ever land, these pins flip.
    """

    def test_renderer_serves_no_token_consume_routes(self):
        app = create_app({"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"})
        rules = sorted(str(rule) for rule in app.url_map.iter_rules())
        self.assertEqual([r for r in rules if r.startswith("/auth/activate")], [])
        self.assertEqual([r for r in rules if r.startswith("/auth/reset/")], [])

    def test_mailed_link_shapes_are_dead_in_the_port(self):
        app = create_app({"TESTING": True, "SECRET_KEY": "test-only-secret", "AIP_URL": "http://aip.test"})
        browser = app.test_client()
        self.assertEqual(browser.get("/auth/activate/opaque-token-1").status_code, 404)
        self.assertEqual(browser.get("/auth/reset/opaque-token-2").status_code, 404)


class ReadTrackingCutoffTests(unittest.TestCase):
    """Unread cutoff rules mirror upstream helpers.py topic/forum_is_unread."""

    def test_topic_unread_matches_upstream_cutoff_rules(self):
        now = datetime(2026, 10, 20, tzinfo=timezone.utc)
        fresh = now - timedelta(hours=1)
        old = now - timedelta(days=30)
        self.assertFalse(read_tracking.topic_is_unread(last_updated=fresh, authenticated=False, now=now))
        self.assertFalse(read_tracking.topic_is_unread(last_updated=fresh, tracker_length=0, now=now))
        self.assertFalse(read_tracking.topic_is_unread(last_updated=old, now=now))
        self.assertTrue(read_tracking.topic_is_unread(last_updated=now - timedelta(days=7), now=now))
        self.assertTrue(read_tracking.topic_is_unread(last_updated=fresh, now=now))
        self.assertTrue(read_tracking.topic_is_unread(
            last_updated=fresh, forumsread_cleared=now - timedelta(hours=2), now=now))
        self.assertFalse(read_tracking.topic_is_unread(
            last_updated=fresh, forumsread_cleared=now, now=now))
        self.assertTrue(read_tracking.topic_is_unread(
            last_updated=fresh, topicsread_last_read=now - timedelta(hours=2), now=now))
        self.assertFalse(read_tracking.topic_is_unread(
            last_updated=fresh, topicsread_last_read=fresh, now=now))
        self.assertFalse(read_tracking.topic_is_unread(last_updated=None, now=now))

    def test_forum_unread_matches_upstream_cutoff_rules(self):
        now = datetime(2026, 10, 20, tzinfo=timezone.utc)
        fresh = now - timedelta(hours=1)
        old = now - timedelta(days=30)
        self.assertFalse(read_tracking.forum_is_unread(
            topic_count=3, last_post_id="p", last_post_created=fresh, authenticated=False, now=now))
        self.assertFalse(read_tracking.forum_is_unread(
            topic_count=3, last_post_id="p", last_post_created=fresh, tracker_length=0, now=now))
        self.assertFalse(read_tracking.forum_is_unread(
            topic_count=0, last_post_id="p", last_post_created=fresh, now=now))
        self.assertFalse(read_tracking.forum_is_unread(
            topic_count=3, last_post_id=None, last_post_created=fresh, now=now))
        self.assertFalse(read_tracking.forum_is_unread(
            topic_count=3, last_post_id="p", last_post_created=old, now=now))
        self.assertTrue(read_tracking.forum_is_unread(
            topic_count=3, last_post_id="p", last_post_created=fresh, now=now))
        self.assertFalse(read_tracking.forum_is_unread(
            topic_count=3, last_post_id="p", last_post_created=fresh,
            forumsread={"last_read": now, "cleared": None}, now=now))
        self.assertTrue(read_tracking.forum_is_unread(
            topic_count=3, last_post_id="p", last_post_created=fresh,
            forumsread={"last_read": now - timedelta(hours=2), "cleared": None}, now=now))
        # Upstream quirk, pinned as-is: a cleared forum with a stale last_read
        # still counts as unread (the cleared branch falls through to the
        # last_read comparison), unlike the topic rule.
        self.assertTrue(read_tracking.forum_is_unread(
            topic_count=3, last_post_id="p", last_post_created=fresh,
            forumsread={"last_read": now - timedelta(hours=2), "cleared": now}, now=now))
        self.assertFalse(read_tracking.forum_is_unread(
            topic_count=3, last_post_id="p", last_post_created=fresh,
            forumsread={"last_read": now, "cleared": now}, now=now))

    def test_iso_timestamps_parse_like_sdk_rows(self):
        now = datetime(2026, 10, 20, tzinfo=timezone.utc)
        self.assertTrue(read_tracking.topic_is_unread(
            last_updated=(now - timedelta(hours=1)).isoformat(), now=now))
        self.assertFalse(read_tracking.topic_is_unread(
            last_updated=(now - timedelta(days=30)).isoformat(), now=now))
        self.assertFalse(read_tracking.topic_is_unread(
            last_updated=(now - timedelta(hours=1)).isoformat(),
            topicsread_last_read=now.isoformat(), now=now))


class ForumTopicUnreadRenderingTests(unittest.TestCase):
    def test_forum_view_marks_unread_and_read_topics(self):
        now = datetime.now(timezone.utc)
        fresh = (now - timedelta(hours=1)).isoformat()
        old = (now - timedelta(days=30)).isoformat()
        stamped = now.isoformat()
        client_api = Mock()
        client_api.session.return_value = {"actorId": "2"}
        client_api.read.side_effect = lambda query: {
            "Forum": [{"id": "1", "title": "Fresh", "locked": False}],
            "Topic": [
                {"id": "t1", "title": "Fresh unread", "username": "a", "post_count": 1,
                 "last_updated": fresh, "locked": False, "important": False, "hidden": False},
                {"id": "t2", "title": "Fresh read", "username": "a", "post_count": 1,
                 "last_updated": fresh, "locked": False, "important": False, "hidden": False},
                {"id": "t4", "title": "Old below cutoff", "username": "a", "post_count": 1,
                 "last_updated": old, "locked": False, "important": False, "hidden": False},
            ],
            "TopicRead": [{"id": "m2", "topic": "t2", "forum": "1", "last_read": stamped}],
            "ForumRead": [],
            "User": [{"id": "2", "username": "member", "activated": True}],
        }[query["read"]]
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret"},
            sdk_factory=Mock(return_value=client_api),
            sessions={"s": {"access_token": "test-only-token"}},
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "s")
        response = browser.get("/forum/1")
        self.assertEqual(response.status_code, 200)
        rows = response.data.decode().split("forum-row")
        row = {title: next(chunk for chunk in rows if title in chunk)
               for title in ("Fresh unread", "Fresh read", "Old below cutoff")}
        self.assertIn("topic-unread", row["Fresh unread"])
        self.assertIn("topic-read", row["Fresh read"])
        self.assertNotIn("topic-unread", row["Fresh read"])
        self.assertIn("topic-read", row["Old below cutoff"])

    def test_forum_index_marks_unread_and_read_forums(self):
        now = datetime.now(timezone.utc)
        fresh = (now - timedelta(hours=1)).isoformat()
        old = (now - timedelta(days=30)).isoformat()
        stamped = now.isoformat()
        client_api = Mock()
        client_api.session.return_value = {"actorId": "2"}
        client_api.read.side_effect = lambda query: {
            "User": [{"id": "2", "username": "member", "primary_group": "2", "activated": True}],
            "Category": [{"id": "1", "title": "General", "description": "", "position": 1}],
            "Forum": [
                {"id": "1", "category": "1", "title": "Fresh Forum", "description": "",
                 "position": 1, "locked": False, "topic_count": 2, "post_count": 4,
                 "last_post": "p1", "last_post_created": fresh},
                {"id": "2", "category": "1", "title": "Quiet Forum", "description": "",
                 "position": 2, "locked": False, "topic_count": 1, "post_count": 1,
                 "last_post": "p2", "last_post_created": old},
            ],
            "ForumRead": [{"id": "m", "forum": "2", "last_read": stamped, "cleared": stamped}],
        }[query["read"]]
        app = create_app(
            {"TESTING": True, "SECRET_KEY": "test-only-secret"},
            sdk_factory=Mock(return_value=client_api),
            sessions={"s": {"access_token": "test-only-token"}},
        )
        browser = app.test_client()
        browser.set_cookie("aip_session", "s")
        response = browser.get("/")
        self.assertEqual(response.status_code, 200)
        rows = response.data.decode().split("category-row")
        fresh_row = next(chunk for chunk in rows if "Fresh Forum" in chunk)
        quiet_row = next(chunk for chunk in rows if "Quiet Forum" in chunk)
        self.assertIn("forum-unread", fresh_row)
        self.assertIn("forum-read", quiet_row)
        self.assertNotIn("forum-unread", quiet_row)


if __name__ == "__main__":
    unittest.main()
