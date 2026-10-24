"""Small allowlist-only summaries for the live FaceReview browser journey."""

import hashlib
import json
import math
import re
from typing import Any
from urllib.parse import urlsplit


LIST_EXTENSIONS = {
    "Video.homeSearch",
    "Video.homeVideoAll",
    "Video.homeCategory",
    "Video.homePersonalized",
}
EMOTIONS = {"all", "happy", "sad", "neutral", "surprise", "angry"}
READ_FIELDS = {
    "Video": {"id", "youtube_url", "title", "channel_name", "category", "duration", "view_count", "is_deleted"},
    "VideoBookmark": {"id", "video", "user", "created_at"},
    "VideoLike": {"id", "video", "user", "created_at"},
    "Comment": {"id", "video", "user", "content", "is_modified", "is_deleted", "created_at", "updated_at"},
    "User": {
        "id", "email", "name", "role", "profile_image_id", "is_tutorial_done",
        "is_verify_email_done", "is_deleted", "created_at",
    },
    "UserFavoriteGenre": {"id", "user", "genre", "created_at", "updated_at"},
}
READ_RELATIONS = {"Comment": {"user": {"id", "name"}}}
SAFE_FAILURE_TYPES = {"AssertionError", "Error", "TimeoutError", "TargetClosedError", "RuntimeError"}
SAFE_BOOKMARK_LABELS = {"즐겨찾기 추가", "즐겨찾기 해제"}
READ_QUERY_KEYS = {"read", "select", "filter", "sort", "limit", "offset", "after"}
READ_QUERY_DIAGNOSTIC_KEYS = READ_QUERY_KEYS | {"traverse"}
READ_ENVELOPE_KEYS = {"query", "contractFingerprint"}
SAFE_WIRE_CODES = {
    "WIRE.UNKNOWN_KEY",
    "WIRE.CONTRACT_MISMATCH",
    "WIRE.UNAUTHENTICATED",
    "WIRE.TOKEN_EXPIRED",
    "WIRE.BAD_VALUE",
    "WIRE.NOT_EXPOSED",
    "WIRE.ACCESS_DENIED",
    "WIRE.FORBIDDEN",
    "WIRE.FIELD_NOT_EXPOSED",
    "WIRE.UNKNOWN_FIELD",
    "WIRE.READ_ONLY",
    "WIRE.INVALID_QUERY",
    "WIRE.OUTPUT_INVALID",
    "WIRE.INVALID_STATE",
    "WIRE.MISSING_TARGET",
}


SAFE_NETWORK_FAILURES = {
    "net::ERR_ABORTED",
    "net::ERR_BLOCKED_BY_CLIENT",
    "net::ERR_CONNECTION_CLOSED",
    "net::ERR_CONNECTION_REFUSED",
    "net::ERR_FAILED",
    "net::ERR_NAME_NOT_RESOLVED",
    "net::ERR_TIMED_OUT",
}
FILTER_OPS = {"eq", "ne", "lt", "lte", "gt", "gte", "in", "contains", "like"}


def member_password_evidence(requests: list[dict[str, object]], ui: dict[str, object]) -> dict[str, object]:
    gates = {}
    for path, method in [("/auth/email/verification", "POST"), ("/auth/password/verification", "POST"),
                         ("/auth/password", "PATCH"), ("/auth/logout", "POST")]:
        rows = [row for row in requests if row.get("path") == path]
        gates[path] = (len(rows) == 1 and rows[0].get("method") == method and rows[0].get("status") == 200
                       and all(rows[0].get(key) is True for key in ("ok", "body_captured", "payload_matches"))
                       and rows[0].get("refresh_cookie_present" if path == "/auth/logout" else "authorization_present") is True
                       and (path != "/auth/password/verification" or rows[0].get("reset_token_received") is True))
    logins = [row for row in requests if row.get("path") == "/auth/login"]
    gates["login_old_and_new"] = (len(logins) == 2 and [row.get("password_kind") for row in logins] == ["old", "new"]
                                  and all(row.get("status") == 200 and row.get("method") == "POST"
                                          and all(row.get(key) is True for key in ("ok", "body_captured", "payload_matches")) for row in logins))
    gates["ui"] = all(ui.get(key) is True for key in ("password_page", "code_form", "new_password_form", "signed_out", "new_login", "reloaded_profile"))
    return {"gates": gates, "verified": all(gates.values())}


def member_comment_action(payload: object) -> str | None:
    """Recognize one member comment write without retaining IDs or content."""
    request = payload.get("request") if isinstance(payload, dict) else None
    if not isinstance(request, dict):
        return None
    atomic = request.get("atomic")
    if set(request) == {"atomic"} and isinstance(atomic, list) and len(atomic) == 1:
        op = atomic[0]
        if isinstance(op, dict) and set(op) == {"create", "values"} and op.get("create") == "Comment":
            values = op["values"]
            if isinstance(values, dict) and set(values) == {"video", "user", "content"} and all(isinstance(value, str) and value for value in values.values()):
                return "create"
        return None
    action = {"Comment.edit": "edit", "Comment.delete": "delete"}.get(request.get("apply")) if isinstance(request.get("apply"), str) else None
    target = request.get("target")
    if not action or not isinstance(target, dict) or set(target) != {"ids"}:
        return None
    ids = target["ids"]
    if not isinstance(ids, list) or len(ids) != 1 or not isinstance(ids[0], str) or not ids[0]:
        return None
    if action == "delete" and set(request) == {"apply", "target"}:
        return action
    values = request.get("values")
    if action == "edit" and set(request) == {"apply", "target", "values"} and isinstance(values, dict) and set(values) == {"content"} and isinstance(values["content"], str) and values["content"]:
        return action
    return None


def member_comment_evidence(requests: list[dict[str, object]], action: str, reloaded_render_matches: bool) -> dict[str, object]:
    comment_replies = [row for row in requests if row.get("path") == "/apply" and row.get("comment_action") in {"create", "edit", "delete"}]
    replies = [row for row in comment_replies if row.get("comment_action") == action]
    accepted = (action in {"create", "edit", "delete"} and len(comment_replies) == 1 and len(replies) == 1
                and replies[0].get("status") == 200 and replies[0].get("body_captured") is True
                and replies[0].get("ok") is True and replies[0].get("comment_target_matches") is True
                and replies[0].get("comment_content_matches") is True)
    return {"action": action if action in {"create", "edit", "delete"} else "unrecognized",
            "mutation_response_count": len(replies), "server_accepted": accepted,
            "reloaded_render_matches": reloaded_render_matches is True,
            "verified": accepted and reloaded_render_matches is True}


def member_like_action(payload: object) -> str | None:
    """Classify only the SDK's exact like operation; never retain its IDs or values."""
    request = payload.get("request") if isinstance(payload, dict) else None
    if not isinstance(request, dict):
        return None
    if request.get("apply") == "VideoLike.remove":
        return "remove"
    atomic = request.get("atomic")
    if isinstance(atomic, list) and len(atomic) == 1 and isinstance(atomic[0], dict) and atomic[0].get("create") == "VideoLike":
        return "add"
    return None


def member_like_evidence(requests: list[dict[str, object]], action: str, reloaded_active: bool) -> dict[str, object]:
    replies = [row for row in requests if row.get("path") == "/apply" and row.get("like_action") == action]
    accepted = (len(replies) == 1 and replies[0].get("status") == 200
                and replies[0].get("body_captured") is True and replies[0].get("ok") is True)
    persisted_ui = action in {"add", "remove"} and reloaded_active is (action == "add")
    return {"action": action if action in {"add", "remove"} else "unrecognized",
            "mutation_response_count": len(replies), "server_accepted": accepted,
            "reloaded_active": reloaded_active, "verified": accepted and persisted_ui}


def member_logout_evidence(requests: list[dict[str, object]], state: dict[str, object]) -> dict[str, object]:
    """A local clear alone cannot prove that the server accepted logout."""
    replies = [row for row in requests if row.get("path") == "/auth/logout"]
    accepted = (
        len(replies) == 1
        and replies[0].get("status") == 200
        and replies[0].get("body_captured") is True
        and replies[0].get("ok") is True
    )
    cleared = {key: state.get(key) is True for key in ("signed_out", "access_token_cleared", "user_id_cleared")}
    return {"logout_response_count": len(replies), "server_accepted": accepted,
            **cleared, "verified": accepted and all(cleared.values())}


def fixture_thumbnail_redirect_url(
    url: str, resource_type: str, fixture_youtube_id: str, local_thumbnail_url: str
) -> str | None:
    """Route only the fixture's availability-check image to its local static image."""
    request = urlsplit(url)
    local = urlsplit(local_thumbnail_url)
    if (
        resource_type == "image"
        and request.scheme == "https"
        and request.hostname == "img.youtube.com"
        and request.path == f"/vi/{fixture_youtube_id}/mqdefault.jpg"
        and not request.query
        and local.scheme == "https"
        and local.hostname == "127.0.0.1"
        and local.port is not None
        and local.path == "/fixture-thumbnail.svg"
    ):
        return local_thumbnail_url
    return None


def youtube_iframe_api_stub(url: str, resource_type: str) -> str | None:
    """Return a inert loader response only for YouTube's API script; never fetch or play media."""
    parsed = urlsplit(url)
    if resource_type != "script" or parsed.hostname not in {"youtube.com", "www.youtube.com"} or parsed.path != "/iframe_api":
        return None
    return "/* Player playback is outside this browser journey; keep the provider API unavailable. */"


def response_envelope_evidence(payload: Any) -> dict[str, object]:
    """Summarize only top-level response shape; never retain response values or unknown key names."""
    if isinstance(payload, dict):
        known_keys = sorted(key for key in payload if key in {"ok", "code", "msg", "rows", "deps", "maxAgeMs"})
        if "ok" not in payload:
            ok_type = "missing"
        else:
            ok = payload["ok"]
            ok_type = "bool" if isinstance(ok, bool) else type(ok).__name__
        summary: dict[str, object] = {
            "json_kind": "object",
            "known_keys": known_keys,
            "unknown_key_count": len(payload) - len(known_keys),
            "ok_type": ok_type,
        }
        rows = payload.get("rows")
        if isinstance(rows, list):
            summary["rows_length"] = len(rows)
        code = safe_wire_code(payload.get("code"))
        if code is not None:
            summary["code"] = code
        return summary
    if isinstance(payload, list):
        return {"json_kind": "array", "array_length": len(payload)}
    return {"json_kind": "null" if payload is None else type(payload).__name__}


SAFE_MEDIA_TYPES = {"application/json", "text/html", "text/plain"}
SAFE_APP_ERROR_CLASSES = {"TypeError", "AbortError", "SyntaxError", "NetworkError", "DOMException"}
SAFE_APP_PHASES = {"received", "fetch_rejected", "clone_read_failed"}
SAFE_APP_PARSES = {"object", "array", "json_decode_error", "other"}
SAFE_AUTH_FETCH_FRAMES = {"fetchWithRefresh", "connectTypedExtensions", "typedClient", "postWithToken", "send", "fetch"}
SAFE_AUTH_PATHS = {"/auth/check-email", "/auth/login", "/auth/reissue", "/session", "/read", "/apply", "/extension"}


def body_failure_reason(error: Any) -> str:
    """Map a Playwright body-read failure to a reason code; the exception text is never retained."""
    name = type(error).__name__
    message = str(error)
    if name == "TargetClosedError" or "has been closed" in message:
        return "target_closed"
    if name == "TimeoutError":
        return "timeout"
    if name == "Error":
        if "No resource with given identifier" in message or "No data found for resource" in message:
            return "resource_evicted"
        return "playwright_error"
    return "unrecognized"


def _media_type(content_type: Any) -> str:
    if not isinstance(content_type, str) or not content_type.strip():
        return "missing"
    media = content_type.split(";", 1)[0].strip().lower()
    return media if media in SAFE_MEDIA_TYPES else "other"


def auth_transport_trace_evidence(observations: Any) -> dict[str, object]:
    """Keep only auth-branch inputs and a bounded call path; never retain headers or token values."""
    if not isinstance(observations, list):
        observations = []
    rows: list[dict[str, object]] = []
    for item in observations:
        if not isinstance(item, dict):
            continue
        path = item.get("path")
        if not isinstance(path, str) or not path.startswith("/") or "?" in path:
            continue
        safe_path = path if path in SAFE_AUTH_PATHS else "other"
        code = safe_wire_code(item.get("code"))
        content_type = _media_type(item.get("content_type"))
        frames = item.get("fetch_frames")
        safe_frames = [frame for frame in frames if frame in SAFE_AUTH_FETCH_FRAMES] if isinstance(frames, list) else []
        rows.append({
            "method": item.get("method") if item.get("method") in {"GET", "POST", "PUT", "PATCH", "DELETE"} else "other",
            "path": safe_path,
            "status": item.get("status") if isinstance(item.get("status"), int) and 0 <= item["status"] <= 599 else None,
            "authorization_header_present": item.get("authorization_header_present") is True,
            "content_type": content_type,
            "body_captured": item.get("body_captured") is True,
            "envelope_ok": item.get("ok") if isinstance(item.get("ok"), bool) else None,
            "envelope_shape": "error" if item.get("ok") is False and code is not None else "success" if item.get("ok") is True else "other",
            "code": code,
            "fetch_frames": safe_frames,
            "request_started_ms": item.get("request_started_ms") if isinstance(item.get("request_started_ms"), (int, float)) else None,
            "response_headers_ms": item.get("response_headers_ms") if isinstance(item.get("response_headers_ms"), (int, float)) else None,
            "response_body_complete_ms": item.get("response_body_complete_ms") if isinstance(item.get("response_body_complete_ms"), (int, float)) else None,
            "request_finished_ms": item.get("request_finished_ms") if isinstance(item.get("request_finished_ms"), (int, float)) else None,
            "network_idle_completed_ms": item.get("network_idle_completed_ms") if isinstance(item.get("network_idle_completed_ms"), (int, float)) else None,
            "body_completed_before_network_idle": (
                item.get("request_finished_ms") <= item.get("network_idle_completed_ms")
                if isinstance(item.get("request_finished_ms"), (int, float))
                and isinstance(item.get("network_idle_completed_ms"), (int, float)) else None
            ),
        })
    expired = [row for row in rows if row["code"] == "WIRE.TOKEN_EXPIRED"]
    branch_inputs = [
        row for row in expired
        if row["authorization_header_present"] is True
        and row["content_type"] == "application/json"
        and row["envelope_ok"] is False
        and "fetchWithRefresh" in row["fetch_frames"]
        and row["body_captured"] is True
        and row["body_completed_before_network_idle"] is True
    ]
    return {
        "requests": rows,
        "expired_refusal_count": len(expired),
        "reauth_branch_inputs_observed": len(branch_inputs),
        "reissue_request_count": sum(1 for row in rows if row["path"] == "/auth/reissue"),
    }


def _first_byte_class(raw: bytes) -> str:
    head = raw.lstrip()[:1]
    if not raw:
        return "empty"
    if head == b"{":
        return "json_object"
    if head == b"[":
        return "json_array"
    if head == b"<":
        return "html"
    return "other"


def body_capture_evidence(raw: bytes | None, content_type: Any, failure_reason: str | None) -> dict[str, object]:
    """Separate a body Playwright could not read from a body that was read but is not JSON.

    The body is parsed in memory only; what survives is length, a truncated hash and the allowlisted shape.
    """
    media = _media_type(content_type)
    if raw is None:
        reason = failure_reason or "unrecognized"
        return {
            "body_captured": False,
            "content_type": media,
            "failure_reason": reason,
            "response_shape": {"json_kind": "body_unavailable", "reason": reason},
        }
    evidence: dict[str, object] = {
        "body_captured": True,
        "content_type": media,
        "byte_length": len(raw),
        "sha256_16": hashlib.sha256(raw).hexdigest()[:16],
    }
    try:
        shape = response_envelope_evidence(json.loads(raw.decode("utf-8")))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        shape = {
            "json_kind": "json_decode_error",
            "decode_error_class": type(error).__name__,
            "first_byte_class": _first_byte_class(raw),
        }
    evidence["response_shape"] = shape
    return evidence


def app_bookmark_read_observation(event: Any) -> dict[str, object] | None:
    """Keep the allowlisted fields of what the React app itself received for the bookmark /read."""
    if not isinstance(event, dict) or not isinstance(event.get("seq"), int) or isinstance(event.get("seq"), bool):
        return None
    phase = event.get("phase")
    kept: dict[str, object] = {"seq": event["seq"], "phase": phase if phase in SAFE_APP_PHASES else "unrecognized"}
    for key in ("started_ms", "response_ms", "ended_ms", "status", "byte_length", "rows_length"):
        value = event.get(key)
        if isinstance(value, int) and not isinstance(value, bool):
            kept[key] = value
    if isinstance(event.get("ok"), bool):
        kept["ok"] = event["ok"]
    if "content_type" in event:
        kept["content_type"] = _media_type(event.get("content_type"))
    digest = event.get("sha256_16")
    if isinstance(digest, str) and len(digest) == 16 and all(char in "0123456789abcdef" for char in digest):
        kept["sha256_16"] = digest
    parse = event.get("parse")
    if parse is not None:
        kept["parse"] = parse if parse in SAFE_APP_PARSES else "unrecognized"
    if "error_class" in event:
        error_class = event.get("error_class")
        kept["error_class"] = error_class if error_class in SAFE_APP_ERROR_CLASSES else "unrecognized"
    return kept


def compare_bookmark_read_capture(captured: dict[str, object], app: dict[str, object] | None) -> str:
    """Compare Playwright's body capture with the app's own receipt; only equal bytes count as a match."""
    if app is None:
        return "app_observation_missing"
    if app.get("phase") != "received" or "sha256_16" not in app:
        return "app_observation_incomplete"
    if captured.get("body_captured") is not True:
        return "playwright_body_unavailable"
    same = captured.get("sha256_16") == app.get("sha256_16") and captured.get("byte_length") == app.get("byte_length")
    return "match" if same else "mismatch"


def timeline_phase(at_ms: Any, marks: dict[str, int]) -> str:
    """Place a timestamp relative to the recorded reload and browser-close marks."""
    if not isinstance(at_ms, int) or isinstance(at_ms, bool):
        return "not_observed"
    close_start = marks.get("browser_close_start")
    if close_start is not None and at_ms >= close_start:
        return "after_close_start"
    reload_start = marks.get("reload_start")
    if reload_start is None or at_ms < reload_start:
        return "before_reload" if reload_start is not None else "before_any_mark"
    reload_return = marks.get("reload_return")
    if reload_return is None or at_ms <= reload_return:
        return "during_reload"
    return "after_reload"


def read_request_evidence(body: Any) -> dict[str, object]:
    """Summarize an allowlisted read query's structure without recording filter values."""
    if not isinstance(body, dict):
        return {
            "operation": "unrecognized",
            "read_resource": "unrecognized",
            "request_envelope_keys": [],
            "unknown_envelope_key_count": 0,
            "recognized_query_keys": [],
            "rejected_query_keys": [],
            "unknown_query_key_count": 0,
        }

    wrapped_query = body.get("query")
    if isinstance(wrapped_query, dict):
        query = wrapped_query
        envelope_keys = sorted(key for key in body if key in READ_ENVELOPE_KEYS)
        unknown_envelope_key_count = sum(key not in READ_ENVELOPE_KEYS for key in body)
    else:
        query = body
        envelope_keys = []
        unknown_envelope_key_count = 0

    resource = query.get("read")
    fields = READ_FIELDS.get(resource) if isinstance(resource, str) else None
    operation = "read" if fields is not None else "unrecognized"
    rejected_query_keys = sorted(
        key for key in query if key in READ_QUERY_DIAGNOSTIC_KEYS and key not in READ_QUERY_KEYS
    )
    summary: dict[str, object] = {
        "operation": operation,
        "read_resource": resource if fields is not None else "unrecognized",
        "request_envelope_keys": envelope_keys,
        "unknown_envelope_key_count": unknown_envelope_key_count,
        "recognized_query_keys": sorted(key for key in query if key in READ_QUERY_KEYS),
        "rejected_query_keys": rejected_query_keys,
        "unknown_query_key_count": sum(key not in READ_QUERY_KEYS for key in query),
    }
    select = query.get("select")
    if isinstance(select, list):
        safe_fields = [field for field in select if isinstance(field, str) and fields and field in fields]
        summary["select_fields"] = safe_fields
        relation_summaries: list[dict[str, object]] = []
        unknown_select_field_count = sum(
            1 for field in select if isinstance(field, str) and (not fields or field not in fields)
        )
        unknown_select_field_count += sum(1 for field in select if not isinstance(field, (str, dict)))
        relations = READ_RELATIONS.get(resource, {})
        for item in select:
            if not isinstance(item, dict):
                continue
            relation, nested = next(iter(item.items()), (None, None))
            relation_fields = relations.get(relation) if isinstance(relation, str) else None
            nested_fields = nested.get("select") if isinstance(nested, dict) else None
            if relation_fields is None or not isinstance(nested_fields, list):
                unknown_select_field_count += 1
                continue
            safe_nested = [field for field in nested_fields if isinstance(field, str) and field in relation_fields]
            unknown_select_field_count += len(nested_fields) - len(safe_nested)
            relation_summaries.append({"relation": relation, "fields": safe_nested})
        if relation_summaries:
            summary["select_relations"] = relation_summaries
        summary["unknown_select_field_count"] = unknown_select_field_count

    filters = query.get("filter")
    if isinstance(filters, list):
        safe_filters: list[dict[str, str]] = []
        unknown_filter_key_count = 0
        for item in filters:
            if not isinstance(item, dict):
                unknown_filter_key_count += 1
                continue
            unknown_filter_key_count += sum(key not in {"field", "op", "value"} for key in item)
            field = item.get("field")
            op = item.get("op")
            if isinstance(field, str) and fields and field in fields:
                safe_filters.append({
                    "field": field,
                    "op": op if isinstance(op, str) and op in FILTER_OPS else "unrecognized",
                    "value_type": type(item.get("value")).__name__,
                })
            else:
                unknown_filter_key_count += 1
        summary["filter_fields"] = safe_filters
        summary["unknown_filter_key_count"] = unknown_filter_key_count

    limit = query.get("limit")
    if isinstance(limit, int) and not isinstance(limit, bool) and 0 <= limit <= 100000:
        summary["limit"] = limit
    return summary


def safe_wire_code(value: Any) -> str | None:
    return value if isinstance(value, str) and value in SAFE_WIRE_CODES else None


def safe_browser_failure_type(value: Any) -> str:
    name = type(value).__name__
    return name if name in SAFE_FAILURE_TYPES else "unrecognized"


def safe_browser_route_class(url: Any, fixture_video_id: str) -> str:
    if not isinstance(url, str):
        return "unrecognized"
    path = urlsplit(url).path
    if path == "/main":
        return "main"
    if path in {"/auth/1", "/auth/2"}:
        return "auth"
    if path == f"/watch/{fixture_video_id}":
        return "watch_fixture"
    if path.startswith("/watch/"):
        return "watch_other"
    return "other"


def profile_render_evidence(snapshot: Any) -> dict[str, object]:
    """Keep only route, layout and animation measurements needed to diagnose the profile screenshot."""
    if not isinstance(snapshot, dict):
        return {}
    route = snapshot.get("path")
    route_class = route.split("?", 1)[0].removesuffix("/") if isinstance(route, str) else ""
    route_class = route_class.removeprefix("/")
    if route_class not in {"my", "edit", "main", "auth/1", "auth/2"}:
        route_class = "other"

    def dimension(value: Any) -> int | None:
        if not isinstance(value, (int, float)) or isinstance(value, bool) or not math.isfinite(value):
            return None
        return min(10000, max(0, math.floor(value + 0.5)))

    def opacity(value: Any) -> float | None:
        return round(value, 4) if isinstance(value, (int, float)) and not isinstance(value, bool) and 0 <= value <= 1 else None

    nodes: list[dict[str, object]] = []
    raw_nodes = snapshot.get("nodes")
    if isinstance(raw_nodes, dict):
        for name in ("my-page-container", "my-page-user-container", "my-page-watched-contents-container", "my-page-emotion-container"):
            raw = raw_nodes.get(name)
            if not isinstance(raw, dict):
                continue
            display = raw.get("display")
            visibility = raw.get("visibility")
            own_opacity = opacity(raw.get("opacity"))
            effective_opacity = opacity(raw.get("effective_opacity"))
            animation_name = raw.get("animation_name")
            animation_fill_mode = raw.get("animation_fill_mode")
            animation_play_state = raw.get("animation_play_state")
            transition_properties = raw.get("transition_properties")
            rect = raw.get("rect") if isinstance(raw.get("rect"), dict) else {}
            width = dimension(rect.get("width"))
            height = dimension(rect.get("height"))
            nodes.append({
                "node": name,
                "display": display if display in {"block", "flex", "grid", "inline", "inline-flex", "inline-block", "none"} else "other",
                "visibility": visibility if visibility in {"visible", "hidden", "collapse"} else "other",
                "opacity": own_opacity,
                "effective_opacity": effective_opacity,
                "transform": raw.get("transform") if raw.get("transform") in {"none", "applied"} else "other",
                "animation_name": animation_name if animation_name in {"none", "fadeInUp", "spin", "pulse"} else "other",
                "animation_duration_ms": dimension(raw.get("animation_duration_ms")),
                "animation_delay_ms": dimension(raw.get("animation_delay_ms")),
                "animation_fill_mode": animation_fill_mode if animation_fill_mode in {"none", "forwards", "backwards", "both"} else "other",
                "animation_play_state": animation_play_state if animation_play_state in {"idle", "running", "paused", "finished"} else "other",
                "transition_properties": sorted({item for item in transition_properties if item in {"opacity", "transform", "visibility"}}) if isinstance(transition_properties, list) else [],
                "transition_duration_ms": dimension(raw.get("transition_duration_ms")),
                "rect": {"width": width, "height": height},
                "visually_ready": (
                    display != "none" and visibility == "visible" and effective_opacity is not None
                    and effective_opacity >= 0.99 and width is not None and width > 0
                    and height is not None and height > 0
                ),
            })

    animations: list[dict[str, object]] = []
    raw_animations = snapshot.get("animations")
    if isinstance(raw_animations, list):
        for raw in raw_animations[:20]:
            if not isinstance(raw, dict):
                continue
            name = raw.get("name")
            play_state = raw.get("play_state")
            properties = raw.get("properties")
            animations.append({
                "name": name if name in {"fadeInUp", "spin", "pulse"} else "other",
                "play_state": play_state if play_state in {"idle", "running", "paused", "finished"} else "other",
                "current_ms": dimension(raw.get("current_ms")),
                "duration_ms": dimension(raw.get("duration_ms")),
                "delay_ms": dimension(raw.get("delay_ms")),
                "properties": sorted({item for item in properties if item in {"opacity", "transform", "visibility"}}) if isinstance(properties, list) else [],
            })

    viewport = snapshot.get("viewport") if isinstance(snapshot.get("viewport"), dict) else {}
    body_text_length = dimension(snapshot.get("body_text_length"))
    return {
        "route": route_class,
        "api_bodies_settled": snapshot.get("api_bodies_settled") is True,
        "body_text_length": body_text_length,
        "viewport": {"width": dimension(viewport.get("width")), "height": dimension(viewport.get("height"))},
        "nodes": nodes,
        "animations": animations,
    }


def profile_content_animation_incomplete(evidence: Any) -> bool:
    """Gate the screenshot on the profile's own fade animation, not unrelated page motion."""
    animations = evidence.get("animations") if isinstance(evidence, dict) else None
    if not isinstance(animations, list):
        return False
    return any(
        isinstance(animation, dict)
        and animation.get("name") == "fadeInUp"
        and animation.get("play_state") in {"running", "paused"}
        for animation in animations
    )


def safe_bookmark_control_evidence(value: Any) -> list[dict[str, str]]:
    if not isinstance(value, list):
        return []
    result: list[dict[str, str]] = []
    for item in value[:10]:
        if not isinstance(item, dict):
            continue
        label = item.get("label")
        pressed = item.get("pressed")
        result.append({
            "label": label if isinstance(label, str) and label in SAFE_BOOKMARK_LABELS else "unrecognized",
            "pressed": pressed if isinstance(pressed, str) and pressed in {"true", "false"} else "unrecognized",
        })
    return result


def failed_read_requests(api_requests: list[dict[str, object]]) -> list[dict[str, object]]:
    """Select already-sanitized non-successful read rows; HTTP status alone is not success."""
    return [row for row in api_requests if row.get("path") == "/read" and row.get("ok") is not True]


def safe_browser_caller_frames(frames: Any) -> list[dict[str, object]]:
    """Keep only bounded source locations from the pinned local React dev server."""
    if not isinstance(frames, list):
        return []
    result: list[dict[str, object]] = []
    for frame in frames:
        if not isinstance(frame, dict) or not isinstance(frame.get("url"), str):
            continue
        parsed = urlsplit(frame["url"])
        path = parsed.path
        line = frame.get("lineNumber")
        column = frame.get("columnNumber")
        if (parsed.hostname not in {"localhost", "127.0.0.1"} or parsed.port != 5173
                or not path.startswith("/src/") or not isinstance(line, int) or line < 0
                or not isinstance(column, int) or column < 0):
            continue
        result.append({"path": path[:240], "line": min(line, 100000), "column": min(column, 100000)})
        if len(result) >= 4:
            break
    return result


def console_error_evidence(message_type: Any, message_text: Any, location: Any) -> dict[str, object]:
    """Classify a console error without storing its raw text or arbitrary source URL."""
    safe_type = message_type if message_type in {"error", "warning", "log", "debug", "info"} else "unrecognized"
    text = message_text if isinstance(message_text, str) else ""
    wire_code = next((code for code in SAFE_WIRE_CODES if code in text), None)
    if wire_code is not None:
        classification = "wire_response_error"
    elif "Failed to load resource" in text or "net::ERR_" in text:
        classification = "network_resource_error"
    elif "ResizeObserver loop" in text:
        classification = "resize_observer_error"
    else:
        classification = "opaque_console_error"

    source = "unmapped"
    if isinstance(location, dict) and isinstance(location.get("url"), str):
        parsed = urlsplit(location["url"])
        path = parsed.path
        line = location.get("lineNumber")
        column = location.get("columnNumber")
        if parsed.hostname in {"localhost", "127.0.0.1"} and parsed.port == 5173 and path.startswith("/src/"):
            source = path[:240]
            if isinstance(line, int) and 0 < line <= 100000:
                source += f":{line}"
                if isinstance(column, int) and 0 <= column <= 100000:
                    source += f":{column}"

    result: dict[str, object] = {
        "console_message_type": safe_type,
        "classification": classification,
        "source": source,
    }
    if wire_code is not None:
        result["wire_code"] = wire_code
    return result


def external_media_failure_evidence(url: Any, resource_type: Any, failure: Any) -> dict[str, str] | None:
    """Classify an intentionally blocked media request without retaining its URL or ID."""
    if not isinstance(url, str) or resource_type not in {"image", "media", "script", "other"}:
        return None
    host = (urlsplit(url).hostname or "").lower()
    host_class = next((name for suffixes, name in (
        (("youtube.com", "youtube-nocookie.com"), "youtube"),
        (("ytimg.com",), "ytimg"),
        (("googlevideo.com",), "googlevideo"),
    ) if any(host == suffix or host.endswith("." + suffix) for suffix in suffixes)), None)
    if host_class is None:
        return None
    failure_code = failure if isinstance(failure, str) and failure in SAFE_NETWORK_FAILURES else "unrecognized"
    return {"host_class": host_class, "resource_type": resource_type, "failure_code": failure_code}


def member_search_request_matches(payload: object, keyword: str) -> bool:
    if not isinstance(payload, dict) or payload.get("extension") != "Video.homeSearch":
        return False
    inputs = payload.get("input")
    return (isinstance(inputs, dict)
            and type(inputs.get("page")) is int and type(inputs.get("size")) is int
            and inputs == {"emotions": "", "keyword": keyword, "keyword_type": "title", "page": 1, "size": 20})


def member_search_evidence(requests: list[dict[str, object]], empty: bool, rendered: bool) -> dict[str, object]:
    rows = [row for row in requests if row.get("path") == "/extension"
            and row.get("extension_request", {}).get("extension") == "Video.homeSearch"]
    accepted = (len(rows) == 1 and rows[0].get("status") == 200
                and all(rows[0].get(key) is True for key in
                        ("body_captured", "ok", "search_input_matches", "contract_matches_runtime")))
    matches = (len(rows) == 1 and type(rows[0].get("response_list_length")) is int
               and rows[0]["response_list_length"] == (0 if empty else 1)
               and rows[0].get("contains_fixture_video") is (not empty))
    return {"search_response_count": len(rows), "server_accepted": bool(accepted),
            "response_matches_fixture": matches, "render_matches": rendered,
            "verified": bool(accepted and matches and rendered is True)}


def extension_request_evidence(body: Any) -> dict[str, object]:
    """Return only the allowlisted extension name and page/emotion inputs."""
    if not isinstance(body, dict):
        return {}
    name = body.get("extension", body.get("name"))
    if name not in LIST_EXTENSIONS:
        return {}
    inputs = body.get("input")
    if not isinstance(inputs, dict):
        inputs = {}
    row: dict[str, object] = {"extension": name}
    page = inputs.get("page")
    if isinstance(page, int) and not isinstance(page, bool) and 1 <= page <= 100000:
        row["page"] = page
    emotion = inputs.get("emotion")
    if isinstance(emotion, str) and emotion in EMOTIONS:
        row["emotion"] = emotion
    return row


def _list(value: Any) -> list[Any] | None:
    if isinstance(value, str):
        try:
            value = json.loads(value)
        except (TypeError, ValueError):
            return None
    return value if isinstance(value, list) else None


def _contains_video(items: list[Any], fixture_video_id: str) -> bool:
    return any(isinstance(item, dict) and item.get("video_id") == fixture_video_id for item in items)


def extension_response_evidence(extension: str, payload: Any, fixture_video_id: str) -> dict[str, object]:
    """Summarize only list lengths and whether the dedicated fixture video is present."""
    if extension not in LIST_EXTENSIONS or not isinstance(payload, dict):
        return {}
    output = payload.get("output")
    if not isinstance(output, dict):
        output = {}

    if extension == "Video.homeCategory":
        categories = _list(output.get("categories"))
        videos = [video for category in categories or [] if isinstance(category, dict)
                  for video in (_list(category.get("videos")) or [])]
        return {
            "response_list_length": len(videos) if categories is not None else None,
            "category_count": len(categories) if categories is not None else None,
            "contains_fixture_video": _contains_video(videos, fixture_video_id),
        }

    videos = _list(output.get("videos"))
    return {
        "response_list_length": len(videos) if videos is not None else None,
        "contains_fixture_video": _contains_video(videos or [], fixture_video_id),
    }


_SAFE_SCHEMA_NAME = re.compile(r"^[A-Za-z][A-Za-z0-9_.]{0,63}$")
_SAFE_PAGE_ERROR_NAMES = {"Error", "TypeError", "ReferenceError", "RangeError", "SyntaxError"}


def _schema_name(value: Any) -> str:
    return value if isinstance(value, str) and _SAFE_SCHEMA_NAME.match(value) else "unrecognized"


def admin_request_evidence(path: str, body: Any) -> dict[str, str]:
    """Names only for the admin console: the resource, aggregate, extension, operation or transition a request names. Never a value."""
    if not isinstance(body, dict):
        return {}
    if path == "/read" and isinstance(body.get("query"), dict):
        query = body["query"]
        return {"aggregate": _schema_name(query.get("aggregate"))} if "aggregate" in query else {"read": _schema_name(query.get("read"))}
    if path == "/extension":
        return {"extension": _schema_name(body.get("extension"))}
    if path == "/jobs/enqueue":
        return {"operation": _schema_name(body.get("operation"))}
    if path == "/apply":
        # The SDK wraps a write as {key, request}.
        applied = body.get("request") if isinstance(body.get("request"), dict) else body
        return {"apply": _schema_name(applied.get("apply") or ("atomic" if "atomic" in applied else "create"))}
    return {}


def aggregate_count(envelope: Any) -> int | None:
    """The one scalar of a successful aggregate answer, or None. Only a plain non-negative integer is a count; anything else is evidence of nothing."""
    if not isinstance(envelope, dict) or envelope.get("ok") is not True:
        return None
    rows = envelope.get("rows")
    if not isinstance(rows, list) or len(rows) != 1:
        return None
    value = rows[0]
    return value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else None


def safe_page_error_name(error: Any) -> str:
    """Playwright wraps a page error in its own class; the JavaScript name is `error.name`. Only a few well-known names are kept."""
    name = getattr(error, "name", type(error).__name__)
    return name if name in _SAFE_PAGE_ERROR_NAMES else "unrecognized"
