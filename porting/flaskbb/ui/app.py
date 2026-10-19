"""Thin server-rendered FlaskBB theme host; all forum data goes through the AIP caller SDK."""
import base64
import hashlib
import json
import os
import secrets
import ssl
import time
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

from flask import Flask, abort, redirect, render_template, request, url_for
from joserfc import jwt
from joserfc.jwk import KeySet
from joserfc.jwt import JWTClaimsRegistry

from aip_sdk import AipError, connect

from read_tracking import forum_is_unread, topic_is_unread

TRACK = Path(__file__).resolve().parents[1]
UPSTREAM = TRACK / "upstream"


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def _https_context(ca_file: str | None) -> ssl.SSLContext:
    return ssl.create_default_context(cafile=ca_file or None)


def create_app(config=None, sdk_factory=connect, sessions=None, flows=None):
    app = Flask(
        __name__,
        template_folder=str(Path(__file__).parent / "templates"),
        static_folder=str(UPSTREAM / "flaskbb" / "static"),
    )
    app.config.from_mapping(
        SECRET_KEY=os.environ.get("FLASKBB_UI_SECRET"),
        AIP_URL=os.environ.get("AIP_URL", "http://127.0.0.1:58083"),
        OIDC_ISSUER=os.environ.get("OIDC_ISSUER", "https://127.0.0.1:58094/realms/flaskbb-port"),
        OIDC_AUTHORIZATION_ENDPOINT=os.environ.get("OIDC_AUTHORIZATION_ENDPOINT", "https://127.0.0.1:58094/realms/flaskbb-port/protocol/openid-connect/auth"),
        OIDC_TOKEN_ENDPOINT=os.environ.get("OIDC_TOKEN_ENDPOINT", "https://127.0.0.1:58094/realms/flaskbb-port/protocol/openid-connect/token"),
        OIDC_JWKS_URI=os.environ.get("OIDC_JWKS_URI", "https://127.0.0.1:58094/realms/flaskbb-port/protocol/openid-connect/certs"),
        OIDC_CLIENT_ID=os.environ.get("OIDC_CLIENT_ID", "aip-flaskbb"),
        OIDC_REDIRECT_URI=os.environ.get("OIDC_REDIRECT_URI", "http://127.0.0.1:58082/oidc/callback"),
        OIDC_CA_CERT=os.environ.get("OIDC_CA_CERT"),
        COOKIE_SECURE=os.environ.get("COOKIE_SECURE", "0") == "1",
    )
    if config:
        app.config.update(config)
    if not app.config.get("SECRET_KEY"):
        raise RuntimeError("FLASKBB_UI_SECRET must be set to a test-only random value")
    session_store = sessions if sessions is not None else {}
    flow_store = flows if flows is not None else {}

    app.config.update(
        SESSION_COOKIE_NAME="aip_session",
        SESSION_COOKIE_HTTPONLY=True,
        SESSION_COOKIE_SAMESITE="Lax",
        SESSION_COOKIE_SECURE=app.config["COOKIE_SECURE"],
    )

    @app.context_processor
    def session_context():
        sid = request.cookies.get("aip_session")
        identity = session_store.get(sid) if sid else None
        user = {"username": identity.get("username")} if identity else None
        return {"user": user, "csrf_token": identity.get("csrf_token") if identity else None}

    def aip_client():
        session_id = request.cookies.get("aip_session")
        identity = session_store.get(session_id) if session_id else None
        if not identity:
            abort(401)
        return sdk_factory(app.config["AIP_URL"], identity["access_token"])

    def require_csrf(identity):
        if not secrets.compare_digest(request.form.get("csrf_token", ""), identity["csrf_token"]):
            abort(403)

    def current_aip_user(client):
        session_info = client.session()
        actor_id = session_info.get("actorId")
        if not actor_id:
            abort(401)
        users = client.read({
            "read": "User",
            "select": ["id", "username", "primary_group", "activated"],
            "filter": [{"field": "id", "op": "eq", "value": actor_id}],
            "limit": 1,
        })
        if not users or not users[0].get("activated"):
            abort(403)
        return users[0]

    def can_contribute(client, user, target, permission):
        """Mirror upstream's visible post controls; AIP still decides every mutation."""
        if not user or not user.get("primary_group"):
            return False
        if permission == "posttopic" and target.get("locked"):
            return False
        if permission == "postreply" and target.get("hidden"):
            return False
        try:
            groups = client.read({
                "read": "Group",
                "select": ["id", "admin", "super_mod", "mod", "posttopic", "postreply"],
                "limit": 100,
            })
            memberships = client.read({
                "read": "UserGroup",
                "select": ["id", "user", "group"],
                "filter": [{"field": "user", "op": "eq", "value": user["id"]}],
                "limit": 1000,
            })
            moderators = client.read({
                "read": "ForumModerator",
                "select": ["id"],
                "filter": [
                    {"field": "forum", "op": "eq", "value": target["forum_id"]},
                    {"field": "user", "op": "eq", "value": user["id"]},
                ],
                "limit": 1,
            })
        except AipError as error:
            app.logger.warning("AIP permission metadata unavailable code=%s request_id=%s", error.code, error.request_id)
            return False

        memberships_ids = {str(row.get("group")) for row in memberships}
        memberships_ids.add(str(user["primary_group"]))
        user_groups = [row for row in groups if str(row.get("id")) in memberships_ids]
        is_moderator = bool(moderators) or any(
            group.get("admin") or group.get("super_mod") or group.get("mod") for group in user_groups
        )
        has_permission = any(group.get(permission) for group in user_groups)
        if permission == "posttopic":
            return is_moderator or has_permission
        if permission == "postreply":
            return is_moderator or (not target.get("locked") and has_permission)
        return False

    def submit_bundle(client, operations, key, on_success):
        result = client.bundle(operations, key=key)
        if result.get("ok") is True:
            return redirect(on_success(), code=303)
        code = result.get("code", "")
        app.logger.warning("AIP mutation rejected code=%s request_id=%s", code, result.get("requestId"))
        if code in {"WIRE.FORBIDDEN", "WIRE.ACCESS_DENIED", "WIRE.ACCESS_NOT_DECLARED"}:
            abort(403)
        if code in {"WIRE.UNAUTHENTICATED", "WIRE.TOKEN_EXPIRED"}:
            abort(401)
        abort(503)

    def reject_aip_error(error):
        app.logger.warning("AIP request rejected code=%s request_id=%s", error.code, error.request_id)
        abort(401 if error.code == "WIRE.UNAUTHENTICATED" else 503)

    def verified_id_token(raw_token, expected_nonce):
        context = _https_context(app.config.get("OIDC_CA_CERT"))
        with urllib.request.urlopen(app.config["OIDC_JWKS_URI"], context=context, timeout=5) as response:
            key_set = KeySet.import_key_set(json.loads(response.read()))
        decoded = jwt.decode(raw_token, key_set, algorithms=["RS256"])
        claims = decoded.claims
        JWTClaimsRegistry(
            iss={"essential": True, "value": app.config["OIDC_ISSUER"]},
            aud={"essential": True, "values": [app.config["OIDC_CLIENT_ID"]]},
            sub={"essential": True}, exp={"essential": True}, iat={"essential": True},
            nonce={"essential": True, "value": expected_nonce},
        ).validate(claims)
        audiences = claims["aud"] if isinstance(claims["aud"], list) else [claims["aud"]]
        if (len(audiences) > 1 or claims.get("azp") is not None) and claims.get("azp") != app.config["OIDC_CLIENT_ID"]:
            raise ValueError("ID token authorized party is invalid")
        if claims["exp"] <= int(time.time()) or claims["iat"] > int(time.time()) + 30:
            raise ValueError("ID token time window is invalid")
        if decoded.header.get("alg") != "RS256":
            raise ValueError("ID token algorithm is not allowed")
        return claims

    @app.get("/auth/login")
    def login():
        flow_id = secrets.token_urlsafe(32)
        state = secrets.token_urlsafe(32)
        nonce = secrets.token_urlsafe(32)
        verifier = secrets.token_urlsafe(64)
        challenge = b64url(hashlib.sha256(verifier.encode("ascii")).digest())
        flow_store[flow_id] = {"state": state, "nonce": nonce, "verifier": verifier}
        query_params = {
            "client_id": app.config["OIDC_CLIENT_ID"],
            "redirect_uri": app.config["OIDC_REDIRECT_URI"],
            "response_type": "code",
            "scope": "openid profile email",
            "state": state,
            "nonce": nonce,
            "code_challenge": challenge,
            "code_challenge_method": "S256",
        }
        action = request.args.get("kc_action")
        if action in {"register", "UPDATE_PASSWORD"}:
            query_params["kc_action"] = action
        query = urllib.parse.urlencode(query_params)
        response = redirect(app.config["OIDC_AUTHORIZATION_ENDPOINT"] + "?" + query, code=302)
        response.set_cookie("aip_oidc_flow", flow_id, httponly=True, secure=app.config["COOKIE_SECURE"], samesite="Lax", max_age=300)
        return response

    @app.get("/oidc/callback")
    def oidc_callback():
        flow_id = request.cookies.get("aip_oidc_flow")
        flow = flow_store.pop(flow_id, None) if flow_id else None
        if not flow or request.args.get("state") != flow["state"] or not request.args.get("code"):
            abort(400)
        context = _https_context(app.config.get("OIDC_CA_CERT"))
        token_body = urllib.parse.urlencode({
            "grant_type": "authorization_code",
            "client_id": app.config["OIDC_CLIENT_ID"],
            "redirect_uri": app.config["OIDC_REDIRECT_URI"],
            "code": request.args["code"],
            "code_verifier": flow["verifier"],
        }).encode("ascii")
        token_request = urllib.request.Request(app.config["OIDC_TOKEN_ENDPOINT"], data=token_body, headers={"Content-Type": "application/x-www-form-urlencoded"})
        with urllib.request.urlopen(token_request, context=context, timeout=8) as response:
            token_response = json.loads(response.read())
        claims = verified_id_token(token_response["id_token"], flow["nonce"])
        sdk = sdk_factory(app.config["AIP_URL"], token_response["access_token"])
        try:
            sdk.discover()  # forces the official SDK's server-validated /session before storing the session
        except AipError as error:
            app.logger.warning("AIP login rejected code=%s request_id=%s", error.code, error.request_id)
            if error.code == "WIRE.UNAUTHENTICATED":
                abort(401)
            abort(503)
        identity_id = secrets.token_urlsafe(32)
        session_store[identity_id] = {
            "access_token": token_response["access_token"],
            "sub": claims["sub"],
            "username": claims.get("preferred_username"),
            "csrf_token": secrets.token_urlsafe(32),
        }
        response = redirect(url_for("forum_index"), code=303)
        response.set_cookie("aip_session", identity_id, httponly=True, secure=app.config["COOKIE_SECURE"], samesite="Lax")
        response.delete_cookie("aip_oidc_flow")
        return response

    @app.get("/auth/register")
    def register():
        return redirect(url_for("login", kc_action="register"))

    @app.get("/auth/forgot-password")
    @app.get("/auth/reset-password")
    def forgot_password():
        return redirect(url_for("login", kc_action="UPDATE_PASSWORD"))

    @app.route("/auth/logout", methods=["GET", "POST"])
    def logout():
        identity_id = request.cookies.get("aip_session")
        identity = session_store.get(identity_id) if identity_id else None
        if request.method == "POST":
            if not identity or not secrets.compare_digest(request.form.get("csrf_token", ""), identity["csrf_token"]):
                abort(403)
        if identity_id:
            session_store.pop(identity_id, None)
        response = redirect(url_for("forum_index"), code=303)
        response.delete_cookie("aip_session")
        return response

    @app.get("/")
    def forum_index():
        session_id = request.cookies.get("aip_session")
        identity = session_store.get(session_id) if session_id else None
        if not identity:
            return render_template("login.html", login_url=url_for("login"), registration_url=url_for("login", kc_action="register"))
        client = aip_client()
        try:
            session_info = client.session()
            if not session_info.get("actorId"):
                abort(401)
            user_rows = client.read({"read": "User", "select": ["id", "username", "primary_group", "activated"], "filter": [{"field": "id", "op": "eq", "value": session_info["actorId"]}], "limit": 1})
            categories = client.read({"read": "Category", "select": ["id", "title", "description", "position"]})
            forums = client.read({"read": "Forum", "select": ["id", "category", "title", "description", "position", "locked", "topic_count", "post_count", "last_post", "last_post_created"]})
            forum_marks = client.read({"read": "ForumRead", "select": ["forum", "last_read", "cleared"]})
        except AipError as error:
            reject_aip_error(error)
        if not user_rows or not user_rows[0].get("activated"):
            abort(403)
        marks_by_forum = {str(row["forum"]): row for row in forum_marks}
        categories_by_id = {str(row["id"]): {**row, "forums": []} for row in categories}
        for forum in forums:
            forum["unread"] = forum_is_unread(
                topic_count=forum.get("topic_count") or 0,
                last_post_id=forum.get("last_post"),
                last_post_created=forum.get("last_post_created"),
                forumsread=marks_by_forum.get(str(forum["id"])),
            )
            categories_by_id.get(str(forum["category"]), {"forums": []})["forums"].append(forum)
        return render_template("forum_index.html", categories=list(categories_by_id.values()), user=user_rows[0], logout_url=url_for("logout"))

    @app.get("/forum/<forum_id>")
    def forum_view(forum_id):
        client = aip_client()
        try:
            forum = client.read({"read": "Forum", "select": ["id", "category", "title", "description", "locked"], "filter": [{"field": "id", "op": "eq", "value": forum_id}], "limit": 1})
            topics = client.read({"read": "Topic", "select": ["id", "forum", "title", "username", "date_created", "last_updated", "locked", "important", "hidden", "views", "post_count"], "filter": [{"field": "forum", "op": "eq", "value": forum_id}], "sort": [{"field": "important", "dir": "desc"}, {"field": "last_updated", "dir": "desc"}, {"field": "id", "dir": "asc"}]})
        except AipError as error:
            reject_aip_error(error)
        if not forum:
            abort(404)
        identity = session_store.get(request.cookies.get("aip_session"), {})
        topic_marks = {}
        forum_mark = None
        try:
            user = current_aip_user(client) if identity else None
            if identity:
                topic_mark_rows = client.read({"read": "TopicRead", "select": ["topic", "last_read"], "filter": [{"field": "forum", "op": "eq", "value": forum_id}]})
                forum_mark_rows = client.read({"read": "ForumRead", "select": ["last_read", "cleared"], "filter": [{"field": "forum", "op": "eq", "value": forum_id}], "limit": 1})
                topic_marks = {str(row["topic"]): row["last_read"] for row in topic_mark_rows}
                forum_mark = forum_mark_rows[0] if forum_mark_rows else None
        except AipError as error:
            reject_aip_error(error)
        cleared = forum_mark.get("cleared") if forum_mark else None
        topics = [{**row, "unread": topic_is_unread(
            last_updated=row.get("last_updated"),
            topicsread_last_read=topic_marks.get(str(row["id"])),
            forumsread_cleared=cleared,
            authenticated=bool(identity),
        )} for row in topics]
        return render_template(
            "forum_view.html", forum=forum[0], topics=topics, logout_url=url_for("logout"),
            csrf_token=identity.get("csrf_token"),
            can_post_topic=can_contribute(client, user, {**forum[0], "forum_id": forum_id}, "posttopic"),
        )

    @app.post("/forum/<forum_id>/topic")
    def create_topic(forum_id):
        identity = session_store.get(request.cookies.get("aip_session"))
        if not identity:
            abort(401)
        require_csrf(identity)
        title = request.form.get("title", "").strip()
        content = request.form.get("content", "").strip()
        if not title or len(title) > 255 or not content:
            abort(400)
        client = aip_client()
        try:
            user = current_aip_user(client)
            topic_id = str(uuid.uuid4())
            post_id = str(uuid.uuid4())
            operations = [
                {"create": "Topic", "values": {"id": topic_id, "forum": forum_id, "title": title, "user": user["id"], "username": user["username"], "locked": False, "important": False, "views": 0, "published": False, "deleted": False, "hidden": False, "hidden_at": None, "hidden_by": None}},
                {"create": "Post", "values": {"id": post_id, "topic": topic_id, "forum": forum_id, "user": user["id"], "username": user["username"], "content": content, "hidden": False, "published": False, "version": 0, "deleted": False}},
                {"apply": "Post.publish", "target": {"ids": [post_id]}},
                {"create": "TopicFirstPost", "values": {"topic": topic_id, "post": post_id}},
                {"apply": "Topic.publish", "target": {"ids": [topic_id]}},
            ]
            return submit_bundle(client, operations, f"flaskbb-topic-{topic_id}", lambda: url_for("topic_view", topic_id=topic_id))
        except AipError as error:
            reject_aip_error(error)

    @app.post("/topic/<topic_id>/reply")
    def create_reply(topic_id):
        identity = session_store.get(request.cookies.get("aip_session"))
        if not identity:
            abort(401)
        require_csrf(identity)
        content = request.form.get("content", "").strip()
        if not content:
            abort(400)
        client = aip_client()
        try:
            user = current_aip_user(client)
            topics = client.read({"read": "Topic", "select": ["id", "forum"], "filter": [{"field": "id", "op": "eq", "value": topic_id}], "limit": 1})
            if not topics:
                abort(404)
            post_id = str(uuid.uuid4())
            operations = [
                {"create": "Post", "values": {"id": post_id, "topic": topic_id, "forum": topics[0]["forum"], "user": user["id"], "username": user["username"], "content": content, "hidden": False, "published": False, "version": 0, "deleted": False}},
                {"apply": "Post.publish", "target": {"ids": [post_id]}},
                {"apply": "Topic.bump", "target": {"ids": [topic_id]}},
            ]
            return submit_bundle(client, operations, f"flaskbb-reply-{post_id}", lambda: url_for("topic_view", topic_id=topic_id))
        except AipError as error:
            reject_aip_error(error)

    @app.get("/topic/<topic_id>")
    def topic_view(topic_id):
        client = aip_client()
        try:
            topics = client.read({"read": "Topic", "select": ["id", "forum", "title", "username", "date_created", "last_updated", "locked", "hidden"], "filter": [{"field": "id", "op": "eq", "value": topic_id}], "limit": 1})
            posts = client.read({"read": "Post", "select": ["id", "topic", "username", "content", "date_created", "date_modified", "hidden"], "filter": [{"field": "topic", "op": "eq", "value": topic_id}]})
        except AipError as error:
            reject_aip_error(error)
        if not topics:
            abort(404)
        identity = session_store.get(request.cookies.get("aip_session"), {})
        client = aip_client()
        try:
            user = current_aip_user(client) if identity else None
        except AipError as error:
            reject_aip_error(error)
        topic = {**topics[0], "forum_id": topics[0]["forum"]}
        return render_template(
            "topic_view.html", topic=topic, posts=posts, logout_url=url_for("logout"),
            csrf_token=identity.get("csrf_token"),
            can_reply=can_contribute(client, user, topic, "postreply"),
        )

    @app.get("/search")
    def search():
        q = request.args.get("q", "").strip()
        search_type = request.args.get("type", "all")
        topics, posts, users, forums = [], [], [], []
        if q:
            client = aip_client()
            try:
                if search_type in {"all", "topic"}:
                    topics = client.read({
                        "read": "Topic",
                        "select": ["id", "forum", "title", "username", "date_created"],
                        "filter": [{"field": "title", "op": "contains", "value": q}],
                        "limit": 50,
                    })
                if search_type in {"all", "post"}:
                    posts = client.read({
                        "read": "Post",
                        "select": ["id", "topic", "username", "content", "date_created"],
                        "filter": [{"field": "content", "op": "contains", "value": q}],
                        "limit": 50,
                    })
                if search_type in {"all", "user"}:
                    users = client.read({
                        "read": "User",
                        "select": ["id", "username"],
                        "filter": [{"field": "username", "op": "contains", "value": q}],
                        "limit": 50,
                    })
                if search_type in {"all", "forum"}:
                    forums = client.read({
                        "read": "Forum",
                        "select": ["id", "category", "title", "description"],
                        "filter": [{"field": "title", "op": "contains", "value": q}],
                        "limit": 50,
                    })
            except AipError as error:
                reject_aip_error(error)
        identity = session_store.get(request.cookies.get("aip_session"), {})
        return render_template(
            "search.html",
            query=q,
            search_type=search_type,
            topics=topics,
            posts=posts,
            users=users,
            forums=forums,
            logout_url=url_for("logout") if identity else None,
            csrf_token=identity.get("csrf_token"),
        )

    @app.get("/users")
    def member_list():
        client = aip_client()
        try:
            members = client.read({
                "read": "User",
                "select": ["id", "username", "primary_group", "post_count"],
                "limit": 100,
            })
        except AipError as error:
            reject_aip_error(error)
        identity = session_store.get(request.cookies.get("aip_session"), {})
        return render_template(
            "member_list.html",
            members=members,
            logout_url=url_for("logout") if identity else None,
            csrf_token=identity.get("csrf_token"),
        )

    @app.get("/user/<username>")
    def user_profile(username):
        client = aip_client()
        try:
            users = client.read({
                "read": "User",
                "select": ["id", "username", "primary_group", "activated", "date_joined", "location", "website", "signature", "avatar", "notes", "post_count"],
                "filter": [{"field": "username", "op": "prefix", "value": username}],
                "limit": 10,
            })
            profile_user = next((u for u in users if u.get("username") == username), None)
            if not profile_user:
                abort(404)
            topics = client.read({
                "read": "Topic",
                "select": ["id", "title", "date_created"],
                "filter": [{"field": "user", "op": "eq", "value": profile_user["id"]}],
                "limit": 10,
            })
            posts = client.read({
                "read": "Post",
                "select": ["id", "topic", "content", "date_created"],
                "filter": [{"field": "user", "op": "eq", "value": profile_user["id"]}],
                "limit": 10,
            })
        except AipError as error:
            reject_aip_error(error)
        identity = session_store.get(request.cookies.get("aip_session"), {})
        return render_template(
            "user_profile.html",
            profile_user=profile_user,
            topics=topics,
            posts=posts,
            logout_url=url_for("logout") if identity else None,
            csrf_token=identity.get("csrf_token"),
        )

    @app.get("/user/settings")
    def user_settings():
        client = aip_client()
        try:
            user = current_aip_user(client)
        except AipError as error:
            reject_aip_error(error)
        identity = session_store.get(request.cookies.get("aip_session"), {})
        return render_template(
            "user_settings.html",
            current_user=user,
            saved=request.args.get("saved") == "1",
            logout_url=url_for("logout"),
            csrf_token=identity.get("csrf_token"),
        )

    @app.post("/user/settings")
    def update_user_settings():
        identity = session_store.get(request.cookies.get("aip_session"))
        if not identity:
            abort(401)
        require_csrf(identity)
        client = aip_client()
        try:
            user = current_aip_user(client)
            patch = {}
            for field in ["signature", "avatar", "website", "location", "theme", "language"]:
                val = request.form.get(field, "").strip()
                if val:
                    patch[field] = val
            operations = [
                {
                    "apply": "User.updateProfile",
                    "target": {"ids": [user["id"]]},
                    "values": patch,
                }
            ]
            return submit_bundle(
                client,
                operations,
                f"flaskbb-profile-{user['id']}-{secrets.token_hex(4)}",
                lambda: url_for("user_settings", saved=1),
            )
        except AipError as error:
            reject_aip_error(error)

    @app.get("/conversations")
    def conversations_root():
        return redirect(url_for("conversations_inbox"))

    @app.get("/conversations/inbox")
    def conversations_inbox():
        client = aip_client()
        try:
            user = current_aip_user(client)
            conversations = client.read({
                "read": "Conversation",
                "select": ["id", "from_user", "to_user", "shared_id", "subject", "date_modified", "unread", "trash"],
                "filter": [
                    {"field": "trash", "op": "eq", "value": False},
                    {"field": "user", "op": "eq", "value": user["id"]},
                ],
                "limit": 50,
            })
        except AipError as error:
            reject_aip_error(error)
        identity = session_store.get(request.cookies.get("aip_session"), {})
        return render_template(
            "conversations.html",
            mode="list",
            folder="inbox",
            conversations=conversations,
            current_user=user,
            logout_url=url_for("logout"),
            csrf_token=identity.get("csrf_token"),
        )

    @app.get("/conversations/trash")
    def conversations_trash():
        client = aip_client()
        try:
            user = current_aip_user(client)
            conversations = client.read({
                "read": "Conversation",
                "select": ["id", "from_user", "to_user", "shared_id", "subject", "date_modified", "unread", "trash"],
                "filter": [
                    {"field": "trash", "op": "eq", "value": True},
                    {"field": "user", "op": "eq", "value": user["id"]},
                ],
                "limit": 50,
            })
        except AipError as error:
            reject_aip_error(error)
        identity = session_store.get(request.cookies.get("aip_session"), {})
        return render_template(
            "conversations.html",
            mode="list",
            folder="trash",
            conversations=conversations,
            current_user=user,
            logout_url=url_for("logout"),
            csrf_token=identity.get("csrf_token"),
        )

    @app.get("/conversations/new")
    def conversation_new():
        client = aip_client()
        try:
            user = current_aip_user(client)
        except AipError as error:
            reject_aip_error(error)
        identity = session_store.get(request.cookies.get("aip_session"), {})
        return render_template(
            "conversations.html",
            mode="new",
            folder="inbox",
            to_username=request.args.get("to", ""),
            current_user=user,
            logout_url=url_for("logout"),
            csrf_token=identity.get("csrf_token"),
        )

    @app.post("/conversations/new")
    def create_conversation():
        identity = session_store.get(request.cookies.get("aip_session"))
        if not identity:
            abort(401)
        require_csrf(identity)
        to_username = request.form.get("to_username", "").strip()
        subject = request.form.get("subject", "").strip()
        message_text = request.form.get("message", "").strip()
        if not to_username or not subject or not message_text:
            abort(400)
        client = aip_client()
        try:
            user = current_aip_user(client)
            recipients = client.read({
                "read": "User",
                "select": ["id", "username", "activated"],
                "filter": [{"field": "username", "op": "prefix", "value": to_username}],
                "limit": 10,
            })
            to_user = next((u for u in recipients if u.get("username") == to_username and u.get("activated")), None)
            if not to_user:
                abort(404)
            shared_id = str(uuid.uuid4())
            now_iso = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
            conv_ops = [
                {
                    "create": "Conversation",
                    "values": {
                        "user": user["id"],
                        "from_user": user["id"],
                        "to_user": to_user["id"],
                        "shared_id": shared_id,
                        "subject": subject,
                        "date_created": now_iso,
                        "date_modified": now_iso,
                        "trash": False,
                        "draft": False,
                        "unread": False,
                    },
                },
                {
                    "create": "Conversation",
                    "values": {
                        "user": to_user["id"],
                        "from_user": user["id"],
                        "to_user": to_user["id"],
                        "shared_id": shared_id,
                        "subject": subject,
                        "date_created": now_iso,
                        "date_modified": now_iso,
                        "trash": False,
                        "draft": False,
                        "unread": True,
                    },
                },
            ]
            conv_result = client.bundle(conv_ops, key=f"flaskbb-conv-{shared_id}")
            if not conv_result.get("ok"):
                reject_aip_error(AipError(conv_result.get("code", "WIRE.REJECTED"), conv_result.get("message", "Conversation creation rejected")))

            created_list = conv_result.get("created") or []
            sender_conv_id = created_list[0]["id"] if len(created_list) > 0 and isinstance(created_list[0], dict) else None
            recipient_conv_id = created_list[1]["id"] if len(created_list) > 1 and isinstance(created_list[1], dict) else None

            if not sender_conv_id:
                read_convs = client.read({
                    "read": "Conversation",
                    "select": ["id", "user"],
                    "filter": [{"field": "shared_id", "op": "eq", "value": shared_id}],
                })
                for c in read_convs:
                    if str(c.get("user")) == str(user["id"]):
                        sender_conv_id = c["id"]
                    elif str(c.get("user")) == str(to_user["id"]):
                        recipient_conv_id = c["id"]

            msg_ops = []
            if sender_conv_id:
                msg_ops.append({
                    "create": "Message",
                    "values": {
                        "conversation": sender_conv_id,
                        "user": user["id"],
                        "message": message_text,
                        "date_created": now_iso,
                    },
                })
            if recipient_conv_id:
                msg_ops.append({
                    "create": "Message",
                    "values": {
                        "conversation": recipient_conv_id,
                        "user": user["id"],
                        "message": message_text,
                        "date_created": now_iso,
                    },
                })
            if msg_ops:
                msg_result = client.bundle(msg_ops, key=f"flaskbb-msg-{shared_id}")
                if not msg_result.get("ok"):
                    reject_aip_error(AipError(msg_result.get("code", "WIRE.REJECTED"), msg_result.get("message", "Message creation rejected")))

            return redirect(url_for("conversations_inbox"), code=303)
        except AipError as error:
            reject_aip_error(error)

    @app.get("/conversations/<int:conversation_id>")
    def conversation_view(conversation_id):
        client = aip_client()
        try:
            user = current_aip_user(client)
            convs = client.read({
                "read": "Conversation",
                "select": ["id", "user", "from_user", "to_user", "shared_id", "subject", "date_created", "date_modified", "unread", "trash"],
                "filter": [{"field": "id", "op": "eq", "value": str(conversation_id)}],
                "limit": 1,
            })
            if not convs:
                abort(404)
            conv = convs[0]
            if str(conv["user"]) != str(user["id"]):
                abort(403)
            messages = client.read({
                "read": "Message",
                "select": ["id", "conversation", "user", "message", "date_created"],
                "filter": [{"field": "conversation", "op": "eq", "value": str(conversation_id)}],
            })
            if conv.get("unread"):
                try:
                    client.apply({"apply": "Conversation.markRead", "target": {"ids": [str(conversation_id)]}})
                except AipError:
                    pass
        except AipError as error:
            reject_aip_error(error)
        identity = session_store.get(request.cookies.get("aip_session"), {})
        return render_template(
            "conversations.html",
            mode="view",
            folder="trash" if conv.get("trash") else "inbox",
            conversation=conv,
            messages=messages,
            current_user=user,
            logout_url=url_for("logout"),
            csrf_token=identity.get("csrf_token"),
        )

    @app.post("/conversations/<int:conversation_id>/reply")
    def conversation_reply(conversation_id):
        identity = session_store.get(request.cookies.get("aip_session"))
        if not identity:
            abort(401)
        require_csrf(identity)
        message_text = request.form.get("message", "").strip()
        if not message_text:
            abort(400)
        client = aip_client()
        try:
            user = current_aip_user(client)
            convs = client.read({
                "read": "Conversation",
                "select": ["id", "user", "shared_id"],
                "filter": [{"field": "id", "op": "eq", "value": str(conversation_id)}],
                "limit": 1,
            })
            if not convs or str(convs[0]["user"]) != str(user["id"]):
                abort(404)
            current_conv = convs[0]
            shared_id = current_conv.get("shared_id")
            now_iso = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())

            other_conv_id = None
            if shared_id:
                matching_convs = client.read({
                    "read": "Conversation",
                    "select": ["id", "user", "shared_id"],
                    "filter": [{"field": "shared_id", "op": "eq", "value": shared_id}],
                })
                for c in matching_convs:
                    if str(c.get("user")) != str(user["id"]):
                        other_conv_id = c["id"]
                        break

            operations = [
                {
                    "create": "Message",
                    "values": {
                        "conversation": str(conversation_id),
                        "user": user["id"],
                        "message": message_text,
                        "date_created": now_iso,
                    },
                }
            ]
            if other_conv_id:
                operations.append({
                    "create": "Message",
                    "values": {
                        "conversation": other_conv_id,
                        "user": user["id"],
                        "message": message_text,
                        "date_created": now_iso,
                    },
                })
                operations.append({
                    "apply": "Conversation.bump",
                    "target": {"ids": [other_conv_id]},
                })

            return submit_bundle(
                client,
                operations,
                f"flaskbb-msg-{conversation_id}-{secrets.token_hex(4)}",
                lambda: url_for("conversation_view", conversation_id=conversation_id),
            )
        except AipError as error:
            reject_aip_error(error)

    @app.post("/conversations/<int:conversation_id>/trash")
    def conversation_trash(conversation_id):
        identity = session_store.get(request.cookies.get("aip_session"))
        if not identity:
            abort(401)
        require_csrf(identity)
        client = aip_client()
        try:
            operations = [
                {"apply": "Conversation.moveToTrash", "target": {"ids": [str(conversation_id)]}}
            ]
            return submit_bundle(
                client,
                operations,
                f"flaskbb-trash-{conversation_id}-{secrets.token_hex(4)}",
                lambda: url_for("conversations_inbox"),
            )
        except AipError as error:
            reject_aip_error(error)

    @app.post("/conversations/<int:conversation_id>/restore")
    def conversation_restore(conversation_id):
        identity = session_store.get(request.cookies.get("aip_session"))
        if not identity:
            abort(401)
        require_csrf(identity)
        client = aip_client()
        try:
            operations = [
                {"apply": "Conversation.restoreFromTrash", "target": {"ids": [str(conversation_id)]}}
            ]
            return submit_bundle(
                client,
                operations,
                f"flaskbb-restore-{conversation_id}-{secrets.token_hex(4)}",
                lambda: url_for("conversations_trash"),
            )
        except AipError as error:
            reject_aip_error(error)

    @app.errorhandler(401)
    def unauthenticated(_error):
        return render_template("error.html", title="Sign in required", message="The identity provider session is not connected to an AIP actor."), 401

    @app.errorhandler(403)
    def forbidden(_error):
        return render_template("error.html", title="Account unavailable", message="The account is not activated in the AIP user record."), 403

    @app.errorhandler(503)
    def aip_unavailable(_error):
        return render_template("error.html", title="Forum temporarily unavailable", message="The AIP service rejected or could not complete this request."), 503

    return app


if __name__ == "__main__":
    create_app().run(host=os.environ.get("FLASKBB_UI_HOST", "127.0.0.1"), port=int(os.environ.get("FLASKBB_UI_PORT", "58082")), debug=False)
