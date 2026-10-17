import sys
import unittest
from urllib.parse import parse_qs, urlparse
from pathlib import Path
from unittest.mock import Mock

UI_ROOT = Path(__file__).resolve().parents[1] / "ui"
sys.path.insert(0, str(UI_ROOT))

from app import create_app  # noqa: E402


class UiHostTests(unittest.TestCase):
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
        self.assertEqual([call.args[0]["read"] for call in client_api.read.call_args_list], ["User", "Category", "Forum"])
        forum_query = client_api.read.call_args_list[-1].args[0]
        self.assertIn("title", forum_query["select"])

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
        client_api.bundle.assert_not_called()


if __name__ == "__main__":
    unittest.main()
